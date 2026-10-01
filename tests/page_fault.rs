#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

use core::panic::PanicInfo;

use spin::Once;

use baddcafe_os::idt::{Idt, InterruptStackFrame, read_cr2};
use baddcafe_os::{QemuExitCode, exit_qemu, gdt, serial_print, serial_println};

const PAGE_FAULT: u8 = 14;
const UNMAPPED: u64 = 0xdead_beaf;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    baddcafe_os::serial::SERIAL.lock().init();
    serial_print!("page_fault::write_to_unmapped_page... ");

    gdt::init();
    TEST_IDT
        .call_once(|| {
            let mut idt = Idt::new();
            idt.set_handler_with_error_code(PAGE_FAULT, page_fault_handler);
            idt
        })
        .load();

    // `*mut u8` and not a wider type: a debug build checks alignment before the
    // access, and an unaligned dereference would panic instead of faulting.
    unsafe { *(UNMAPPED as *mut u8) = 42 };

    panic!("execution continued after a page fault");
}

static TEST_IDT: Once<Idt> = Once::new();

extern "x86-interrupt" fn page_fault_handler(_stack_frame: InterruptStackFrame, error_code: u64) {
    assert_eq!(read_cr2(), UNMAPPED);
    // Bit 0 clear: nothing was mapped there. Bit 1 set: the access was a write.
    assert_eq!(error_code & 0b11, 0b10);

    serial_println!("[ok]");
    exit_qemu(QemuExitCode::Success)
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    baddcafe_os::test_panic_handler(info)
}
