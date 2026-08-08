use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::min;
use core::fmt;

use crate::filesystem::fat32::{FileNodeHandle, INVALID_NODE_HANDLE};
use crate::filesystem::sirius::FileSystemError;
use crate::serial_println_core;

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
    pub dirty_files: usize,
}

pub trait CacheFilesystemDriver {
    fn read_file(&mut self, node_id: FileNodeHandle) -> Result<Vec<u8>, FileSystemError>;
    fn read_file_range(
        &mut self,
        node_id: FileNodeHandle,
        offset: usize,
        len: usize,
    ) -> Result<Vec<u8>, FileSystemError>;
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

    fn apply_write_inmem(&mut self, offset: usize, data: &[u8]) -> usize {
        let end = offset + data.len();
        if end > self.data.len() {
            self.data.resize(end, 0);
            self.file_size = end;
        }
        self.data[offset..end].copy_from_slice(data);
        self.is_dirty = true;
        data.len()
    }
}

pub struct FileCache<D: CacheFilesystemDriver> {
    pub driver: D,
    current_memory: usize,
    max_memory: usize,
    access_tick: u64,
    files: BTreeMap<FileNodeHandle, CachedFile>,
    directory_children: BTreeMap<FileNodeHandle, Vec<FileNodeHandle>>,
    directory_hints: BTreeMap<FileNodeHandle, CacheImportance>,
}

impl<D: CacheFilesystemDriver> FileCache<D> {
    pub fn new(driver: D, max_memory: usize) -> Self {
        Self {
            driver,
            files: BTreeMap::new(),
            directory_children: BTreeMap::new(),
            directory_hints: BTreeMap::new(),
            current_memory: 0,
            max_memory,
            access_tick: 0,
        }
    }

    pub fn read_file(&mut self, node_id: FileNodeHandle) -> Result<Vec<u8>, FileSystemError> {
        self.access_tick = self.access_tick.wrapping_add(1);

        if let Some(cached) = self.files.get_mut(&node_id) {
            serial_println_core!(
                "file_cache: cache hit node={:#x} size={} importance={}",
                node_id,
                cached.file_size,
                cached.importance
            );
            cached.last_access = self.access_tick;
            return Ok(cached.data.clone());
        }

        serial_println_core!("file_cache: cache miss node={:#x}", node_id);
        let data = self.driver.read_file(node_id)?;
        let data_len = data.len();

        if data_len == 0 {
            return Ok(Vec::new());
        }

        self.ensure_space(data_len);
        self.current_memory += data_len;
        let importance = self.get_effective_importance(node_id);
        self.files.insert(
            node_id,
            CachedFile::new(data.clone(), self.access_tick, importance),
        );

        serial_println_core!(
            "file_cache: loaded node={:#x} mem={}/{}",
            node_id,
            self.current_memory,
            self.max_memory
        );
        Ok(data)
    }

    pub fn read_file_range(
        &mut self,
        node_id: FileNodeHandle,
        offset: usize,
        len: usize,
    ) -> Result<Vec<u8>, FileSystemError> {
        self.access_tick = self.access_tick.wrapping_add(1);

        if let Some(cached) = self.files.get_mut(&node_id) {
            cached.last_access = self.access_tick;
            let mut result = Vec::with_capacity(len);
            result.extend_from_slice(cached.get_slice(offset, len));
            return Ok(result);
        }

        if let Ok(range_data) = self.driver.read_file_range(node_id, offset, len) {
            return Ok(range_data);
        }

        let data = self.driver.read_file(node_id)?;
        let end = min(offset + len, data.len());
        Ok(data[offset..end].to_vec())
    }

    pub fn write_file(
        &mut self,
        node_id: FileNodeHandle,
        offset: usize,
        data: &[u8],
    ) -> Result<usize, FileSystemError> {
        self.access_tick = self.access_tick.wrapping_add(1);

        if !self.files.contains_key(&node_id) {
            let existing = self.driver.read_file(node_id).unwrap_or_default();
            let existing_len = existing.len();
            self.ensure_space(existing_len);
            self.current_memory += existing_len;
            let importance = self.get_effective_importance(node_id);
            self.files.insert(
                node_id,
                CachedFile::new(existing, self.access_tick, importance),
            );
        }

        let cached = self.files.get_mut(&node_id).unwrap();
        let old_size = cached.data.len();
        let written = cached.apply_write_inmem(offset, data);

        let new_size = cached.data.len();
        if new_size > old_size {
            self.current_memory += new_size - old_size;
        }

        serial_println_core!(
            "file_cache: buffered write node={:#x} offset={} len={} dirty=true",
            node_id,
            offset,
            written
        );
        Ok(written)
    }

    pub fn flush_file(&mut self, node_id: FileNodeHandle) -> Result<(), FileSystemError> {
        if let Some(cached) = self.files.get_mut(&node_id) {
            if cached.is_dirty {
                self.driver.write_file(node_id, 0, &cached.data)?;
                cached.is_dirty = false;
                serial_println_core!("file_cache: flushed node={:#x}", node_id);
            }
        }
        Ok(())
    }

    pub fn flush_nodes(&mut self, node_ids: &[FileNodeHandle]) -> Result<(), FileSystemError> {
        for &node_id in node_ids {
            if let Some(cached) = self.files.get_mut(&node_id) {
                if cached.is_dirty {
                    self.driver.write_file(node_id, 0, &cached.data)?;
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
            if let Some(cached) = self.files.get_mut(&node_id) {
                self.driver.write_file(node_id, 0, &cached.data)?;
                cached.is_dirty = false;
                serial_println_core!("file_cache: flush_all: flushed node={:#x}", node_id);
            }
        }
        Ok(())
    }

    pub fn pin_file(&mut self, node_id: FileNodeHandle) -> Result<(), FileSystemError> {
        if !self.files.contains_key(&node_id) {
            self.read_file(node_id)?;
        }
        if let Some(cached) = self.files.get_mut(&node_id) {
            cached.pinned = true;
            cached.importance = CacheImportance::Resident;
            serial_println_core!("file_cache: pinned node={:#x}", node_id);
        }
        Ok(())
    }

    pub fn unpin_file(&mut self, node_id: FileNodeHandle) -> Result<(), FileSystemError> {
        if let Some(cached) = self.files.get_mut(&node_id) {
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
        if let Some(children) = self.directory_children.get(&node_id) {
            let children: Vec<FileNodeHandle> = children.clone();
            for child_id in children {
                if let Some(cached) = self.files.get_mut(&child_id) {
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
        if let Some(children) = self.directory_children.remove(&node_id) {
            for child_id in children {
                if let Some(cached) = self.files.remove(&child_id) {
                    if !cached.pinned {
                        freed += cached.data.len();
                    } else {
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

    pub fn stats(&self) -> CacheStats {
        let dirty_files = self.files.values().filter(|f| f.is_dirty).count();
        CacheStats {
            total_files: self.files.len(),
            total_bytes: self.current_memory,
            max_bytes: self.max_memory,
            dirty_files,
        }
    }

    pub fn invalidate(&mut self, node_id: FileNodeHandle) {
        if let Some(cached) = self.files.remove(&node_id) {
            self.current_memory -= cached.data.len();
            serial_println_core!("file_cache: invalidated node={:#x}", node_id);
        }
    }

    pub fn clear(&mut self) {
        self.files.clear();
        self.directory_children.clear();
        self.directory_hints.clear();
        self.current_memory = 0;
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
        let mut best_score: u64 = 0;
        let mut best_node_handle: FileNodeHandle = INVALID_NODE_HANDLE;

        for (&node_id, cached) in &self.files {
            if cached.pinned {
                continue;
            }
            let priority = cached.importance.eviction_priority();
            if priority == u64::MAX {
                continue;
            }
            let age = self.access_tick.wrapping_sub(cached.last_access);
            let score = priority.saturating_mul(age.max(1));
            if (score > best_score) {
                best_node_handle = node_id;
                best_score = score;
            }
        }

        if best_node_handle != INVALID_NODE_HANDLE {
            if let Some(cached) = self.files.get(&best_node_handle) {
                if cached.is_dirty {
                    let data = cached.data.clone();
                    if self.driver.write_file(best_node_handle, 0, &data).is_err() {
                        serial_println_core!(
                            "file_cache: evict flush failed for node={:#x}, skipping",
                            best_node_handle
                        );
                        return false;
                    }
                }
            }
            if let Some(cached) = self.files.remove(&best_node_handle) {
                self.current_memory -= cached.data.len();
                serial_println_core!("file_cache: evicted node={:#x}", best_node_handle);
                return true;
            }
        }

        false
    }
}
