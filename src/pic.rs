use crate::port::{inb, outb};

pub const PIC1_OFFSET: u8 = 32;
pub const PIC2_OFFSET: u8 = PIC1_OFFSET + 8;

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xa0;
const PIC2_DATA: u16 = 0xa1;

const INIT: u8 = 0x11;
const MODE_8086: u8 = 0x01;
const END_OF_INTERRUPT: u8 = 0x20;

/// Everything masked except IRQ0 (timer), IRQ1 (keyboard) and IRQ2 (the line the
/// second chip is wired to).
const PIC1_MASK: u8 = 0xf8;
const PIC2_MASK: u8 = 0xff;

/// The first eight IRQs default to vectors 8-15, which on a 64-bit CPU are CPU
/// exception vectors — a timer tick would arrive as a double fault. Remapping the
/// two chips above 32 is the whole point of this function.
pub unsafe fn init() {
    unsafe {
        outb(PIC1_COMMAND, INIT);
        io_wait();
        outb(PIC2_COMMAND, INIT);
        io_wait();

        outb(PIC1_DATA, PIC1_OFFSET);
        io_wait();
        outb(PIC2_DATA, PIC2_OFFSET);
        io_wait();

        // How the chips are chained: the slave hangs off the master's IRQ2 line.
        outb(PIC1_DATA, 1 << 2);
        io_wait();
        outb(PIC2_DATA, 2);
        io_wait();

        outb(PIC1_DATA, MODE_8086);
        io_wait();
        outb(PIC2_DATA, MODE_8086);
        io_wait();

        outb(PIC1_DATA, PIC1_MASK);
        outb(PIC2_DATA, PIC2_MASK);
    }
}

/// Until it is acknowledged the controller keeps the line busy and sends nothing
/// more. A missing EOI is a keyboard that answers exactly one keypress.
pub unsafe fn end_of_interrupt(vector: u8) {
    unsafe {
        if vector >= PIC2_OFFSET {
            outb(PIC2_COMMAND, END_OF_INTERRUPT);
        }
        outb(PIC1_COMMAND, END_OF_INTERRUPT);
    }
}

/// The 8259 is older and slower than the bus; a read from an unused port wastes
/// enough time for it to latch the previous command.
unsafe fn io_wait() {
    unsafe { inb(0x80) };
}
