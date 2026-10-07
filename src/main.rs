#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(baddcafe_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;

use bootloader::BootInfo;

use baddcafe_os::vga_buffer::WRITER;
use baddcafe_os::{allocator, hlt_loop, memory};

#[unsafe(no_mangle)]
pub extern "C" fn _start(boot_info: &'static BootInfo) -> ! {
    baddcafe_os::init();
    memory::init(boot_info);
    allocator::init_heap().expect("heap initialization failed");
    WRITER.lock().clear_screen();

    #[cfg(test)]
    test_main();

    #[cfg(not(test))]
    banner();

    hlt_loop()
}

#[cfg(not(test))]
fn banner() {
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    use baddcafe_os::{idt, println, serial_println};
    use baddcafe_os::vga_buffer::{Color, ColorCode};

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

    let squares: Vec<u64> = (1..=8).map(|n| n * n).collect();
    let boxed = Box::new(0xbadd_cafe_u64);
    println!("Heap: {:?}, {:#x} at {:p}", squares, *boxed, boxed);

    println!();
    println!("Timer and keyboard interrupts are on. Type:");
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use baddcafe_os::println;
    use baddcafe_os::vga_buffer::{Color, ColorCode};

    WRITER
        .lock()
        .set_color(ColorCode::new(Color::White, Color::Red));
    println!("{}", info);
    hlt_loop()
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    baddcafe_os::test_panic_handler(info)
}
