/*

wait why am i even doing this
why not just use file descriptors, store mappings of paths to open file handles, if another process wants the same file as some cached file, he gets the handle
and the handle indexes into an array of pointers to a page with this file loaded in RAM
that was even my first idea
and thats works even for multiple processes and is just better i suppose
whatever this is a fun idea at least
i suppose i was thinking in terms of a memory arena so that caching scheme would be less wasteful in memory
also thats a bit better for actual cache locality

actually the below is a whole in-memory filesystem
i made a filesystem instead of file cache
w/e

groups of arrays, hashable groups
hash is by some parenting part of the file path
we do this because file operations are very often based on locality
maybe we can even let the process specify how much cache it wants for files at given paths
e.g. declare_path_cache_importance(path, Importance::VERY_HIGH) / declare_path_cache_size(path, 64)
this would give bigger/smaller chunks of the global cache array to the given path and its children caches
the hash is somehow computed from path and has to be fast
/home/game/fonts - 1001001
/home/game - 0001001
/home - 0000001
/ - 0000000
or something
here home would have the parenting hash and nest its local file cache inside, game needs a big hash chunk, fonts inside game dont need much space for any more nested cache structures
maybe i could even let the process specify that a given directory will be flat
and specify the most important files in a given directory
maybe elevate important files outside of the whole structure, and have them lifetime cached until they arent needed anymore (also a declaration made by the process - start_file_cache_residency(path) / stop_file_cache_residency(path)

can we make optimizations based on the fact that FAT32 limits filesizes to 8 bytes?
so each directory is 8-bytes long, so we hash 8 bits for each directory part of the path
this data assumption definitely has optimization potential

*/

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::min;
use core::fmt;
use core::mem::size_of;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;

use crate::filesystem::fat32::FileNodeHandle;
use crate::filesystem::sirius::FileSystemError;
use crate::{serial_println, serial_println_core};

const PAGE_SIZE: usize = 4096;
pub const FS_CACHE_SIZE: usize = 64 * 1024 * 1024;

const DEBUG_LOGS: bool = false;

macro_rules! serial_println_core {
    ($($arg:tt)*) => {
        if DEBUG_LOGS {
            $crate::serial_println_core!($($arg)*);
        }
    };
}
pub struct CacheStats {
    pub total_files: usize,
    pub total_bytes: usize,
    pub max_bytes: usize,
}

#[derive(Debug)]
pub struct CachePage {
    data: Vec<u8>, //TODO:
    file_size: usize,
    access_tick: u64,
    is_dirty: bool,
    pinned: bool,
}

impl CachePage {
    fn new(data: Vec<u8>, tick: u64) -> Self {
        let file_size = data.len();
        Self {
            data,
            file_size,
            access_tick: tick,
            is_dirty: false,
            pinned: false,
        }
    }

    fn get_slice(&self, offset: usize, len: usize) -> &[u8] {
        let end = min(offset + len, self.file_size);
        &self.data[offset..end]
    }
}

#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct CachePagePtr(NonNull<CachePage>);

impl CachePagePtr {
    pub fn new(ptr: *const CachePage) -> Option<Self> {
        Some(Self(NonNull::new(ptr as *mut CachePage)?))
    }

    pub const fn from_raw(ptr: *mut CachePage) -> Self {
        Self(unsafe { NonNull::new_unchecked(ptr) })
    }

    pub const fn as_ptr(self) -> *mut CachePage {
        self.0.as_ptr()
    }

    pub const fn as_const_ptr(self) -> *const CachePage {
        self.0.as_ptr() as *const CachePage
    }

    pub unsafe fn as_mut_ref(&mut self) -> &mut CachePage {
        self.0.as_mut()
    }

    pub unsafe fn as_ref(&self) -> &CachePage {
        self.0.as_ref()
    }

    // Add these helper methods
    pub unsafe fn get_mut(&mut self) -> &mut CachePage {
        self.0.as_mut()
    }

    pub unsafe fn get(&self) -> &CachePage {
        self.0.as_ref()
    }
}

impl Deref for CachePagePtr {
    type Target = CachePage;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref() }
    }
}
impl DerefMut for CachePagePtr {
    fn deref_mut(&mut self) -> &mut CachePage {
        unsafe { self.0.as_mut() }
    }
}

unsafe impl Send for CachePagePtr {}
unsafe impl Sync for CachePagePtr {}

// Add conversions
impl From<*mut CachePage> for CachePagePtr {
    fn from(ptr: *mut CachePage) -> Self {
        Self::from_raw(ptr)
    }
}

impl From<*const CachePage> for CachePagePtr {
    fn from(ptr: *const CachePage) -> Self {
        Self::new(ptr).expect("null pointer")
    }
}

impl From<&mut CachePage> for CachePagePtr {
    fn from(page: &mut CachePage) -> Self {
        Self::from_raw(page)
    }
}

impl From<&CachePage> for CachePagePtr {
    fn from(page: &CachePage) -> Self {
        Self::from_raw(page as *const CachePage as *mut CachePage)
    }
}

pub type FileSystemResult<T> = Result<T, FileSystemError>;

pub trait CacheFilesystemDriver {
    fn read_file(&mut self, node_id: FileNodeHandle) -> FileSystemResult<Vec<u8>>;
    fn read_file_range(
        &mut self,
        node_id: FileNodeHandle,
        offset: usize,
        len: usize,
    ) -> FileSystemResult<Vec<u8>>;
    fn file_size(&self, node_id: FileNodeHandle) -> FileSystemResult<usize>;
    fn read_dir(&mut self, node_id: FileNodeHandle) -> FileSystemResult<Vec<String>>;
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum CacheImportance {
    Minimal = 0,
    Low = 1,
    Normal = 2,
    High = 3,
    VeryHigh = 4,
    Critical = 5,
    Resident = 6,
}

impl fmt::Display for CacheImportance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            CacheImportance::Minimal => "Minimal",
            CacheImportance::Low => "Low",
            CacheImportance::Normal => "Normal",
            CacheImportance::High => "High",
            CacheImportance::VeryHigh => "VeryHigh",
            CacheImportance::Critical => "Critical",
            CacheImportance::Resident => "Resident",
        };
        write!(f, "{}", s)
    }
}

impl CacheImportance {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Minimal),
            1 => Some(Self::Low),
            2 => Some(Self::Normal),
            3 => Some(Self::High),
            4 => Some(Self::VeryHigh),
            5 => Some(Self::Critical),
            6 => Some(Self::Resident),
            _ => None,
        }
    }

    fn eviction_priority(&self) -> u64 {
        match self {
            Self::Minimal => 0,
            Self::Low => 1,
            Self::Normal => 2,
            Self::High => 3,
            Self::VeryHigh => 4,
            Self::Critical => 5,
            Self::Resident => u64::MAX,
        }
    }
}

struct CachedFile {
    data: Vec<u8>,
    file_size: usize,
    last_access: u64,
    is_dirty: bool,
    pinned: bool,
    importance: CacheImportance,
}

impl CachedFile {
    fn new(data: Vec<u8>, tick: u64, importance: CacheImportance) -> Self {
        let file_size = data.len();
        Self {
            data,
            file_size,
            last_access: tick,
            is_dirty: false,
            pinned: false,
            importance,
        }
    }

    fn get_slice(&self, offset: usize, len: usize) -> &[u8] {
        let end = min(offset + len, self.file_size);
        &self.data[offset..end]
    }
}

pub struct FileCache {
    current_memory: usize,
    max_memory: usize,
    access_tick: u64,

    files: BTreeMap<FileNodeHandle, CachedFile>,

    directory_children: BTreeMap<FileNodeHandle, Vec<FileNodeHandle>>,
    directory_hints: BTreeMap<FileNodeHandle, CacheImportance>,
}

impl FileCache {
    pub fn new(max_memory: usize) -> Self {
        Self {
            files: BTreeMap::new(),
            directory_children: BTreeMap::new(),
            directory_hints: BTreeMap::new(),
            current_memory: 0,
            max_memory,
            access_tick: 0,
        }
    }

    pub fn read_file(
        &mut self,
        driver: &mut dyn CacheFilesystemDriver,
        node_id: FileNodeHandle,
    ) -> FileSystemResult<Vec<u8>> {
        self.access_tick = self.access_tick.wrapping_add(1);

        // Check cache
        if let Some(cached) = self.files.get_mut(&node_id) {
            serial_println_core!(
                "file_cache: cache hit for node {:#x}, size: {}, importance: {}",
                node_id,
                cached.file_size,
                cached.importance
            );
            cached.last_access = self.access_tick;
            return Ok(cached.data.clone());
        }

        serial_println_core!(
            "file_cache: cache miss for node {:#x}, loading from disk",
            node_id
        );

        // Cache miss - load from disk
        let data = driver.read_file(node_id)?;
        let data_len = data.len();

        serial_println_core!("FileCache: fat32path: loaded {} bytes from disk", data_len);

        if data_len == 0 {
            serial_println_core!("FileCache: fat32path: data is empty, not caching");
            return Ok(Vec::new());
        }

        // Check space
        self.ensure_space(data_len);

        // Cache the file
        self.current_memory += data_len;
        let importance = self.get_effective_importance(node_id);
        self.files.insert(
            node_id,
            CachedFile::new(data.clone(), self.access_tick, importance),
        );

        serial_println_core!(
            "file_cache: cached file {:#x}, memory usage: {}/{}",
            node_id,
            self.current_memory,
            self.max_memory
        );

        Ok(data)
    }

    /// Read a partial range of a file
    pub fn read_file_range(
        &mut self,
        driver: &mut dyn CacheFilesystemDriver,
        node_id: FileNodeHandle,
        offset: usize,
        len: usize,
    ) -> FileSystemResult<Vec<u8>> {
        self.access_tick = self.access_tick.wrapping_add(1);

        // Check cache
        if let Some(cached) = self.files.get_mut(&node_id) {
            cached.last_access = self.access_tick;
            let mut result = Vec::with_capacity(len);
            result.extend_from_slice(cached.get_slice(offset, len));
            return Ok(result);
        }

        // Try to read just the range from disk
        if let Ok(range_data) = driver.read_file_range(node_id, offset, len) {
            return Ok(range_data);
        }

        // Fallback: load entire file, then return range
        let data = driver.read_file(node_id)?;
        let end = min(offset + len, data.len());
        Ok(data[offset..end].to_vec())
    }

    pub fn pin_file(
        &mut self,
        driver: &mut dyn CacheFilesystemDriver,
        node_id: FileNodeHandle,
    ) -> FileSystemResult<()> {
        // Ensure file is cached
        if !self.files.contains_key(&node_id) {
            self.read_file(driver, node_id)?;
        }

        if let Some(cached) = self.files.get_mut(&node_id) {
            cached.pinned = true;
            cached.importance = CacheImportance::Resident;
            serial_println_core!("file_cache: pinned file {:#x}", node_id);
        }

        Ok(())
    }

    pub fn unpin_file(&mut self, node_id: FileNodeHandle) -> FileSystemResult<()> {
        if let Some(cached) = self.files.get_mut(&node_id) {
            cached.pinned = false;
            cached.importance = CacheImportance::Normal;
            serial_println_core!("file_cache: unpinned file {:#x}", node_id);
        }
        Ok(())
    }

    /// Reserve cache space for a directory with given importance
    pub fn reserve_cache(
        &mut self,
        node_id: FileNodeHandle,
        importance: CacheImportance,
    ) -> FileSystemResult<()> {
        serial_println_core!(
            "file_cache: setting importance {} for directory {:#x}",
            importance,
            node_id
        );
        self.directory_hints.insert(node_id, importance);

        // Update importance of already-cached files in this directory
        if let Some(children) = self.directory_children.get(&node_id) {
            let importance = importance;
            for &child_id in children {
                if let Some(cached) = self.files.get_mut(&child_id) {
                    if !cached.pinned {
                        cached.importance = importance;
                    }
                }
            }
        }

        Ok(())
    }

    /// Evict all files under a directory
    pub fn evict_directory(&mut self, node_id: FileNodeHandle) -> FileSystemResult<usize> {
        serial_println_core!("file_cache: evicting directory {:#x}", node_id);
        let mut freed = 0usize;

        if let Some(children) = self.directory_children.remove(&node_id) {
            for child_id in children {
                if let Some(cached) = self.files.remove(&child_id) {
                    if !cached.pinned {
                        freed += cached.data.len();
                        serial_println_core!(
                            "file_cache: evicted file {:#x}, freed {} bytes",
                            child_id,
                            cached.data.len()
                        );
                    } else {
                        // Re-insert pinned files
                        self.files.insert(child_id, cached);
                    }
                }
            }
        }

        self.current_memory -= freed;
        self.directory_hints.remove(&node_id);

        Ok(freed)
    }

    pub fn register_file_in_directory(&mut self, dir_id: FileNodeHandle, file_id: FileNodeHandle) {
        self.directory_children
            .entry(dir_id)
            .or_insert_with(Vec::new)
            .push(file_id);
    }

    pub fn is_cached(&self, node_id: FileNodeHandle) -> bool {
        self.files.contains_key(&node_id)
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            total_files: self.files.len(),
            total_bytes: self.current_memory,
            max_bytes: self.max_memory,
        }
    }

    pub fn invalidate(&mut self, node_id: FileNodeHandle) {
        if let Some(cached) = self.files.remove(&node_id) {
            self.current_memory -= cached.data.len();
            serial_println_core!("file_cache: invalidated file {:#x}", node_id);
        }
    }

    pub fn clear(&mut self) {
        self.files.clear();
        self.directory_children.clear();
        self.directory_hints.clear();
        self.current_memory = 0;
        serial_println_core!("file_cache: cache cleared");
    }

    fn get_effective_importance(&self, node_id: FileNodeHandle) -> CacheImportance {
        self.directory_hints
            .get(&node_id)
            .copied()
            .unwrap_or(CacheImportance::Normal)
    }

    fn ensure_space(&mut self, needed: usize) {
        while self.current_memory + needed > self.max_memory {
            if !self.evict_one() {
                serial_println_core!("file_cache: cannot free enough space");
                break;
            }
        }
    }

    fn evict_one(&mut self) -> bool {
        let mut best_candidate: Option<(FileNodeHandle, u64)> = None;

        for (&node_id, cached) in &self.files {
            if cached.pinned {
                continue;
            }

            let priority = cached.importance.eviction_priority();
            let score = if priority == u64::MAX {
                continue;
            } else {
                let age = self.access_tick.wrapping_sub(cached.last_access);
                (u64::MAX - priority) * age
            };

            match best_candidate {
                None => best_candidate = Some((node_id, score)),
                Some((_, best_score)) if score < best_score => {
                    best_candidate = Some((node_id, score));
                }
                _ => {}
            }
        }

        if let Some((node_id, _)) = best_candidate {
            if let Some(cached) = self.files.remove(&node_id) {
                let freed = cached.data.len();
                self.current_memory -= freed;
                serial_println_core!(
                    "file_cache: evicted file {:#x} (importance: {}, age: {}), freed {} bytes",
                    node_id,
                    cached.importance,
                    self.access_tick.wrapping_sub(cached.last_access),
                    freed
                );
                return true;
            }
        }

        false
    }
}
