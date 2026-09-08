#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

mod idt;
mod port;
mod serial;
mod vga_buffer;

use core::panic::PanicInfo;
use vga_buffer::WRITER;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    serial::SERIAL.lock().init();
    idt::init();
    WRITER.lock().clear_screen();

    #[cfg(test)]
    test_main();

    #[cfg(not(test))]
    banner();

    loop {}
}

#[cfg(not(test))]
fn banner() {
    use vga_buffer::{Color, ColorCode};

    serial_println!("BaddCafe OS: serial is alive on COM1");

    println!("BaddCafe OS");
    println!();
    println!("Booted by bootloader 0.9.35, long mode, paging on.");
    println!("Screen is memory: {} cells of {} bytes at {:#x}.", 80 * 25, 2, 0xb8000);

    WRITER
        .lock()
        .set_color(ColorCode::new(Color::LightGreen, Color::Black));
    println!("println! works, and it formats: {} + {} = {}", 2, 2, 2 + 2);

    WRITER
        .lock()
        .set_color(ColorCode::new(Color::LightGray, Color::Black));
    idt::int3();
    println!("...and we came back from the exception.");
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use vga_buffer::{Color, ColorCode};

    WRITER
        .lock()
        .set_color(ColorCode::new(Color::White, Color::Red));
    println!("{}", info);
    loop {}
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("[failed]");
    serial_println!("{}", info);
    exit_qemu(QemuExitCode::Failed)
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

/// QEMU's isa-debug-exit device turns a write to its port into the process exit
/// status `(value << 1) | 1`, which is why the manifest declares 33 as success.
#[cfg(test)]
pub fn exit_qemu(code: QemuExitCode) -> ! {
    unsafe { port::outl(0xf4, code as u32) }
    loop {}
}

#[cfg(test)]
pub trait Testable {
    fn run(&self);
}

#[cfg(test)]
impl<T: Fn()> Testable for T {
    fn run(&self) {
        serial_print!("{}... ", core::any::type_name::<T>());
        self();
        serial_println!("[ok]");
    }
}

#[cfg(test)]
fn test_runner(tests: &[&dyn Testable]) {
    serial_println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }
    exit_qemu(QemuExitCode::Success)
}

#[test_case]
fn trivial_assertion() {
    assert_eq!(1 + 1, 2);
}

#[test_case]
fn breakpoint_exception_returns() {
    idt::int3();
}
