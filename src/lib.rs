#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

pub mod allocator;
pub mod gdt;
pub mod interrupt;
pub mod interrupts;
pub mod memory;
pub mod processor;
pub mod rr;
pub mod serial;
pub mod sshell;
pub mod std_thread;
pub mod task;
pub mod thread_pool;
pub mod timer;
pub mod vga_buffer;

pub fn init() {
    gdt::init();
    interrupts::init_idt();
    unsafe { interrupts::PICS.lock().initialize() };
    x86_64::instructions::interrupts::enable();
}

pub fn hlt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}
