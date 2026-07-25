use crate::filesystem::fat32::Fat32Driver;
use crate::filesystem::fat32::FileNodeHandle;
#[cfg(feature = "use_cached_fs")]
use crate::filesystem::file_cache::FAT32Cache;
use crate::io::ata::AtaPioDriver;
use crate::io::disk::{DiskOpError, MockDiskDevice, SECTOR_SIZE, get_disk_mgr, init_disk};
use crate::serial_println_core;
use alloc::boxed::Box;
#[cfg(feature = "use_cached_fs")]
use alloc::collections::btree_map::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use bitflags::bitflags;
use lazy_static::lazy_static;
use spin::{Mutex, Once};

#[cfg(feature = "use_cached_fs")]
use {CacheFilesystemDriver, CacheImportance, FAT32Cache, CacheStats};

pub const FS_CACHE_SIZE: usize = 64 * 1024 * 1024;

lazy_static! {
    pub static ref SIRIUS: Once<Mutex<Sirius>> = Once::new();
}

pub fn get_sirius() -> spin::MutexGuard<'static, Sirius> {
    SIRIUS.get().unwrap().lock()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileType {
    File,
    Directory,
}

bitflags! {
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct FileAttributes: u8 {
        const READ = 0b00000001;
        const WRITE = 0b00000010;
        const EXECUTE = 0b00000100;

        const HIDDEN = 0b00001000;
        //NOTE: actually careful with 8th bit, modify the node_id packing code
        const RESERVED = 0b11110000;
    }
}

impl FileAttributes {
    pub const FILE_READONLY: Self = Self::READ;
    pub const FILE_READ_WRITE: Self = Self::READ.union(Self::WRITE);
    pub const FILE_READ_WRITE_EXECUTE: Self = Self::READ.union(Self::WRITE).union(Self::EXECUTE);

    pub const DIR_DEFAULT: Self = Self::READ;
}

// impl core::fmt::Debug for FileAttributes {
//     fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
//         write!(f, "{:#x}", self.bits());
//         Ok(())
//     }
// }

// Canonical name length for DirEntryFlat. Must match the rustspace definition.
pub const FS_NAME_LEN: usize = 16;

/// Flat directory entry written into the userspace output buffer by sys_list_dir.
#[repr(C)]
pub struct DirEntryFlat {
    pub name: [u8; FS_NAME_LEN],
    pub name_len: u8,
    pub is_dir: u8,
    pub _pad: [u8; 6],
    pub size: u64,
    pub created_time: u32,
    pub modified_time: u32,
}

/// Flat stat result written into a userspace-provided buffer by sys_stat_file
#[repr(C)]
pub struct StatFlat {
    pub name: [u8; FS_NAME_LEN],
    pub name_len: u8,
    pub is_dir: u8,
    pub size: u64,
    pub created_time: u32,
    pub modified_time: u32,
}

#[derive(Debug, Clone)]
pub struct FileNode {
    pub node_id: FileNodeHandle,
    pub name: String,
    pub file_type: FileType,
    pub size: usize,
    pub created_time: u32,
    pub modified_time: u32,
    pub attributes: FileAttributes,
}

pub type FileSystemResult<T> = Result<T, FileSystemError>;

#[derive(Debug, Clone, Copy)]
pub enum FileSystemError {
    NotFound,
    PermissionDenied,
    FileExists,
    IsDirectory,
    NotDirectory,
    DiskOpError,
    InvalidPath,
    FileSizeExceeded,
    InvalidFilename,
    DirectoryNotEmpty,
    NoSpace,
    DirectoryFull,
    IoError,
    NotSupported,
}

impl From<DiskOpError> for FileSystemError {
    fn from(_value: DiskOpError) -> Self {
        FileSystemError::DiskOpError
    }
}

pub trait FilesystemDriver: Send + Sync {
    fn read_file(
        &mut self,
        node_id: FileNodeHandle,
        offset: usize,
        out_buffer: &mut [u8],
    ) -> FileSystemResult<usize>;

    fn write_file(
        &mut self,
        node_id: FileNodeHandle,
        offset: usize,
        data: &[u8],
    ) -> FileSystemResult<usize>;

    fn find_node(&self, path: &str) -> FileSystemResult<FileNodeHandle>;
    fn get_node(&self, node_id: FileNodeHandle) -> FileSystemResult<FileNode>;

    fn list_directory(&self, node_id: FileNodeHandle) -> FileSystemResult<Vec<FileNode>>;

    fn create_file(
        &mut self,
        parent_id: FileNodeHandle,
        name: &str,
    ) -> FileSystemResult<FileNodeHandle>;
    fn create_directory(
        &mut self,
        parent_id: FileNodeHandle,
        name: &str,
    ) -> FileSystemResult<FileNodeHandle>;

    fn delete(&mut self, node_id: FileNodeHandle) -> FileSystemResult<()>;

    fn root_node(&self) -> FileNodeHandle;

    #[cfg(feature = "use_cached_fs")]
    fn read_whole_file(&mut self, path: &str) -> FileSystemResult<Vec<u8>> {
        let node_id = self.find_node(path)?;
        let node = self.get_node(node_id)?;

        if node.file_type != FileType::File {
            return Err(FileSystemError::IsDirectory);
        }

        let mut buffer = vec![0u8; node.size];
        let bytes_read = self.read_file(node_id, 0, &mut buffer)?;
        serial_println_core!("read_whole_file: bytes_read={}", bytes_read);
        buffer.truncate(bytes_read);
        Ok(buffer)
    }
}

#[cfg(feature = "use_cached_fs")]
struct CachedDriver {
    inner: Box<dyn FilesystemDriver>,
    cache: FAT32Cache,
    path_cache: BTreeMap<FileNodeHandle, String>,
}

#[cfg(feature = "use_cached_fs")]
impl CachedDriver {
    fn new(inner: Box<dyn FilesystemDriver>, cache_size: usize) -> Self {
        Self {
            inner,
            cache: FAT32Cache::new(cache_size),
            path_cache: BTreeMap::new(),
        }
    }

    fn cache_path(&mut self, node_id: FileNodeHandle, path: &str) {
        if !self.path_cache.contains_key(&node_id) {
            self.path_cache.insert(node_id, String::from(path));
        }
    }
}

#[cfg(feature = "use_cached_fs")]
impl FilesystemDriver for CachedDriver {
    fn read_file(
        &mut self,
        node_id: FileNodeHandle,
        offset: usize,
        out_buffer: &mut [u8],
    ) -> FileSystemResult<usize> {
        // Get the path for this node (needed for cache lookup)
        let node = self.inner.get_node(node_id)?;
        let path = self.path_cache.get(&node_id).cloned();

        if let Some(path) = path {
            let mut adapter = DriverAdapter {
                inner: &mut *self.inner,
            };

            match self.cache.read_file(&mut adapter, &path) {
                Ok(cached_data) => {
                    if cached_data.is_empty() || offset >= cached_data.len() {
                        return Ok(0);
                    }

                    let len = out_buffer
                        .len()
                        .min(cached_data.len().saturating_sub(offset));
                    if len > 0 {
                        out_buffer[..len].copy_from_slice(&cached_data[offset..offset + len]);
                    }
                    serial_println_core!(
                        "read_file from cached driver: found cached data, length={}, out_buffer.len()={}. cached_data.len()={}",
                        len,
                        out_buffer.len(),
                        cached_data.len()
                    );
                    Ok(len)
                }
                Err(_) => {
                    let len = self.inner.read_file(node_id, offset, out_buffer);
                    match len {
                        Ok(bytes_read) => {
                            serial_println_core!(
                                "read_file from cached driver: did not find cached data, bytes_read={}",
                                bytes_read
                            );
                        }
                        _ => {
                            serial_println_core!(
                                "read_file from cached driver: did not find cached data, and got a read error"
                            );
                        }
                    }
                    len
                }
            }
        } else {
            // No path cached - direct read
            self.inner.read_file(node_id, offset, out_buffer)
        }
    }

    fn write_file(
        &mut self,
        node_id: FileNodeHandle,
        offset: usize,
        data: &[u8],
    ) -> FileSystemResult<usize> {
        // Writes go directly to disk (and invalidate cache if needed)
        self.inner.write_file(node_id, offset, data)
    }

    fn find_node(&self, path: &str) -> FileSystemResult<FileNodeHandle> {
        self.inner.find_node(path)
    }

    fn get_node(&self, node_id: FileNodeHandle) -> FileSystemResult<FileNode> {
        self.inner.get_node(node_id)
    }

    fn list_directory(&self, node_id: FileNodeHandle) -> FileSystemResult<Vec<FileNode>> {
        self.inner.list_directory(node_id)
    }

    fn create_file(
        &mut self,
        parent_id: FileNodeHandle,
        name: &str,
    ) -> FileSystemResult<FileNodeHandle> {
        self.inner.create_file(parent_id, name)
    }

    fn create_directory(
        &mut self,
        parent_id: FileNodeHandle,
        name: &str,
    ) -> FileSystemResult<FileNodeHandle> {
        self.inner.create_directory(parent_id, name)
    }

    fn delete(&mut self, node_id: FileNodeHandle) -> FileSystemResult<()> {
        self.inner.delete(node_id)
    }

    fn root_node(&self) -> FileNodeHandle {
        self.inner.root_node()
    }
}

/// Adapter to make our VFS driver work with the cache's driver trait
#[cfg(feature = "use_cached_fs")]
struct DriverAdapter<'a> {
    inner: &'a mut dyn FilesystemDriver,
}

#[cfg(feature = "use_cached_fs")]
impl<'a> CacheFilesystemDriver for DriverAdapter<'a> {
    fn read_whole_file(&mut self, path: &str) -> FileSystemResult<Vec<u8>> {
        self.inner.read_whole_file(path).map_err(|e| e.into())
    }

    fn read_file_range(
        &mut self,
        path: &str,
        offset: usize,
        len: usize,
    ) -> FileSystemResult<Vec<u8>> {
        let node_id = self
            .inner
            .find_node(path)
            .map_err(|_| FileSystemError::NotFound)?;
        let node = self
            .inner
            .get_node(node_id)
            .map_err(|_| FileSystemError::NotFound)?;

        let read_len = len.min(node.size.saturating_sub(offset));
        let mut buffer = vec![0u8; read_len];

        self.inner
            .read_file(node_id, offset, &mut buffer)
            .map_err(|_| FileSystemError::IoError)?;

        Ok(buffer)
    }

    fn file_size(&self, path: &str) -> FileSystemResult<usize> {
        let node_id = self
            .inner
            .find_node(path)
            .map_err(|_| FileSystemError::NotFound)?;
        let node = self
            .inner
            .get_node(node_id)
            .map_err(|_| FileSystemError::NotFound)?;
        Ok(node.size)
    }

    fn resolve_path(&self, path: &str) -> FileSystemResult<FileNodeHandle> {
        self.inner
            .find_node(path)
            .map_err(|_| FileSystemError::NotFound)
    }

    fn read_dir(&mut self, path: &str) -> FileSystemResult<Vec<alloc::string::String>> {
        let node_id = self
            .inner
            .find_node(path)
            .map_err(|_| FileSystemError::NotFound)?;
        let nodes = self
            .inner
            .list_directory(node_id)
            .map_err(|_| FileSystemError::IoError)?;

        Ok(nodes.into_iter().map(|n| n.name).collect())
    }
}

pub struct Sirius {
    pub driver: Box<dyn FilesystemDriver>,

    #[cfg(feature = "use_cached_fs")]
    cache_enabled: bool,
}

impl Sirius {
    #[cfg(not(feature = "use_cached_fs"))]
    pub fn new(driver: Box<dyn FilesystemDriver>) -> Self {
        Self { driver }
    }

    #[cfg(feature = "use_cached_fs")]
    pub fn new(driver: Box<dyn FilesystemDriver>) -> Self {
        Self {
            driver,
            cache_enabled: true,
        }
    }

    #[cfg(feature = "use_cached_fs")]
    pub fn new_with_cache(
        driver: Box<dyn FilesystemDriver>,
        cache_size: usize,
        enable: bool,
    ) -> Self {
        if enable {
            Self {
                driver: Box::new(CachedDriver::new(driver, cache_size)),
                cache_enabled: true,
            }
        } else {
            Self {
                driver,
                cache_enabled: false,
            }
        }
    }

    pub fn resolve_path(&self, path: &str) -> FileSystemResult<FileNode> {
        let node_id = self.driver.find_node(path)?;
        self.driver.get_node(node_id)
    }

    pub fn open_file(&self, path: &str) -> FileSystemResult<FileNode> {
        let node = self.resolve_path(path)?;
        if node.file_type == FileType::File {
            Ok(node)
        } else {
            Err(FileSystemError::IsDirectory)
        }
    }

    pub fn list_directory(&self, path: &str) -> FileSystemResult<Vec<FileNode>> {
        let node = self.resolve_path(path)?;
        serial_println_core!(
            "Resolved path '{}' to node ID {:#x}, type: {:?}",
            path,
            node.node_id,
            node.file_type
        );
        if node.file_type == FileType::Directory {
            self.driver.list_directory(node.node_id)
        } else {
            Err(FileSystemError::NotDirectory)
        }
    }

    pub fn read_file(
        &mut self,
        path: &str,
        offset: usize,
        buffer: &mut [u8],
    ) -> FileSystemResult<usize> {
        let node = self.resolve_path(path)?;
        serial_println_core!(
            "Reading file '{}' (node ID {:#x}) at offset {}, buffer size {}",
            path,
            node.node_id,
            offset,
            buffer.len()
        );
        if node.file_type != FileType::File {
            return Err(FileSystemError::IsDirectory);
        }

        #[cfg(feature = "use_cached_fs")]
        if let Some(cached) = self.as_cached_driver_mut() {
            cached.cache_path(node.node_id, path);
        }

        self.driver.read_file(node.node_id, offset, buffer)
    }

    pub fn write_file(
        &mut self,
        path: &str,
        offset: usize,
        data: &[u8],
    ) -> FileSystemResult<usize> {
        let node = self.resolve_path(path)?;
        if node.file_type != FileType::File {
            return Err(FileSystemError::IsDirectory);
        }
        self.driver.write_file(node.node_id, offset, data)
    }

    pub fn create_file(&mut self, path: &str) -> FileSystemResult<FileNode> {
        let (parent_path, name) = self.split_path(path)?;
        let parent = self.resolve_path(parent_path.as_str())?;

        serial_println_core!(
            "Sirius: create_file: path: {}, parent_path: {}, name: {}",
            path,
            parent_path,
            name
        );

        if parent.file_type != FileType::Directory {
            serial_println_core!("Error: Sirius: create_file: parent is not a directory");
            return Err(FileSystemError::NotDirectory);
        }

        let node_id = self.driver.create_file(parent.node_id, name.as_str())?;
        serial_println_core!("Sirius: create_file: created file: {:#x}", node_id);

        let node = self.driver.get_node(node_id)?;

        #[cfg(feature = "use_cached_fs")]
        if let Some(cached) = self.as_cached_driver_mut() {
            cached.cache_path(node_id, path);
        }

        Ok(node)
    }

    pub fn create_directory(&mut self, path: &str) -> FileSystemResult<FileNode> {
        let (parent_path, name) = self.split_path(path)?;
        let parent = self.resolve_path(parent_path.as_str())?;

        serial_println_core!(
            "Sirius: create_directory: path: {}, parent_path: {}, name: {}",
            path,
            parent_path,
            name
        );

        if parent.file_type != FileType::Directory {
            return Err(FileSystemError::NotDirectory);
        }

        let node_id = self
            .driver
            .create_directory(parent.node_id, name.as_str())?;
        serial_println_core!(
            "Sirius: create_directory: created directory: {:#x}",
            node_id
        );

        self.driver.get_node(node_id)
    }

    pub fn delete(&mut self, path: &str) -> FileSystemResult<()> {
        serial_println_core!("Sirius: delete: looking for path: {}", path);
        let node = self.resolve_path(path)?;

        serial_println_core!(
            "Sirius: delete: path: {}, resolved node: {}",
            path,
            node.name
        );

        self.driver.delete(node.node_id)
    }

    #[cfg(feature = "use_cached_fs")]
    pub fn pin_file(&mut self, path: &str) -> FileSystemResult<()> {
        if let Some(cached) = self.as_cached_driver_mut() {
            // Access the cache through the CachedDriver
            // This requires exposing methods on CachedDriver
            cached.pin_file(path).map_err(|e| e.into())
        } else {
            Err(FileSystemError::NotSupported)
        }
    }

    #[cfg(feature = "use_cached_fs")]
    pub fn unpin_file(&mut self, path: &str) -> FileSystemResult<()> {
        if let Some(cached) = self.as_cached_driver_mut() {
            cached.unpin_file(path).map_err(|e| e.into())
        } else {
            Err(FileSystemError::NotSupported)
        }
    }

    #[cfg(feature = "use_cached_fs")]
    pub fn reserve_cache(
        &mut self,
        path: &str,
        importance: CacheImportance,
    ) -> FileSystemResult<()> {
        if let Some(cached) = self.as_cached_driver_mut() {
            cached.reserve_cache(path, importance).map_err(|e| e.into())
        } else {
            Err(FileSystemError::NotSupported)
        }
    }

    #[cfg(feature = "use_cached_fs")]
    pub fn evict_directory(&mut self, path: &str) -> FileSystemResult<usize> {
        if let Some(cached) = self.as_cached_driver_mut() {
            cached.evict_directory(path).map_err(|e| e.into())
        } else {
            Err(FileSystemError::NotSupported)
        }
    }

    #[cfg(feature = "use_cached_fs")]
    pub fn cache_stats(&self) -> Option<CacheStats> {
        self.as_cached_driver().map(|c| c.cache_stats())
    }

    #[cfg(feature = "use_cached_fs")]
    fn as_cached_driver_mut(&mut self) -> Option<&mut CachedDriver> {
        let driver_ref: &mut dyn FilesystemDriver = &mut *self.driver;
        unsafe {
            let ptr = driver_ref as *mut dyn FilesystemDriver as *mut CachedDriver;
            if self.cache_enabled {
                Some(&mut *ptr)
            } else {
                None
            }
        }
    }

    #[cfg(feature = "use_cached_fs")]
    fn as_cached_driver(&self) -> Option<&CachedDriver> {
        let driver_ref: &dyn FilesystemDriver = &*self.driver;
        unsafe {
            let ptr = driver_ref as *const dyn FilesystemDriver as *const CachedDriver;
            if self.cache_enabled {
                Some(&*ptr)
            } else {
                None
            }
        }
    }

    fn split_path(&self, path: &str) -> FileSystemResult<(String, String)> {
        if path.is_empty() || path == "/" {
            return Err(FileSystemError::InvalidPath);
        }

        let path = if path.starts_with('/') {
            &path[1..]
        } else {
            path
        };

        match path.rfind('/') {
            Some(pos) => {
                let parent = if pos == 0 {
                    String::from("/")
                } else {
                    String::from(&path[..pos])
                };
                let name = String::from(&path[pos + 1..]);
                Ok((parent, name))
            }
            None => Ok((String::from("/"), String::from(path))),
        }
    }
}

#[cfg(feature = "use_cached_fs")]
impl CachedDriver {
    pub fn pin_file(&mut self, path: &str) -> FileSystemResult<()> {
        let mut adapter = DriverAdapter {
            inner: &mut *self.inner,
        };
        self.cache.pin_file(&mut adapter, path)
    }

    pub fn unpin_file(&mut self, path: &str) -> FileSystemResult<()> {
        self.cache.unpin_file(path)
    }

    pub fn reserve_cache(
        &mut self,
        path: &str,
        importance: CacheImportance,
    ) -> FileSystemResult<()> {
        self.cache.reserve_directory(path, importance)
    }

    pub fn evict_directory(&mut self, path: &str) -> FileSystemResult<usize> {
        self.cache.evict_directory(path)
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }
}

#[cfg(not(feature = "use_cached_fs"))]
pub fn init_filesystem(fat32_image: &[u8]) -> Result<(), &'static str> {
    let mut disk = MockDiskDevice::new(fat32_image.len() / 512 + 1);
    disk.load_image(fat32_image)
        .map_err(|_| "Failed to load disk image")?;
    init_disk(Box::new(disk));

    let boot_sector_data = &fat32_image[..512];

    let fat32_driver =
        Fat32Driver::new(boot_sector_data).map_err(|_| "Failed to initialize FAT32 driver")?;

    SIRIUS.call_once(|| Mutex::new(Sirius::new(Box::new(fat32_driver))));

    serial_println_core!("Filesystem initialized (no cache)");
    Ok(())
}

#[cfg(feature = "use_cached_fs")]
pub fn init_filesystem(fat32_image: &[u8]) -> Result<(), &'static str> {
    init_filesystem_with_cache(fat32_image, FS_CACHE_SIZE, true)
}

#[cfg(feature = "use_cached_fs")]
pub fn init_filesystem_with_cache(
    fat32_image: &[u8],
    cache_size: usize,
    enable_cache: bool,
) -> Result<(), &'static str> {
    let mut disk = MockDiskDevice::new(fat32_image.len() / 512 + 1);
    disk.load_image(fat32_image)
        .map_err(|_| "Failed to load disk image")?;
    init_disk(Box::new(disk));

    let boot_sector_data = &fat32_image[..512];

    let fat32_driver =
        Fat32Driver::new(boot_sector_data).map_err(|_| "Failed to initialize FAT32 driver")?;

    SIRIUS.call_once(|| {
        Mutex::new(Sirius::new_with_cache(
            Box::new(fat32_driver),
            cache_size,
            enable_cache,
        ))
    });

    if enable_cache {
        serial_println_core!(
            "Filesystem initialized with cache size: {}",
            cache_size
        );
    } else {
        serial_println_core!("Filesystem initialized (cache disabled)");
    }
    Ok(())
}

#[cfg(not(feature = "use_cached_fs"))]
pub fn init_filesystem_ata() -> Result<(), &'static str> {
    let ata_driver =
        AtaPioDriver::check_primary_bus_present().ok_or("No ATA drive found on primary bus")?;

    init_disk(Box::new(ata_driver));

    let mut boot_sector_buf = [0u8; SECTOR_SIZE];
    {
        let mut disk = get_disk_mgr();
        disk.read_sector(0, &mut boot_sector_buf)
            .map_err(|_| "Failed to read boot sector from ATA drive")?;
    }

    let fat32_driver = Fat32Driver::new(&boot_sector_buf)
        .map_err(|_| "Failed to initialize FAT32 driver from ATA")?;

    SIRIUS.call_once(|| Mutex::new(Sirius::new(Box::new(fat32_driver))));

    serial_println_core!("Filesystem initialized from ATA drive (no cache)");
    Ok(())
}

#[cfg(feature = "use_cached_fs")]
pub fn init_filesystem_ata() -> Result<(), &'static str> {
    init_filesystem_ata_with_cache(FS_CACHE_SIZE, true)
}

#[cfg(feature = "use_cached_fs")]
pub fn init_filesystem_ata_with_cache(
    cache_size: usize,
    enable_cache: bool,
) -> Result<(), &'static str> {
    let ata_driver =
        AtaPioDriver::check_primary_bus_present().ok_or("No ATA drive found on primary bus")?;

    init_disk(Box::new(ata_driver));

    let mut boot_sector_buf = [0u8; SECTOR_SIZE];
    {
        let mut disk = get_disk_mgr();
        disk.read_sector(0, &mut boot_sector_buf)
            .map_err(|_| "Failed to read boot sector from ATA drive")?;
    }

    let fat32_driver = Fat32Driver::new(&boot_sector_buf)
        .map_err(|_| "Failed to initialize FAT32 driver from ATA")?;

    SIRIUS.call_once(|| {
        Mutex::new(Sirius::new_with_cache(
            Box::new(fat32_driver),
            cache_size,
            enable_cache,
        ))
    });

    if enable_cache {
        serial_println_core!(
            "Filesystem initialized from ATA drive with cache size: {}",
            cache_size
        );
    } else {
        serial_println_core!("Filesystem initialized from ATA drive (cache disabled)");
    }
    Ok(())
}
