#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

pub mod arch;
pub mod drivers;
pub mod memory;
pub mod sched;
pub mod sync;
pub mod syscall;
pub mod task;

pub use arch::x86_64::{gdt, interrupts, smp, timer};
pub use drivers::{ata, serial, sshell, vga_buffer};
pub use memory::{allocator, paging};
pub use sched::{context, processor, rr, std_thread, thread_pool};
pub use sync::interrupt;
use x86_64::structures::paging::OffsetPageTable;

use crate::memory::BootInfoFrameAllocator;

pub fn hlt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

use bootloader::BootInfo;
pub fn init(boot_info: &'static BootInfo) -> (BootInfoFrameAllocator, OffsetPageTable<'static>) {
    use x86_64::VirtAddr;

    arch::x86_64::gdt::init();
    arch::x86_64::interrupts::init_idt();
    unsafe { arch::x86_64::interrupts::PICS.lock().initialize() };
    x86_64::instructions::interrupts::enable();

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_map) };
    let mut mapper = unsafe { paging::init(phys_mem_offset, &mut frame_allocator) };
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("Heap initialization failed");

    if let Err(e) = drivers::ata::init_global_filesystem() {
        println!("Failed to init filesystem: {}", e);
    }

    (frame_allocator, mapper)
}
