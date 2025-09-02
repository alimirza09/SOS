#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

pub mod arch;
pub mod drivers;
pub mod memory;
pub mod sched;
pub mod sync;
pub mod task;

pub use arch::x86_64::{gdt, interrupts, smp, timer};
pub use drivers::{serial, sshell, vga_buffer};
pub use memory::{allocator, paging};
pub use sched::{context, processor, rr, std_thread, thread_pool};
pub use sync::interrupt;

pub fn init() {
    arch::x86_64::gdt::init();
    arch::x86_64::interrupts::init_idt();
    unsafe { arch::x86_64::interrupts::PICS.lock().initialize() };
    x86_64::instructions::interrupts::enable();
}

pub fn hlt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}
