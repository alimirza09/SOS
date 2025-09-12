use alloc::string::{String, ToString};
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
            // High bytes
            self.sector_count_port.write((count >> 8) as u8);
            self.lba_low_port.write((lba >> 24) as u8);
            self.lba_mid_port.write((lba >> 32) as u8);
            self.lba_high_port.write((lba >> 40) as u8);

            // Low bytes
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
            // High bytes
            self.sector_count_port.write((count >> 8) as u8);
            self.lba_low_port.write((lba >> 24) as u8);
            self.lba_mid_port.write((lba >> 32) as u8);
            self.lba_high_port.write((lba >> 40) as u8);

            // Low bytes
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

            if i % 1000 == 0 {
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

        // Wait for BSY to clear and DRQ to set
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

            if i % 1000 == 0 {
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

            if i % 100 == 0 {
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

            if i % 1000 == 0 {
                crate::serial_println!("ATA: Waiting for ready, status: 0x{:02X}", status);
            }
        }

        crate::serial_println!("ATA: Timeout waiting for ready");
        Err(AtaError::Timeout)
    }

    pub fn diagnose(&mut self, device: AtaDevice) -> Result<(), AtaError> {
        crate::serial_println!("ATA: Diagnosing device {:?}", device);

        // Test basic communication
        self.select_device(device)?;
        self.wait_ready()?;

        // Read status to verify communication
        let status = unsafe { self.alt_status_port.read() };
        crate::serial_println!("ATA: Device status: 0x{:02X}", status);

        if status == 0xFF {
            return Err(AtaError::DeviceNotFound);
        }

        Ok(())
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

        // Check if device exists (word 0 != 0)
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

        let sector_size = 512; // Assume 512 for simplicity

        Self {
            model,
            serial,
            firmware,
            sectors,
            supports_lba48,
            sector_size,
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

// Helper functions for safe access
pub fn read_sectors(
    controller: bool,
    device: AtaDevice,
    lba: u64,
    count: u16,
    buffer: &mut [u8],
) -> Result<(), AtaError> {
    if controller {
        PRIMARY_ATA.lock().read_sectors(device, lba, count, buffer)
    } else {
        SECONDARY_ATA
            .lock()
            .read_sectors(device, lba, count, buffer)
    }
}

pub fn identify_drive(controller: bool, device: AtaDevice) -> Result<DriveInfo, AtaError> {
    if controller {
        PRIMARY_ATA.lock().identify(device)
    } else {
        SECONDARY_ATA.lock().identify(device)
    }
}
