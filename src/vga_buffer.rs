use core::fmt::{self, Write};
use core::ptr;

use spin::Mutex;

use crate::idt::without_interrupts;
use crate::port::outb;

const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;
const BUFFER_ADDR: *mut ScreenChar = 0xb8000 as *mut ScreenChar;

#[allow(dead_code)]
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
}

/// Field order is the hardware's, not ours: character byte first, attribute second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct ScreenChar {
    ascii_character: u8,
    color_code: ColorCode,
}

pub struct Writer {
    column_position: usize,
    color_code: ColorCode,
}

impl Writer {
    pub const fn new(color_code: ColorCode) -> Writer {
        Writer {
            column_position: 0,
            color_code,
        }
    }

    pub fn set_color(&mut self, color_code: ColorCode) {
        self.color_code = color_code;
    }

    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            0x08 => self.backspace(),
            byte => {
                if self.column_position >= BUFFER_WIDTH {
                    self.new_line();
                }

                let row = BUFFER_HEIGHT - 1;
                let col = self.column_position;
                self.write_cell(
                    row,
                    col,
                    ScreenChar {
                        ascii_character: byte,
                        color_code: self.color_code,
                    },
                );
                self.column_position += 1;
            }
        }
        self.update_cursor();
    }

    pub fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                0x08 | b'\n' | 0x20..=0x7e => self.write_byte(byte),
                _ => self.write_byte(0xfe),
            }
        }
    }

    pub fn clear_screen(&mut self) {
        for row in 0..BUFFER_HEIGHT {
            self.clear_row(row);
        }
        self.column_position = 0;
        self.update_cursor();
    }

    pub fn backspace(&mut self) {
        if self.column_position == 0 {
            return;
        }
        self.column_position -= 1;
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        self.write_cell(BUFFER_HEIGHT - 1, self.column_position, blank);
    }

    fn new_line(&mut self) {
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                let character = self.read_cell(row, col);
                self.write_cell(row - 1, col, character);
            }
        }
        self.clear_row(BUFFER_HEIGHT - 1);
        self.column_position = 0;
    }

    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        for col in 0..BUFFER_WIDTH {
            self.write_cell(row, col, blank);
        }
    }

    /// Volatile: nothing in this program ever reads these cells back, so a normal
    /// store would be legal for the compiler to delete. The video hardware is the
    /// reader, and the compiler cannot see it.
    /// The blinking cursor is state inside the card, not a cell in the buffer: it is
    /// drawn wherever these two registers point, whatever we wrote where.
    fn update_cursor(&self) {
        let position = ((BUFFER_HEIGHT - 1) * BUFFER_WIDTH + self.column_position) as u16;
        unsafe {
            outb(0x3d4, 0x0f);
            outb(0x3d5, position as u8);
            outb(0x3d4, 0x0e);
            outb(0x3d5, (position >> 8) as u8);
        }
    }

    fn write_cell(&mut self, row: usize, col: usize, character: ScreenChar) {
        unsafe { ptr::write_volatile(BUFFER_ADDR.add(row * BUFFER_WIDTH + col), character) }
    }

    fn read_cell(&self, row: usize, col: usize) -> ScreenChar {
        unsafe { ptr::read_volatile(BUFFER_ADDR.add(row * BUFFER_WIDTH + col)) }
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

pub static WRITER: Mutex<Writer> =
    Mutex::new(Writer::new(ColorCode::new(Color::LightGray, Color::Black)));

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::vga_buffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    without_interrupts(|| WRITER.lock().write_fmt(args).unwrap());
}
