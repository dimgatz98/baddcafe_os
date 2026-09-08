use core::fmt::{self, Write};

use spin::Mutex;

use crate::port::{inb, outb};

const COM1: u16 = 0x3f8;

const DATA: u16 = 0;
const INTERRUPT_ENABLE: u16 = 1;
const FIFO_CONTROL: u16 = 2;
const LINE_CONTROL: u16 = 3;
const MODEM_CONTROL: u16 = 4;
const LINE_STATUS: u16 = 5;

const OUTPUT_EMPTY: u8 = 1 << 5;

pub struct Serial {
    base: u16,
}

impl Serial {
    pub const fn new(base: u16) -> Serial {
        Serial { base }
    }

    /// The chip has more registers than I/O ports, so the two divisor registers are
    /// reachable only while DLAB (bit 7 of the line control register) is set.
    pub fn init(&mut self) {
        unsafe {
            outb(self.base + INTERRUPT_ENABLE, 0x00);
            outb(self.base + LINE_CONTROL, 0x80);
            outb(self.base + DATA, 0x03);
            outb(self.base + INTERRUPT_ENABLE, 0x00);
            outb(self.base + LINE_CONTROL, 0x03);
            outb(self.base + FIFO_CONTROL, 0xc7);
            outb(self.base + MODEM_CONTROL, 0x0b);
        }
    }

    fn write_byte(&mut self, byte: u8) {
        unsafe {
            while inb(self.base + LINE_STATUS) & OUTPUT_EMPTY == 0 {}
            outb(self.base + DATA, byte);
        }
    }
}

impl fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
        Ok(())
    }
}

pub static SERIAL: Mutex<Serial> = Mutex::new(Serial::new(COM1));

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => ($crate::serial::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    SERIAL.lock().write_fmt(args).unwrap();
}
