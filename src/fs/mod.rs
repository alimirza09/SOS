#[allow(dead_code)]
use crate::alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};
use crate::ata::{AtaError, read_sector, write_sector};

struct SuperBlock {
    bytes_per_sector: usize,
    sectors_per_cluster: usize,
    total_sectors: usize,
    fs_type: String,
    label: String,
}

impl SuperBlock {
    fn new() -> Self {
        Self {
            bytes_per_sector: 512,
            sectors_per_cluster: 8,
            total_sectors: 1024, // fake disk = 1024 * 512 = 512 KiB
            fs_type: "FA".into(),
            label: "RAMDISK".into(),
        }
    }

    fn cluster_size(&self) -> usize {
        self.bytes_per_sector * self.sectors_per_cluster
    }
}

struct DirEntry {
    /// Not guaranteed to be NUL-terminated
    name: String,
    start_cluster: usize,
    /// Size of the file in bytes
    size: usize,
}

pub struct RamDiskFs {
    superblock: SuperBlock,
    disk: Vec<u8>,
    dir: BTreeMap<String, DirEntry>,
    fat: Vec<Option<usize>>,
    next_free_cluster: usize,
}

impl RamDiskFs {
    fn new() -> Self {
        let superblock = SuperBlock::new();
        let disk_size = superblock.total_sectors * superblock.bytes_per_sector;

        let cluster_count = superblock.total_sectors;
        Self {
            superblock: superblock,
            disk: alloc::vec![0; disk_size],
            dir: BTreeMap::new(),
            fat: crate::alloc::vec![None; cluster_count],
            next_free_cluster: 1, // cluster 0 reserved
        }
    }

    fn alloc_cluster(&mut self) -> usize {
        let c = self.next_free_cluster;
        self.next_free_cluster += 1;
        c
    }

    fn cluster_offset(&self, cluster: usize) -> usize {
        cluster * self.superblock.cluster_size()
    }

    fn create_file(&mut self, name: &str, data: &[u8]) {
        let cluster_size = self.superblock.cluster_size();
        let mut prev: Option<usize> = None;
        let mut first_cluster = None;

        for chunk in data.chunks(cluster_size) {
            let cluster = self.alloc_cluster();
            let offset = self.cluster_offset(cluster);

            // copy chunk into disk
            self.disk[offset..offset + chunk.len()].copy_from_slice(chunk);

            // link FAT
            if let Some(p) = prev {
                self.fat[p] = Some(cluster);
            } else {
                first_cluster = Some(cluster);
            }

            prev = Some(cluster);
        }

        // store dir entry
        self.dir.insert(
            name.into(),
            DirEntry {
                name: name.into(),
                start_cluster: first_cluster.unwrap(),
                size: data.len(),
            },
        );
    }

    fn read_file(&self, name: &str) -> Option<Vec<u8>> {
        let entry = self.dir.get(name)?;
        let mut data = Vec::with_capacity(entry.size);

        let mut cluster = Some(entry.start_cluster);
        while let Some(c) = cluster {
            let offset = self.cluster_offset(c);
            let next = self.fat[c]; // follow chain

            if let Some(n) = next {
                // copy full cluster
                data.extend_from_slice(&self.disk[offset..offset + self.superblock.cluster_size()]);
                cluster = Some(n);
            } else {
                // last cluster → only copy remaining bytes
                let remain = entry.size - data.len();
                data.extend_from_slice(&self.disk[offset..offset + remain]);
                break;
            }
        }

        Some(data)
    }
}

// ATA-backed filesystem
pub struct AtaFilesystem {
    device_id: usize,
    fat: Vec<Option<u32>>,
    directory: BTreeMap<String, AtaDirEntry>,
    next_free_sector: u32,
}

struct AtaDirEntry {
    name: String,
    start_sector: u32,
    size: u32,
}

impl AtaFilesystem {
    pub fn new(device_id: usize) -> Result<Self, AtaError> {
        // Initialize a simple filesystem on the ATA device
        let mut fs = AtaFilesystem {
            device_id,
            fat: Vec::new(),
            directory: BTreeMap::new(),
            next_free_sector: 100, // Start after reserved boot sectors
        };

        // Try to load existing filesystem or create a new one
        match fs.load_metadata() {
            Ok(()) => {
                crate::println!("Loaded existing filesystem from ATA device {}", device_id);
            }
            Err(_) => {
                crate::println!("Creating new filesystem on ATA device {}", device_id);
                fs.format()?;
            }
        }

        Ok(fs)
    }

    fn load_metadata(&mut self) -> Result<(), AtaError> {
        // Try to load filesystem metadata from sector 1
        let mut buffer = [0u8; 512];
        read_sector(self.device_id, 1, &mut buffer)?;

        // Check for our filesystem signature
        if &buffer[0..8] != b"SOS_FS01" {
            return Err(AtaError::BadCommand);
        }

        // For simplicity, we'll just mark the filesystem as valid
        // In a real implementation, you'd parse the metadata here
        Ok(())
    }

    fn format(&mut self) -> Result<(), AtaError> {
        // Create filesystem metadata in sector 1
        let mut buffer = [0u8; 512];
        buffer[0..8].copy_from_slice(b"SOS_FS01");

        // Write next free sector
        buffer[8..12].copy_from_slice(&self.next_free_sector.to_le_bytes());

        write_sector(self.device_id, 1, &buffer)?;
        crate::println!(
            "Formatted ATA device {} with SOS filesystem",
            self.device_id
        );
        Ok(())
    }

    pub fn create_file(&mut self, name: &str, data: &[u8]) -> Result<(), AtaError> {
        let sectors_needed = (data.len() + 511) / 512; // Round up to sectors
        let start_sector = self.next_free_sector;

        // Write data to sectors
        for (i, chunk) in data.chunks(512).enumerate() {
            let mut sector_buffer = [0u8; 512];
            sector_buffer[..chunk.len()].copy_from_slice(chunk);
            write_sector(self.device_id, start_sector + i as u32, &sector_buffer)?;
        }

        // Update directory
        self.directory.insert(
            name.to_string(),
            AtaDirEntry {
                name: name.to_string(),
                start_sector,
                size: data.len() as u32,
            },
        );

        self.next_free_sector += sectors_needed as u32;
        self.save_metadata()?;

        crate::println!("Created file '{}' with {} bytes", name, data.len());
        Ok(())
    }

    pub fn read_file(&self, name: &str) -> Result<Vec<u8>, AtaError> {
        let entry = self.directory.get(name).ok_or(AtaError::SectorNotFound)?;

        let sectors_needed = (entry.size + 511) / 512;
        let mut data = Vec::with_capacity(entry.size as usize);

        for i in 0..sectors_needed {
            let mut sector_buffer = [0u8; 512];
            read_sector(self.device_id, entry.start_sector + i, &mut sector_buffer)?;

            if i == sectors_needed - 1 {
                // Last sector - only copy remaining bytes
                let remaining = entry.size as usize - data.len();
                data.extend_from_slice(&sector_buffer[..remaining]);
            } else {
                data.extend_from_slice(&sector_buffer);
            }
        }

        Ok(data)
    }

    fn save_metadata(&self) -> Result<(), AtaError> {
        let mut buffer = [0u8; 512];
        buffer[0..8].copy_from_slice(b"SOS_FS01");
        buffer[8..12].copy_from_slice(&self.next_free_sector.to_le_bytes());

        write_sector(self.device_id, 1, &buffer)
    }

    pub fn list_files(&self) -> Vec<(String, u32)> {
        self.directory
            .iter()
            .map(|(name, entry)| (name.clone(), entry.size))
            .collect()
    }

    pub fn delete_file(&mut self, name: &str) -> Result<(), AtaError> {
        if self.directory.remove(name).is_some() {
            self.save_metadata()?;
            crate::println!("Deleted file '{}'", name);
            Ok(())
        } else {
            Err(AtaError::SectorNotFound)
        }
    }
}

pub fn test_fs() {
    let mut fs = RamDiskFs::new();

    let big_data = alloc::vec![69u8; 5000];
    fs.create_file("big.bin", &big_data);

    if let Some(c) = fs.read_file("big.bin") {
        crate::println!("Read back {} bytes from RAM filesystem", c.len());
    }
}

pub fn test_ata_filesystem() {
    crate::println!("Testing ATA filesystem...");

    match AtaFilesystem::new(0) {
        Ok(mut ata_fs) => {
            // Test creating a file
            let test_data = b"Hello, ATA filesystem! This is a test file.";
            match ata_fs.create_file("test.txt", test_data) {
                Ok(()) => {
                    crate::println!("Successfully created test.txt");

                    // Test reading the file back
                    match ata_fs.read_file("test.txt") {
                        Ok(data) => {
                            if data == test_data {
                                crate::println!("File read/write test PASSED!");
                            } else {
                                crate::println!("File read/write test FAILED - data mismatch");
                            }
                        }
                        Err(e) => {
                            crate::println!("Failed to read file: {:?}", e);
                        }
                    }

                    // List files
                    let files = ata_fs.list_files();
                    crate::println!("Files in filesystem:");
                    for (name, size) in files {
                        crate::println!("  {} ({} bytes)", name, size);
                    }
                }
                Err(e) => {
                    crate::println!("Failed to create file: {:?}", e);
                }
            }
        }
        Err(e) => {
            crate::println!("Failed to initialize ATA filesystem: {:?}", e);
        }
    }
}
