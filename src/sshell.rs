use crate::interrupts::wait_for_keypress;
use crate::{print, println};

pub fn read_line(buf: &mut [u8]) -> usize {
    let mut i = 0;
    loop {
        let c = wait_for_keypress();
        match c {
            '\n' | '\r' => {
                println!();
                break;
            }
            '\x08' => {
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
    i
}
