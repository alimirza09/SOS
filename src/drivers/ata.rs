use alloc::string::{String, ToString};
use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};

// ATA/IDE register offsets
const ATA_DATA: u16 = 0x00;
const ATA_ERROR: u16 = 0x01;
const ATA_FEATURES: u16 = 0x01;
const ATA_SECTOR_COUNT: u16 = 0x02;
const ATA_LBA_LOW: u16 = 0x03;
const ATA_LBA_MID: u16 = 0x04;
const ATA_LBA_HIGH: u16 = 0x05;
const ATA_DRIVE_SELECT: u16 = 0x06;
const ATA_STATUS: u16 = 0x07;
const ATA_COMMAND: u16 = 0x07;

// Alternate status register (device control)
const ATA_ALT_STATUS: u16 = 0x206;
const ATA_DEVICE_CONTROL: u16 = 0x206;

// ATA Commands
const ATA_CMD_READ_SECTORS: u8 = 0x20;
const ATA_CMD_WRITE_SECTORS: u8 = 0x30;
const ATA_CMD_IDENTIFY: u8 = 0xEC;
const ATA_CMD_FLUSH: u8 = 0xE7;

// Status register bits
const ATA_STATUS_ERR: u8 = 0x01; // Error
const ATA_STATUS_DRQ: u8 = 0x08; // Data Request
const ATA_STATUS_BSY: u8 = 0x80; // Busy
const ATA_STATUS_RDY: u8 = 0x40; // Ready

// Drive select bits
const ATA_DRIVE_MASTER: u8 = 0xA0; // Master drive, LBA mode
const ATA_DRIVE_SLAVE: u8 = 0xB0; // Slave drive, LBA mode

#[derive(Debug, Clone, Copy)]
pub enum AtaError {
    DriveNotReady,
    SectorNotFound,
    WriteProtected,
    Timeout,
    BadCommand,
    GeneralError,
}

pub struct AtaDevice {
    base_port: u16,
    is_slave: bool,
    sectors: u64,
    model: [u8; 40],
}

impl AtaDevice {
    fn new(base_port: u16, is_slave: bool) -> Option<Self> {
        let mut device = AtaDevice {
            base_port,
            is_slave,
            sectors: 0,
            model: [0; 40],
        };

        if device.identify().is_ok() {
            Some(device)
        } else {
            None
        }
    }

    fn data_port(&self) -> Port<u16> {
        Port::new(self.base_port + ATA_DATA)
    }

    fn error_port(&self) -> PortReadOnly<u8> {
        PortReadOnly::new(self.base_port + ATA_ERROR)
    }

    fn features_port(&self) -> PortWriteOnly<u8> {
        PortWriteOnly::new(self.base_port + ATA_FEATURES)
    }

    fn sector_count_port(&self) -> Port<u8> {
        Port::new(self.base_port + ATA_SECTOR_COUNT)
    }

    fn lba_low_port(&self) -> Port<u8> {
        Port::new(self.base_port + ATA_LBA_LOW)
    }

    fn lba_mid_port(&self) -> Port<u8> {
        Port::new(self.base_port + ATA_LBA_MID)
    }

    fn lba_high_port(&self) -> Port<u8> {
        Port::new(self.base_port + ATA_LBA_HIGH)
    }

    fn drive_select_port(&self) -> Port<u8> {
        Port::new(self.base_port + ATA_DRIVE_SELECT)
    }

    fn status_port(&self) -> PortReadOnly<u8> {
        PortReadOnly::new(self.base_port + ATA_STATUS)
    }

    fn command_port(&self) -> PortWriteOnly<u8> {
        PortWriteOnly::new(self.base_port + ATA_COMMAND)
    }

    fn alt_status_port(&self) -> PortReadOnly<u8> {
        PortReadOnly::new(self.base_port + ATA_ALT_STATUS)
    }

    fn select_drive(&self) {
        let drive_byte = if self.is_slave {
            ATA_DRIVE_SLAVE
        } else {
            ATA_DRIVE_MASTER
        };

        unsafe {
            self.drive_select_port().write(drive_byte);
        }

        // Small delay after drive selection
        self.wait_400ns();
    }

    fn wait_400ns(&self) {
        // Read alternate status 4 times for ~400ns delay
        unsafe {
            for _ in 0..4 {
                let _ = self.alt_status_port().read();
            }
        }
    }

    fn wait_not_busy(&self) -> Result<(), AtaError> {
        let mut timeout = 1000000; // Timeout counter

        unsafe {
            while timeout > 0 {
                let status = self.status_port().read();
                if status & ATA_STATUS_BSY == 0 {
                    return Ok(());
                }
                timeout -= 1;
            }
        }

        Err(AtaError::Timeout)
    }

    fn wait_ready(&self) -> Result<(), AtaError> {
        self.wait_not_busy()?;

        unsafe {
            let status = self.status_port().read();
            if status & ATA_STATUS_ERR != 0 {
                return Err(AtaError::GeneralError);
            }
            if status & ATA_STATUS_RDY == 0 {
                return Err(AtaError::DriveNotReady);
            }
        }

        Ok(())
    }

    fn wait_drq(&self) -> Result<(), AtaError> {
        let mut timeout = 1000000;

        unsafe {
            while timeout > 0 {
                let status = self.status_port().read();
                if status & ATA_STATUS_BSY == 0 {
                    if status & ATA_STATUS_ERR != 0 {
                        return Err(AtaError::GeneralError);
                    }
                    if status & ATA_STATUS_DRQ != 0 {
                        return Ok(());
                    }
                }
                timeout -= 1;
            }
        }

        Err(AtaError::Timeout)
    }

    fn identify(&mut self) -> Result<(), AtaError> {
        self.select_drive();
        self.wait_ready()?;

        // Send IDENTIFY command
        unsafe {
            self.command_port().write(ATA_CMD_IDENTIFY);
        }

        // Wait for data to be ready
        self.wait_drq()?;

        // Read 256 words (512 bytes) of identification data
        let mut identify_data = [0u16; 256];
        unsafe {
            for i in 0..256 {
                identify_data[i] = self.data_port().read();
            }
        }

        // Extract number of sectors (words 60-61 for LBA28)
        self.sectors = ((identify_data[61] as u64) << 16) | (identify_data[60] as u64);

        // Extract model string (words 27-46)
        for i in 0..20 {
            let word = identify_data[27 + i];
            self.model[i * 2] = (word >> 8) as u8;
            self.model[i * 2 + 1] = (word & 0xFF) as u8;
        }

        Ok(())
    }

    pub fn read_sectors(&self, lba: u32, count: u8, buffer: &mut [u8]) -> Result<(), AtaError> {
        if buffer.len() < (count as usize * 512) {
            return Err(AtaError::BadCommand);
        }

        self.select_drive();
        self.wait_ready()?;

        // Set up LBA addressing
        unsafe {
            self.sector_count_port().write(count);
            self.lba_low_port().write((lba & 0xFF) as u8);
            self.lba_mid_port().write(((lba >> 8) & 0xFF) as u8);
            self.lba_high_port().write(((lba >> 16) & 0xFF) as u8);

            let drive_head = if self.is_slave {
                ATA_DRIVE_SLAVE | ((lba >> 24) & 0x0F) as u8
            } else {
                ATA_DRIVE_MASTER | ((lba >> 24) & 0x0F) as u8
            };
            self.drive_select_port().write(drive_head);
        }

        self.wait_400ns();

        // Send read command
        unsafe {
            self.command_port().write(ATA_CMD_READ_SECTORS);
        }

        // Read data for each sector
        for sector in 0..count {
            self.wait_drq()?;

            let sector_offset = (sector as usize) * 512;
            let sector_data = &mut buffer[sector_offset..sector_offset + 512];

            // Read 256 words (512 bytes)
            unsafe {
                let mut data_port = self.data_port();
                for i in (0..512).step_by(2) {
                    let word = data_port.read();
                    sector_data[i] = (word & 0xFF) as u8;
                    sector_data[i + 1] = (word >> 8) as u8;
                }
            }
        }

        Ok(())
    }

    pub fn write_sectors(&self, lba: u32, count: u8, buffer: &[u8]) -> Result<(), AtaError> {
        if buffer.len() < (count as usize * 512) {
            return Err(AtaError::BadCommand);
        }

        self.select_drive();
        self.wait_ready()?;

        // Set up LBA addressing
        unsafe {
            self.sector_count_port().write(count);
            self.lba_low_port().write((lba & 0xFF) as u8);
            self.lba_mid_port().write(((lba >> 8) & 0xFF) as u8);
            self.lba_high_port().write(((lba >> 16) & 0xFF) as u8);

            let drive_head = if self.is_slave {
                ATA_DRIVE_SLAVE | ((lba >> 24) & 0x0F) as u8
            } else {
                ATA_DRIVE_MASTER | ((lba >> 24) & 0x0F) as u8
            };
            self.drive_select_port().write(drive_head);
        }

        self.wait_400ns();

        // Send write command
        unsafe {
            self.command_port().write(ATA_CMD_WRITE_SECTORS);
        }

        // Write data for each sector
        for sector in 0..count {
            self.wait_drq()?;

            let sector_offset = (sector as usize) * 512;
            let sector_data = &buffer[sector_offset..sector_offset + 512];

            // Write 256 words (512 bytes)
            unsafe {
                let mut data_port = self.data_port();
                for i in (0..512).step_by(2) {
                    let word = (sector_data[i + 1] as u16) << 8 | sector_data[i] as u16;
                    data_port.write(word);
                }
            }
        }

        // Flush cache
        unsafe {
            self.command_port().write(ATA_CMD_FLUSH);
        }
        self.wait_ready()?;

        Ok(())
    }

    pub fn get_sector_count(&self) -> u64 {
        self.sectors
    }

    pub fn get_model(&self) -> &str {
        // Convert model bytes to string, trimming whitespace
        let model_str = core::str::from_utf8(&self.model).unwrap_or("Unknown");
        model_str.trim()
    }
}

pub struct AtaController {
    devices: Vec<AtaDevice>,
}

impl AtaController {
    pub fn new() -> Self {
        let mut controller = AtaController {
            devices: Vec::new(),
        };

        controller.probe_devices();
        controller
    }

    fn probe_devices(&mut self) {
        // Primary ATA controller (0x1F0)
        if let Some(device) = AtaDevice::new(0x1F0, false) {
            crate::println!(
                "Found ATA device: Primary Master - {} sectors",
                device.get_sector_count()
            );
            crate::println!("Model: {}", device.get_model());
            self.devices.push(device);
        }

        if let Some(device) = AtaDevice::new(0x1F0, true) {
            crate::println!(
                "Found ATA device: Primary Slave - {} sectors",
                device.get_sector_count()
            );
            crate::println!("Model: {}", device.get_model());
            self.devices.push(device);
        }

        // Secondary ATA controller (0x170)
        if let Some(device) = AtaDevice::new(0x170, false) {
            crate::println!(
                "Found ATA device: Secondary Master - {} sectors",
                device.get_sector_count()
            );
            crate::println!("Model: {}", device.get_model());
            self.devices.push(device);
        }

        if let Some(device) = AtaDevice::new(0x170, true) {
            crate::println!(
                "Found ATA device: Secondary Slave - {} sectors",
                device.get_sector_count()
            );
            crate::println!("Model: {}", device.get_model());
            self.devices.push(device);
        }

        if self.devices.is_empty() {
            crate::println!("No ATA devices found");
        }
    }

    pub fn get_device(&self, index: usize) -> Option<&AtaDevice> {
        self.devices.get(index)
    }

    pub fn get_device_count(&self) -> usize {
        self.devices.len()
    }

    pub fn read_sectors(
        &self,
        device_index: usize,
        lba: u32,
        count: u8,
        buffer: &mut [u8],
    ) -> Result<(), AtaError> {
        if let Some(device) = self.devices.get(device_index) {
            device.read_sectors(lba, count, buffer)
        } else {
            Err(AtaError::BadCommand)
        }
    }

    pub fn write_sectors(
        &self,
        device_index: usize,
        lba: u32,
        count: u8,
        buffer: &[u8],
    ) -> Result<(), AtaError> {
        if let Some(device) = self.devices.get(device_index) {
            device.write_sectors(lba, count, buffer)
        } else {
            Err(AtaError::BadCommand)
        }
    }
}

lazy_static! {
    pub static ref ATA_CONTROLLER: Mutex<AtaController> = Mutex::new(AtaController::new());
}

// Convenience functions for global access
pub fn init_ata() {
    lazy_static::initialize(&ATA_CONTROLLER);
    crate::println!("ATA driver initialized");
}

pub fn read_sector(device: usize, lba: u32, buffer: &mut [u8; 512]) -> Result<(), AtaError> {
    ATA_CONTROLLER.lock().read_sectors(device, lba, 1, buffer)
}

pub fn write_sector(device: usize, lba: u32, buffer: &[u8; 512]) -> Result<(), AtaError> {
    ATA_CONTROLLER.lock().write_sectors(device, lba, 1, buffer)
}

pub fn get_device_info(device: usize) -> Option<(u64, String)> {
    let controller = ATA_CONTROLLER.lock();
    if let Some(dev) = controller.get_device(device) {
        Some((dev.get_sector_count(), dev.get_model().to_string()))
    } else {
        None
    }
}

// Test function to demonstrate ATA usage
pub fn test_ata() {
    crate::println!("Testing ATA driver...");

    let controller = ATA_CONTROLLER.lock();

    if controller.get_device_count() == 0 {
        crate::println!("No ATA devices available for testing");
        return;
    }

    // Test reading from the first device
    let mut buffer = [0u8; 512];
    match controller.read_sectors(0, 0, 1, &mut buffer) {
        Ok(()) => {
            crate::println!("Successfully read sector 0:");
            // Print first 64 bytes
            for i in (0..64).step_by(16) {
                crate::print!("{:04X}: ", i);
                for j in 0..16 {
                    if i + j < buffer.len() {
                        crate::print!("{:02X} ", buffer[i + j]);
                    }
                }
                crate::println!();
            }
        }
        Err(e) => {
            crate::println!("Failed to read sector 0: {:?}", e);
        }
    }

    // Test writing and reading back (be careful with this!)
    let test_data = [0x42u8; 512]; // Pattern to write
    let test_lba = 1000; // Use a safe sector far from boot sectors

    crate::println!("Testing write/read cycle at LBA {}...", test_lba);

    match controller.write_sectors(0, test_lba, 1, &test_data) {
        Ok(()) => {
            crate::println!("Write successful, reading back...");

            let mut read_buffer = [0u8; 512];
            match controller.read_sectors(0, test_lba, 1, &mut read_buffer) {
                Ok(()) => {
                    if read_buffer == test_data {
                        crate::println!("Read/write test PASSED!");
                    } else {
                        crate::println!("Read/write test FAILED - data mismatch");
                    }
                }
                Err(e) => {
                    crate::println!("Read test failed: {:?}", e);
                }
            }
        }
        Err(e) => {
            crate::println!("Write test failed: {:?}", e);
        }
    }
}
