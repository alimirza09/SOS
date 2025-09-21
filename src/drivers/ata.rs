use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};

const ATA_CMD_READ_SECTORS: u8 = 0x20;
const ATA_CMD_READ_SECTORS_EXT: u8 = 0x24;
const ATA_CMD_WRITE_SECTORS: u8 = 0x30;
const ATA_CMD_WRITE_SECTORS_EXT: u8 = 0x34;
const ATA_CMD_FLUSH_CACHE: u8 = 0xE7;
const ATA_CMD_IDENTIFY: u8 = 0xEC;

const ATA_STATUS_BSY: u8 = 0x80;
const ATA_STATUS_DRQ: u8 = 0x08;
const ATA_STATUS_ERR: u8 = 0x01;
const ATA_STATUS_DF: u8 = 0x20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaDevice {
    Master = 0,
    Slave = 1,
}

#[derive(Debug, Clone, Copy)]
pub enum AtaError {
    Timeout,
    NotReady,
    Error(u8),
    DeviceNotFound,
    BufferTooSmall,
    InvalidSectorSize,
    UnsupportedOperation,
    InvalidLba,
    CommandFailed,
    DeviceFault,
}

impl core::fmt::Display for AtaError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            AtaError::Error(code) => write!(f, "ATA error: 0x{:02X}", code),
            AtaError::Timeout => write!(f, "ATA timeout"),
            AtaError::NotReady => write!(f, "ATA device not ready"),
            AtaError::DeviceNotFound => write!(f, "ATA device not found"),
            AtaError::BufferTooSmall => write!(f, "Buffer too small"),
            AtaError::InvalidSectorSize => write!(f, "Invalid sector size"),
            AtaError::UnsupportedOperation => write!(f, "Unsupported operation"),
            AtaError::InvalidLba => write!(f, "Invalid LBA"),
            AtaError::CommandFailed => write!(f, "Command failed"),
            AtaError::DeviceFault => write!(f, "Device fault"),
        }
    }
}

pub struct AtaController {
    pub data_port: Port<u16>,
    pub error_port: PortReadOnly<u8>,
    pub features_port: PortWriteOnly<u8>,
    pub sector_count_port: Port<u8>,
    pub lba_low_port: Port<u8>,
    pub lba_mid_port: Port<u8>,
    pub lba_high_port: Port<u8>,
    pub device_port: Port<u8>,
    pub status_port: PortReadOnly<u8>,
    pub command_port: PortWriteOnly<u8>,
    pub control_port: PortWriteOnly<u8>,
    pub alt_status_port: PortReadOnly<u8>,

    pub supports_lba48: [bool; 2],
    pub max_sectors: [u64; 2],
}

impl AtaController {
    pub const fn new(base: u16) -> Self {
        Self {
            data_port: Port::new(base),
            error_port: PortReadOnly::new(base + 1),
            features_port: PortWriteOnly::new(base + 1),
            sector_count_port: Port::new(base + 2),
            lba_low_port: Port::new(base + 3),
            lba_mid_port: Port::new(base + 4),
            lba_high_port: Port::new(base + 5),
            device_port: Port::new(base + 6),
            status_port: PortReadOnly::new(base + 7),
            command_port: PortWriteOnly::new(base + 7),
            control_port: PortWriteOnly::new(base + 0x206),
            alt_status_port: PortReadOnly::new(base + 0x206),
            supports_lba48: [false; 2],
            max_sectors: [0; 2],
        }
    }

    pub fn read_sectors(
        &mut self,
        device: AtaDevice,
        lba: u64,
        count: u16,
        buffer: &mut [u8],
    ) -> Result<(), AtaError> {
        if buffer.len() < (count as usize * 512) {
            return Err(AtaError::BufferTooSmall);
        }

        let device_idx = device as usize;
        crate::serial_println!("ATA: Reading {} sectors from LBA {}", count, lba);

        if lba > 0xFFFFFFF || count > 256 || self.supports_lba48[device_idx] {
            self.read_sectors_lba48(device, lba, count, buffer)
        } else {
            self.read_sectors_lba28(device, lba as u32, count as u8, buffer)
        }
    }

    fn read_sectors_lba48(
        &mut self,
        device: AtaDevice,
        lba: u64,
        count: u16,
        buffer: &mut [u8],
    ) -> Result<(), AtaError> {
        self.select_device(device)?;
        self.wait_ready()?;

        unsafe {
            self.sector_count_port.write((count >> 8) as u8);
            self.lba_low_port.write((lba >> 24) as u8);
            self.lba_mid_port.write((lba >> 32) as u8);
            self.lba_high_port.write((lba >> 40) as u8);

            self.sector_count_port.write(count as u8);
            self.lba_low_port.write(lba as u8);
            self.lba_mid_port.write((lba >> 8) as u8);
            self.lba_high_port.write((lba >> 16) as u8);

            self.device_port.write(0x40 | ((device as u8) << 4));
            self.command_port.write(ATA_CMD_READ_SECTORS_EXT);
        }

        self.read_data_sectors(count, buffer)
    }

    fn read_sectors_lba28(
        &mut self,
        device: AtaDevice,
        lba: u32,
        count: u8,
        buffer: &mut [u8],
    ) -> Result<(), AtaError> {
        self.select_device(device)?;
        self.wait_ready()?;

        unsafe {
            self.sector_count_port.write(count);
            self.lba_low_port.write(lba as u8);
            self.lba_mid_port.write((lba >> 8) as u8);
            self.lba_high_port.write((lba >> 16) as u8);
            self.device_port
                .write(0xE0 | ((device as u8) << 4) | ((lba >> 24) as u8 & 0x0F));
            self.command_port.write(ATA_CMD_READ_SECTORS);
        }

        self.read_data_sectors(count as u16, buffer)
    }

    fn read_data_sectors(&mut self, count: u16, buffer: &mut [u8]) -> Result<(), AtaError> {
        for sector in 0..count {
            self.wait_data_ready()?;

            let sector_start = sector as usize * 512;
            for i in (0..512).step_by(2) {
                let word = unsafe { self.data_port.read() };
                buffer[sector_start + i] = word as u8;
                buffer[sector_start + i + 1] = (word >> 8) as u8;
            }
        }
        Ok(())
    }

    pub fn write_sectors(
        &mut self,
        device: AtaDevice,
        lba: u64,
        buffer: &[u8],
    ) -> Result<(), AtaError> {
        if buffer.len() % 512 != 0 {
            return Err(AtaError::InvalidSectorSize);
        }

        let count = buffer.len() / 512;
        let device_idx = device as usize;

        crate::serial_println!("ATA: Writing {} sectors at LBA {}", count, lba);

        if lba > 0xFFFFFFF || count > 256 || self.supports_lba48[device_idx] {
            self.write_sectors_lba48(device, lba, count as u16, buffer)
        } else {
            self.write_sectors_lba28(device, lba as u32, count as u8, buffer)
        }
    }

    fn write_sectors_lba48(
        &mut self,
        device: AtaDevice,
        lba: u64,
        count: u16,
        buffer: &[u8],
    ) -> Result<(), AtaError> {
        self.select_device(device)?;
        self.wait_ready()?;

        unsafe {
            self.sector_count_port.write((count >> 8) as u8);
            self.lba_low_port.write((lba >> 24) as u8);
            self.lba_mid_port.write((lba >> 32) as u8);
            self.lba_high_port.write((lba >> 40) as u8);

            self.sector_count_port.write(count as u8);
            self.lba_low_port.write(lba as u8);
            self.lba_mid_port.write((lba >> 8) as u8);
            self.lba_high_port.write((lba >> 16) as u8);

            self.device_port.write(0x40 | ((device as u8) << 4));
            self.command_port.write(ATA_CMD_WRITE_SECTORS_EXT);
        }

        self.write_data_sectors(count, buffer)
    }

    fn write_sectors_lba28(
        &mut self,
        device: AtaDevice,
        lba: u32,
        count: u8,
        buffer: &[u8],
    ) -> Result<(), AtaError> {
        self.select_device(device)?;
        self.wait_ready()?;

        unsafe {
            self.sector_count_port.write(count);
            self.lba_low_port.write(lba as u8);
            self.lba_mid_port.write((lba >> 8) as u8);
            self.lba_high_port.write((lba >> 16) as u8);
            self.device_port
                .write(0xE0 | ((device as u8) << 4) | ((lba >> 24) as u8 & 0x0F));
            self.command_port.write(ATA_CMD_WRITE_SECTORS);
        }

        self.write_data_sectors(count as u16, buffer)
    }

    fn write_data_sectors(&mut self, count: u16, buffer: &[u8]) -> Result<(), AtaError> {
        for sector in 0..count {
            self.wait_data_ready()?;

            let sector_start = sector as usize * 512;
            for i in (0..512).step_by(2) {
                let word =
                    (buffer[sector_start + i + 1] as u16) << 8 | (buffer[sector_start + i] as u16);
                unsafe { self.data_port.write(word) };
            }
        }

        unsafe { self.command_port.write(ATA_CMD_FLUSH_CACHE) };
        self.wait_ready()?;

        Ok(())
    }

    fn wait_data_ready(&mut self) -> Result<(), AtaError> {
        for i in 0..10000 {
            let status = unsafe { self.alt_status_port.read() };

            if (status & ATA_STATUS_ERR) != 0 {
                let error = unsafe { self.error_port.read() };
                crate::serial_println!(
                    "ATA: Data ready error - status: 0x{:02X}, error: 0x{:02X}",
                    status,
                    error
                );
                return Err(AtaError::Error(error));
            }

            if (status & ATA_STATUS_DF) != 0 {
                crate::serial_println!("ATA: Device fault detected");
                return Err(AtaError::DeviceFault);
            }

            if (status & ATA_STATUS_DRQ) != 0 {
                return Ok(());
            }

            if i % 1000 == 0 && i > 0 {
                crate::serial_println!("ATA: Waiting for data ready, status: 0x{:02X}", status);
            }
        }

        crate::serial_println!("ATA: Timeout waiting for data ready");
        Err(AtaError::Timeout)
    }

    pub fn identify(&mut self, device: AtaDevice) -> Result<DriveInfo, AtaError> {
        crate::serial_println!("ATA: Starting IDENTIFY for device {:?}", device);

        self.disable_interrupts();
        self.select_device(device)?;
        self.wait_ready()?;

        unsafe {
            self.sector_count_port.write(0);
            self.lba_low_port.write(0);
            self.lba_mid_port.write(0);
            self.lba_high_port.write(0);
            self.device_port.write(0xA0 | ((device as u8) << 4));
            self.command_port.write(ATA_CMD_IDENTIFY);
        }

        for i in 0..10000 {
            let status = unsafe { self.alt_status_port.read() };

            if status == 0xFF {
                return Err(AtaError::DeviceNotFound);
            }

            if (status & ATA_STATUS_ERR) != 0 {
                let error = unsafe { self.error_port.read() };
                crate::serial_println!(
                    "ATA: IDENTIFY error - status: 0x{:02X}, error: 0x{:02X}",
                    status,
                    error
                );
                return Err(AtaError::Error(error));
            }

            if (status & ATA_STATUS_DF) != 0 {
                crate::serial_println!("ATA: Device fault during IDENTIFY");
                return Err(AtaError::DeviceFault);
            }

            if (status & ATA_STATUS_DRQ) != 0 {
                break;
            }

            if i % 1000 == 0 && i > 0 {
                crate::serial_println!("ATA: Waiting for IDENTIFY data, status: 0x{:02X}", status);
            }
        }

        let mut data = [0u16; 256];
        for word in &mut data {
            *word = unsafe { self.data_port.read() };
        }

        crate::serial_println!("ATA: IDENTIFY completed successfully");
        let info = DriveInfo::from_identify_data(&data);

        let device_idx = device as usize;
        self.supports_lba48[device_idx] = info.supports_lba48;
        self.max_sectors[device_idx] = info.sectors;

        Ok(info)
    }

    fn disable_interrupts(&mut self) {
        unsafe {
            self.control_port.write(0x02);
        }
    }

    fn delay_400ns(&mut self) {
        for _ in 0..4 {
            unsafe {
                self.alt_status_port.read();
            }
        }
    }

    fn select_device(&mut self, device: AtaDevice) -> Result<(), AtaError> {
        let value = 0xA0 | ((device as u8) << 4);
        unsafe {
            self.device_port.write(value);
        }
        self.delay_400ns();

        for i in 0..1000 {
            let status = unsafe { self.alt_status_port.read() };
            if status != 0xFF && (status & ATA_STATUS_BSY) == 0 {
                return Ok(());
            }

            if i % 100 == 0 && i > 0 {
                crate::serial_println!("ATA: Selecting device, status: 0x{:02X}", status);
            }
        }

        crate::serial_println!("ATA: Device selection timeout");
        Err(AtaError::DeviceNotFound)
    }

    fn wait_ready(&mut self) -> Result<(), AtaError> {
        self.delay_400ns();

        for i in 0..10000 {
            let status = unsafe { self.alt_status_port.read() };

            if status == 0xFF {
                return Err(AtaError::DeviceNotFound);
            }

            if (status & ATA_STATUS_ERR) != 0 {
                let error = unsafe { self.error_port.read() };
                crate::serial_println!(
                    "ATA: Ready error - status: 0x{:02X}, error: 0x{:02X}",
                    status,
                    error
                );
                return Err(AtaError::Error(error));
            }

            if (status & ATA_STATUS_DF) != 0 {
                crate::serial_println!("ATA: Device fault");
                return Err(AtaError::DeviceFault);
            }

            if (status & ATA_STATUS_BSY) == 0 {
                return Ok(());
            }

            if i % 1000 == 0 && i > 0 {
                crate::serial_println!("ATA: Waiting for ready, status: 0x{:02X}", status);
            }
        }

        crate::serial_println!("ATA: Timeout waiting for ready");
        Err(AtaError::Timeout)
    }
}

pub struct DriveInfo {
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub sectors: u64,
    pub supports_lba48: bool,
    pub sector_size: u16,
}

impl DriveInfo {
    pub fn from_identify_data(data: &[u16; 256]) -> Self {
        let model = extract_string(data, 27, 20);
        let serial = extract_string(data, 10, 10);
        let firmware = extract_string(data, 23, 4);

        if data[0] == 0 {
            return Self {
                model: "No Device".to_string(),
                serial: "".to_string(),
                firmware: "".to_string(),
                sectors: 0,
                supports_lba48: false,
                sector_size: 512,
            };
        }

        let lba_supported = (data[49] & (1 << 9)) != 0;

        if !lba_supported {
            let cylinders = data[1] as u32;
            let heads = data[3] as u32;
            let sectors_per_track = data[6] as u32;
            let total_sectors = cylinders * heads * sectors_per_track;

            return Self {
                model,
                serial,
                firmware,
                sectors: total_sectors as u64,
                supports_lba48: false,
                sector_size: 512,
            };
        }

        let lba28_sectors = ((data[61] as u32) << 16) | (data[60] as u32);
        let supports_lba48 = (data[83] & (1 << 10)) != 0;

        let sectors = if supports_lba48 {
            let lba48_sectors = ((data[103] as u64) << 48)
                | ((data[102] as u64) << 32)
                | ((data[101] as u64) << 16)
                | (data[100] as u64);
            lba48_sectors
        } else {
            lba28_sectors as u64
        };

        Self {
            model,
            serial,
            firmware,
            sectors,
            supports_lba48,
            sector_size: 512,
        }
    }

    pub fn capacity_mb(&self) -> u64 {
        (self.sectors * self.sector_size as u64) / (1024 * 1024)
    }

    pub fn capacity_gb(&self) -> u64 {
        self.capacity_mb() / 1024
    }
}

fn extract_string(data: &[u16; 256], start_word: usize, word_count: usize) -> String {
    let mut result = String::new();

    for i in 0..word_count {
        if start_word + i >= 256 {
            break;
        }

        let word = data[start_word + i];
        let bytes = [(word >> 8) as u8, (word & 0xFF) as u8];

        for &byte in &bytes {
            if byte == 0 {
                break;
            }
            if byte >= 0x20 && byte <= 0x7E {
                result.push(byte as char);
            }
        }
    }

    result.trim().to_string()
}

pub static PRIMARY_ATA: Mutex<AtaController> = Mutex::new(AtaController::new(0x1F0));
pub static SECONDARY_ATA: Mutex<AtaController> = Mutex::new(AtaController::new(0x170));

fn with_controller<F, R>(primary: bool, f: F) -> R
where
    F: FnOnce(&mut AtaController) -> R,
{
    if primary {
        f(&mut PRIMARY_ATA.lock())
    } else {
        f(&mut SECONDARY_ATA.lock())
    }
}

pub fn read_sectors(
    primary: bool,
    device: AtaDevice,
    lba: u64,
    count: u16,
    buffer: &mut [u8],
) -> Result<(), AtaError> {
    with_controller(primary, |controller| {
        controller.read_sectors(device, lba, count, buffer)
    })
}

pub fn write_sectors(
    primary: bool,
    device: AtaDevice,
    lba: u64,
    buffer: &[u8],
) -> Result<(), AtaError> {
    with_controller(primary, |controller| {
        controller.write_sectors(device, lba, buffer)
    })
}

pub fn identify_drive(primary: bool, device: AtaDevice) -> Result<DriveInfo, AtaError> {
    with_controller(primary, |controller| controller.identify(device))
}

use crate::alloc::{collections::BTreeMap, vec};

#[allow(dead_code)]
struct SuperBlock {
    bytes_per_sector: usize,
    sectors_per_cluster: usize,
    total_sectors: u64,
    fs_type: String,
    label: String,
    start_lba: u64,
}

impl SuperBlock {
    fn new(start_lba: u64, total_sectors: u64) -> Self {
        Self {
            bytes_per_sector: 512,
            sectors_per_cluster: 8,
            total_sectors,
            fs_type: "ATA_FS".into(),
            label: "ATADISK".into(),
            start_lba,
        }
    }

    fn cluster_size(&self) -> usize {
        self.bytes_per_sector * self.sectors_per_cluster
    }

    fn sectors_per_cluster(&self) -> u16 {
        self.sectors_per_cluster as u16
    }
}

#[derive(Debug, Clone)]
struct DirEntry {
    name: String,
    start_cluster: u64,
    size: usize,
    is_directory: bool,
}

pub struct AtaFileSystem {
    controller: bool,
    device: AtaDevice,
    superblock: SuperBlock,
    directory: BTreeMap<String, DirEntry>,
    fat: BTreeMap<u64, Option<u64>>,
    next_free_cluster: u64,
}

impl AtaFileSystem {
    pub fn new(
        controller: bool,
        device: AtaDevice,
        start_lba: u64,
        size_sectors: u64,
    ) -> Result<Self, AtaError> {
        crate::serial_println!(
            "ATA FS: Initializing filesystem at LBA {} with {} sectors",
            start_lba,
            size_sectors
        );

        let drive_info = identify_drive(controller, device)?;
        if start_lba + size_sectors > drive_info.sectors {
            crate::serial_println!("ATA FS: Error - filesystem range exceeds drive capacity");
            return Err(AtaError::InvalidLba);
        }

        let superblock = SuperBlock::new(start_lba, size_sectors);

        let mut fs = Self {
            controller,
            device,
            superblock,
            directory: BTreeMap::new(),
            fat: BTreeMap::new(),
            next_free_cluster: 1,
        };

        crate::serial_println!("ATA FS: Checking for existing filesystem...");
        match fs.load_superblock() {
            Ok(_) => {
                crate::serial_println!("ATA FS: Found existing filesystem, loading...");
                fs.load_directory()?;
                fs.load_fat()?;
            }
            Err(_) => {
                crate::serial_println!("ATA FS: Creating new filesystem...");
                fs.format()?;
            }
        }

        Ok(fs)
    }

    pub fn format(&mut self) -> Result<(), AtaError> {
        crate::serial_println!("ATA FS: Formatting filesystem...");

        self.directory.clear();
        self.fat.clear();
        self.next_free_cluster = 1;

        self.write_superblock()?;
        self.write_directory()?;
        self.write_fat()?;

        crate::serial_println!("ATA FS: Format complete");
        Ok(())
    }

    fn cluster_to_lba(&self, cluster: u64) -> u64 {
        self.superblock.start_lba + cluster * self.superblock.sectors_per_cluster as u64
    }

    fn allocate_cluster(&mut self) -> u64 {
        let cluster = self.next_free_cluster;
        self.next_free_cluster += 1;
        self.fat.insert(cluster, None);
        cluster
    }

    pub fn create_file(&mut self, name: &str, data: &[u8]) -> Result<(), AtaError> {
        if self.directory.contains_key(name) {
            return Err(AtaError::CommandFailed);
        }

        crate::serial_println!("ATA FS: Creating file '{}' ({} bytes)", name, data.len());

        let cluster_size = self.superblock.cluster_size();
        let mut clusters = Vec::new();

        for (i, chunk) in data.chunks(cluster_size).enumerate() {
            let cluster = self.allocate_cluster();
            clusters.push(cluster);

            let mut buffer = vec![0u8; cluster_size];
            buffer[..chunk.len()].copy_from_slice(chunk);

            let lba = self.cluster_to_lba(cluster);
            write_sectors(self.controller, self.device, lba, &buffer)?;

            crate::serial_println!(
                "ATA FS: Wrote chunk {} to cluster {} (LBA {})",
                i,
                cluster,
                lba
            );
        }

        for i in 0..clusters.len() {
            let next_cluster = if i + 1 < clusters.len() {
                Some(clusters[i + 1])
            } else {
                None
            };
            self.fat.insert(clusters[i], next_cluster);
        }

        let first_cluster = clusters.first().copied().unwrap_or(0);
        self.directory.insert(
            name.to_string(),
            DirEntry {
                name: name.to_string(),
                start_cluster: first_cluster,
                size: data.len(),
                is_directory: false,
            },
        );

        self.write_directory()?;
        self.write_fat()?;

        crate::serial_println!("ATA FS: File '{}' created successfully", name);
        Ok(())
    }

    pub fn read_file(&self, name: &str) -> Result<Vec<u8>, AtaError> {
        let entry = self.directory.get(name).ok_or(AtaError::DeviceNotFound)?;

        crate::serial_println!("ATA FS: Reading file '{}' ({} bytes)", name, entry.size);

        let mut data = Vec::with_capacity(entry.size);
        let mut current_cluster = Some(entry.start_cluster);
        let cluster_size = self.superblock.cluster_size();

        while let Some(cluster) = current_cluster {
            let lba = self.cluster_to_lba(cluster);
            let mut buffer = vec![0u8; cluster_size];

            read_sectors(
                self.controller,
                self.device,
                lba,
                self.superblock.sectors_per_cluster(),
                &mut buffer,
            )?;

            let remaining_bytes = entry.size - data.len();
            let bytes_to_copy = remaining_bytes.min(cluster_size);

            data.extend_from_slice(&buffer[..bytes_to_copy]);

            current_cluster = self.fat.get(&cluster).and_then(|&next| next);

            if data.len() >= entry.size {
                break;
            }
        }

        crate::serial_println!("ATA FS: Successfully read {} bytes", data.len());
        Ok(data)
    }

    pub fn list_files(&self) -> Vec<(String, usize, bool)> {
        self.directory
            .iter()
            .map(|(name, entry)| (name.clone(), entry.size, entry.is_directory))
            .collect()
    }

    pub fn delete_file(&mut self, name: &str) -> Result<(), AtaError> {
        let entry = self
            .directory
            .remove(name)
            .ok_or(AtaError::DeviceNotFound)?;

        crate::serial_println!("ATA FS: Deleting file '{}'", name);

        let mut current_cluster = Some(entry.start_cluster);
        while let Some(cluster) = current_cluster {
            let next = self.fat.remove(&cluster).flatten();
            current_cluster = next;
        }

        self.write_directory()?;
        self.write_fat()?;

        crate::serial_println!("ATA FS: File '{}' deleted successfully", name);
        Ok(())
    }

    fn load_superblock(&mut self) -> Result<(), AtaError> {
        crate::serial_println!(
            "ATA FS: Reading superblock from LBA {}",
            self.superblock.start_lba
        );

        let mut buffer = [0u8; 512];
        read_sectors(
            self.controller,
            self.device,
            self.superblock.start_lba,
            1,
            &mut buffer,
        )?;

        let signature = &buffer[0..6];
        if signature == b"ATA_FS" {
            crate::serial_println!("ATA FS: Found valid filesystem signature");
            Ok(())
        } else {
            crate::serial_println!("ATA FS: No valid filesystem signature found");
            Err(AtaError::DeviceNotFound)
        }
    }

    fn write_superblock(&self) -> Result<(), AtaError> {
        let mut buffer = [0u8; 512];
        buffer[0..6].copy_from_slice(b"ATA_FS");

        write_sectors(
            self.controller,
            self.device,
            self.superblock.start_lba,
            &buffer,
        )
    }

    fn load_directory(&mut self) -> Result<(), AtaError> {
        Ok(())
    }

    fn write_directory(&self) -> Result<(), AtaError> {
        Ok(())
    }

    fn load_fat(&mut self) -> Result<(), AtaError> {
        Ok(())
    }

    fn write_fat(&self) -> Result<(), AtaError> {
        Ok(())
    }
}

pub static GLOBAL_FS: Mutex<Option<AtaFileSystem>> = Mutex::new(None);

pub fn init_global_filesystem() -> Result<(), AtaError> {
    let drive_info = identify_drive(true, AtaDevice::Slave)?;
    crate::serial_println!("Drive capacity: {} sectors", drive_info.sectors);

    let start_lba = if drive_info.sectors > 200 {
        100
    } else {
        return Err(AtaError::InvalidLba);
    };

    let filesystem_size = if drive_info.sectors > 1000 {
        500
    } else {
        drive_info.sectors / 2
    };

    crate::serial_println!(
        "Creating filesystem at LBA {} with {} sectors",
        start_lba,
        filesystem_size
    );

    let fs = AtaFileSystem::new(true, AtaDevice::Slave, start_lba, filesystem_size)?;
    *GLOBAL_FS.lock() = Some(fs);
    crate::serial_println!("Global ATA filesystem initialized successfully");
    Ok(())
}

pub fn fs_create_file(filename: &str, data: &[u8]) -> Result<(), AtaError> {
    let mut fs_guard = GLOBAL_FS.lock();
    let fs = fs_guard.as_mut().ok_or(AtaError::DeviceNotFound)?;
    fs.create_file(filename, data)
}

pub fn fs_read_file(filename: &str) -> Result<Vec<u8>, AtaError> {
    let fs_guard = GLOBAL_FS.lock();
    let fs = fs_guard.as_ref().ok_or(AtaError::DeviceNotFound)?;
    fs.read_file(filename)
}

pub fn fs_delete_file(filename: &str) -> Result<(), AtaError> {
    let mut fs_guard = GLOBAL_FS.lock();
    let fs = fs_guard.as_mut().ok_or(AtaError::DeviceNotFound)?;
    fs.delete_file(filename)
}

pub fn fs_list_files() -> Result<Vec<(String, usize, bool)>, AtaError> {
    let fs_guard = GLOBAL_FS.lock();
    let fs = fs_guard.as_ref().ok_or(AtaError::DeviceNotFound)?;
    Ok(fs.list_files())
}

pub fn test_ata_driver_comprehensive() {
    crate::serial_println!("=== COMPREHENSIVE ATA DRIVER TEST START ===");

    let devices_to_test = [
        ("Primary Master", AtaDevice::Master, true),
        ("Primary Slave", AtaDevice::Slave, true),
        ("Secondary Master", AtaDevice::Master, false),
        ("Secondary Slave", AtaDevice::Slave, false),
    ];

    let mut found_devices = 0;

    for (name, device, use_primary) in devices_to_test.iter() {
        crate::serial_println!("Checking {}...", name);

        match identify_drive(*use_primary, *device) {
            Ok(info) => {
                found_devices += 1;
                crate::serial_println!("{} found:", name);
                crate::serial_println!("  Model: {}", info.model);
                crate::serial_println!("  Serial: {}", info.serial);
                crate::serial_println!("  Firmware: {}", info.firmware);
                crate::serial_println!("  Sectors: {}", info.sectors);
                crate::serial_println!(
                    "  Capacity: {} MB ({} GB)",
                    info.capacity_mb(),
                    info.capacity_gb()
                );
                crate::serial_println!("  LBA48 Support: {}", info.supports_lba48);
                crate::serial_println!("  Sector Size: {} bytes", info.sector_size);

                if info.sectors > 0 {
                    test_read_sectors(name, *use_primary, *device);
                }
            }
            Err(e) => {
                crate::serial_println!("{} error: {:?}", name, e);
            }
        }
    }

    crate::serial_println!("Found {} ATA devices total", found_devices);
    crate::serial_println!("=== COMPREHENSIVE ATA DRIVER TEST COMPLETE ===");
}

fn test_read_sectors(name: &str, primary: bool, device: AtaDevice) {
    let mut buffer = [0u8; 512];

    match read_sectors(primary, device, 0, 1, &mut buffer) {
        Ok(()) => {
            crate::serial_println!("Successfully read sector 0 from {}", name);

            if buffer[510] == 0x55 && buffer[511] == 0xAA {
                crate::serial_println!("Valid MBR signature found");
            } else {
                crate::serial_println!("MBR: Invalid or missing signature");
            }

            crate::serial_print!("First 32 bytes of sector 0: ");
            for i in 0..32 {
                if i % 16 == 0 {
                    crate::serial_println!();
                    crate::serial_print!("{:04X}: ", i);
                }
                crate::serial_print!("{:02X} ", buffer[i]);
            }
            crate::serial_println!();

            crate::serial_print!("As ASCII: ");
            for i in 0..32 {
                let c = buffer[i];
                if c >= 0x20 && c <= 0x7E {
                    crate::serial_print!("{}", c as char);
                } else {
                    crate::serial_print!(".");
                }
            }
            crate::serial_println!();
        }
        Err(e) => {
            crate::serial_println!("Error reading sector from {}: {:?}", name, e);
        }
    }
}

pub fn test_disk_identification() -> Result<(), AtaError> {
    crate::serial_println!("=== DISK IDENTIFICATION TEST ===");

    let mut sector_0 = [0u8; 512];
    read_sectors(true, AtaDevice::Slave, 0, 1, &mut sector_0)?;

    crate::serial_println!("First 64 bytes of sector 0 (Primary Slave):");
    for i in (0..64).step_by(16) {
        crate::serial_print!("{:04X}: ", i);
        for j in 0..16 {
            crate::serial_print!("{:02X} ", sector_0[i + j]);
        }
        crate::serial_print!(" ");
        for j in 0..16 {
            let c = sector_0[i + j];
            if c >= 0x20 && c <= 0x7E {
                crate::serial_print!("{}", c as char);
            } else {
                crate::serial_print!(".");
            }
        }
        crate::serial_println!();
    }

    if sector_0[510] == 0x55 && sector_0[511] == 0xAA {
        crate::serial_println!("Found MBR signature - this looks like a boot disk");
    } else {
        crate::serial_println!("No MBR signature - this looks like a data disk");
    }

    Ok(())
}
