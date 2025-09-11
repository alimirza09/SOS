use alloc::string::{String, ToString};
use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};

const ATA_CMD_READ_SECTORS: u8 = 0x20;
const ATA_CMD_READ_SECTORS_EXT: u8 = 0x24;
const ATA_CMD_WRITE_SECTORS: u8 = 0x30;
const ATA_CMD_WRITE_SECTORS_EXT: u8 = 0x34;
const ATA_CMD_FLUSH_CACHE: u8 = 0xE7;

const ATA_STATUS_DRQ: u8 = 0x08;
const ATA_STATUS_ERR: u8 = 0x01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaDevice {
    Master = 0,
    Slave = 1,
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
            self.sector_count_port.write((count >> 8) as u8);
            self.lba_low_port.write((lba >> 24) as u8);
            self.lba_mid_port.write((lba >> 32) as u8);
            self.lba_high_port.write((lba >> 40) as u8);

            self.sector_count_port.write(count as u8);
            self.lba_low_port.write(lba as u8);
            self.lba_mid_port.write((lba >> 8) as u8);
            self.lba_high_port.write((lba >> 16) as u8);

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
        for _ in 0..10000 {
            let status = unsafe { self.alt_status_port.read() };

            if (status & ATA_STATUS_ERR) != 0 {
                let error = unsafe { self.error_port.read() };
                return Err(AtaError::Error(error));
            }

            if (status & ATA_STATUS_DRQ) != 0 {
                return Ok(());
            }
        }
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
            self.command_port.write(0xEC);
        }

        for _ in 0..10000 {
            let status = unsafe { self.alt_status_port.read() };

            if status == 0 {
                return Err(AtaError::NotReady);
            }

            if (status & 0x01) != 0 {
                let error = unsafe { self.error_port.read() };
                return Err(AtaError::Error(error));
            }

            if (status & 0x08) != 0 {
                break;
            }
        }

        let mut data = [0u16; 256];
        for word in &mut data {
            *word = unsafe { self.data_port.read() };
        }

        crate::serial_println!("ATA: IDENTIFY completed successfully");
        let identify_data = data;
        let info = DriveInfo::from_identify_data(&identify_data);

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

        for _ in 0..1000 {
            let status = unsafe { self.alt_status_port.read() };
            if status != 0xFF && (status & 0x80) == 0 {
                return Ok(());
            }
        }
        Err(AtaError::DeviceNotFound)
    }

    fn wait_ready(&mut self) -> Result<(), AtaError> {
        self.delay_400ns();

        for _ in 0..10000 {
            let status = unsafe { self.alt_status_port.read() };

            if status == 0xFF {
                return Err(AtaError::DeviceNotFound);
            }

            if (status & 0x01) != 0 {
                let error = unsafe { self.error_port.read() };
                return Err(AtaError::Error(error));
            }

            if (status & 0x80) == 0 {
                return Ok(());
            }
        }
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

        crate::serial_println!(
            "ATA Debug: Word 60 = 0x{:04X}, Word 61 = 0x{:04X}",
            data[60],
            data[61]
        );
        crate::serial_println!(
            "ATA Debug: Word 83 = 0x{:04X} (LBA48 support check)",
            data[83]
        );
        crate::serial_println!(
            "ATA Debug: Words 100-103 = 0x{:04X} 0x{:04X} 0x{:04X} 0x{:04X}",
            data[100],
            data[101],
            data[102],
            data[103]
        );

        let lba_supported = (data[49] & (1 << 9)) != 0;
        crate::serial_println!("ATA Debug: LBA supported: {}", lba_supported);

        if !lba_supported {
            crate::serial_println!("ATA Debug: Using CHS mode (legacy)");

            let cylinders = data[1] as u32;
            let heads = data[3] as u32;
            let sectors_per_track = data[6] as u32;
            let total_sectors = cylinders * heads * sectors_per_track;
            crate::serial_println!(
                "ATA Debug: CHS: C={} H={} S={} = {} sectors",
                cylinders,
                heads,
                sectors_per_track,
                total_sectors
            );

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
        crate::serial_println!("ATA Debug: LBA28 sectors: {}", lba28_sectors);

        let supports_lba48 = (data[83] & (1 << 10)) != 0;
        crate::serial_println!("ATA Debug: LBA48 supported: {}", supports_lba48);

        let sectors = if supports_lba48 {
            let lba48_sectors = ((data[103] as u64) << 48)
                | ((data[102] as u64) << 32)
                | ((data[101] as u64) << 16)
                | (data[100] as u64);
            crate::serial_println!("ATA Debug: LBA48 sectors: {}", lba48_sectors);

            if lba48_sectors > lba28_sectors as u64 {
                lba48_sectors
            } else {
                lba28_sectors as u64
            }
        } else {
            lba28_sectors as u64
        };

        crate::serial_println!("ATA Debug: Final sector count: {}", sectors);

        let sector_size = if (data[106] & (1 << 12)) != 0 {
            if (data[106] & (1 << 13)) == 0 {
                ((data[118] as u32) << 16 | data[117] as u32) as u16 * 2
            } else {
                512
            }
        } else {
            512
        };

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

#[derive(Debug, Clone, Copy)]
pub enum AtaError {
    Timeout,
    NotReady,
    Error(u8),
    DeviceNotFound,
    BufferTooSmall,
    InvalidSectorSize,
    UnsupportedOperation,
}

pub static mut PRIMARY_ATA: AtaController = AtaController::new(0x1F0);
pub static mut SECONDARY_ATA: AtaController = AtaController::new(0x170);
