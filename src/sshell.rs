use crate::task::keyboard::read_line;
use crate::{print, println};

pub async fn shell() {
    let mut buf = [0u8; 1024];

    let mut i = 0;
    loop {
        let c = read_line().await.unwrap();
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
    // i
}
