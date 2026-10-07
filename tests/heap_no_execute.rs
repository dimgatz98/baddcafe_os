#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

use alloc::boxed::Box;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};

use bootloader::BootInfo;
use spin::Once;

use baddcafe_os::idt::{Idt, InterruptStackFrame, read_cr2};
use baddcafe_os::{QemuExitCode, allocator, exit_qemu, gdt, memory, serial_print, serial_println};

const PAGE_FAULT: u8 = 14;
const RET: u8 = 0xc3;

static TARGET: AtomicU64 = AtomicU64::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn _start(boot_info: &'static BootInfo) -> ! {
    baddcafe_os::serial::SERIAL.lock().init();
    serial_print!("heap_no_execute::jump_into_heap... ");

    gdt::init();
    TEST_IDT
        .call_once(|| {
            let mut idt = Idt::new();
            idt.set_handler_with_error_code(PAGE_FAULT, page_fault_handler);
            idt
        })
        .load();

    memory::init(boot_info);
    allocator::init_heap().expect("heap initialization failed");

    // A single `ret`: if the NX bit were ignored, the call would simply return.
    let code = Box::into_raw(Box::new(RET));
    TARGET.store(code as u64, Ordering::SeqCst);
    let f: extern "C" fn() = unsafe { core::mem::transmute(code) };
    f();

    panic!("executed an instruction fetched from the heap");
}

static TEST_IDT: Once<Idt> = Once::new();

extern "x86-interrupt" fn page_fault_handler(_stack_frame: InterruptStackFrame, error_code: u64) {
    assert_eq!(read_cr2(), TARGET.load(Ordering::SeqCst));
    // Bit 0: the page is present. Bit 4: the access was an instruction fetch. Bit 1
    // clear: not a write.
    assert_eq!(error_code & 0b1_0011, 0b1_0001);

    serial_println!("[ok]");
    exit_qemu(QemuExitCode::Success)
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    baddcafe_os::test_panic_handler(info)
}
