#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

use core::panic::PanicInfo;
use core::ptr;

use spin::Once;

use baddcafe_os::idt::{Idt, InterruptStackFrame};
use baddcafe_os::{QemuExitCode, exit_qemu, gdt, serial_print, serial_println};

const DOUBLE_FAULT: u8 = 8;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    baddcafe_os::serial::SERIAL.lock().init();
    serial_print!("stack_overflow::double_fault_survives_stack_overflow... ");

    gdt::init();
    TEST_IDT
        .call_once(|| {
            let mut idt = Idt::new();
            idt.set_diverging_handler_with_error_code(DOUBLE_FAULT, double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
            idt
        })
        .load();

    stack_overflow();

    panic!("execution continued after a stack overflow");
}

static TEST_IDT: Once<Idt> = Once::new();

/// The volatile read is not there to do anything: it stops the compiler from turning
/// the self-call into a jump, which would recurse forever without growing the stack.
#[allow(unconditional_recursion)]
fn stack_overflow() {
    stack_overflow();
    unsafe { ptr::read_volatile(&0u8) };
}

extern "x86-interrupt" fn double_fault_handler(
    _stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    serial_println!("[ok]");
    exit_qemu(QemuExitCode::Success)
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    baddcafe_os::test_panic_handler(info)
}
