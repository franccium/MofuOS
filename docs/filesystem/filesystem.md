# MofuOS — Filesystem Subsystem (Sirius VFS)

## Files

```
kernel/src/filesystem/
  mod.rs         — declares fat32, file_cache, sirius modules; re-exports key types
  sirius.rs      — Sirius<D> VFS facade, FilesystemDriver trait, CachedDriver<D>,
                   FsDriverAdapter<D>, init_filesystem, init_filesystem_ata
  file_cache.rs  — FileCache<D>, CacheFilesystemDriver trait, CacheImportance,
                   CacheStats, writeback cache logic
  fat32/
    mod.rs         — Fat32Driver: full read/write/create/delete implementation
    direntry.rs    — DirectoryEntry, FatFileAttributes, fat_time_to_unix_timestamp
    boot_sector.rs — BootSector parsing from raw bytes
    test_data.rs   — create_fat32_image() helper for in-memory test images
kernel/src/io/
  ata.rs         — AtaPioDriver: ATA PIO primary bus master, implements DiskDevice
  disk.rs        — DiskDevice trait, MockDiskDevice, DiskManager, DISK global
  mod.rs         — pub mod ata, disk, serial
 os_disk_fat32/   — seed files for the FAT32 disk image (test.txt)
 storage/ata_disk.img — 64MB FAT32 image used by cargo xtask run (persistent, auto-created by xtask at storage/ata_disk.img)
 scripts/
   create_ata_disk.sh — generates storage/ata_disk.img with mkfs.fat -F 32
```

## How to Run with Real Persistent Disk

```
cargo xtask run        # auto-creates storage/ata_disk.img (64M) if missing, QEMU: -device piix3-ide,id=ide -device ide-hd,drive=ata0,bus=ide.0,unit=0 -drive file=storage/ata_disk.img,format=raw,id=ata0,if=none
cargo xtask ata-disk   # explicitly (re)create storage/ata_disk.img
```

`storage/ata_disk.img` is a raw FAT32 image on the host — writes from inside the kernel
are committed to the file and survive QEMU exit. `MockDiskDevice` is not persistent.

To recreate the disk image from scratch:
```
bash scripts/create_ata_disk.sh -i disk_templates/fat32_os_disk_template_default -o ata_disk.img -s 64
# or: cargo xtask ata-disk
```

## Architecture: Sirius VFS

Sirius is the VFS facade. It is generic over its driver type — no dynamic dispatch.

### Type Hierarchy (with cache feature enabled)

```
Sirius<CachedDriver<Fat32Driver>>
         |
         CachedDriver<Fat32Driver>
           .cache: FileCache<FsDriverAdapter<Fat32Driver>>
                    |
                    FsDriverAdapter<Fat32Driver>
                      .0: Fat32Driver
```

Without cache: `Sirius<Fat32Driver>` — direct, no intermediate layer.

### SIRIUS Static

The global static is concretely typed via cfg gates. No generics in a static.

```rust
#[cfg(feature = "use_cached_fs")]
lazy_static! {
    pub static ref SIRIUS: Once<Mutex<Sirius<CachedDriver<Fat32Driver>>>> = Once::new();
}

#[cfg(not(feature = "use_cached_fs"))]
lazy_static! {
    pub static ref SIRIUS: Once<Mutex<Sirius<Fat32Driver>>> = Once::new();
}
```

`get_sirius()` returns the matching concrete `MutexGuard` type. All call sites just
call `get_sirius()` — the concrete type is inferred.

### FilesystemDriver Trait

```rust
pub trait FilesystemDriver: Send + Sync {
    fn read_file(&mut self, node_id, offset, out_buffer) -> FileSystemResult<usize>;
    fn write_file(&mut self, node_id, offset, data) -> FileSystemResult<usize>;
    fn find_node(&self, path: &str) -> FileSystemResult<FileNodeHandle>;
    fn get_node(&self, node_id) -> FileSystemResult<FileNode>;
    fn list_directory(&self, node_id) -> FileSystemResult<Vec<FileNode>>;
    fn create_file(&mut self, parent_id, name) -> FileSystemResult<FileNodeHandle>;
    fn create_directory(&mut self, parent_id, name) -> FileSystemResult<FileNodeHandle>;
    fn delete(&mut self, node_id) -> FileSystemResult<()>;
    fn root_node(&self) -> FileNodeHandle;
}
```

### Sirius<D> API Methods

All take string paths. Leading `/` optional. Root = `/`.

- `resolve_path(path)` -> `FileNode`
- `open_file(path)` -> `FileNode` (asserts File type)
- `list_directory(path)` -> `Vec<FileNode>`
- `read_file(path, offset, &mut [u8])` -> bytes read
- `write_file(path, offset, &[u8])` -> bytes written
- `create_file(path)` -> `FileNode`
- `create_directory(path)` -> `FileNode`
- `delete(path)`

Cache-specific methods available only on `Sirius<CachedDriver<D>>` (separate impl block):

- `pin_file(path)` — load into cache, mark Resident, never evict
- `unpin_file(path)` — revert to Normal importance
- `reserve_cache(path, importance)` — set CacheImportance for a directory node
- `evict_directory(path)` — drop all cached children of a directory
- `flush_node(node_id)` — flush one dirty file to disk
- `flush_nodes(&[FileNodeHandle])` — flush a set of files in one pass
- `flush_all()` — flush all dirty files
- `cache_stats()` -> `CacheStats`

### Filesystem Initialization

Two init paths — mutually exclusive (both call `init_disk` + `SIRIUS.call_once`):

```rust
// In-memory MockDisk (testing)
pub fn init_filesystem(fat32_image: &[u8]) -> Result<(), &'static str>

// Real ATA drive
pub fn init_filesystem_ata() -> Result<(), &'static str>

// ATA with explicit cache size (use_cached_fs only)
pub fn init_filesystem_ata_with_cache(cache_size: usize) -> Result<(), &'static str>
```

With `use_cached_fs`, `init_filesystem` and `init_filesystem_ata` use `FS_CACHE_SIZE`
(64 MB) by default. Both wrap the `Fat32Driver` in `CachedDriver::new(driver, cache_size)`.

Neither is called in the default `cargo xtask run` boot. `main()` calls `test_ata_filesystem()`
when testing.

### FileSystemError

```rust
pub enum FileSystemError {
    NotFound, PermissionDenied, FileExists, IsDirectory, NotDirectory,
    DiskOpError, InvalidPath, FileSizeExceeded, InvalidFilename,
    DirectoryNotEmpty, NoSpace, DirectoryFull, IoError, NotSupported,
}
```

`From<DiskOpError>` implemented.

## File Cache

File: `kernel/src/filesystem/file_cache.rs`

### Design

`FileCache<D: CacheFilesystemDriver>` owns its driver. No dynamic dispatch — all
calls to the underlying driver go through `self.driver` directly on the concrete type.

`CacheFilesystemDriver` is the cache's view of the driver (whole-file ops):
```rust
pub trait CacheFilesystemDriver {
    fn read_file(&mut self, node_id) -> Result<Vec<u8>, FileSystemError>;
    fn read_file_range(&mut self, node_id, offset, len) -> Result<Vec<u8>, FileSystemError>;
    fn write_file(&mut self, node_id, offset, data) -> Result<usize, FileSystemError>;
    fn file_size(&self, node_id) -> Result<usize, FileSystemError>;
    fn read_dir(&mut self, node_id) -> Result<Vec<String>, FileSystemError>;
}
```

`FsDriverAdapter<D: FilesystemDriver>` is an owned newtype wrapping a `FilesystemDriver`
that implements `CacheFilesystemDriver`. This is what sits inside `FileCache` when
used from `CachedDriver`. The inner driver is accessed via `.0`.

### CachedDriver<D>

```rust
pub struct CachedDriver<D: FilesystemDriver> {
    pub cache: FileCache<FsDriverAdapter<D>>,
}
```

The inner `FilesystemDriver` lives at `self.cache.driver.0`. `CachedDriver` itself
implements `FilesystemDriver`, so it is a transparent drop-in replacement.

For operations the cache doesn't handle (find_node, get_node, list_directory,
create_file, create_directory, root_node): delegates straight to `cache.driver.0`.

For `delete`: invalidates the cache entry first, then delegates.

### Writeback Cache Policy

Writes do NOT go to disk immediately. `write_file` in `CachedDriver`:
1. If the file is not cached, loads it from disk first (so partial writes are safe).
2. Patches the in-memory buffer via `apply_write_inmem`.
3. Marks the entry `is_dirty = true`.
4. Returns the byte count. Disk is not touched.

Writes reach disk only when:
- `flush_file(node_id)` is called explicitly
- `flush_nodes(&[FileNodeHandle])` is called (batch flush by node ID slice)
- `flush_all()` is called (all dirty entries)
- `evict_one()` selects a dirty entry — it flushes to disk before evicting.
  If the flush fails, the entry is skipped (not evicted, not lost).

### Eviction

LRU-weighted by importance. Score = `(u64::MAX - priority) * age`. Higher score =
better eviction candidate (old + unimportant). Pinned entries and Resident entries
are never evicted. Dirty entries are flushed before removal.

`CacheImportance` levels (ascending, Resident = never evict):
`Minimal < Low < Normal < High < VeryHigh < Critical < Resident`

### Cache Syscalls — full spec: `docs/syscalls.md:2` (30-35)

| Number | Name            | Behavior                                                    |
|--------|-----------------|-------------------------------------------------------------|
| 30     | PinFile         | path -> pin in cache at Resident importance                 |
| 31     | UnpinFile       | path -> revert to Normal importance                         |
| 32     | ReserveCache    | path, importance(0..6) -> set directory-level hint          |
| 33     | EvictDirectory  | path -> drop all cached children of directory; returns freed bytes |
| 34     | GetCacheStats   | *CacheStatsFlat -> total_files, total_bytes, max_bytes,     |
|        |                 | dirty_files                                                 |
| 35     | FlushFileCache  | flush all open fds of calling process (by node_id slice)    |

`flush_all_file_caches()` — kernel-callable global flush, `#[cfg(feature = "use_cached_fs")]`,
defined in `syscall.rs`. Flushes all dirty entries regardless of process.

### Known Issues

- `directory_children` is never populated — `register_file_in_directory` is never
  called. This means `evict_directory` and `reserve_cache` directory hints have no
  effect on files. Fix: decode parent_cluster from `FileNodeHandle` bits 55-32 when
  loading a file into cache, and register it then.
- `get_effective_importance` looks up the file's own node_id in `directory_hints`,
  but hints are stored under directory node IDs. Same root cause as above.

## FAT32 Driver

### FileNodeHandle Packing

```
bit 63     : reserved_flag (always 0)
bits 62-56 : FAT attributes byte (7 bits, low 7 of the u8)
bits 55-32 : parent_cluster (24 bits, max cluster 0xFFFFFF)
bits 31-0  : file cluster (24 bits)
```

`encode_node_id(entry, parent_cluster)` / `decode_node_id(node_id) -> (cluster, parent_cluster, attrs)`

The `is_directory` check uses attrs bits from the node_id — no disk read needed.

### Boot Sector Geometry

`Fat32Driver::new` computes from BPB:

```
fat_start_sector   = reserved_sectors
root_start_sector  = fat_start_sector + num_fats * fat_size_32
data_start_sector  = root_start_sector   (root_dir_sectors = 0 for FAT32)
cluster_size       = sectors_per_cluster * bytes_per_sector
total_sectors      = total_sectors_32 if != 0, else total_sectors_16
available_sectors  = total_sectors - data_start_sector
max_cluster        = available_sectors / sectors_per_cluster + ROOT_CLUSTER(2)
```

IMPORTANT: `mkfs.fat` sets `total_sectors_32 = 0` for volumes <= 32MB and uses
`total_sectors_16`. Always prefer `total_sectors_32` when non-zero, fall back to
`total_sectors_16`. Ignoring this causes an underflow panic.

### FAT Entry Bit Masking (critical)

FAT32 entries are 28 bits. Top 4 bits are reserved.

`read_fat_entry` returns `raw_u32 & 0x0FFF_FFFF`.

`write_fat_entry` does read-modify-write preserving top 4 bits:
`new_entry = (existing & 0xF000_0000) | (value & 0x0FFF_FFFF)`

End-of-chain constants (28-bit):
```rust
END_OF_CHAIN:             0x0FFF_FFFF
BAD_CLUSTER:              0x0FFF_FFF7
FAT_ENTRY_RESERVED_BEGIN: 0x0FFF_FFF8
FAT_ENTRY_RESERVED_END:   0x0FFF_FFFE
```

### write_file Algorithm

1. `find_entry_by_cluster(parent_cluster, cluster)` — get current file size
2. Walk cluster chain to `offset / cluster_size`, allocating new clusters if needed
3. For each cluster to write:
   - Partial (not cluster-aligned): read-modify-write
   - Full cluster: write directly, no read needed
   - If chain ends: allocate + link a new cluster
4. If `offset + data.len() > old_size`: update `file_size` in the direntry on disk
5. Returns bytes written

### Cluster Chain Operations

```
read_cluster_chain(start, buf, disk)    — reads entire chain into buf
get_cluster_chain_length(start, disk)   — counts clusters in chain
find_free_cluster(disk)                 — linear scan of FAT for entry == 0
allocate_clusters(count, disk)          — allocates a new linked chain, returns first
free_cluster_chain(start, disk)         — sets all entries to 0
clear_clusters(start, count, disk)      — zero-fills cluster data sectors
expand_directory(dir_cluster, ...)      — appends a new cluster to a dir's chain
```

### Directory Entry Operations

`read_directory_entries(cluster, disk)` — returns ALL entries including deleted and
. / .. entries; callers must call `.retain(|e| e.is_valid())` for valid entries only.

`write_direntry(dir_cluster, index, entry, disk)` — reads entire dir cluster chain,
patches entry at byte offset `index * 32`, writes back. Expensive per call.

`find_free_slot_in_directory` — scans for deleted/empty slot; calls `expand_directory`
if none found.

`find_entry_by_cluster(parent_cluster, target_cluster, disk)` — linear scan of parent
directory entries by first cluster. Used by `read_file`/`write_file` to get file size.

### FAT32 Filename Encoding

8.3 format only. `set_filename` validates stem <= 8, ext <= 3, returns
`Err(InvalidFilename)` if violated. `get_filename()` trims trailing spaces, joins
with `.` if extension non-empty.

All userspace paths must use valid 8.3 names. LFN not supported [ISSUE-F5].

## Userspace Filesystem Syscalls — canonical table: `docs/syscalls.md:2`

| Number | Name          | Notes                                                        |
|--------|---------------|--------------------------------------------------------------|
| 20     | OpenFile      | path_ptr, path_len, flags -> fd index or MAX                 |
| 21     | CloseFile     | fd -> swap-removes from process fd table (O(1))              |
| 22     | ReadFile      | fd, offset, buf_ptr, count -> bytes read or MAX (explicit offset, no per-fd cursor) |
| 23     | WriteFile     | fd, offset, buf_ptr, count -> bytes written or MAX (explicit offset) |
| 24     | StatFile      | path_ptr, path_len, *StatFlat -> 0 or MAX                    |
| 25     | ListDir       | path_ptr, path_len, *DirEntryFlat, out_len -> entry count or MAX |
| 26     | CreateFile    | path_ptr, path_len -> 0 or MAX                               |
| 27     | CreateDir     | path_ptr, path_len -> 0 or MAX                               |
| 28     | Delete        | path_ptr, path_len -> 0 or MAX                               |
| 30-35  | Cache ops     | see Cache Syscalls above — Pin/Unpin/Reserve/Evict/GetStats/Flush |
| 600    | CreateCircularBuffer | size, page_flags, *CircularBufferInfo -> 0 or MAX (double-mapped ring) |
| 970    | GetCpuInfo    | *CpuInfoFlat, len -> 0 ok, 2 invalid ptr                     |

Wire types `DirEntryFlat` and `StatFlat` are `#[repr(C)]` defined in both
`kernel/src/process/syscall.rs` and `user/rustspace/src/lib.rs`.
`FS_NAME_LEN` (16) must be identical on both sides.

`FileDescriptor` in `Process`:
```rust
pub struct FileDescriptor {
    pub node_id: usize,  // FileNodeHandle — packed FAT32 cluster + parent + attrs
    pub flags: u8,       // FD_FLAG_READ=0x01, FD_FLAG_WRITE=0x02
    // no offset — ReadFile/WriteFile take explicit offset arg
}
```

`sys_close_file` uses swap-remove — fd indices are NOT stable after a close.

## ATA PIO Driver

File: `kernel/src/io/ata.rs`

Primary bus, master drive (unit 0). 28-bit LBA. Polling only (no DMA, no IRQs).

I/O ports:

| Port  | Name                                               |
|-------|----------------------------------------------------|
| 0x1F0 | Data (16-bit r/w)                                  |
| 0x1F1 | Error (read)                                       |
| 0x1F2 | Sector count                                       |
| 0x1F3 | LBA low                                            |
| 0x1F4 | LBA mid                                            |
| 0x1F5 | LBA high                                           |
| 0x1F6 | Drive/Head (LBA28 bits [27:24] + master select 0xE0) |
| 0x1F7 | Status / Command                                   |
| 0x3F6 | Alternate status (read without clearing IRQ)       |

Commands: READ_SECTORS=0x20, WRITE_SECTORS=0x30, CACHE_FLUSH=0xE7, IDENTIFY=0xEC.

After write: issue CACHE_FLUSH and poll BSY clear to ensure disk commit.
`POLL_TIMEOUT_ITERS = 100_000` before returning `Err(Timeout)`.

## Known Issues / TODOs

- [ISSUE-F5] FAT32: 8.3 filenames only, no LFN support
- `write_direntry` re-reads/re-writes entire directory chain on every call
- `find_free_cluster` is a linear scan of the entire FAT; FSInfo sector hints unused
- No timestamp update on write/create
- MockDiskDevice writes not persistent — only ATA path persists across reboots
- No ATA secondary bus or slave drive support (primary master only)
- File cache `directory_children` never populated — evict_directory and
  reserve_cache directory hints have no effect on files (see File Cache section)
