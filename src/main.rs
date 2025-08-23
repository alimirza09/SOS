#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use sos::println;
use sos::sshell::read_line;
use sos::vga_buffer::{clear_screen, set_colors, Color};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    set_colors(Color::Green, Color::Black);
    clear_screen();
    println!("Welcome to sOS");

    sos::init();

    let mut buffer = [0u8; 128];
    let n = read_line(&mut buffer);
    println!("You typed: {}", core::str::from_utf8(&buffer[..n]).unwrap());
    sos::hlt_loop();
}

/// This function is called on panic.
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    sos::hlt_loop();
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    sos::test_panic_handler(info)
}

#[test_case]
fn trivial_assertion() {
    assert_eq!(1, 1);
}
