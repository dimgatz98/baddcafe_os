#![no_std]
#![no_main]

mod vga_buffer;

use core::panic::PanicInfo;
use vga_buffer::{Color, ColorCode, WRITER};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    WRITER.lock().clear_screen();

    println!("BaddCafe OS");
    println!();
    println!("Booted by bootloader 0.9.35, long mode, paging on.");
    println!("Screen is memory: {} cells of {} bytes at {:#x}.", 80 * 25, 2, 0xb8000);

    WRITER
        .lock()
        .set_color(ColorCode::new(Color::LightGreen, Color::Black));
    println!("println! works, and it formats: {} + {} = {}", 2, 2, 2 + 2);

    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    WRITER
        .lock()
        .set_color(ColorCode::new(Color::White, Color::Red));
    println!("{}", info);
    loop {}
}
