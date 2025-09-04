#[allow(dead_code)]
use crate::alloc::{collections::BTreeMap, string::String, vec::Vec};

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

struct DirEntry {
    /// Not guaranteed to be NUL-terminated
    name: String,
    start_cluster: usize,
    /// Size of the file in bytes
    size: usize,
}

pub fn test_fs() {
    let mut fs = RamDiskFs::new();

    let big_data = alloc::vec![69u8; 5000];
    fs.create_file("big.bin", &big_data);

    if let Some(c) = fs.read_file("big.bin") {
        crate::println!("Read back {:?} bytes", c);
    }
}
