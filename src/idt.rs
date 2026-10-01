use core::arch::asm;
use core::fmt;
use core::mem::size_of;
use core::sync::atomic::{AtomicU64, Ordering};

use spin::Once;

use crate::gdt::{self, DescriptorTablePointer};
use crate::{keyboard, pic, print, println, serial_println};

const BREAKPOINT: u8 = 3;
const DOUBLE_FAULT: u8 = 8;
const PAGE_FAULT: u8 = 14;

pub const TIMER: u8 = pic::PIC1_OFFSET;
pub const KEYBOARD: u8 = pic::PIC1_OFFSET + 1;

/// Present, DPL 0, 64-bit interrupt gate (type 0xe, which also clears IF on entry).
const INTERRUPT_GATE: u16 = 0x8e00;

pub type HandlerFunc = extern "x86-interrupt" fn(InterruptStackFrame);
pub type HandlerFuncWithErrorCode = extern "x86-interrupt" fn(InterruptStackFrame, u64);
pub type DivergingHandlerFuncWithErrorCode =
    extern "x86-interrupt" fn(InterruptStackFrame, u64) -> !;

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

/// Why the access failed, as the CPU pushes it for vector 14.
#[derive(Clone, Copy)]
struct PageFaultErrorCode(u64);

impl PageFaultErrorCode {
    const FLAGS: [(u64, &'static str); 5] = [
        (1 << 0, "PROTECTION_VIOLATION"),
        (1 << 1, "CAUSED_BY_WRITE"),
        (1 << 2, "USER_MODE"),
        (1 << 3, "MALFORMED_TABLE"),
        (1 << 4, "INSTRUCTION_FETCH"),
    ];
}

impl fmt::Debug for PageFaultErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:#07b}", self.0)?;
        let mut separator = " (";
        for (bit, name) in Self::FLAGS {
            if self.0 & bit != 0 {
                f.write_str(separator)?;
                f.write_str(name)?;
                separator = " | ";
            }
        }
        if separator != " (" {
            f.write_str(")")?;
        }
        Ok(())
    }
}

/// The handler address is split across three fields because 64-bit mode widened a
/// 32-bit descriptor without moving what was already there.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Entry {
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

    fn new(address: u64) -> Entry {
        Entry {
            offset_low: address as u16,
            selector: code_segment(),
            options: INTERRUPT_GATE,
            offset_middle: (address >> 16) as u16,
            offset_high: (address >> 32) as u32,
            reserved: 0,
        }
    }

    /// Bits 0-2 of the options word name a stack in the TSS. A non-zero value makes
    /// the CPU switch to that stack before pushing anything, whatever `rsp` held.
    pub fn set_stack_index(&mut self, index: u16) {
        self.options = (self.options & !0b111) | (index + 1);
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

    /// The three setters differ only in the handler signature they accept; a function
    /// type is part of the type in Rust, so one generic entry point is not possible
    /// without erasing exactly the check that makes these safe.
    fn set(&mut self, vector: u8, address: u64) -> &mut Entry {
        let entry = &mut self.entries[vector as usize];
        *entry = Entry::new(address);
        entry
    }

    pub fn set_handler(&mut self, vector: u8, handler: HandlerFunc) -> &mut Entry {
        self.set(vector, handler as u64)
    }

    pub fn set_handler_with_error_code(
        &mut self,
        vector: u8,
        handler: HandlerFuncWithErrorCode,
    ) -> &mut Entry {
        self.set(vector, handler as u64)
    }

    pub fn set_diverging_handler_with_error_code(
        &mut self,
        vector: u8,
        handler: DivergingHandlerFuncWithErrorCode,
    ) -> &mut Entry {
        self.set(vector, handler as u64)
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

fn code_segment() -> u16 {
    let segment: u16;
    unsafe {
        asm!("mov {0:x}, cs", out(reg) segment, options(nomem, nostack, preserves_flags));
    }
    segment
}

/// The CPU writes the address that faulted into CR2, and overwrites it on the next
/// page fault, so a handler must read it before it does anything else that can fault.
pub fn read_cr2() -> u64 {
    let address: u64;
    unsafe {
        asm!("mov {}, cr2", out(reg) address, options(nomem, nostack, preserves_flags));
    }
    address
}

pub fn int3() {
    unsafe {
        asm!("int3", options(nomem, nostack));
    }
}

pub fn enable() {
    unsafe {
        asm!("sti", options(nomem, nostack));
    }
}

pub fn disable() {
    unsafe {
        asm!("cli", options(nomem, nostack));
    }
}

/// Bit 9 of the flags register is IF, the interrupt flag.
pub fn are_enabled() -> bool {
    let flags: u64;
    unsafe {
        asm!("pushfq", "pop {}", out(reg) flags, options(nomem, preserves_flags));
    }
    flags & (1 << 9) != 0
}

/// A handler that takes a lock the interrupted code already holds deadlocks the
/// machine. Holding a lock with interrupts off makes that impossible.
pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let was_enabled = are_enabled();
    if was_enabled {
        disable();
    }
    let result = f();
    if was_enabled {
        enable();
    }
    result
}

static IDT: Once<Idt> = Once::new();
static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn init() {
    IDT.call_once(|| {
        let mut idt = Idt::new();
        idt.set_handler(BREAKPOINT, breakpoint_handler);
        idt.set_diverging_handler_with_error_code(DOUBLE_FAULT, double_fault_handler)
            .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        idt.set_handler_with_error_code(PAGE_FAULT, page_fault_handler);
        idt.set_handler(TIMER, timer_handler);
        idt.set_handler(KEYBOARD, keyboard_handler);
        idt
    })
    .load();
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
    serial_println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

/// There is no returning from a double fault: the CPU does not record enough state
/// to resume the faulting instruction, and trying would triple-fault.
extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    serial_println!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
    panic!("double fault");
}

/// Returning is legal for vector 14 — the faulting instruction is restartable, which is
/// what makes demand paging and copy-on-write possible — but only once the handler has
/// fixed the mapping. We have nothing to fix yet, so we stop instead of faulting again.
extern "x86-interrupt" fn page_fault_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    let address = read_cr2();
    let error_code = PageFaultErrorCode(error_code);
    println!(
        "EXCEPTION: PAGE FAULT\naccessed address: {:#x}\nerror code: {:?}\n{:#?}",
        address, error_code, stack_frame
    );
    serial_println!(
        "EXCEPTION: PAGE FAULT\naccessed address: {:#x}\nerror code: {:?}\n{:#?}",
        address, error_code, stack_frame
    );
    crate::hlt_loop()
}

extern "x86-interrupt" fn timer_handler(_stack_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    unsafe { pic::end_of_interrupt(TIMER) }
}

/// The controller will not raise another key event until its output buffer is read,
/// so port 0x60 must be drained even when the scancode means nothing to us.
extern "x86-interrupt" fn keyboard_handler(_stack_frame: InterruptStackFrame) {
    let scancode = unsafe { crate::port::inb(0x60) };
    if let Some(character) = keyboard::KEYBOARD.lock().decode(scancode) {
        print!("{}", character);
    }
    unsafe { pic::end_of_interrupt(KEYBOARD) }
}
