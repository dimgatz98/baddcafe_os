#![no_std]
#![cfg_attr(test, no_main)]
#![feature(abi_x86_interrupt)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

pub mod gdt;
pub mod idt;
pub mod keyboard;
pub mod pic;
pub mod port;
pub mod serial;
pub mod vga_buffer;

use core::arch::asm;
use core::panic::PanicInfo;

/// Order matters: the GDT must be live before the IDT is built, because an IDT
/// entry stores the code-segment selector that is current when it is written.
pub fn init() {
    serial::SERIAL.lock().init();
    gdt::init();
    idt::init();
    unsafe { pic::init() };
    idt::enable();
}

/// `hlt` parks the CPU until the next interrupt instead of burning it in a spin
/// loop. With interrupts on, the machine still answers the timer and the keyboard.
pub fn hlt_loop() -> ! {
    loop {
        unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

/// QEMU's isa-debug-exit device turns a write to its port into the process exit
/// status `(value << 1) | 1`, which is why the manifest declares 33 as success.
pub fn exit_qemu(code: QemuExitCode) -> ! {
    unsafe { port::outl(0xf4, code as u32) }
    hlt_loop()
}

pub trait Testable {
    fn run(&self);
}

impl<T: Fn()> Testable for T {
    fn run(&self) {
        serial_print!("{}... ", core::any::type_name::<T>());
        self();
        serial_println!("[ok]");
    }
}

pub fn test_runner(tests: &[&dyn Testable]) {
    serial_println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }
    exit_qemu(QemuExitCode::Success)
}

pub fn test_panic_handler(info: &PanicInfo) -> ! {
    serial_println!("[failed]");
    serial_println!("{}", info);
    exit_qemu(QemuExitCode::Failed)
}

#[cfg(test)]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    init();
    test_main();
    hlt_loop()
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    test_panic_handler(info)
}

#[test_case]
fn trivial_assertion() {
    assert_eq!(1 + 1, 2);
}

#[test_case]
fn breakpoint_exception_returns() {
    idt::int3();
}

#[test_case]
fn timer_interrupt_fires() {
    let start = idt::ticks();
    while idt::ticks() == start {}
}

#[test_case]
fn keyboard_decodes_a_shifted_letter() {
    let mut keyboard = keyboard::Keyboard::new();
    assert_eq!(keyboard.decode(0x1e), Some('a'));
    assert_eq!(keyboard.decode(0x2a), None);
    assert_eq!(keyboard.decode(0x1e), Some('A'));
    assert_eq!(keyboard.decode(0xaa), None);
    assert_eq!(keyboard.decode(0x9e), None);
    assert_eq!(keyboard.decode(0x1e), Some('a'));
}
