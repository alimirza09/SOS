#![allow(dead_code)]
use crate::drivers::pci::{PciBar, PciBarType, PciDevice};
use crate::serial_println;
use alloc::vec::Vec;
use core::mem::size_of;
use core::ptr::{copy_nonoverlapping, read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};
use x86_64::structures::paging::{
    FrameAllocator, OffsetPageTable, Page, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::structures::paging::{Mapper, PageSize, Translate};
use x86_64::{PhysAddr, VirtAddr};
use core::sync::atomic::AtomicU64;

// Virtio/GPU constants
const VIRTQ_DESC_F_NEXT: u16 = 1;
const VIRTQ_DESC_F_WRITE: u16 = 2;

const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

const VIRTIO_PCI_COMMON_DFSELECT: u32 = 0x00;
const VIRTIO_PCI_COMMON_DF: u32 = 0x04;
const VIRTIO_PCI_COMMON_GFSELECT: u32 = 0x08;
const VIRTIO_PCI_COMMON_GF: u32 = 0x0C;
const VIRTIO_PCI_COMMON_NUMQ: u32 = 0x12;
const VIRTIO_PCI_COMMON_STATUS: u32 = 0x14;
const VIRTIO_PCI_COMMON_Q_SELECT: u32 = 0x16;
const VIRTIO_PCI_COMMON_Q_SIZE: u32 = 0x18;
const VIRTIO_PCI_COMMON_Q_ENABLE: u32 = 0x1C;
const VIRTIO_PCI_COMMON_Q_NOFF: u32 = 0x1E;
const VIRTIO_PCI_COMMON_Q_DESCLO: u32 = 0x20;
const VIRTIO_PCI_COMMON_Q_DESCHI: u32 = 0x24;
const VIRTIO_PCI_COMMON_Q_AVAILLO: u32 = 0x28;
const VIRTIO_PCI_COMMON_Q_AVAILHI: u32 = 0x2C;
const VIRTIO_PCI_COMMON_Q_USEDLO: u32 = 0x30;
const VIRTIO_PCI_COMMON_Q_USEDHI: u32 = 0x34;

const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0104;
const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0107;
const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0108;
const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0109;

const VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM: u32 = 2;
const QUEUE_SIZE: usize = 256;

// Simple control structures
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CtrlHdr {
    pub type_id: u32,
    pub flags: u32,
    pub fence_id: u64,
    pub ctx_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtGpuCmdResourceCreate2d {
    pub header: CtrlHdr,
    pub resource_id: u32,
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtGpuCmdResourceAttachBacking {
    pub header: CtrlHdr,
    pub resource_id: u32,
    pub nr_entries: u32,
    // followed by MemEntry[] inline in the request buffer
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtGpuCmdSetScanout {
    pub header: CtrlHdr,
    pub rect: Rect,
    pub scanout_id: u32,
    pub resource_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtGpuCmdResourceFlush {
    pub header: CtrlHdr,
    pub rect: Rect,
    pub resource_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtGpuCmdTransferToHost2d {
    pub header: CtrlHdr,
    pub rect: Rect,
    pub offset: u32,
    pub resource_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MemEntry {
    pub addr: u64,
    pub length: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Descriptor {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Available {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; QUEUE_SIZE],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UsedElem {
    pub id: u32,
    pub len: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Used {
    pub flags: u16,
    pub idx: u16,
    pub ring: [UsedElem; QUEUE_SIZE],
}

#[derive(Debug, Clone, Copy)]
struct VirtioCapability {
    cap_type: u8,
    bar: u8,
    offset: u32,
    length: u32,
}

impl Default for VirtioCapability {
    fn default() -> Self {
        Self {
            cap_type: 0,
            bar: 0,
            offset: 0,
            length: 0,
        }
    }
}

pub struct VirtioQueue {
    pub descriptors: *mut Descriptor,
    pub available: *mut Available,
    pub used: *mut Used,
    pub queue_size: usize,
    pub next_desc: u16,
    pub last_used_idx: u16,
}

impl VirtioQueue {
    pub fn new(
        desc: *mut Descriptor,
        avail: *mut Available,
        used: *mut Used,
        qsize: usize,
    ) -> Self {
        Self {
            descriptors: desc,
            available: avail,
            used,
            queue_size: qsize,
            next_desc: 0,
            last_used_idx: 0,
        }
    }

    pub fn add_command<T: Copy>(
        &mut self,
        cmd: &T,
        extra: Option<&[u8]>,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> usize {
        let head_idx = self.next_desc as usize;
        unsafe {
            // Build request buffer (cmd + optional extra inline)
            let req_size = size_of::<T>() + extra.map_or(0, |s| s.len());
            let req_virt = alloc_dma_region(req_size, mapper, frame_allocator);
            copy_nonoverlapping(cmd as *const T as *const u8, req_virt, size_of::<T>());
            if let Some(extra_buf) = extra {
                copy_nonoverlapping(
                    extra_buf.as_ptr(),
                    req_virt.add(size_of::<T>()),
                    extra_buf.len(),
                );
            }
            let req_phys = mapper
                .translate_addr(VirtAddr::new(req_virt as u64))
                .expect("translate_addr failed for req_virt")
                .as_u64();

            // Descriptor: request (device reads)
            let head = &mut *self.descriptors.add(head_idx);
            head.addr = req_phys;
            head.len = req_size as u32;
            head.flags = 0; // device reads request
            head.next = 0;

            let mut next_idx = ((head_idx + 1) % self.queue_size) as u16;

            // Response descriptor (device writes here)
            let resp_size = 64usize; // enough for ctrl header and status
            let resp_virt = alloc_dma_region(resp_size, mapper, frame_allocator);
            let resp_phys = mapper
                .translate_addr(VirtAddr::new(resp_virt as u64))
                .expect("translate_addr failed for resp_virt")
                .as_u64();

            let resp_desc = &mut *self.descriptors.add(next_idx as usize);
            resp_desc.addr = resp_phys;
            resp_desc.len = resp_size as u32;
            resp_desc.flags = VIRTQ_DESC_F_WRITE; // device writes response
            resp_desc.next = 0;

            // Chain head -> resp
            head.flags |= VIRTQ_DESC_F_NEXT;
            head.next = next_idx;

            next_idx = ((next_idx as usize + 1) % self.queue_size) as u16;

            // Publish to avail ring
            let avail = &mut *self.available;
            let ring_index = (avail.idx as usize) % self.queue_size;
            write_volatile(&mut avail.ring[ring_index] as *mut u16, head_idx as u16);
            fence(Ordering::SeqCst);
            write_volatile(&mut avail.idx as *mut u16, avail.idx.wrapping_add(1));

            // Advance next_desc
            self.next_desc = next_idx;

            head_idx
        }
    }

    pub fn wait_for_used(&mut self) -> Option<UsedElem> {
        loop {
            let used_idx = unsafe { read_volatile(&(*self.used).idx) };
            if used_idx != self.last_used_idx {
                let elem = unsafe { (*self.used).ring[(self.last_used_idx as usize) % self.queue_size] };
                self.last_used_idx = self.last_used_idx.wrapping_add(1);
                return Some(elem);
            }
            core::hint::spin_loop();
        }
    }

    pub fn wait_for_used_and_print_response<T>(&mut self, cmd: &T) -> Option<()> {
        loop {
            let used_idx = unsafe { read_volatile(&(*self.used).idx) };
            if used_idx != self.last_used_idx {
                let used_elem = unsafe { (*self.used).ring[(self.last_used_idx as usize) % self.queue_size] };
                self.last_used_idx = self.last_used_idx.wrapping_add(1);

                serial_println!("[DEBUG] used ring advanced: id={}, len={}", used_elem.id, used_elem.len);

                let head_idx = used_elem.id as usize;
                unsafe {
                    let desc = &*self.descriptors.add(head_idx);
                    serial_println!("[DEBUG] head descriptor (phys)=0x{:X}, len={}", desc.addr, desc.len);

                    let cmd_ptr = cmd as *const T as *const u32;
                    let cmd_words = core::mem::size_of::<T>() / 4;
                    let words_to_print = if cmd_words > 8 { 8 } else { cmd_words };
                    for i in 0..words_to_print {
                        let v = read_volatile(cmd_ptr.add(i));
                        serial_println!("[DEBUG] cmd+{}: 0x{:08X}", i * 4, v);
                    }
                }

                return Some(());
            }
            core::hint::spin_loop();
        }
    }
}

pub struct VirtioGpu {
    pub dev: PciDevice,
    pub common_cfg_virt: *mut u8,
    pub notify_virt: *mut u8,
    pub isr_virt: *mut u8,
    pub device_cfg_virt: *mut u8,
    pub fb_virt: *mut u8,
    pub capabilities: [Option<VirtioCapability>; 8],
    pub queue: Option<VirtioQueue>,
}

impl VirtioGpu {
    pub fn new(dev: PciDevice) -> Self {
        Self {
            dev,
            common_cfg_virt: core::ptr::null_mut(),
            notify_virt: core::ptr::null_mut(),
            isr_virt: core::ptr::null_mut(),
            device_cfg_virt: core::ptr::null_mut(),
            fb_virt: core::ptr::null_mut(),
            capabilities: [None, None, None, None, None, None, None, None],
            queue: None,
        }
    }

    pub fn init_and_test(
        &mut self,
        mapper: &mut OffsetPageTable,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) {
        serial_println!("Creating VirtioGPU");
        self.dev.enable();
        self.scan_and_parse_capabilities();
        if let Some(bar4) = self.dev.get_bar(4) {
            serial_println!(
                "Mapping BAR4 for VirtIO structures: addr=0x{:X}, size=0x{:X}",
                bar4.address,
                bar4.size
            );
            match unsafe { map_mmio_range(bar4.address, bar4.size, mapper, frame_allocator) } {
                Ok(bar4_base) => {
                    self.map_virtio_structures(bar4_base);
                    self.init_virtio_device();
                    self.setup_queues(mapper, frame_allocator);
                    self.create_simple_framebuffer(mapper, frame_allocator);
                    self.test_display_flow(mapper, frame_allocator);
                }
                Err(e) => {
                    serial_println!("Failed to map BAR4: {}", e);
                }
            }
        } else {
            serial_println!("BAR4 not found - cannot initialize VirtioGPU");
        }
    }

    fn init_virtio_device(&mut self) {
        serial_println!("Setting up Virtio device (modern interface)...");
        if self.common_cfg_virt.is_null() {
            serial_println!("No common config mapped, cannot initialize");
            return;
        }
        unsafe {
            write_volatile(self.common_cfg_virt.add(VIRTIO_PCI_COMMON_STATUS as usize), 0u8);
            write_volatile(self.common_cfg_virt.add(VIRTIO_PCI_COMMON_STATUS as usize), 1u8);
            write_volatile(self.common_cfg_virt.add(VIRTIO_PCI_COMMON_STATUS as usize), 1u8 | 2u8);

            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_DFSELECT as usize)) as *mut u32, 0u32);
            let features_low = read_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_DF as usize)) as *const u32);
            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_DFSELECT as usize)) as *mut u32, 1u32);
            let features_high = read_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_DF as usize)) as *const u32);

            serial_println!("Device features: 0x{:08X}{:08X}", features_high, features_low);

            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_GFSELECT as usize)) as *mut u32, 0u32);
            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_GF as usize)) as *mut u32, features_low);
            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_GFSELECT as usize)) as *mut u32, 1u32);
            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_GF as usize)) as *mut u32, features_high);

            write_volatile(self.common_cfg_virt.add(VIRTIO_PCI_COMMON_STATUS as usize), 1u8 | 2u8 | 8u8);
            let status = read_volatile(self.common_cfg_virt.add(VIRTIO_PCI_COMMON_STATUS as usize));
            if (status & 8u8) == 0 {
                serial_println!("Device rejected features");
                write_volatile(self.common_cfg_virt.add(VIRTIO_PCI_COMMON_STATUS as usize), 128u8);
                return;
            }

            write_volatile(self.common_cfg_virt.add(VIRTIO_PCI_COMMON_STATUS as usize), 1u8 | 2u8 | 8u8 | 4u8);
            serial_println!("Virtio device successfully initialized!");
        }
    }

    fn setup_queues(&mut self, mapper: &mut OffsetPageTable, frame_allocator: &mut impl FrameAllocator<Size4KiB>) {
        serial_println!("Setting up queues...");
        unsafe {
            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_Q_SELECT as usize)) as *mut u16, 0u16);
            let qsize = read_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_Q_SIZE as usize)) as *const u16) as usize;
            if qsize == 0 {
                serial_println!("Queue 0 not available (size 0)");
                return;
            }
            serial_println!("Queue 0 size {}", qsize);

            let desc_size = (size_of::<Descriptor>() * qsize + 0xFFF) & !0xFFF;
            let avail_size = (size_of::<Available>() + 0xFFF) & !0xFFF;
            let used_size = (size_of::<Used>() + 0xFFF) & !0xFFF;

            let desc_virt = alloc_dma_region(desc_size, mapper, frame_allocator);
            let avail_virt = alloc_dma_region(avail_size, mapper, frame_allocator);
            let used_virt = alloc_dma_region(used_size, mapper, frame_allocator);

            let desc_phys = mapper.translate_addr(VirtAddr::new(desc_virt as u64)).unwrap();
            let avail_phys = mapper.translate_addr(VirtAddr::new(avail_virt as u64)).unwrap();
            let used_phys = mapper.translate_addr(VirtAddr::new(used_virt as u64)).unwrap();

            for i in 0..(desc_size / 8) {
                write_volatile((desc_virt as *mut u64).add(i), 0u64);
            }
            for i in 0..(avail_size / 8) {
                write_volatile((avail_virt as *mut u64).add(i), 0u64);
            }
            for i in 0..(used_size / 8) {
                write_volatile((used_virt as *mut u64).add(i), 0u64);
            }

            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_Q_DESCLO as usize)) as *mut u32, (desc_phys.as_u64() & 0xFFFF_FFFF) as u32);
            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_Q_DESCHI as usize)) as *mut u32, ((desc_phys.as_u64() >> 32) & 0xFFFF_FFFF) as u32);
            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_Q_AVAILLO as usize)) as *mut u32, (avail_phys.as_u64() & 0xFFFF_FFFF) as u32);
            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_Q_AVAILHI as usize)) as *mut u32, ((avail_phys.as_u64() >> 32) & 0xFFFF_FFFF) as u32);
            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_Q_USEDLO as usize)) as *mut u32, (used_phys.as_u64() & 0xFFFF_FFFF) as u32);
            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_Q_USEDHI as usize)) as *mut u32, ((used_phys.as_u64() >> 32) & 0xFFFF_FFFF) as u32);

            write_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_Q_ENABLE as usize)) as *mut u16, 1u16);

            let queue = VirtioQueue::new(desc_virt as *mut Descriptor, avail_virt as *mut Available, used_virt as *mut Used, qsize);
            self.queue = Some(queue);

            serial_println!("Queue 0 set up and enabled");
        }
    }

    fn notify_queue(&mut self) {
        unsafe {
            let q_select = read_volatile((self.common_cfg_virt.add(VIRTIO_PCI_COMMON_Q_SELECT as usize)) as *const u16) as u32;
            let notify_multiplier = if !self.notify_virt.is_null() { read_volatile(self.notify_virt as *const u32) as u32 } else { 0u32 };
            let notify_offset = q_select.wrapping_mul(notify_multiplier);
            serial_println!("[DEBUG] notify_queue: q_select={}, multiplier={}, notify_offset={}", q_select, notify_multiplier, notify_offset);
            if let Some(q) = &self.queue {
                let avail_idx = unsafe { read_volatile(&(*q.available).idx) };
                let used_idx = unsafe { read_volatile(&(*q.used).idx) };
                serial_println!("[DEBUG] before notify: avail.idx={}, used.idx={}", avail_idx, used_idx);
            }
            if !self.notify_virt.is_null() {
                let notify_addr = unsafe { self.notify_virt.add(notify_offset as usize) };
                unsafe { write_volatile(notify_addr as *mut u16, q_select as u16) };
                serial_println!("[DEBUG] wrote notify to {:p}", notify_addr);
            } else {
                serial_println!("[DEBUG] notify_virt is null, cannot notify");
            }
        }
    }

    fn create_simple_framebuffer(&mut self, mapper: &mut OffsetPageTable, frame_allocator: &mut impl FrameAllocator<Size4KiB>) {
        serial_println!("Creating simple framebuffer...");
        let width = 1024u32;
        let height = 768u32;
        let fb_size = (width as usize) * (height as usize) * 4usize;

        const FB_VIRT_BASE: u64 = 0xFFFF_9000_1000_0000u64;
        let fb_va = VirtAddr::new(FB_VIRT_BASE);
        let mut page = Page::containing_address(fb_va);
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;

        let pages_needed = (fb_size + (Size4KiB::SIZE as usize - 1)) / (Size4KiB::SIZE as usize);
        unsafe {
            for _ in 0..pages_needed {
                let frame = frame_allocator.allocate_frame().expect("frame alloc for fb failed");
                mapper.map_to(page, frame, flags, frame_allocator).expect("map_to fb").flush();
                page = Page::containing_address(page.start_address() + Size4KiB::SIZE);
            }
            self.fb_virt = fb_va.as_mut_ptr();
            serial_println!("Framebuffer virtual allocated at {:p}, {} bytes", self.fb_virt, fb_size);
        }

        unsafe {
            let pixels = (fb_size / 4) as usize;
            let fb_ptr = self.fb_virt as *mut u32;
            for i in 0..pixels {
                let x = (i % width as usize) as u32;
                let y = (i / width as usize) as u32;
                let color = if x < width / 2 {
                    if y < height / 2 { 0xFF0000FF } else { 0xFF00FF00 }
                } else {
                    if y < height / 2 { 0xFFFF0000 } else { 0xFFFFFF00 }
                };
                write_volatile(fb_ptr.add(i), color);
            }
        }
    }

    fn test_display_flow(&mut self, mapper: &mut OffsetPageTable, frame_allocator: &mut impl FrameAllocator<Size4KiB>) {
        serial_println!("Running display test flow...");
        let resource_id: u32 = 1;

        // Create resource
        let cmd_create = VirtGpuCmdResourceCreate2d { header: CtrlHdr { type_id: VIRTIO_GPU_CMD_RESOURCE_CREATE_2D, flags: 0, fence_id: 0, ctx_id: 0, padding: 0 }, resource_id, format: VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM, width: 1024, height: 768 };
        let q = self.queue.as_mut().unwrap();
        q.add_command(&cmd_create, None, mapper, frame_allocator);
        self.notify_queue();
        let _ = q.wait_for_used_and_print_response(&cmd_create);
        serial_println!("Resource create completed");

        // Attach backing: build request buffer inline (header + MemEntry array)
        let fb_phys = mapper.translate_addr(VirtAddr::new(self.fb_virt as u64)).unwrap();
        let mem_entry = MemEntry { addr: fb_phys.as_u64(), length: (1024u32 * 768u32 * 4u32) as u32, padding: 0 };
        let mut attach_buf: Vec<u8> = Vec::with_capacity(size_of::<VirtGpuCmdResourceAttachBacking>() + size_of::<MemEntry>());
        unsafe { attach_buf.set_len(size_of::<VirtGpuCmdResourceAttachBacking>() + size_of::<MemEntry>()); }
        unsafe {
            let hdr_ptr = attach_buf.as_mut_ptr() as *mut VirtGpuCmdResourceAttachBacking;
            (*hdr_ptr).header = CtrlHdr { type_id: VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING, flags: 0, fence_id: 0, ctx_id: 0, padding: 0 };
            (*hdr_ptr).resource_id = resource_id;
            (*hdr_ptr).nr_entries = 1;
            let entries_ptr = (attach_buf.as_mut_ptr()).add(size_of::<VirtGpuCmdResourceAttachBacking>()) as *mut MemEntry;
            *entries_ptr = mem_entry;
        }
        // send attach backing: since our add_command expects a typed cmd + extra bytes, we'll send the whole buffer as "extra" and pass a dummy header struct
        let dummy_hdr = CtrlHdr { type_id: VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING, flags: 0, fence_id: 0, ctx_id: 0, padding: 0 };
        q.add_command(&dummy_hdr, Some(&attach_buf), mapper, frame_allocator);
        self.notify_queue();
        let _ = q.wait_for_used_and_print_response(&dummy_hdr);
        serial_println!("Attach backing completed");

        // Transfer to host
        let cmd_transfer = VirtGpuCmdTransferToHost2d { header: CtrlHdr { type_id: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D, flags: 0, fence_id: 0, ctx_id: 0, padding: 0 }, rect: Rect { x: 0, y: 0, width: 1024, height: 768 }, offset: 0, resource_id, padding: 0 };
        // mem_entry same as above
        let mem_entry_bytes = unsafe { core::slice::from_raw_parts((&mem_entry as *const MemEntry) as *const u8, size_of::<MemEntry>()) };
        q.add_command(&cmd_transfer, Some(mem_entry_bytes), mapper, frame_allocator);
        self.notify_queue();
        let _ = q.wait_for_used_and_print_response(&cmd_transfer);
        serial_println!("Transfer to host completed");

        // Set scanout
        let cmd_scanout = VirtGpuCmdSetScanout { header: CtrlHdr { type_id: VIRTIO_GPU_CMD_SET_SCANOUT, flags: 0, fence_id: 0, ctx_id: 0, padding: 0 }, rect: Rect { x: 0, y: 0, width: 1024, height: 768 }, scanout_id: 0, resource_id, padding: 0 };
        q.add_command(&cmd_scanout, None, mapper, frame_allocator);
        self.notify_queue();
        let _ = q.wait_for_used_and_print_response(&cmd_scanout);
        serial_println!("Set scanout completed");

        // Flush resource
        let cmd_flush = VirtGpuCmdResourceFlush { header: CtrlHdr { type_id: VIRTIO_GPU_CMD_RESOURCE_FLUSH, flags: 0, fence_id: 0, ctx_id: 0, padding: 0 }, rect: Rect { x: 0, y: 0, width: 1024, height: 768 }, resource_id, padding: 0 };
        q.add_command(&cmd_flush, None, mapper, frame_allocator);
        self.notify_queue();
        let _ = q.wait_for_used_and_print_response(&cmd_flush);
        serial_println!("Flush resource completed");
    }

    fn scan_and_parse_capabilities(&mut self) {
        serial_println!("Parsing VirtIO capabilities...");
        let cap_ptr = self.read_pci_config(0x34) as u8;
        if cap_ptr == 0 { serial_println!("No capabilities found"); return; }
        let mut current = cap_ptr;
        let mut idx = 0usize;
        while current != 0 && idx < 8 {
            let dword = self.read_pci_config(current);
            let cap_id = (dword & 0xFF) as u8;
            let next = ((dword >> 8) & 0xFF) as u8;
            if cap_id == 0x09 {
                let d2 = self.read_pci_config(current.wrapping_add(4));
                let d3 = self.read_pci_config(current.wrapping_add(8));
                let d4 = self.read_pci_config(current.wrapping_add(12));
                let cfg_type = ((dword >> 24) & 0xFF) as u8;
                let bar = (d2 & 0xFF) as u8;
                let offset = d3;
                let length = d4;
                serial_println!("  VirtIO cap {}: type={}, bar={}, offset=0x{:X}, len=0x{:X}", idx, cfg_type, bar, offset, length);
                self.capabilities[idx] = Some(VirtioCapability { cap_type: cfg_type, bar, offset, length });
                idx += 1;
            }
            current = next;
        }
    }

    fn map_virtio_structures(&mut self, bar4_base: *mut u8) {
        for cap in &self.capabilities {
            if let Some(c) = cap {
                if c.bar == 4 {
                    let ptr = unsafe { bar4_base.add(c.offset as usize) };
                    match c.cap_type {
                        VIRTIO_PCI_CAP_COMMON_CFG => { self.common_cfg_virt = ptr; serial_println!("Common config mapped at {:p} (offset 0x{:X})", ptr, c.offset); }
                        VIRTIO_PCI_CAP_NOTIFY_CFG => { self.notify_virt = ptr; serial_println!("Notify structure mapped at {:p} (offset 0x{:X})", ptr, c.offset); }
                        VIRTIO_PCI_CAP_ISR_CFG => { self.isr_virt = ptr; serial_println!("ISR structure mapped at {:p} (offset 0x{:X})", ptr, c.offset); }
                        VIRTIO_PCI_CAP_DEVICE_CFG => { self.device_cfg_virt = ptr; serial_println!("Device config mapped at {:p} (offset 0x{:X})", ptr, c.offset); }
                        _ => {}
                    }
                }
            }
        }
    }

    fn read_pci_config(&self, offset: u8) -> u32 {
        let address: u32 = (1 << 31) | ((self.dev.bus as u32) << 16) | ((self.dev.slot as u32) << 11) | ((self.dev.func as u32) << 8) | ((offset as u32) & 0xFC);
        use x86_64::instructions::port::Port;
        unsafe { let mut addr_port = Port::new(0xCF8); let mut data_port = Port::new(0xCFC); addr_port.write(address); data_port.read() }
    }
}

// --- helpers to allocate + map DMA regions ---
static NEXT_DMA_VIRT_ADDR: AtomicU64 = AtomicU64::new(0xFFFF_9000_2000_0000u64);

unsafe fn alloc_dma_region(
    size: usize,
    mapper: &mut OffsetPageTable,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> *mut u8 {
    let num_pages = (size + Size4KiB::SIZE as usize - 1) / Size4KiB::SIZE as usize;
    let aligned = num_pages * Size4KiB::SIZE as usize;
    let base = VirtAddr::new(NEXT_DMA_VIRT_ADDR.fetch_add(aligned as u64, Ordering::SeqCst));
    let mut page = Page::containing_address(base);
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;
    for _ in 0..num_pages {
        let frame = frame_allocator.allocate_frame().expect("alloc_dma_region: no frame");
        mapper.map_to(page, frame, flags, frame_allocator).expect("map_to failed").flush();
        page = Page::containing_address(page.start_address() + Size4KiB::SIZE);
    }
    base.as_mut_ptr()
}

// small wrapper for callers that only have mapper/frame_allocator in scope
unsafe fn alloc_dma_buffer(
    size: usize,
    mapper: &mut OffsetPageTable,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> *mut u8 {
    alloc_dma_region(size, mapper, frame_allocator)
}

unsafe fn map_mmio_range(
    phys_addr: u64,
    size: u64,
    mapper: &mut OffsetPageTable,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<*mut u8, &'static str> {
    if phys_addr == 0 || size == 0 { return Err("invalid mmio"); }
    let start = PhysAddr::new(phys_addr);
    let end = PhysAddr::new(phys_addr + size - 1);
    let mut frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(start);
    let end_frame = PhysFrame::containing_address(end);
    const MMIO_BASE_HIGH: u64 = 0xFFFF_8000_0000_0000u64;
    let virt_base = VirtAddr::new(MMIO_BASE_HIGH + phys_addr);
    let mut cur_virt = Page::containing_address(virt_base);
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;
    loop {
        mapper.map_to(cur_virt, frame, flags, frame_allocator).map_err(|_| "map_to failed")?.flush();
        if frame == end_frame { break; }
        let next = frame.start_address().as_u64() + Size4KiB::SIZE;
        frame = PhysFrame::containing_address(PhysAddr::new(next));
        cur_virt = Page::containing_address(cur_virt.start_address() + Size4KiB::SIZE);
    }
    Ok(virt_base.as_mut_ptr())
}

