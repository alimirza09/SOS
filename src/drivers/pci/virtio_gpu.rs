use crate::drivers::pci::PciDevice;
use crate::serial_println;
use core::ptr::read_volatile;
use x86_64::structures::paging::{Mapper, PageSize};
use x86_64::{
    structures::paging::{
        FrameAllocator, OffsetPageTable, Page, PageTableFlags, PhysFrame, Size4KiB,
    },
    PhysAddr, VirtAddr,
};

pub struct VirtioGpu {
    pub dev: PciDevice,
    pub regs_virt: *mut u8,
    pub fb_virt: *mut u8,
}

impl VirtioGpu {
    pub fn new(dev: PciDevice) -> Self {
        Self {
            dev,
            regs_virt: core::ptr::null_mut(),
            fb_virt: core::ptr::null_mut(),
        }
    }

    pub fn init_and_test(
        &mut self,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) {
        serial_println!("Creating VirtioGPU");

        let regs_phys = if self.dev.bars[1] != 0 {
            self.dev.bars[1]
        } else {
            self.dev.bars[0]
        };
        let fb_phys = if self.dev.bars[4] != 0 {
            self.dev.bars[4]
        } else {
            0
        };

        if regs_phys == 0 {
            serial_println!("No regs BAR found for VirtIO-GPU");
            return;
        }

        match unsafe { map_mmio_range(regs_phys, 0x1000, mapper, frame_allocator) } {
            Ok(regs_ptr) => {
                self.regs_virt = regs_ptr;
                serial_println!("VirtIO-GPU regs mapped at {:p}", regs_ptr);
                let val = unsafe { read_volatile(regs_ptr as *const u32) };
                serial_println!("VirtIO-GPU MMIO read {:#X}", val);
            }
            Err(e) => {
                serial_println!("Failed to map regs: {}", e);
                return;
            }
        }

        if fb_phys != 0 {
            match unsafe { map_mmio_range(fb_phys, 8 * 1024 * 1024, mapper, frame_allocator) } {
                Ok(fb_ptr) => {
                    self.fb_virt = fb_ptr;
                    serial_println!("VirtIO-GPU fb mapped at {:p}", fb_ptr);
                }
                Err(e) => {
                    serial_println!("Failed to map fb: {}", e);
                }
            }
        } else {
            serial_println!("No framebuffer BAR found (BAR4==0)");
        }
    }
}

unsafe fn map_mmio_range(
    phys_addr: u64,
    size: u64,
    mapper: &mut OffsetPageTable,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<*mut u8, &'static str> {
    let start = PhysAddr::new(phys_addr);
    let end = PhysAddr::new(phys_addr + size - 1);

    let mut current_frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(start);
    let end_frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(end);

    const MMIO_BASE_HIGH: u64 = 0xFFFF_8000_0000_0000u64;
    let virt_base = VirtAddr::new(MMIO_BASE_HIGH + phys_addr);

    let mut current_virt = Page::containing_address(virt_base);

    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;

    loop {
        mapper
            .map_to(current_virt, current_frame, flags, frame_allocator)
            .map_err(|_| "map_to failed")?
            .flush();

        if current_frame == end_frame {
            break;
        }

        let next_frame_addr = current_frame.start_address().as_u64() + Size4KiB::SIZE;
        current_frame = PhysFrame::containing_address(PhysAddr::new(next_frame_addr));
        current_virt = Page::containing_address(current_virt.start_address() + Size4KiB::SIZE);
    }

    Ok(virt_base.as_u64() as *mut u8)
}
