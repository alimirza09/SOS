use alloc::string::{String, ToString};
use spin::Mutex;
use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};

#[derive(Debug, Clone, Copy)]
pub enum AtaError {
    Timeout,
    NotReady,
    Error(u8),
    DeviceNotFound,
}

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
        }
    }

    fn enable_interrupts(&mut self) {
        unsafe {
            self.control_port.write(0x00);
        }
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

    pub fn identify(&mut self, device: AtaDevice) -> Result<[u16; 256], AtaError> {
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
        Ok(data)
    }

    pub fn read_sectors(
        &mut self,
        device: AtaDevice,
        lba: u32,
        count: u8,
        buffer: &mut [u8],
    ) -> Result<(), AtaError> {
        if count == 0 || buffer.len() < (count as usize * 512) {
            return Err(AtaError::Error(0));
        }

        self.enable_interrupts();

        self.select_device(device)?;
        self.wait_ready()?;

        unsafe {
            self.features_port.write(0);
            self.sector_count_port.write(count);
            self.lba_low_port.write(lba as u8);
            self.lba_mid_port.write((lba >> 8) as u8);
            self.lba_high_port.write((lba >> 16) as u8);
            self.device_port
                .write(0xE0 | ((device as u8) << 4) | ((lba >> 24) as u8 & 0x0F));
            self.command_port.write(0x20);
        }

        for sector in 0..count {
            for _ in 0..10000 {
                let status = unsafe { self.alt_status_port.read() };
                if (status & 0x08) != 0 {
                    break;
                }
                if (status & 0x01) != 0 {
                    let error = unsafe { self.error_port.read() };
                    return Err(AtaError::Error(error));
                }
            }

            let sector_start = sector as usize * 512;
            for i in (0..512).step_by(2) {
                let word = unsafe { self.data_port.read() };
                buffer[sector_start + i] = word as u8;
                buffer[sector_start + i + 1] = (word >> 8) as u8;
            }
        }

        Ok(())
    }
}

pub static mut PRIMARY_ATA: Mutex<AtaController> = Mutex::new(AtaController::new(0x1F0));
pub static mut SECONDARY_ATA: Mutex<AtaController> = Mutex::new(AtaController::new(0x170));

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

pub fn test_ata_driver() {
    crate::serial_println!("=== ATA DRIVER TEST START ===");

    unsafe {
        crate::serial_println!("Checking Primary Master...");
        match PRIMARY_ATA.lock().identify(AtaDevice::Master) {
            Ok(identify_data) => {
                let info = DriveInfo::from_identify_data(&identify_data);
                crate::serial_println!("Primary Master found:");
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

                crate::println!("Primary Master: {} - {} MB", info.model, info.capacity_mb());

                if info.sectors > 0 {
                    let mut buffer = [0u8; 512];
                    match PRIMARY_ATA
                        .lock()
                        .read_sectors(AtaDevice::Master, 0, 1, &mut buffer)
                    {
                        Ok(()) => {
                            crate::serial_println!("Successfully read sector 0");
                            if buffer[510] == 0x55 && buffer[511] == 0xAA {
                                crate::serial_println!("Valid MBR signature found");
                                crate::println!("MBR signature: Valid");
                            } else {
                                crate::println!("MBR signature: Invalid or missing");
                            }

                            crate::serial_print!("First 16 bytes of sector 0: ");
                            for i in 0..16 {
                                crate::serial_print!("{:02X} ", buffer[i]);
                            }
                            crate::serial_println!();
                        }
                        Err(e) => {
                            crate::serial_println!("Error reading sector: {:?}", e);
                        }
                    }
                }
            }
            Err(e) => {
                crate::serial_println!("Primary Master error: {:?}", e);
                crate::println!("Primary Master: Not found");
            }
        }
    }

    crate::serial_println!("=== ATA DRIVER TEST COMPLETE ===");
}
