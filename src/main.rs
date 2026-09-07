#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

mod vga_buffer;

use core::panic::PanicInfo;
use vga_buffer::{Color, ColorCode, WRITER};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    WRITER.lock().clear_screen();

    #[cfg(test)]
    test_main();

    #[cfg(not(test))]
    banner();

    loop {}
}

#[cfg(not(test))]
fn banner() {
    println!("BaddCafe OS");
    println!();
    println!("Booted by bootloader 0.9.35, long mode, paging on.");
    println!("Screen is memory: {} cells of {} bytes at {:#x}.", 80 * 25, 2, 0xb8000);

    WRITER
        .lock()
        .set_color(ColorCode::new(Color::LightGreen, Color::Black));
    println!("println! works, and it formats: {} + {} = {}", 2, 2, 2 + 2);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    WRITER
        .lock()
        .set_color(ColorCode::new(Color::White, Color::Red));
    println!("{}", info);
    loop {}
}

#[cfg(test)]
fn test_runner(tests: &[&dyn Fn()]) {
    println!("Running {} tests", tests.len());
    for test in tests {
        test();
    }
}

#[test_case]
fn trivial_assertion() {
    print!("trivial assertion... ");
    assert_eq!(1 + 1, 2);
    println!("[ok]");
}
