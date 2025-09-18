#![allow(dead_code)]

use core::fmt;
use lazy_static::lazy_static;
use spin::Mutex;
use volatile::Volatile;
use x86_64::instructions::port::Port;

lazy_static! {
    pub static ref WRITER: Mutex<Writer> = Mutex::new(Writer {
        row_position: 0,
        column_position: 0,
        color_code: ColorCode::new(Color::Yellow, Color::Black),
        buffer: unsafe { &mut *(0xb8000 as *mut Buffer) },
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct ColorCode(u8);

impl ColorCode {
    pub const fn new(foreground: Color, background: Color) -> ColorCode {
        ColorCode((background as u8) << 4 | (foreground as u8))
    }
    pub const fn raw(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct ScreenChar {
    ascii_character: u8,
    color_code: ColorCode,
}

pub const BUFFER_HEIGHT: usize = 25;
pub const BUFFER_WIDTH: usize = 80;

#[repr(transparent)]
struct Buffer {
    chars: [[Volatile<ScreenChar>; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

fn update_cursor(row: usize, col: usize) {
    use x86_64::instructions::port::Port;

    let pos: u16 = (row as u16) * (BUFFER_WIDTH as u16) + (col as u16);
    unsafe {
        let mut crtc_index: Port<u8> = Port::new(0x3D4);
        let mut crtc_data: Port<u8> = Port::new(0x3D5);

        crtc_index.write(0x0F);
        crtc_data.write((pos & 0xFF) as u8);
        crtc_index.write(0x0E);
        crtc_data.write(((pos >> 8) & 0xFF) as u8);
    }
}

pub fn enable_cursor(start: u8, end: u8) {
    unsafe {
        let mut index_port = Port::<u8>::new(0x3D4);
        let mut data_port = Port::<u8>::new(0x3D5);

        index_port.write(0x0A);
        let prev_a: u8 = data_port.read();
        data_port.write((prev_a & 0xC0) | (start & 0x1F));

        index_port.write(0x0B);
        let prev_b: u8 = data_port.read();
        data_port.write((prev_b & 0xE0) | (end & 0x1F));
    }
}

pub fn disable_cursor() {
    unsafe {
        let mut index_port = Port::<u8>::new(0x3D4);
        let mut data_port = Port::<u8>::new(0x3D5);

        index_port.write(0x0A);
        let _ = data_port.read();
        data_port.write(0x20);
    }
}

fn set_cursor_pos_cell(cell: usize) {
    let pos: u16 = cell as u16;
    unsafe {
        let mut index_port = Port::<u8>::new(0x3D4);
        let mut data_port = Port::<u8>::new(0x3D5);

        index_port.write(0x0F);
        data_port.write((pos & 0xFF) as u8);

        index_port.write(0x0E);
        data_port.write((pos >> 8) as u8);
    }
}

fn set_cursor_pos_rc(row: usize, col: usize) {
    set_cursor_pos_cell(row * BUFFER_WIDTH + col);
}

pub struct Writer {
    pub row_position: usize,
    pub column_position: usize,
    color_code: ColorCode,
    buffer: &'static mut Buffer,
}

impl Writer {
    #[inline]
    fn put_at(&mut self, row: usize, col: usize, byte: u8) {
        self.buffer.chars[row][col].write(ScreenChar {
            ascii_character: byte,
            color_code: self.color_code,
        });
    }

    #[inline]
    fn sync_hw_cursor(&self) {
        set_cursor_pos_rc(self.row_position, self.column_position);
    }

    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            b'\r' => self.column_position = 0,
            0x08 => self.backspace(),
            byte => {
                if self.column_position >= BUFFER_WIDTH {
                    self.new_line();
                }
                let row = self.row_position;
                let col = self.column_position;
                self.put_at(row, col, byte);
                self.column_position += 1;
            }
        }
        update_cursor(self.row_position, self.column_position);
    }

    pub fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                0x20..=0x7e | b'\n' | b'\r' | 0x08 => self.write_byte(byte),
                _ => self.write_byte(0xfe),
            }
        }
    }

    fn backspace(&mut self) {
        if self.column_position > 0 {
            self.column_position -= 1;
        } else if self.row_position > 0 {
            self.row_position -= 1;
            self.column_position = BUFFER_WIDTH - 1;
        } else {
            return;
        }
        let row = self.row_position;
        let col = self.column_position;
        self.put_at(row, col, b' ');
        update_cursor(row, col);
    }

    fn new_line(&mut self) {
        if self.row_position < BUFFER_HEIGHT - 1 {
            self.row_position += 1;
            self.column_position = 0;
            return;
        }
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                let chr = self.buffer.chars[row][col].read();
                self.buffer.chars[row - 1][col].write(chr);
            }
        }
        self.clear_row(BUFFER_HEIGHT - 1);
        self.column_position = 0;
        update_cursor(self.row_position, self.column_position);
    }

    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        for col in 0..BUFFER_WIDTH {
            self.buffer.chars[row][col].write(blank);
        }
    }

    pub fn set_color(&mut self, fg: Color, bg: Color) {
        self.color_code = ColorCode::new(fg, bg);
    }

    pub fn get_color(&self) -> (Color, Color) {
        let code = self.color_code.raw();
        let fg = unsafe { core::mem::transmute::<u8, Color>(code & 0x0F) };
        let bg = unsafe { core::mem::transmute::<u8, Color>((code & 0xF0) >> 4) };
        (fg, bg)
    }

    pub fn write_colored(&mut self, s: &str, fg: Color, bg: Color) {
        let old = self.color_code;
        self.set_color(fg, bg);
        self.write_string(s);
        self.color_code = old;
        self.sync_hw_cursor();
    }

    pub fn clear_screen(&mut self) {
        for row in 0..BUFFER_HEIGHT {
            self.clear_row(row);
        }
        self.row_position = 0;
        self.column_position = 0;
        self.sync_hw_cursor();
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::vga_buffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! print_colored {
    ($fg:expr, $bg:expr, $($arg:tt)*) => ({
        use core::fmt::Write;
        use x86_64::instructions::interrupts;
        interrupts::without_interrupts(|| {
            let mut w = $crate::vga_buffer::WRITER.lock();
            w.write_colored(&format!($($arg)*), $fg, $bg);
        });
    });
}

#[macro_export]
macro_rules! println_colored {
    ($fg:expr, $bg:expr) => ($crate::print_colored!($fg, $bg, "\n"));
    ($fg:expr, $bg:expr, $($arg:tt)*) => ($crate::print_colored!($fg, $bg, "{}\n", format_args!($($arg)*)));
}

pub fn set_colors(foreground: Color, background: Color) {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        WRITER.lock().set_color(foreground, background);
    });
}

pub fn get_colors() -> (Color, Color) {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| WRITER.lock().get_color())
}

pub fn clear_screen() {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        WRITER.lock().clear_screen();
    });
    update_cursor(0, 0);
}

pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        let mut w = WRITER.lock();
        w.write_fmt(args).unwrap();
        w.sync_hw_cursor();
    });
}

pub fn init_vga_with_cursor() {
    enable_cursor(0, 15);
    x86_64::instructions::interrupts::without_interrupts(|| {
        let w = WRITER.lock();
        set_cursor_pos_rc(w.row_position, w.column_position);
    });
}
