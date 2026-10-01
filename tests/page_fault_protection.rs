#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

use core::panic::PanicInfo;

use spin::Once;

use baddcafe_os::idt::{Idt, InterruptStackFrame, read_cr2};
use baddcafe_os::{QemuExitCode, exit_qemu, gdt, serial_print, serial_println};

const PAGE_FAULT: u8 = 14;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    baddcafe_os::serial::SERIAL.lock().init();
    serial_print!("page_fault_protection::write_to_executable_page... ");

    gdt::init();
    TEST_IDT
        .call_once(|| {
            let mut idt = Idt::new();
            idt.set_handler_with_error_code(PAGE_FAULT, page_fault_handler);
            idt
        })
        .load();

    // A mapped address that is not writable: our own code.
    unsafe { *(_start as *const () as *mut u8) = 0 };

    panic!("execution continued after writing to an executable page");
}

static TEST_IDT: Once<Idt> = Once::new();

extern "x86-interrupt" fn page_fault_handler(_stack_frame: InterruptStackFrame, error_code: u64) {
    assert_eq!(read_cr2(), _start as *const () as u64);
    // Both low bits set: the page is mapped, and a write is what was refused.
    assert_eq!(error_code & 0b11, 0b11);

    serial_println!("[ok]");
    exit_qemu(QemuExitCode::Success)
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    baddcafe_os::test_panic_handler(info)
}
