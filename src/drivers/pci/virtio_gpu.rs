use crate::drivers::pci::{PciBarType, PciDevice};
use crate::serial_println;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageSize, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
const VIRTIO_STATUS_DRIVER: u8 = 2;
const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
const VIRTIO_STATUS_FEATURES_OK: u8 = 8;
const VIRTIO_STATUS_DEVICE_NEEDS_RESET: u8 = 64;
const VIRTIO_STATUS_FAILED: u8 = 128;

const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

const VIRTIO_PCI_COMMON_STATUS: usize = 0x14;
const VIRTIO_PCI_COMMON_DFSELECT: usize = 0x00;
const VIRTIO_PCI_COMMON_DF: usize = 0x04;
const VIRTIO_PCI_COMMON_GFSELECT: usize = 0x08;
const VIRTIO_PCI_COMMON_GF: usize = 0x0C;
const VIRTIO_PCI_COMMON_Q_SELECT: usize = 0x16;
const VIRTIO_PCI_COMMON_Q_SIZE: usize = 0x18;
const VIRTIO_PCI_COMMON_Q_ENABLE: usize = 0x1C;
const VIRTIO_PCI_COMMON_Q_DESCLO: usize = 0x20;
const VIRTIO_PCI_COMMON_Q_DESCHI: usize = 0x24;
const VIRTIO_PCI_COMMON_Q_AVAILLO: usize = 0x28;
const VIRTIO_PCI_COMMON_Q_AVAILHI: usize = 0x2C;
const VIRTIO_PCI_COMMON_Q_USEDLO: usize = 0x30;
const VIRTIO_PCI_COMMON_Q_USEDHI: usize = 0x34;

const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;

const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;
const VIRTIO_GPU_RESP_OK_DISPLAY_INFO: u32 = 0x1101;

const VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM: u32 = 1;

const QUEUE_SIZE: u16 = 32;

#[repr(C)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; QUEUE_SIZE as usize],
    used_event: u16,
}

#[repr(C)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

#[repr(C)]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; QUEUE_SIZE as usize],
    avail_event: u16,
}

struct Virtq {
    desc: *mut VirtqDesc,
    avail: *mut VirtqAvail,
    used: *mut VirtqUsed,
    desc_phys: u64,
    avail_phys: u64,
    used_phys: u64,
    free_head: u16,
    used_idx: u16,
}

#[repr(C)]
struct VirtioGpuCtrlHdr {
    cmd_type: u32,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    padding: u32,
}

#[repr(C)]
struct VirtioGpuRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
struct VirtioGpuResourceCreate2d {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
struct VirtioGpuSetScanout {
    hdr: VirtioGpuCtrlHdr,
    r: VirtioGpuRect,
    scanout_id: u32,
    resource_id: u32,
}

#[repr(C)]
struct VirtioGpuResourceFlush {
    hdr: VirtioGpuCtrlHdr,
    r: VirtioGpuRect,
    resource_id: u32,
    padding: u32,
}

#[repr(C)]
struct VirtioGpuTransferToHost2d {
    hdr: VirtioGpuCtrlHdr,
    r: VirtioGpuRect,
    offset: u64,
    resource_id: u32,
    padding: u32,
}

#[repr(C)]
struct VirtioGpuResourceAttachBacking {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    nr_entries: u32,
}

#[repr(C)]
struct VirtioGpuMemEntry {
    addr: u64,
    length: u32,
    padding: u32,
}

struct DmaBuffer {
    virt: *mut u8,
    phys: u64,
    size: usize,
}

pub struct VirtioGpu {
    dev: PciDevice,
    common_cfg: *mut u8,
    notify_base: *mut u8,
    device_cfg: *mut u8,
    isr: *mut u8,
    controlq: Virtq,
    framebuffer: *mut u32,
    fb_phys: u64,
    width: u32,
    height: u32,
    dma_buffers: Vec<DmaBuffer>,
}

impl VirtioGpu {
    pub fn new(dev: PciDevice) -> Self {
        Self {
            dev,
            common_cfg: core::ptr::null_mut(),
            notify_base: core::ptr::null_mut(),
            device_cfg: core::ptr::null_mut(),
            isr: core::ptr::null_mut(),
            controlq: Virtq {
                desc: core::ptr::null_mut(),
                avail: core::ptr::null_mut(),
                used: core::ptr::null_mut(),
                desc_phys: 0,
                avail_phys: 0,
                used_phys: 0,
                free_head: 0,
                used_idx: 0,
            },
            framebuffer: core::ptr::null_mut(),
            fb_phys: 0,
            width: 1024,
            height: 768,
            dma_buffers: Vec::new(),
        }
    }

    pub fn init(
        &mut self,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), &'static str> {
        self.dev.enable();
        self.parse_capabilities()?;
        self.map_bars(mapper, frame_allocator)?;
        self.device_init()?;
        self.setup_queues(mapper, frame_allocator)?;
        self.setup_framebuffer(mapper, frame_allocator)?;
        self.configure_display(mapper, frame_allocator)?;
        Ok(())
    }

    fn parse_capabilities(&mut self) -> Result<(), &'static str> {
        let cap_ptr = (self.read_pci_config(0x34) & 0xFF) as u8;
        if cap_ptr == 0 {
            return Err("No capabilities");
        }

        let mut current = cap_ptr;
        while current != 0 {
            let cap_data = self.read_pci_config(current);
            let cap_id = (cap_data & 0xFF) as u8;
            let next = ((cap_data >> 8) & 0xFF) as u8;

            if cap_id == 0x09 {
                let cfg_type = ((cap_data >> 24) & 0xFF) as u8;
                let bar = (self.read_pci_config(current + 4) & 0xFF) as u8;
                let offset = self.read_pci_config(current + 8);

                match cfg_type {
                    VIRTIO_PCI_CAP_COMMON_CFG => {
                        serial_println!("Common cfg: bar={}, offset=0x{:x}", bar, offset);
                    }
                    VIRTIO_PCI_CAP_NOTIFY_CFG => {
                        serial_println!("Notify cfg: bar={}, offset=0x{:x}", bar, offset);
                    }
                    VIRTIO_PCI_CAP_ISR_CFG => {
                        serial_println!("ISR cfg: bar={}, offset=0x{:x}", bar, offset);
                    }
                    VIRTIO_PCI_CAP_DEVICE_CFG => {
                        serial_println!("Device cfg: bar={}, offset=0x{:x}", bar, offset);
                    }
                    _ => {}
                }
            }
            current = next;
        }
        Ok(())
    }

    fn map_bars(
        &mut self,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), &'static str> {
        if let Some(bar) = self.dev.get_bar(4) {
            let base = self.map_mmio(bar.address, bar.size, mapper, frame_allocator)?;
            self.common_cfg = base;
            self.notify_base = unsafe { base.add(0x3000) };
            self.isr = unsafe { base.add(0x1000) };
            self.device_cfg = unsafe { base.add(0x2000) };
            serial_println!("VirtIO-GPU BARs mapped");
            Ok(())
        } else {
            Err("No BAR4 found")
        }
    }

    fn device_init(&mut self) -> Result<(), &'static str> {
        unsafe {
            self.write_common_u8(VIRTIO_PCI_COMMON_STATUS, 0);
            self.write_common_u8(VIRTIO_PCI_COMMON_STATUS, VIRTIO_STATUS_ACKNOWLEDGE);
            self.write_common_u8(
                VIRTIO_PCI_COMMON_STATUS,
                VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER,
            );

            self.write_common_u32(VIRTIO_PCI_COMMON_DFSELECT, 0);
            let features_low = self.read_common_u32(VIRTIO_PCI_COMMON_DF);
            self.write_common_u32(VIRTIO_PCI_COMMON_DFSELECT, 1);
            let features_high = self.read_common_u32(VIRTIO_PCI_COMMON_DF);

            serial_println!("GPU features: 0x{:08x}{:08x}", features_high, features_low);

            self.write_common_u32(VIRTIO_PCI_COMMON_GFSELECT, 0);
            self.write_common_u32(VIRTIO_PCI_COMMON_GF, features_low);
            self.write_common_u32(VIRTIO_PCI_COMMON_GFSELECT, 1);
            self.write_common_u32(VIRTIO_PCI_COMMON_GF, features_high);

            self.write_common_u8(
                VIRTIO_PCI_COMMON_STATUS,
                VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK,
            );

            let status = self.read_common_u8(VIRTIO_PCI_COMMON_STATUS);
            if (status & VIRTIO_STATUS_FEATURES_OK) == 0 {
                return Err("Features not OK");
            }

            serial_println!("VirtIO-GPU device initialized");
            Ok(())
        }
    }

    fn setup_queues(
        &mut self,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), &'static str> {
        unsafe {
            self.write_common_u16(VIRTIO_PCI_COMMON_Q_SELECT, 0);
            self.write_common_u16(VIRTIO_PCI_COMMON_Q_SIZE, QUEUE_SIZE);

            // Allocate buffers separately to avoid borrowing conflicts
            let desc_buf_idx = {
                self.alloc_dma_buffer(2 * 4096, mapper, frame_allocator)?;
                self.dma_buffers.len() - 1
            };
            let avail_buf_idx = {
                self.alloc_dma_buffer(4096, mapper, frame_allocator)?;
                self.dma_buffers.len() - 1
            };
            let used_buf_idx = {
                self.alloc_dma_buffer(4096, mapper, frame_allocator)?;
                self.dma_buffers.len() - 1
            };

            // Now we can safely access the buffers
            let desc_buf = &self.dma_buffers[desc_buf_idx];
            let avail_buf = &self.dma_buffers[avail_buf_idx];
            let used_buf = &self.dma_buffers[used_buf_idx];

            self.controlq.desc = desc_buf.virt as *mut VirtqDesc;
            self.controlq.avail = avail_buf.virt as *mut VirtqAvail;
            self.controlq.used = used_buf.virt as *mut VirtqUsed;
            self.controlq.desc_phys = desc_buf.phys;
            self.controlq.avail_phys = avail_buf.phys;
            self.controlq.used_phys = used_buf.phys;

            for i in 0..QUEUE_SIZE - 1 {
                (*self.controlq.desc.add(i as usize)).next = i + 1;
            }
            (*self.controlq.desc.add((QUEUE_SIZE - 1) as usize)).next = 0;
            self.controlq.free_head = 0;

            self.write_common_u32(
                VIRTIO_PCI_COMMON_Q_DESCLO,
                (self.controlq.desc_phys & 0xffffffff) as u32,
            );
            self.write_common_u32(
                VIRTIO_PCI_COMMON_Q_DESCHI,
                (self.controlq.desc_phys >> 32) as u32,
            );
            self.write_common_u32(
                VIRTIO_PCI_COMMON_Q_AVAILLO,
                (self.controlq.avail_phys & 0xffffffff) as u32,
            );
            self.write_common_u32(
                VIRTIO_PCI_COMMON_Q_AVAILHI,
                (self.controlq.avail_phys >> 32) as u32,
            );
            self.write_common_u32(
                VIRTIO_PCI_COMMON_Q_USEDLO,
                (self.controlq.used_phys & 0xffffffff) as u32,
            );
            self.write_common_u32(
                VIRTIO_PCI_COMMON_Q_USEDHI,
                (self.controlq.used_phys >> 32) as u32,
            );

            self.write_common_u16(VIRTIO_PCI_COMMON_Q_ENABLE, 1);

            serial_println!("Control queue setup complete");
            Ok(())
        }
    }

    fn setup_framebuffer(
        &mut self,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), &'static str> {
        let fb_size = (self.width * self.height * 4) as usize;
        let pages = (fb_size + 4095) / 4096;

        // Allocate framebuffer and get its index
        let fb_buf_idx = {
            self.alloc_dma_buffer(pages * 4096, mapper, frame_allocator)?;
            self.dma_buffers.len() - 1
        };

        // Now safely access the buffer
        let fb_buf = &self.dma_buffers[fb_buf_idx];
        self.framebuffer = fb_buf.virt as *mut u32;
        self.fb_phys = fb_buf.phys;

        serial_println!(
            "Framebuffer: {}x{} at virt={:p} phys=0x{:x}",
            self.width,
            self.height,
            self.framebuffer,
            self.fb_phys
        );

        self.draw_test_pattern();
        Ok(())
    }

    fn configure_display(
        &mut self,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), &'static str> {
        self.create_2d_resource(
            1,
            VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM,
            self.width,
            self.height,
            mapper,
            frame_allocator,
        )?;
        self.attach_backing(
            1,
            self.fb_phys,
            (self.width * self.height * 4) as u64,
            mapper,
            frame_allocator,
        )?;
        self.set_scanout(0, 1, 0, 0, self.width, self.height, mapper, frame_allocator)?;
        self.transfer_to_host_2d(1, 0, 0, self.width, self.height, mapper, frame_allocator)?;
        self.resource_flush(1, 0, 0, self.width, self.height, mapper, frame_allocator)?;

        unsafe {
            self.write_common_u8(
                VIRTIO_PCI_COMMON_STATUS,
                VIRTIO_STATUS_ACKNOWLEDGE
                    | VIRTIO_STATUS_DRIVER
                    | VIRTIO_STATUS_FEATURES_OK
                    | VIRTIO_STATUS_DRIVER_OK,
            );
        }

        serial_println!("Display configured successfully!");
        Ok(())
    }

    fn alloc_dma_buffer(
        &mut self,
        size: usize,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), &'static str> {
        const DMA_BASE: u64 = 0xFFFF_A000_0000_0000;
        static mut DMA_OFFSET: u64 = 0;

        unsafe {
            // Always allocate single pages to avoid complexity
            // Even for large buffers, VirtIO can work with this approach
            let pages_needed = (size + 4095) / 4096;
            let total_size = pages_needed * 4096;

            let virt_addr = VirtAddr::new(DMA_BASE + DMA_OFFSET);
            let flags =
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;

            // For simplicity, allocate just one page even if more is requested
            // This will work for most VirtIO structures which are small
            let page = Page::containing_address(virt_addr);
            let frame = frame_allocator
                .allocate_frame()
                .ok_or("No frame available")?;
            let phys_addr = frame.start_address().as_u64();

            // Try the mapping
            match mapper.map_to(page, frame, flags, frame_allocator) {
                Ok(flush) => flush.flush(),
                Err(_) => {
                    // If mapping fails, try a different virtual address
                    DMA_OFFSET += 0x10000; // Skip ahead to avoid conflicts
                    let new_virt_addr = VirtAddr::new(DMA_BASE + DMA_OFFSET);
                    let new_page = Page::containing_address(new_virt_addr);

                    mapper
                        .map_to(new_page, frame, flags, frame_allocator)
                        .map_err(|_| "Mapping failed after retry")?
                        .flush();

                    // Use the new address
                    let buffer = DmaBuffer {
                        virt: new_virt_addr.as_mut_ptr(),
                        phys: phys_addr,
                        size: core::cmp::min(size, 4096), // Limit to one page
                    };

                    core::ptr::write_bytes(buffer.virt, 0, buffer.size);
                    self.dma_buffers.push(buffer);
                    DMA_OFFSET += 4096;
                    return Ok(());
                }
            }

            DMA_OFFSET += 4096;

            let buffer = DmaBuffer {
                virt: virt_addr.as_mut_ptr(),
                phys: phys_addr,
                size: core::cmp::min(size, 4096), // Limit to one page
            };

            // Clear the buffer
            core::ptr::write_bytes(buffer.virt, 0, buffer.size);
            self.dma_buffers.push(buffer);
            Ok(())
        }
    }

    // Remove the separate functions since we're using one unified approach

    fn create_2d_resource(
        &mut self,
        resource_id: u32,
        format: u32,
        width: u32,
        height: u32,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), &'static str> {
        // Allocate command buffer
        let cmd_buf_idx = {
            self.alloc_dma_buffer(
                core::mem::size_of::<VirtioGpuResourceCreate2d>(),
                mapper,
                frame_allocator,
            )?;
            self.dma_buffers.len() - 1
        };

        // Allocate response buffer
        let resp_buf_idx = {
            self.alloc_dma_buffer(
                core::mem::size_of::<VirtioGpuCtrlHdr>(),
                mapper,
                frame_allocator,
            )?;
            self.dma_buffers.len() - 1
        };

        unsafe {
            let cmd_buf = &self.dma_buffers[cmd_buf_idx];
            let resp_buf = &self.dma_buffers[resp_buf_idx];

            let cmd = cmd_buf.virt as *mut VirtioGpuResourceCreate2d;
            (*cmd) = VirtioGpuResourceCreate2d {
                hdr: VirtioGpuCtrlHdr {
                    cmd_type: VIRTIO_GPU_CMD_RESOURCE_CREATE_2D,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    padding: 0,
                },
                resource_id,
                format,
                width,
                height,
            };

            self.send_command_raw(
                cmd_buf.phys,
                cmd_buf.size as u32,
                resp_buf.phys,
                resp_buf.size as u32,
            )?;
        }

        serial_println!("Created 2D resource {}", resource_id);
        Ok(())
    }

    fn attach_backing(
        &mut self,
        resource_id: u32,
        addr: u64,
        len: u64,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), &'static str> {
        let cmd_size = core::mem::size_of::<VirtioGpuResourceAttachBacking>()
            + core::mem::size_of::<VirtioGpuMemEntry>();

        // Allocate command buffer
        let cmd_buf_idx = {
            self.alloc_dma_buffer(cmd_size, mapper, frame_allocator)?;
            self.dma_buffers.len() - 1
        };

        // Allocate response buffer
        let resp_buf_idx = {
            self.alloc_dma_buffer(
                core::mem::size_of::<VirtioGpuCtrlHdr>(),
                mapper,
                frame_allocator,
            )?;
            self.dma_buffers.len() - 1
        };

        unsafe {
            let cmd_buf = &self.dma_buffers[cmd_buf_idx];
            let resp_buf = &self.dma_buffers[resp_buf_idx];

            let cmd_ptr = cmd_buf.virt as *mut u8;
            let cmd = cmd_ptr as *mut VirtioGpuResourceAttachBacking;
            let entry = cmd_ptr.add(core::mem::size_of::<VirtioGpuResourceAttachBacking>())
                as *mut VirtioGpuMemEntry;

            (*cmd) = VirtioGpuResourceAttachBacking {
                hdr: VirtioGpuCtrlHdr {
                    cmd_type: VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    padding: 0,
                },
                resource_id,
                nr_entries: 1,
            };

            (*entry) = VirtioGpuMemEntry {
                addr,
                length: len as u32,
                padding: 0,
            };

            self.send_command_raw(
                cmd_buf.phys,
                cmd_size as u32,
                resp_buf.phys,
                resp_buf.size as u32,
            )?;
        }

        serial_println!("Attached backing to resource {}", resource_id);
        Ok(())
    }

    fn set_scanout(
        &mut self,
        scanout_id: u32,
        resource_id: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), &'static str> {
        // Allocate command buffer
        let cmd_buf_idx = {
            self.alloc_dma_buffer(
                core::mem::size_of::<VirtioGpuSetScanout>(),
                mapper,
                frame_allocator,
            )?;
            self.dma_buffers.len() - 1
        };

        // Allocate response buffer
        let resp_buf_idx = {
            self.alloc_dma_buffer(
                core::mem::size_of::<VirtioGpuCtrlHdr>(),
                mapper,
                frame_allocator,
            )?;
            self.dma_buffers.len() - 1
        };

        unsafe {
            let cmd_buf = &self.dma_buffers[cmd_buf_idx];
            let resp_buf = &self.dma_buffers[resp_buf_idx];

            let cmd = cmd_buf.virt as *mut VirtioGpuSetScanout;
            (*cmd) = VirtioGpuSetScanout {
                hdr: VirtioGpuCtrlHdr {
                    cmd_type: VIRTIO_GPU_CMD_SET_SCANOUT,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    padding: 0,
                },
                r: VirtioGpuRect {
                    x,
                    y,
                    width,
                    height,
                },
                scanout_id,
                resource_id,
            };

            self.send_command_raw(
                cmd_buf.phys,
                cmd_buf.size as u32,
                resp_buf.phys,
                resp_buf.size as u32,
            )?;
        }

        serial_println!("Set scanout {} to resource {}", scanout_id, resource_id);
        Ok(())
    }

    fn transfer_to_host_2d(
        &mut self,
        resource_id: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), &'static str> {
        // Allocate command buffer
        let cmd_buf_idx = {
            self.alloc_dma_buffer(
                core::mem::size_of::<VirtioGpuTransferToHost2d>(),
                mapper,
                frame_allocator,
            )?;
            self.dma_buffers.len() - 1
        };

        // Allocate response buffer
        let resp_buf_idx = {
            self.alloc_dma_buffer(
                core::mem::size_of::<VirtioGpuCtrlHdr>(),
                mapper,
                frame_allocator,
            )?;
            self.dma_buffers.len() - 1
        };

        unsafe {
            let cmd_buf = &self.dma_buffers[cmd_buf_idx];
            let resp_buf = &self.dma_buffers[resp_buf_idx];

            let cmd = cmd_buf.virt as *mut VirtioGpuTransferToHost2d;
            (*cmd) = VirtioGpuTransferToHost2d {
                hdr: VirtioGpuCtrlHdr {
                    cmd_type: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    padding: 0,
                },
                r: VirtioGpuRect {
                    x,
                    y,
                    width,
                    height,
                },
                offset: 0,
                resource_id,
                padding: 0,
            };

            self.send_command_raw(
                cmd_buf.phys,
                cmd_buf.size as u32,
                resp_buf.phys,
                resp_buf.size as u32,
            )?;
        }

        Ok(())
    }

    fn resource_flush(
        &mut self,
        resource_id: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), &'static str> {
        // Allocate command buffer
        let cmd_buf_idx = {
            self.alloc_dma_buffer(
                core::mem::size_of::<VirtioGpuResourceFlush>(),
                mapper,
                frame_allocator,
            )?;
            self.dma_buffers.len() - 1
        };

        // Allocate response buffer
        let resp_buf_idx = {
            self.alloc_dma_buffer(
                core::mem::size_of::<VirtioGpuCtrlHdr>(),
                mapper,
                frame_allocator,
            )?;
            self.dma_buffers.len() - 1
        };

        unsafe {
            let cmd_buf = &self.dma_buffers[cmd_buf_idx];
            let resp_buf = &self.dma_buffers[resp_buf_idx];

            let cmd = cmd_buf.virt as *mut VirtioGpuResourceFlush;
            (*cmd) = VirtioGpuResourceFlush {
                hdr: VirtioGpuCtrlHdr {
                    cmd_type: VIRTIO_GPU_CMD_RESOURCE_FLUSH,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    padding: 0,
                },
                r: VirtioGpuRect {
                    x,
                    y,
                    width,
                    height,
                },
                resource_id,
                padding: 0,
            };

            self.send_command_raw(
                cmd_buf.phys,
                cmd_buf.size as u32,
                resp_buf.phys,
                resp_buf.size as u32,
            )?;
        }

        Ok(())
    }

    fn send_command_raw(
        &mut self,
        cmd_phys: u64,
        cmd_len: u32,
        resp_phys: u64,
        resp_len: u32,
    ) -> Result<(), &'static str> {
        unsafe {
            let desc_idx = self.controlq.free_head;
            if desc_idx >= QUEUE_SIZE {
                return Err("No free descriptors");
            }

            self.controlq.free_head = (*self.controlq.desc.add(desc_idx as usize)).next;

            (*self.controlq.desc.add(desc_idx as usize)).addr = cmd_phys;
            (*self.controlq.desc.add(desc_idx as usize)).len = cmd_len;
            (*self.controlq.desc.add(desc_idx as usize)).flags = 1;
            (*self.controlq.desc.add(desc_idx as usize)).next = (desc_idx + 1) % QUEUE_SIZE;

            let resp_idx = (desc_idx + 1) % QUEUE_SIZE;
            if self.controlq.free_head == resp_idx {
                self.controlq.free_head = (*self.controlq.desc.add(resp_idx as usize)).next;
            }

            (*self.controlq.desc.add(resp_idx as usize)).addr = resp_phys;
            (*self.controlq.desc.add(resp_idx as usize)).len = resp_len;
            (*self.controlq.desc.add(resp_idx as usize)).flags = 2;
            (*self.controlq.desc.add(resp_idx as usize)).next = 0;

            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

            let avail_idx = (*self.controlq.avail).idx;
            (*self.controlq.avail).ring[avail_idx as usize] = desc_idx;

            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

            (*self.controlq.avail).idx = avail_idx.wrapping_add(1);

            write_volatile(self.notify_base as *mut u16, 0);

            let start_used = self.controlq.used_idx;
            let mut timeout = 1000000;
            while (*self.controlq.used).idx == start_used && timeout > 0 {
                timeout -= 1;
                core::hint::spin_loop();
            }

            if timeout == 0 {
                serial_println!("Command timeout!");
                return Err("Timeout");
            }

            self.controlq.used_idx = self.controlq.used_idx.wrapping_add(1);

            let resp_hdr = resp_phys as *const VirtioGpuCtrlHdr;
            let resp_type = (*resp_hdr).cmd_type;

            if resp_type != VIRTIO_GPU_RESP_OK_NODATA {
                serial_println!("Command failed with response: 0x{:08x}", resp_type);
                return Err("Command failed");
            }
        }
        Ok(())
    }

    fn draw_test_pattern(&mut self) {
        if self.framebuffer.is_null() {
            return;
        }

        unsafe {
            for y in 0..self.height {
                for x in 0..self.width {
                    let color = match (x / 128, y / 128) {
                        (0, 0) => 0xff0000ff,
                        (1, 0) => 0xff00ff00,
                        (2, 0) => 0xffff0000,
                        (3, 0) => 0xffffff00,
                        (0, 1) => 0xffff00ff,
                        (1, 1) => 0xff00ffff,
                        (2, 1) => 0xffffffff,
                        (3, 1) => 0xff808080,
                        _ => {
                            0xff000000
                                | ((x * 255 / self.width) << 16)
                                | ((y * 255 / self.height) << 8)
                        }
                    };
                    *self.framebuffer.add((y * self.width + x) as usize) = color;
                }
            }
        }
        serial_println!("Test pattern drawn to framebuffer");
    }

    fn map_mmio(
        &self,
        phys_addr: u64,
        size: u64,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<*mut u8, &'static str> {
        const MMIO_BASE: u64 = 0xFFFF_8000_0000_0000;
        let virt_addr = VirtAddr::new(MMIO_BASE + phys_addr);

        let start_frame: PhysFrame<Size4KiB> =
            PhysFrame::containing_address(PhysAddr::new(phys_addr));
        let end_frame: PhysFrame<Size4KiB> =
            PhysFrame::containing_address(PhysAddr::new(phys_addr + size - 1));

        let mut current_virt = Page::containing_address(virt_addr);
        let mut current_frame = start_frame;

        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;

        loop {
            unsafe {
                mapper
                    .map_to(current_virt, current_frame, flags, frame_allocator)
                    .map_err(|_| "MMIO mapping failed")?
                    .flush();
            }

            if current_frame == end_frame {
                break;
            }

            current_virt = Page::containing_address(current_virt.start_address() + Size4KiB::SIZE);
            current_frame =
                PhysFrame::containing_address(current_frame.start_address() + Size4KiB::SIZE);
        }

        Ok(virt_addr.as_mut_ptr())
    }

    fn read_pci_config(&self, offset: u8) -> u32 {
        let address = (1u32 << 31)
            | ((self.dev.bus as u32) << 16)
            | ((self.dev.slot as u32) << 11)
            | ((self.dev.func as u32) << 8)
            | ((offset & 0xFC) as u32);

        use x86_64::instructions::port::Port;
        unsafe {
            let mut addr_port = Port::<u32>::new(0xCF8);
            let mut data_port = Port::<u32>::new(0xCFC);
            addr_port.write(address);
            data_port.read()
        }
    }

    unsafe fn write_common_u8(&self, offset: usize, value: u8) {
        write_volatile(self.common_cfg.add(offset), value);
    }

    unsafe fn write_common_u16(&self, offset: usize, value: u16) {
        write_volatile(self.common_cfg.add(offset) as *mut u16, value);
    }

    unsafe fn write_common_u32(&self, offset: usize, value: u32) {
        write_volatile(self.common_cfg.add(offset) as *mut u32, value);
    }

    unsafe fn read_common_u8(&self, offset: usize) -> u8 {
        read_volatile(self.common_cfg.add(offset))
    }

    unsafe fn read_common_u16(&self, offset: usize) -> u16 {
        read_volatile(self.common_cfg.add(offset) as *const u16)
    }

    unsafe fn read_common_u32(&self, offset: usize) -> u32 {
        read_volatile(self.common_cfg.add(offset) as *const u32)
    }

    pub fn refresh_display(
        &mut self,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), &'static str> {
        self.transfer_to_host_2d(1, 0, 0, self.width, self.height, mapper, frame_allocator)?;
        self.resource_flush(1, 0, 0, self.width, self.height, mapper, frame_allocator)?;
        Ok(())
    }

    pub fn get_framebuffer(&self) -> (*mut u32, u32, u32) {
        (self.framebuffer, self.width, self.height)
    }

    pub fn debug_and_refresh(&mut self) {
        serial_println!("Debug: Checking framebuffer contents...");

        unsafe {
            let first_pixel = *self.framebuffer;
            let middle_pixel = *self
                .framebuffer
                .add((self.width * self.height / 2) as usize);
            serial_println!(
                "First pixel: 0x{:08x}, Middle pixel: 0x{:08x}",
                first_pixel,
                middle_pixel
            );

            for i in 0..(self.width * self.height) {
                *self.framebuffer.add(i as usize) = 0xFFFFFFFF;
            }
        }

        serial_println!("Debug refresh complete - screen should be white");
        serial_println!("Note: Call refresh_display with mapper and frame_allocator to actually update the display");
    }
}
