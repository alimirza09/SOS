use alloc::string::String;
use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};

#[derive(Debug, Clone, Copy)]
pub enum AtaError {
    Timeout,
    NotReady,
    Error(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaDevice {
    Master,
    Slave,
}

pub struct AtaController {
    data_port: Port<u16>,
    error_port: PortReadOnly<u8>,
    sector_count_port: Port<u8>,
    lba_low_port: Port<u8>,
    lba_mid_port: Port<u8>,
    lba_high_port: Port<u8>,
    device_port: Port<u8>,
    status_port: PortReadOnly<u8>,
    command_port: PortWriteOnly<u8>,
    control_port: PortWriteOnly<u8>,
}

impl AtaController {
    pub const fn new(base: u16) -> Self {
        Self {
            data_port: Port::new(base),
            error_port: PortReadOnly::new(base + 1),
            sector_count_port: Port::new(base + 2),
            lba_low_port: Port::new(base + 3),
            lba_mid_port: Port::new(base + 4),
            lba_high_port: Port::new(base + 5),
            device_port: Port::new(base + 6),
            status_port: PortReadOnly::new(base + 7),
            command_port: PortWriteOnly::new(base + 7),
            control_port: PortWriteOnly::new(base + 0x206),
        }
    }

    fn wait_ready(&mut self) -> Result<(), AtaError> {
        for _ in 0..4 {
            unsafe {
                self.status_port.read();
            } // wait 400ns
        }

        for _ in 0..100000 {
            let status = unsafe { self.status_port.read() };
            if status & 0x80 == 0 {
                return Ok(());
            }
        }
        Err(AtaError::Timeout)
    }

    fn wait_data_request(&mut self) -> Result<(), AtaError> {
        for _ in 0..100000 {
            let status = unsafe { self.status_port.read() };
            if status & 0x08 != 0 {
                return Ok(());
            }
            if status & 0x01 != 0 {
                let error = unsafe { self.error_port.read() };
                return Err(AtaError::Error(error));
            }
        }
        Err(AtaError::Timeout)
    }

    fn select_device(&mut self, device: AtaDevice) {
        let value = match device {
            AtaDevice::Master => 0xA0,
            AtaDevice::Slave => 0xB0,
        };
        unsafe {
            self.device_port.write(value);
        }
    }

    pub fn identify(&mut self, device: AtaDevice) -> Result<[u16; 256], AtaError> {
        self.select_device(device);
        self.wait_ready()?;

        // Send IDENTIFY command
        unsafe {
            self.command_port.write(0xEC);
        }

        // Check if device exists
        if unsafe { self.status_port.read() } == 0 {
            return Err(AtaError::NotReady);
        }

        self.wait_data_request()?;

        // Read identify data
        let mut data = [0; 256];
        for word in &mut data {
            *word = unsafe { self.data_port.read() };
        }

        Ok(data)
    }

    pub fn read_sectors(
        &mut self,
        device: AtaDevice,
        lba: u32,
        count: u8,
        buffer: &mut [u8],
    ) -> Result<(), AtaError> {
        assert!(buffer.len() >= count as usize * 512);

        self.select_device(device);
        self.wait_ready()?;

        // Set sector count
        unsafe {
            self.sector_count_port.write(count);
        }

        // Set LBA registers
        unsafe {
            self.lba_low_port.write(lba as u8);
            self.lba_mid_port.write((lba >> 8) as u8);
            self.lba_high_port.write((lba >> 16) as u8);
            self.device_port
                .write(0xE0 | ((device as u8) << 4) | ((lba >> 24) as u8 & 0x0F));
        }

        // Send read command
        unsafe {
            self.command_port.write(0x20);
        }

        for sector in 0..count {
            self.wait_data_request()?;

            // Read sector
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
        lba: u32,
        count: u8,
        buffer: &[u8],
    ) -> Result<(), AtaError> {
        assert!(buffer.len() >= count as usize * 512);

        self.select_device(device);
        self.wait_ready()?;

        // Set sector count
        unsafe {
            self.sector_count_port.write(count);
        }

        // Set LBA registers
        unsafe {
            self.lba_low_port.write(lba as u8);
            self.lba_mid_port.write((lba >> 8) as u8);
            self.lba_high_port.write((lba >> 16) as u8);
            self.device_port
                .write(0xE0 | ((device as u8) << 4) | ((lba >> 24) as u8 & 0x0F));
        }

        // Send write command
        unsafe {
            self.command_port.write(0x30);
        }

        for sector in 0..count {
            self.wait_data_request()?;

            // Write sector
            let sector_start = sector as usize * 512;
            for i in (0..512).step_by(2) {
                let word =
                    (buffer[sector_start + i + 1] as u16) << 8 | buffer[sector_start + i] as u16;
                unsafe {
                    self.data_port.write(word);
                }
            }
        }

        Ok(())
    }
}

// Primary and secondary ATA controllers
pub static mut PRIMARY_ATA: AtaController = AtaController::new(0x1F0);
pub static mut SECONDARY_ATA: AtaController = AtaController::new(0x170);

pub fn test_ata_driver() {
    use crate::println;
    use x86_64::instructions::interrupts;

    println!("Testing ATA driver...");

    interrupts::without_interrupts(|| {
        unsafe {
            // Test identifying master device on primary controller
            match PRIMARY_ATA.identify(AtaDevice::Master) {
                Ok(identify_data) => {
                    println!("Primary Master device identified successfully");

                    // Extract some basic information from identify data
                    let serial_number = extract_string(&identify_data, 10, 20);
                    let model_number = extract_string(&identify_data, 27, 40);
                    let capacity = (identify_data[60] as u32) | ((identify_data[61] as u32) << 16);

                    println!("Model: {}", model_number);
                    println!("Serial: {}", serial_number);
                    println!("Sectors: {}", capacity);

                    // Test reading a sector (LBA 0)
                    let mut buffer = [0u8; 512];
                    if let Err(e) = PRIMARY_ATA.read_sectors(AtaDevice::Master, 0, 1, &mut buffer) {
                        println!("Error reading sector: {:?}", e);
                        return;
                    }

                    println!("Successfully read sector 0");

                    // Check if it looks like a valid MBR (optional)
                    if buffer[510] == 0x55 && buffer[511] == 0xAA {
                        println!("MBR signature found (55 AA)");
                    }
                }
                Err(AtaError::NotReady) => {
                    println!("No device found on Primary Master");
                }
                Err(e) => {
                    println!("Error identifying device: {:?}", e);
                }
            }
        }
    });

    println!("ATA test completed");
}

// Helper function to extract string from identify data
fn extract_string(data: &[u16; 256], start_index: usize, length: usize) -> String {
    let mut result = String::new();
    for i in 0..length {
        let word = data[start_index + i];
        let bytes = [(word & 0xFF) as u8, (word >> 8) as u8];

        // Skip spaces and convert to characters
        for byte in bytes.iter() {
            if *byte != 0 && *byte != b' ' {
                result.push(*byte as char);
            }
        }
    }
    result
}
