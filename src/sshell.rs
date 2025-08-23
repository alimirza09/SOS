use crate::interrupts::wait_for_keypress;
use crate::{print, println};

use x86_64::instructions::port::Port;
pub fn read_line(buf: &mut [u8]) -> usize {
    let mut i = 0;
    loop {
        let c = wait_for_keypress();
        match c {
            '\n' | '\r' => {
                println!();
            }
            '\x08' => {
                // backspace
                if i > 0 {
                    i -= 1;
                    print!("\x08");
                }
            }
            _ => {
                if i < buf.len() {
                    buf[i] = c as u8;
                    i += 1;
                    print!("{}", c);
                }
            }
        }
    }
}

pub fn enable_cursor(start: u8, end: u8) {
    unsafe {
        let mut index_port = Port::<u8>::new(0x3D4);
        let mut data_port = Port::<u8>::new(0x3D5);

        index_port.write(0x0A);
        let prev_a: u8 = data_port.read();
        data_port.write((prev_a & 0xC0) | start);

        index_port.write(0x0B);
        let prev_b: u8 = data_port.read();
        data_port.write((prev_b & 0xE0) | end);
    }
}

pub fn disable_cursor() {
    unsafe {
        let mut index_port = Port::<u8>::new(0x3D4);
        let mut data_port = Port::<u8>::new(0x3D5);

        index_port.write(0x0A);
        data_port.write(0x20);
    }
}

pub fn update_cursor(row: usize, col: usize) {
    let pos: u16 = (row * 80 + col) as u16;
    unsafe {
        let mut index_port = Port::<u8>::new(0x3D4);
        let mut data_port = Port::<u8>::new(0x3D5);

        index_port.write(0x0F);
        data_port.write((pos & 0xFF) as u8);

        index_port.write(0x0E);
        data_port.write(((pos >> 8) & 0xFF) as u8);
    }
}
