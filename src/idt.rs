use core::arch::asm;
use core::fmt;
use core::mem::size_of;

use spin::Once;

use crate::{println, serial_println};

const BREAKPOINT: u8 = 3;

/// Present, DPL 0, 64-bit interrupt gate (type 0xe, which also clears IF on entry).
const INTERRUPT_GATE: u16 = 0x8e00;

pub type HandlerFunc = extern "x86-interrupt" fn(InterruptStackFrame);

/// What the CPU pushes before jumping to a handler. Field order is the hardware's.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct InterruptStackFrame {
    pub instruction_pointer: u64,
    pub code_segment: u64,
    pub cpu_flags: u64,
    pub stack_pointer: u64,
    pub stack_segment: u64,
}

impl fmt::Debug for InterruptStackFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("InterruptStackFrame")
            .field("instruction_pointer", &format_args!("{:#x}", self.instruction_pointer))
            .field("code_segment", &format_args!("{:#x}", self.code_segment))
            .field("cpu_flags", &format_args!("{:#x}", self.cpu_flags))
            .field("stack_pointer", &format_args!("{:#x}", self.stack_pointer))
            .field("stack_segment", &format_args!("{:#x}", self.stack_segment))
            .finish()
    }
}

/// The handler address is split across three fields because 64-bit mode widened a
/// 32-bit descriptor without moving what was already there.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct Entry {
    offset_low: u16,
    selector: u16,
    options: u16,
    offset_middle: u16,
    offset_high: u32,
    reserved: u32,
}

impl Entry {
    const fn missing() -> Entry {
        Entry {
            offset_low: 0,
            selector: 0,
            options: 0,
            offset_middle: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    fn new(handler: HandlerFunc) -> Entry {
        let address = handler as u64;
        Entry {
            offset_low: address as u16,
            selector: code_segment(),
            options: INTERRUPT_GATE,
            offset_middle: (address >> 16) as u16,
            offset_high: (address >> 32) as u32,
            reserved: 0,
        }
    }
}

#[repr(C, align(16))]
pub struct Idt {
    entries: [Entry; 256],
}

impl Idt {
    pub const fn new() -> Idt {
        Idt {
            entries: [Entry::missing(); 256],
        }
    }

    pub fn set_handler(&mut self, vector: u8, handler: HandlerFunc) {
        self.entries[vector as usize] = Entry::new(handler);
    }

    /// `&'static` because the CPU keeps reading this table long after `load` returns.
    pub fn load(&'static self) {
        let pointer = DescriptorTablePointer {
            limit: (size_of::<Idt>() - 1) as u16,
            base: self as *const Idt as u64,
        };
        unsafe {
            asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
        }
    }
}

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

fn code_segment() -> u16 {
    let segment: u16;
    unsafe {
        asm!("mov {0:x}, cs", out(reg) segment, options(nomem, nostack, preserves_flags));
    }
    segment
}

pub fn int3() {
    unsafe {
        asm!("int3", options(nomem, nostack));
    }
}

static IDT: Once<Idt> = Once::new();

pub fn init() {
    IDT.call_once(|| {
        let mut idt = Idt::new();
        idt.set_handler(BREAKPOINT, breakpoint_handler);
        idt
    })
    .load();
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
    serial_println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}
