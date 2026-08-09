use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::data_structures::hash_map_fx::FxHashMap;
use crate::filesystem::fat32::{FileNodeHandle, INVALID_NODE_HANDLE};
use crate::filesystem::sirius::FileSystemError;
use crate::serial_println_core;

pub const FS_CACHE_SIZE: usize = 16 * 1024 * 1024;
pub const FS_CACHE_MAP_FILE_COUNT: usize = 1024;

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
    pub dirty_files: usize,
}

pub trait CacheFilesystemDriver {
    /// Read the entire file into out, out must be pre-sized to file_size
    /// Returns the number of bytes actually written
    fn read_file_into(
        &mut self,
        node_id: FileNodeHandle,
        out: &mut [u8],
    ) -> Result<usize, FileSystemError>;

    fn write_file(
        &mut self,
        node_id: FileNodeHandle,
        offset: usize,
        data: &[u8],
    ) -> Result<usize, FileSystemError>;

    fn file_size(&self, node_id: FileNodeHandle) -> Result<usize, FileSystemError>;

    fn read_dir(&mut self, node_id: FileNodeHandle) -> Result<Vec<String>, FileSystemError>;
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
    pub fn from_u8(value: u8) -> Option<Self> {
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

/// Per-file metadata
struct CachedFile {
    arena_offset: usize,
    len: usize,
    last_access: u64,
    is_dirty: bool,
    pinned: bool,
    importance: CacheImportance,
}

impl CachedFile {
    fn new(arena_offset: usize, len: usize, tick: u64, importance: CacheImportance) -> Self {
        Self {
            arena_offset,
            len,
            last_access: tick,
            is_dirty: false,
            pinned: false,
            importance,
        }
    }
}

/// On eviction the entry is removed from the map and its arena region is marked free 
/// The arena is compacted when a new file would exceed max_memory but there is 
/// enough free space fragmented across evicted regions
pub struct FileCache<D: CacheFilesystemDriver> {
    pub driver: D,
    arena: Vec<u8>,
    arena_push_offset: usize,
    current_memory_used: usize,
    max_memory: usize,
    access_tick: u64,
    files: FxHashMap<FileNodeHandle, CachedFile>,
    directory_children: FxHashMap<FileNodeHandle, Vec<FileNodeHandle>>,
    directory_hints: FxHashMap<FileNodeHandle, CacheImportance>,
}

impl<D: CacheFilesystemDriver> FileCache<D> {
    pub fn new(driver: D, max_memory: usize) -> Self {
        let mut arena = Vec::with_capacity(max_memory);
        arena.resize(max_memory, 0u8);
        Self {
            driver,
            arena,
            arena_push_offset: 0,
            current_memory_used: 0,
            max_memory,
            access_tick: 0,
            files: FxHashMap::with_capacity(FS_CACHE_MAP_FILE_COUNT),
            directory_children: FxHashMap::default(),
            directory_hints: FxHashMap::default(),
        }
    }

    fn arena_push_alloc(&mut self, len: usize) -> Option<usize> {
        if self.arena_push_offset + len <= self.max_memory {
            let offset = self.arena_push_offset;
            self.arena_push_offset += len;
            Some(offset)
        } else {
            None
        }
    }

    /// Slide all live file data down to fill gaps left by evicted entries
    fn compact(&mut self) {
        let mut live: Vec<(FileNodeHandle, usize, usize)> = self
            .files
            .iter()
            .map(|(&id, f)| (id, f.arena_offset, f.len))
            .collect();
        live.sort_by_key(|&(_, offset, _)| offset);

        let mut write_offset = 0usize;
        for (id, old_offset, len) in live {
            if old_offset != write_offset {
                self.arena.copy_within(old_offset..old_offset + len, write_offset);
                if let Some(f) = self.files.get_mut(id) {
                    f.arena_offset = write_offset;
                }
            }
            write_offset += len;
        }
        self.arena_push_offset = write_offset;
        serial_println_core!(
            "file_cache: compacted arena, used={}/{}",
            self.arena_push_offset,
            self.max_memory
        );
    }

    fn ensure_space(&mut self, needed: usize) {
        // first try to evict without compaction
        while self.arena_push_offset + needed > self.max_memory && self.current_memory_used > 0 {
            if !self.evict_one() {
                break;
            }
        }
        // if still not enough contiguous space at the top, compact and retry
        if self.arena_push_offset + needed > self.max_memory && self.current_memory_used + needed <= self.max_memory {
            self.compact();
        }
    }

    /// Read `len` bytes starting at `offset` from the cached file directly into `out`
    /// Loads from disk on miss. Returns bytes copied.
    pub fn read_file_range(
        &mut self,
        node_id: FileNodeHandle,
        offset: usize,
        out: &mut [u8],
    ) -> Result<usize, FileSystemError> {
        self.access_tick = self.access_tick.wrapping_add(1);

        if let Some(cached) = self.files.get_mut(node_id) {
            cached.last_access = self.access_tick;
            let file_len = cached.len;
            if offset >= file_len {
                return Ok(0);
            }
            let copy_len = out.len().min(file_len - offset);
            let src_off = cached.arena_offset + offset;
            out[..copy_len].copy_from_slice(&self.arena[src_off..src_off + copy_len]);
            serial_println_core!(
                "file_cache: hit node={:#x} offset={} copy={}",
                node_id,
                offset,
                copy_len
            );
            return Ok(copy_len);
        }

        // cache miss: load the whole file
        serial_println_core!("file_cache: miss node={:#x}", node_id);
        let file_size = self.driver.file_size(node_id)?;

        if file_size == 0 {
            return Ok(0);
        }

        self.ensure_space(file_size);

        let arena_off = match self.arena_push_alloc(file_size) {
            Some(off) => off,
            None => {
                // arena full even after eviction, read bypassing cache
                //TODO: this can just randomly allocate a very big buffer and that should not ever happen
                serial_println_core!("file_cache: arena full, bypass read node={:#x}", node_id);
                let mut tmp = alloc::vec![0u8; file_size];
                let read = self.driver.read_file_into(node_id, &mut tmp)?;
                let end = out.len().min(read.saturating_sub(offset));
                if end > 0 {
                    out[..end].copy_from_slice(&tmp[offset..offset + end]);
                }
                return Ok(end);
            }
        };

        let read = self.driver.read_file_into(node_id, &mut self.arena[arena_off..arena_off + file_size])?;
        let importance = self.get_effective_importance(node_id);
        self.files.insert(node_id, CachedFile::new(arena_off, read, self.access_tick, importance));
        self.current_memory_used += read;

        serial_println_core!(
            "file_cache: loaded node={:#x} size={} mem={}/{}",
            node_id,
            read,
            self.current_memory_used,
            self.max_memory
        );

        if offset >= read {
            return Ok(0);
        }
        let copy_len = out.len().min(read - offset);
        out[..copy_len].copy_from_slice(&self.arena[arena_off + offset..arena_off + offset + copy_len]);
        Ok(copy_len)
    }

    pub fn write_file(
        &mut self,
        node_id: FileNodeHandle,
        offset: usize,
        data: &[u8],
    ) -> Result<usize, FileSystemError> {
        self.access_tick = self.access_tick.wrapping_add(1);

        // ensure file is in cache before patching
        if !self.files.contains_key(node_id) {
            let file_size = self.driver.file_size(node_id).unwrap_or(0);
            if file_size > 0 {
                self.ensure_space(file_size);
                if let Some(arena_off) = self.arena_push_alloc(file_size) {
                    let read = self.driver.read_file_into(node_id, &mut self.arena[arena_off..arena_off + file_size])?;
                    let importance = self.get_effective_importance(node_id);
                    self.files.insert(node_id, CachedFile::new(arena_off, read, self.access_tick, importance));
                    self.current_memory_used += read;
                }
            } else {
                // new/empty file: allocate space for the incoming write
                let needed = offset + data.len();
                self.ensure_space(needed);
                if let Some(arena_off) = self.arena_push_alloc(needed) {
                    self.arena[arena_off..arena_off + needed].fill(0);
                    let importance = self.get_effective_importance(node_id);
                    self.files.insert(node_id, CachedFile::new(arena_off, 0, self.access_tick, importance));
                    self.current_memory_used += 0;
                }
            }
        }

        let write_end = offset + data.len();

        // if the write extends beyond current allocation evict this entry, compact, 
        // reallocate with the new size, copy old data back, then apply the write
        let needs_grow = {
            if let Some(f) = self.files.get(node_id) {
                write_end > f.arena_offset + (self.arena_push_offset - f.arena_offset).min(f.len + (self.max_memory - self.arena_push_offset))
                    || write_end > f.len && f.arena_offset + f.len < self.arena_push_offset
            } else {
                false
            }
        };

        if needs_grow {
            // save old data
            let (old_off, old_len) = {
                let f = self.files.get(node_id).unwrap();
                (f.arena_offset, f.len)
            };
            let new_len = write_end.max(old_len);
            // stash old bytes into a temporary vec, remove entry, compact, re-insert
            //TODO: stream from a CPU scratch buffer
            let mut old_data = alloc::vec![0u8; old_len];
            old_data.copy_from_slice(&self.arena[old_off..old_off + old_len]);

            if let Some(f) = self.files.remove(node_id) {
                self.current_memory_used -= f.len;
            }
            self.compact();
            self.ensure_space(new_len);
            if let Some(arena_off) = self.arena_push_alloc(new_len) {
                self.arena[arena_off..arena_off + old_len].copy_from_slice(&old_data);
                if new_len > old_len {
                    self.arena[arena_off + old_len..arena_off + new_len].fill(0);
                }
                let importance = self.get_effective_importance(node_id);
                self.files.insert(node_id, CachedFile::new(arena_off, new_len, self.access_tick, importance));
                self.current_memory_used += new_len;
            }
        }

        if let Some(cached) = self.files.get_mut(node_id) {
            let dst_off = cached.arena_offset + offset;
            self.arena[dst_off..dst_off + data.len()].copy_from_slice(data);
            if write_end > cached.len {
                self.current_memory_used += write_end - cached.len;
                cached.len = write_end;
            }
            cached.is_dirty = true;
            cached.last_access = self.access_tick;
            serial_println_core!(
                "file_cache: write node={:#x} offset={} len={} dirty",
                node_id,
                offset,
                data.len()
            );
            Ok(data.len())
        } else {
            Err(FileSystemError::IoError)
        }
    }

    pub fn flush_file(&mut self, node_id: FileNodeHandle) -> Result<(), FileSystemError> {
        if let Some(cached) = self.files.get_mut(node_id) {
            if cached.is_dirty {
                let off = cached.arena_offset;
                let len = cached.len;
                self.driver.write_file(node_id, 0, &self.arena[off..off + len])?;
                cached.is_dirty = false;
                serial_println_core!("file_cache: flushed node={:#x}", node_id);
            }
        }
        Ok(())
    }

    pub fn flush_nodes(&mut self, node_ids: &[FileNodeHandle]) -> Result<(), FileSystemError> {
        for &node_id in node_ids {
            if let Some(cached) = self.files.get_mut(node_id) {
                if cached.is_dirty {
                    let off = cached.arena_offset;
                    let len = cached.len;
                    self.driver.write_file(node_id, 0, &self.arena[off..off + len])?;
                    cached.is_dirty = false;
                    serial_println_core!("file_cache: flush_nodes: flushed node={:#x}", node_id);
                }
            }
        }
        Ok(())
    }

    pub fn flush_all(&mut self) -> Result<(), FileSystemError> {
        let dirty_ids: Vec<FileNodeHandle> = self
            .files
            .iter()
            .filter(|(_, f)| f.is_dirty)
            .map(|(&id, _)| id)
            .collect();

        for node_id in dirty_ids {
            if let Some(cached) = self.files.get_mut(node_id) {
                if cached.is_dirty {
                    let off = cached.arena_offset;
                    let len = cached.len;
                    self.driver.write_file(node_id, 0, &self.arena[off..off + len])?;
                    cached.is_dirty = false;
                    serial_println_core!("file_cache: flush_all: flushed node={:#x}", node_id);
                }
            }
        }
        Ok(())
    }

    pub fn pin_file(&mut self, node_id: FileNodeHandle) -> Result<(), FileSystemError> {
        if !self.files.contains_key(node_id) {
            // load into cache by doing a zero-length probe read
            let file_size = self.driver.file_size(node_id)?;
            if file_size > 0 {
                let mut dummy = [0u8; 0];
                self.read_file_range(node_id, 0, &mut dummy)?;
            }
        }
        if let Some(cached) = self.files.get_mut(node_id) {
            cached.pinned = true;
            cached.importance = CacheImportance::Resident;
            serial_println_core!("file_cache: pinned node={:#x}", node_id);
        }
        Ok(())
    }

    pub fn unpin_file(&mut self, node_id: FileNodeHandle) -> Result<(), FileSystemError> {
        if let Some(cached) = self.files.get_mut(node_id) {
            cached.pinned = false;
            cached.importance = CacheImportance::Normal;
            serial_println_core!("file_cache: unpinned node={:#x}", node_id);
        }
        Ok(())
    }

    pub fn reserve_cache(
        &mut self,
        node_id: FileNodeHandle,
        importance: CacheImportance,
    ) -> Result<(), FileSystemError> {
        self.directory_hints.insert(node_id, importance);
        if let Some(children) = self.directory_children.get(node_id) {
            let children: Vec<FileNodeHandle> = children.clone();
            for child_id in children {
                if let Some(cached) = self.files.get_mut(child_id) {
                    if !cached.pinned {
                        cached.importance = importance;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn evict_directory(&mut self, node_id: FileNodeHandle) -> Result<usize, FileSystemError> {
        let mut freed = 0usize;
        if let Some(children) = self.directory_children.remove(node_id) {
            for child_id in children {
                if let Some(cached) = self.files.get(child_id) {
                    if !cached.pinned {
                        freed += cached.len;
                        self.files.remove(child_id);
                        self.current_memory_used -= freed;
                    }
                }
            }
        }
        self.directory_hints.remove(node_id);
        Ok(freed)
    }

    pub fn register_file_in_directory(&mut self, dir_id: FileNodeHandle, file_id: FileNodeHandle) {
        self.directory_children
            .entry(dir_id)
            .or_insert_with(Vec::new)
            .push(file_id);
    }

    pub fn is_cached(&self, node_id: FileNodeHandle) -> bool {
        self.files.contains_key(node_id)
    }

    pub fn stats(&self) -> CacheStats {
        let dirty_files = self.files.values().filter(|f| f.is_dirty).count();
        CacheStats {
            total_files: self.files.len(),
            total_bytes: self.current_memory_used,
            max_bytes: self.max_memory,
            dirty_files,
        }
    }

    pub fn invalidate(&mut self, node_id: FileNodeHandle) {
        if let Some(cached) = self.files.remove(node_id) {
            self.current_memory_used -= cached.len;
            serial_println_core!("file_cache: invalidated node={:#x}", node_id);
        }
    }

    pub fn clear(&mut self) {
        self.files.clear();
        self.directory_children.clear();
        self.directory_hints.clear();
        self.current_memory_used = 0;
        self.arena_push_offset = 0;
    }

    fn get_effective_importance(&self, node_id: FileNodeHandle) -> CacheImportance {
        self.directory_hints
            .get(node_id)
            .copied()
            .unwrap_or(CacheImportance::Normal)
    }

    fn evict_one(&mut self) -> bool {
        let mut best_score: u64 = 0;
        let mut best_id: FileNodeHandle = INVALID_NODE_HANDLE;

        for (&node_id, cached) in self.files.iter() {
            if cached.pinned {
                continue;
            }
            let priority = cached.importance.eviction_priority();
            if priority == u64::MAX {
                continue;
            }
            let age = self.access_tick.wrapping_sub(cached.last_access);
            let score = priority.saturating_mul(age.max(1));
            if score > best_score {
                best_id = node_id;
                best_score = score;
            }
        }

        if best_id == INVALID_NODE_HANDLE {
            return false;
        }

        // flush before eviction if dirty
        if let Some(cached) = self.files.get(best_id) {
            if cached.is_dirty {
                let off = cached.arena_offset;
                let len = cached.len;
                if self.driver.write_file(best_id, 0, &self.arena[off..off + len]).is_err() {
                    serial_println_core!(
                        "file_cache: evict flush failed for node={:#x}, skipping",
                        best_id
                    );
                    return false;
                }
            }
        }

        if let Some(cached) = self.files.remove(best_id) {
            self.current_memory_used -= cached.len;
            serial_println_core!("file_cache: evicted node={:#x} len={}", best_id, cached.len);
            return true;
        }

        false
    }
}
