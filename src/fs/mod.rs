use alloc::borrow::ToOwned;

use crate::alloc::string::String;

struct SuperBlock {
    /// Total number of sectors
    total_sectors: usize,
    /// Number of sectors in each block
    sector_per_cluster: usize,
    /// Number of bytes in each sector
    bytes_per_sector: usize,
    /// Number of sectors available
    available_sectors: usize,
    /// Total number of directory entries
    total_direntries: usize,
    /// Number of available dir entries
    available_direntries: usize,
    /// File system type (FA for SFAT)
    fs_type: String,
    /// Reserved, all set to 0
    reserved: [u8; 11],
    /// Not guaranteed to be NUL-terminated
    label: String,
}
impl SuperBlock {
    fn new() -> Self {
        SuperBlock {
            total_sectors: 0,
            sector_per_cluster: 8,
            bytes_per_sector: 512,
            available_sectors: 12800,
            total_direntries: 4,
            available_direntries: 4,
            fs_type: "FA".to_owned(),
            reserved: [0; 11],
            label: "VFS-3".to_owned(),
        }
    }
}

struct DirEntry {
    /// Not guaranteed to be NUL-terminated
    name: [u8; 10],
    /// First data block
    fat_entry: usize,
    /// Size of the file in bytes
    size: usize,
}

fn init_fs() {
    let super_block = SuperBlock::new();
}
