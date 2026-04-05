pub mod boot_sector;
pub mod direntry;
pub mod test_data;

use crate::filesystem::fat32::direntry::FatFileAttributes;
use crate::filesystem::sirius::{
    FileAttributes, FileNode, FileSystemError, FileSystemResult, FileType, FilesystemDriver,
};
use crate::io::{DISK, disk::get_disk_mgr, serial};
use crate::serial_println;
use alloc::string::String;
use alloc::vec::Vec;
use boot_sector::BootSector;
use direntry::{DIRECTORY_ENTRY_SIZE, DirectoryEntry};

pub const ROOT_CLUSTER: u32 = 2;
// node_id packing assumes up to 2^24 clusters
pub const MAX_CLUSTER: u32 = 0xFFFFFF;

pub type FileNodeHandle = usize;
pub const INVALID_NODE_HANDLE: FileNodeHandle = 0;

fn encode_node_id(entry: &DirectoryEntry, parent_cluster: u32) -> FileNodeHandle {
    let cluster = entry.get_first_cluster();
    let reserved_flag = 0; // can use for something
    let attrs = entry.attributes;

    ((reserved_flag as usize) << 63)
        | ((attrs as usize & 0x7F) << 56)
        | ((parent_cluster as usize & 0xFFFFFF) << 32)
        | (cluster as usize & 0xFFFFFF)
}

fn decode_node_id(node_id: FileNodeHandle) -> (u32, u32, u8) {
    let attrs = ((node_id >> 56) & 0x7F) as u8;
    let parent_cluster = ((node_id >> 32) & 0xFFFFFF) as u32;
    let cluster = (node_id & 0xFFFFFF) as u32;

    (cluster, parent_cluster, attrs)
}

const fn file_attributes_from_fat_attributes(fat_attributes: u8) -> FileAttributes {
    if fat_attributes & FatFileAttributes::Directory as u8 != 0 {
        FileAttributes::DIR_DEFAULT
    } else {
        if fat_attributes & FatFileAttributes::ReadOnly as u8 == 0 {
            FileAttributes::FILE_READ_WRITE
        } else {
            FileAttributes::FILE_READONLY
        }
    }
}

pub struct Fat32Driver {
    boot_sector: BootSector,
    fat_start_sector: u64,
    root_start_sector: u64,
    data_start_sector: u64,
    sectors_per_cluster: u32,

    cluster_size: usize,

    root_dir_node_id: FileNodeHandle,
    root_direntry: DirectoryEntry,
    root_filenode: FileNode,
}

pub const END_OF_CHAIN: u32 = 0x0FFFFFFF;
pub const BAD_CLUSTER: u32 = 0xFFFFFFF7;
pub const FAT_ENTRY_RESERVED_BEGIN: u32 = 0xFFFFFFF8;
pub const FAT_ENTRY_RESERVED_END: u32 = 0xFFFFFFFE;
pub const TOP_FAT_ENTRY: u32 = 0xFFFFFFFF;

impl Fat32Driver {
    pub fn new(boot_sector_data: &[u8]) -> FileSystemResult<Self> {
        let boot_sector = BootSector::from_bytes(boot_sector_data)?;

        let fat_start_sector = boot_sector.reserved_sectors as u64;
        let sectors_per_cluster = boot_sector.sectors_per_cluster as u32;
        let fat_size = boot_sector.fat_size_32 as u64;
        let root_start_sector = fat_start_sector + (boot_sector.num_fats as u64 * fat_size);
        let root_dir_sectors = 0u64;
        let data_start_sector = root_start_sector + root_dir_sectors;
        let cluster_size = (sectors_per_cluster as usize) * (boot_sector.bytes_per_sector as usize);

        let root_direntry = DirectoryEntry::create_for_root();
        let root_dir_node_id = encode_node_id(&root_direntry, boot_sector.root_cluster);
        let root_filenode = FileNode {
            node_id: root_dir_node_id,
            name: String::from("/"),
            file_type: FileType::Directory,
            size: 0,
            created_time: 0,
            modified_time: 0,
            attributes: FileAttributes::DIR_DEFAULT,
        };

        Ok(Self {
            boot_sector,
            fat_start_sector,
            root_start_sector,
            data_start_sector,
            sectors_per_cluster,
            cluster_size,
            root_dir_node_id,
            root_direntry,
            root_filenode,
        })
    }

    fn cluster_to_sector(&self, cluster: u32) -> u64 {
        if cluster >= 2 {
            self.data_start_sector + (((cluster - 2) as u64) * self.sectors_per_cluster as u64)
        } else {
            self.root_start_sector
        }
    }

    fn read_fat_entry(&self, cluster: u32) -> FileSystemResult<u32> {
        let fat_offset = (cluster as u64) * 4;
        let fat_sector =
            self.fat_start_sector + (fat_offset / self.boot_sector.bytes_per_sector as u64);
        let offset_in_sector = (fat_offset % self.boot_sector.bytes_per_sector as u64) as usize;

        let mut sector_buffer = alloc::vec![0u8; self.boot_sector.bytes_per_sector as usize];

        {
            let mut disk_mgr = get_disk_mgr();
            disk_mgr.read_sector(fat_sector, &mut sector_buffer)?
        }

        let entry_bytes = [
            sector_buffer[offset_in_sector],
            sector_buffer[offset_in_sector + 1],
            sector_buffer[offset_in_sector + 2],
            sector_buffer[offset_in_sector + 3],
        ];

        Ok(u32::from_le_bytes(entry_bytes))
    }

    fn get_next_cluster(&self, cluster: u32) -> FileSystemResult<Option<u32>> {
        if cluster == 0 {
            return Ok(None);
        }

        let fat_entry = self.read_fat_entry(cluster)?;

        match fat_entry {
            END_OF_CHAIN => Ok(None),
            BAD_CLUSTER => Err(FileSystemError::DiskOpError),
            FAT_ENTRY_RESERVED_BEGIN..=FAT_ENTRY_RESERVED_END => Ok(None),
            next_cluster => Ok(Some(next_cluster)),
        }
    }

    fn read_cluster_chain(
        &self,
        start_cluster: u32,
        out_buffer: &mut [u8],
    ) -> FileSystemResult<usize> {
        let mut current_cluster = start_cluster;
        let mut bytes_read = 0;

        while bytes_read < out_buffer.len() && current_cluster < TOP_FAT_ENTRY {
            let sector = self.cluster_to_sector(current_cluster);
            let bytes_to_read = core::cmp::min(self.cluster_size, out_buffer.len() - bytes_read);

            {
                let mut cluster_buffer = alloc::vec![0u8; self.cluster_size];
                let mut disk_mgr = get_disk_mgr();
                disk_mgr.read_sectors(
                    sector,
                    self.sectors_per_cluster as usize,
                    &mut cluster_buffer,
                )?;
                out_buffer[bytes_read..bytes_read + bytes_to_read]
                    .copy_from_slice(&cluster_buffer[..bytes_to_read]);
            }

            bytes_read += bytes_to_read;

            match self.get_next_cluster(current_cluster)? {
                Some(next) => current_cluster = next,
                None => break,
            }
        }

        Ok(bytes_read)
    }

    // Read directory entries starting with a given cluster
    fn read_directory_entries(&self, start_cluster: u32) -> FileSystemResult<Vec<DirectoryEntry>> {
        // Read up to 4 clusters
        let mut buffer = alloc::vec![0u8; self.cluster_size * 4];

        let bytes_read = self.read_cluster_chain(start_cluster, &mut buffer)?;

        let entry_count = bytes_read / DIRECTORY_ENTRY_SIZE;
        let mut valid_entries = Vec::with_capacity(entry_count);

        for i in 0..entry_count {
            let offset = i * DIRECTORY_ENTRY_SIZE;
            let entry = DirectoryEntry::from_bytes(&buffer[offset..offset + DIRECTORY_ENTRY_SIZE]);
            if entry.is_valid() && !entry.is_volume_id() {
                valid_entries.push(entry);
            }
        }

        Ok(valid_entries)
    }

    fn find_entry_by_cluster(
        &self,
        parent_cluster: u32,
        target_cluster: u32,
    ) -> FileSystemResult<DirectoryEntry> {
        let entries = self.read_directory_entries(parent_cluster)?;

        for entry in entries {
            if entry.get_first_cluster() == target_cluster {
                return Ok(entry);
            }
        }

        Err(FileSystemError::NotFound)
    }

    fn find_direntry(&self, path: &str) -> FileSystemResult<(DirectoryEntry, u32)> {
        let path_parts: Vec<&str> = path.split("/").filter(|p| !p.is_empty()).collect();
        let part_count = path_parts.len();

        if path_parts.is_empty() {
            return Ok((self.root_direntry, self.boot_sector.root_cluster));
        }

        let mut found_entry = None;
        let mut current_cluster = self.boot_sector.root_cluster;
        let mut parent_cluster = self.boot_sector.root_cluster;

        for (i, path_part) in path_parts.iter().enumerate() {
            let entries = self.read_directory_entries(current_cluster)?;
            let entry = entries
                .iter()
                .find(|e| e.get_filename() == *path_part)
                .ok_or(FileSystemError::NotFound)?;

            if i < part_count - 1 && !entry.is_directory() {
                return Err(FileSystemError::NotDirectory);
            }

            parent_cluster = current_cluster;
            current_cluster = entry.get_first_cluster();
            found_entry = Some(*entry);
        }

        assert!(found_entry.is_some());
        let entry = found_entry.unwrap();

        Ok((entry, parent_cluster))
    }
}

impl FilesystemDriver for Fat32Driver {
    fn read_file(
        &mut self,
        node_id: FileNodeHandle,
        offset: usize,
        out_buffer: &mut [u8],
    ) -> FileSystemResult<usize> {
        let (cluster, parent_cluster, attributes) = decode_node_id(node_id);

        serial_println!(
            "FAT32Driver: read_file called with node_id={:#x}, cluster={}, offset={}, out_buffer_size={}",
            node_id,
            cluster,
            offset,
            out_buffer.len()
        );

        if attributes & FatFileAttributes::Directory as u8 != 0 {
            return Err(FileSystemError::IsDirectory);
        }

        if cluster == 0 {
            return Err(FileSystemError::InvalidPath);
        }

        let entry = self.find_entry_by_cluster(parent_cluster, cluster)?;
        let file_size = entry.file_size as usize;

        if offset >= file_size {
            serial_println!(
                "FAT32Driver: Offset {} is beyond file size {}",
                offset,
                file_size
            );
            return Err(FileSystemError::FileSizeExceeded);
        }

        let skip_clusters = offset / self.cluster_size;
        let offset_in_cluster = offset % self.cluster_size;

        // Skip to correct cluster
        serial_println!(
            "FAT32Driver: Reading file at cluster {}, offset_in_cluster {}, skip_clusters {}",
            cluster,
            offset_in_cluster,
            skip_clusters
        );
        let mut current_cluster = cluster;
        for _ in 0..skip_clusters {
            match self.get_next_cluster(current_cluster)? {
                Some(next) => current_cluster = next,
                None => return Ok(0),
            }
        }
        serial_println!(
            "FAT32Driver: Positioned at cluster {} after skipping",
            current_cluster
        );

        // Read cluster and offset
        let mut temp_buffer = alloc::vec![0u8; self.cluster_size];
        let sector = self.cluster_to_sector(current_cluster);
        serial_println!(
            "FAT32Driver: Reading cluster {}, sector {}, offset_in_cluster {}",
            current_cluster,
            sector,
            offset_in_cluster
        );

        {
            serial_println!(
                "FAT32Driver: Issuing read_sectors for sector {}, count {}, temp_buffer size {}",
                sector,
                self.sectors_per_cluster,
                temp_buffer.len()
            );
            let mut disk_mgr = get_disk_mgr();
            disk_mgr.read_sectors(sector, self.sectors_per_cluster as usize, &mut temp_buffer)?
        }

        let available = self.cluster_size - offset_in_cluster;
        let bytes_to_read = core::cmp::min(
            available,
            core::cmp::min(out_buffer.len(), file_size - offset),
        );

        serial_println!(
            "FAT32Driver: Reading cluster {}, sector {}, offset_in_cluster {}, bytes_to_read {}",
            current_cluster,
            sector,
            offset_in_cluster,
            bytes_to_read
        );

        out_buffer[..bytes_to_read]
            .copy_from_slice(&temp_buffer[offset_in_cluster..offset_in_cluster + bytes_to_read]);

        serial_println!(
            "FAT32Driver: Read {} bytes from file (offset {})",
            bytes_to_read,
            offset
        );

        Ok(bytes_to_read)
    }

    fn write_file(
        &mut self,
        node_id: FileNodeHandle,
        offset: usize,
        data: &[u8],
    ) -> FileSystemResult<usize> {
        Err(FileSystemError::NotSupported)
    }

    fn get_node(&self, node_id: FileNodeHandle) -> FileSystemResult<FileNode> {
        let (cluster, parent_cluster, attrs) = decode_node_id(node_id);

        if cluster != ROOT_CLUSTER {
            let entry = self.find_entry_by_cluster(parent_cluster, cluster)?;

            Ok(FileNode {
                node_id: node_id,
                name: entry.get_filename(),
                file_type: entry.get_file_type(),
                size: entry.file_size as usize,
                created_time: entry.get_creation_timestamp(),
                modified_time: entry.get_modified_timestamp(),
                attributes: file_attributes_from_fat_attributes(entry.attributes),
            })
        } else {
            Ok(self.root_filenode.clone())
        }
    }

    fn list_directory(&self, node_id: FileNodeHandle) -> FileSystemResult<Vec<FileNode>> {
        let (dir_cluster, _, attributes) = decode_node_id(node_id);

        if attributes & FatFileAttributes::Directory as u8 == 0 {
            return Err(FileSystemError::NotDirectory);
        }

        let entries = self.read_directory_entries(dir_cluster)?;
        let mut nodes = Vec::with_capacity(entries.len());

        serial_println!(
            "FAT32Driver: list_directory found {} entries in dir_cluster {}",
            entries.len(),
            dir_cluster
        );
        serial_println!(
            "FAT32Driver: list_directory entries: {}",
            entries
                .iter()
                .map(|e| e.get_filename())
                .collect::<Vec<String>>()
                .join(", ")
        );

        for entry in entries {
            let entry_cluster = entry.get_first_cluster();
            let is_dir = entry.is_directory();
            let size = entry.file_size;
            let node_id = encode_node_id(&entry, dir_cluster);

            serial_println!(
                "FAT32Driver: list_directory Encoding file '{}' with cluster={}, size={}, is_dir={}, node_id={:#x}",
                entry.get_filename(),
                entry_cluster,
                size,
                is_dir,
                node_id
            );

            nodes.push(FileNode {
                node_id: node_id,
                name: entry.get_filename(),
                file_type: entry.get_file_type(),
                size: size as usize,
                created_time: entry.get_creation_timestamp(),
                modified_time: entry.get_modified_timestamp(),
                attributes: file_attributes_from_fat_attributes(entry.attributes),
            });
        }

        Ok(nodes)
    }

    fn find_node(&self, path: &str) -> FileSystemResult<FileNodeHandle> {
        let (entry, parent_cluster) = self.find_direntry(path)?;

        let is_dir = entry.is_directory();
        let size = entry.file_size;

        serial_println!(
            "FAT32Driver: find_node found '{}' at cluster {}, parent_cluster {}, is_dir={}",
            path,
            entry.get_first_cluster(),
            parent_cluster,
            is_dir
        );
        assert!(!(is_dir && size != 0));

        let node_id = encode_node_id(&entry, parent_cluster);
        serial_println!("find_node: Encoding node_id {:#x}", node_id);

        Ok(node_id)
    }

    fn create_file(
        &mut self,
        parent_id: FileNodeHandle,
        name: &str,
    ) -> FileSystemResult<FileNodeHandle> {
        Err(FileSystemError::NotSupported)
    }

    fn create_directory(
        &mut self,
        parent_id: FileNodeHandle,
        name: &str,
    ) -> FileSystemResult<FileNodeHandle> {
        Err(FileSystemError::NotSupported)
    }

    fn delete(&mut self, node_id: FileNodeHandle) -> FileSystemResult<()> {
        Err(FileSystemError::NotSupported)
    }

    fn root_node(&self) -> FileNodeHandle {
        self.root_dir_node_id
    }
}
