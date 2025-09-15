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
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
    pub header_type: u8,
    pub bars: [PciBar; 6],
    pub command: u16,
    pub status: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct PciBar {
    pub address: u64,
    pub size: u64,
    pub bar_type: PciBarType,
    pub prefetchable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PciBarType {
    None,
    Memory32,
    Memory64,
    Io,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtGpuCmdSetScanout {
    pub header: virtio_gpu::CtrlHdr,
    pub rect: virtio_gpu::Rect,
    pub scanout_id: u32,
    pub resource_id: u32,
    pub padding: u32,
}

impl Default for PciBar {
    fn default() -> Self {
        Self {
            address: 0,
            size: 0,
            bar_type: PciBarType::None,
            prefetchable: false,
        }
    }
}

impl PciDevice {
    pub fn from_location(bus: u8, slot: u8, func: u8) -> Option<Self> {
        let vendor_device = pci_read_config(bus, slot, func, 0x00);
        if vendor_device == 0xFFFF_FFFF {
            return None;
        }

        let vendor_id = (vendor_device & 0xFFFF) as u16;
        let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;

        let class_info = pci_read_config(bus, slot, func, 0x08);
        let revision = (class_info & 0xFF) as u8;
        let prog_if = ((class_info >> 8) & 0xFF) as u8;
        let subclass = ((class_info >> 16) & 0xFF) as u8;
        let class_code = ((class_info >> 24) & 0xFF) as u8;

        let header_cmd_status = pci_read_config(bus, slot, func, 0x04);
        let command = (header_cmd_status & 0xFFFF) as u16;
        let status = ((header_cmd_status >> 16) & 0xFFFF) as u16;

        let header_type_raw = pci_read_config(bus, slot, func, 0x0C);
        let header_type = ((header_type_raw >> 16) & 0xFF) as u8;

        let bars = read_bars(bus, slot, func);

        Some(PciDevice {
            bus,
            slot,
            func,
            vendor_id,
            device_id,
            class_code,
            subclass,
            prog_if,
            revision,
            header_type,
            bars,
            command,
            status,
        })
    }

    pub fn enable(&self) {
        let addr = (1 << 31)
            | ((self.bus as u32) << 16)
            | ((self.slot as u32) << 11)
            | ((self.func as u32) << 8)
            | 0x04;

        let mut port = Port::<u32>::new(0xCF8);
        unsafe { port.write(addr) };
        let mut data_port = Port::<u32>::new(0xCFC);
        let mut command: u32 = unsafe { data_port.read() };

        command |= 1 | (1 << 1) | (1 << 2);

        unsafe {
            port.write(addr);
            data_port.write(command);
        }

        serial_println!(
            "PCI device {}:{}:{} enabled with command 0x{:04X}",
            self.bus,
            self.slot,
            self.func,
            command
        );
    }

    pub fn get_bar(&self, index: usize) -> Option<&PciBar> {
        if index < 6 && self.bars[index].bar_type != PciBarType::None {
            Some(&self.bars[index])
        } else {
            None
        }
    }

    pub fn print_info(&self) {
        serial_println!("PCI Device {}:{}:{}", self.bus, self.slot, self.func);
        serial_println!(
            "  Vendor: 0x{:04X}, Device: 0x{:04X}",
            self.vendor_id,
            self.device_id
        );
        serial_println!(
            "  Class: 0x{:02X}, Subclass: 0x{:02X}, ProgIF: 0x{:02X}",
            self.class_code,
            self.subclass,
            self.prog_if
        );
        serial_println!(
            "  Header Type: 0x{:02X}, Revision: 0x{:02X}",
            self.header_type,
            self.revision
        );
        serial_println!(
            "  Command: 0x{:04X}, Status: 0x{:04X}",
            self.command,
            self.status
        );

        for (i, bar) in self.bars.iter().enumerate() {
            if bar.bar_type != PciBarType::None {
                serial_println!(
                    "  BAR{}: 0x{:016X} (size: 0x{:X}, type: {:?}, prefetch: {})",
                    i,
                    bar.address,
                    bar.size,
                    bar.bar_type,
                    bar.prefetchable
                );
            }
        }
    }
}

fn read_bars(bus: u8, slot: u8, func: u8) -> [PciBar; 6] {
    let mut bars = [PciBar::default(); 6];
    let mut i = 0;

    while i < 6 {
        let offset = 0x10 + (i * 4) as u8;
        let original = pci_read_config(bus, slot, func, offset);

        if original == 0 {
            i += 1;
            continue;
        }

        pci_write_config(bus, slot, func, offset, 0xFFFFFFFF);
        let size_mask = pci_read_config(bus, slot, func, offset);

        pci_write_config(bus, slot, func, offset, original);

        if size_mask == 0 {
            i += 1;
            continue;
        }

        let is_io = (original & 1) != 0;

        if is_io {
            let address = (original & 0xFFFFFFFC) as u64;
            let size = calculate_bar_size(size_mask & 0xFFFFFFFC);

            bars[i] = PciBar {
                address,
                size,
                bar_type: PciBarType::Io,
                prefetchable: false,
            };
            i += 1;
        } else {
            let bar_type_bits = (original >> 1) & 0x3;
            let prefetchable = (original & 0x8) != 0;

            match bar_type_bits {
                0 => {
                    let address = (original & 0xFFFFFFF0) as u64;
                    let size = calculate_bar_size(size_mask & 0xFFFFFFF0);

                    bars[i] = PciBar {
                        address,
                        size,
                        bar_type: PciBarType::Memory32,
                        prefetchable,
                    };
                    i += 1;
                }
                2 => {
                    if i + 1 >= 6 {
                        i += 1;
                        continue;
                    }

                    let high_offset = 0x10 + ((i + 1) * 4) as u8;
                    let original_high = pci_read_config(bus, slot, func, high_offset);

                    pci_write_config(bus, slot, func, high_offset, 0xFFFFFFFF);
                    let size_mask_high = pci_read_config(bus, slot, func, high_offset);

                    pci_write_config(bus, slot, func, high_offset, original_high);

                    let address = ((original_high as u64) << 32) | ((original & 0xFFFFFFF0) as u64);
                    let size_mask_64 =
                        ((size_mask_high as u64) << 32) | ((size_mask & 0xFFFFFFF0) as u64);
                    let size = calculate_bar_size_64(size_mask_64);

                    bars[i] = PciBar {
                        address,
                        size,
                        bar_type: PciBarType::Memory64,
                        prefetchable,
                    };

                    i += 2;
                }
                _ => {
                    i += 1;
                }
            }
        }
    }

    bars
}

fn calculate_bar_size(size_mask: u32) -> u64 {
    if size_mask == 0 {
        return 0;
    }

    let size = (!size_mask).wrapping_add(1);
    size as u64
}

fn calculate_bar_size_64(size_mask: u64) -> u64 {
    if size_mask == 0 {
        return 0;
    }

    (!size_mask).wrapping_add(1)
}

pub fn scan_pci() -> Vec<PciDevice> {
    let mut devices = Vec::new();

    for bus in 0..=255u8 {
        for slot in 0..32u8 {
            for func in 0..8u8 {
                if let Some(dev) = PciDevice::from_location(bus, slot, func) {
                    devices.push(dev);

                    if func == 0 && (dev.header_type & 0x80) == 0 {
                        break;
                    }
                }
            }
        }
    }

    serial_println!("Found {} PCI devices", devices.len());
    devices
}

pub fn find_virtio_gpu() -> Option<PciDevice> {
    for dev in scan_pci() {
        if dev.vendor_id == 0x1AF4 && (dev.device_id == 0x1050 || dev.device_id == 0x1010) {
            serial_println!("Found VirtIO-GPU device:");
            dev.print_info();
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

fn pci_write_config(bus: u8, slot: u8, func: u8, offset: u8, value: u32) {
    let address: u32 = (1 << 31)
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);

    unsafe {
        outl(0xCF8, address);
        outl(0xCFC, value);
    }
}

pub fn test_pci() {
    serial_println!("=== PCI Device Scan ===");
    let devices = scan_pci();

    for dev in &devices {
        dev.print_info();
        serial_println!();
    }

    serial_println!("Looking for VirtIO-GPU...");
    if let Some(_gpu) = find_virtio_gpu() {
        serial_println!("VirtIO-GPU found and details printed above");
    } else {
        serial_println!("No VirtIO-GPU device found");
    }
}
