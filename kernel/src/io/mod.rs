pub mod disk;
pub mod serial;

pub use disk::{DISK, DiskDevice, DiskManager, DiskOpError, MockDiskDevice, init_disk};
