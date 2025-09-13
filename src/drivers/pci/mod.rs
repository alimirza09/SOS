use crate::serial_println;
use alloc::vec::Vec;
use x86_64::instructions::port::Port;
pub mod virtio_gpu;
pub use virtio_gpu::*;

#[derive(Debug, Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub bars: [u64; 6],
}

impl PciDevice {
    pub fn from_location(bus: u8, slot: u8, func: u8) -> Option<Self> {
        let vendor_device = pci_read_config(bus, slot, func, 0x00);
        if vendor_device == 0xFFFF_FFFF {
            return None;
        }

        let vendor_id = (vendor_device & 0xFFFF) as u16;
        let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;

        let mut bars = [0u64; 6];
        for i in 0..6 {
            bars[i] = pci_read_bar(bus, slot, func, i as u8);
        }

        Some(PciDevice {
            bus,
            slot,
            func,
            vendor_id,
            device_id,
            bars,
        })
    }

    pub fn enable(&self) {
        let addr = (1 << 31)                // enable bit
            | ((self.bus as u32) << 16)
            | ((self.slot as u32) << 11)
            | ((self.func as u32) << 8)
            | 0x04; // command register offset

        // Read current command register
        let mut port = x86_64::instructions::port::Port::<u32>::new(0xCF8);
        unsafe { port.write(addr) };
        let mut data_port = x86_64::instructions::port::Port::<u32>::new(0xCFC);
        let mut command: u32 = unsafe { data_port.read() };

        // Enable I/O space, memory space, bus mastering
        command |= 1 | (1 << 1) | (1 << 2);

        unsafe {
            port.write(addr);
            data_port.write(command);
        }
    }
}

pub fn scan_pci() -> Vec<PciDevice> {
    let mut devices = Vec::new();
    for bus in 0..=255 {
        for slot in 0..32 {
            for func in 0..8 {
                if let Some(dev) = PciDevice::from_location(bus, slot, func) {
                    devices.push(dev);
                }
            }
        }
    }
    devices
}

pub fn find_virtio_gpu() -> Option<PciDevice> {
    for dev in scan_pci() {
        if dev.vendor_id == 0x1AF4 && dev.device_id == 0x1050 {
            serial_println!("Found VirtIO-GPU: {:?}", dev);
            for i in 0..6 {
                serial_println!("VirtIO-GPU BAR{} = {:#X}", i, dev.bars[i]);
            }
            return Some(dev);
        }
    }
    None
}

unsafe fn outl(port: u16, val: u32) {
    let mut p = Port::<u32>::new(port);
    p.write(val);
}

unsafe fn inl(port: u16) -> u32 {
    let mut p = Port::<u32>::new(port);
    p.read()
}

fn pci_read_config(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    let address: u32 = (1 << 31)
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);

    unsafe {
        outl(0xCF8, address);
        inl(0xCFC)
    }
}

fn pci_read_bar(bus: u8, slot: u8, func: u8, index: u8) -> u64 {
    let offset = 0x10 + (index * 4);
    let low = pci_read_config(bus, slot, func, offset);
    // For 64-bit BARs: check if bit 2 of BAR says it's 64-bit
    if (low & 0x6) == 0x4 {
        let high = pci_read_config(bus, slot, func, offset + 4);
        ((high as u64) << 32) | ((low & 0xFFFF_FFF0) as u64)
    } else {
        (low & 0xFFFF_FFF0) as u64
    }
}

pub fn test_pci() {
    serial_println!("Trying to find VirtIO-GPU");
    if find_virtio_gpu().is_none() {
        serial_println!("None Found");
    }
}
