#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

use core::future::poll_fn;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{Poll, Waker};

use bootloader::BootInfo;
use spin::{Mutex, Once};

use baddcafe_os::idt::{self, Idt, InterruptStackFrame};
use baddcafe_os::task::{Task, executor::Executor};
use baddcafe_os::{
    QemuExitCode, allocator, exit_qemu, memory, pic, port, serial_print, serial_println,
};

static TEST_IDT: Once<Idt> = Once::new();
static TICKS: AtomicUsize = AtomicUsize::new(0);
static WAITER: Mutex<Option<Waker>> = Mutex::new(None);

#[unsafe(no_mangle)]
pub extern "C" fn _start(boot_info: &'static BootInfo) -> ! {
    baddcafe_os::init();
    memory::init(boot_info);
    allocator::init_heap().expect("heap initialization failed");
    serial_print!("executor_interrupt::timer_wakes_a_sleeping_executor... ");
    idt::disable();
    TEST_IDT
        .call_once(|| {
            let mut idt = Idt::new();
            idt.set_handler(idt::TIMER, timer_handler);
            idt
        })
        .load();
    // This test IDT handles only the timer; mask the keyboard and slave PIC.
    unsafe { port::outb(0x21, 0xfe) };
    idt::enable();

    let mut executor = Executor::new();
    executor.spawn(Task::new(async {
        let mut waits = 0;
        for _ in 0..16 {
            let start = TICKS.load(Ordering::Relaxed);
            let mut registered = false;
            poll_fn(|context| {
                idt::without_interrupts(|| {
                    if TICKS.load(Ordering::Relaxed) != start {
                        Poll::Ready(())
                    } else {
                        assert!(!registered, "pending task was polled without a timer wake");
                        registered = true;
                        waits += 1;
                        *WAITER.lock() = Some(context.waker().clone());
                        Poll::Pending
                    }
                })
            })
            .await;
        }
        assert!(waits > 0, "test never suspended for an interrupt");
        serial_println!("[ok]");
        exit_qemu(QemuExitCode::Success)
    }));
    executor.run()
}

extern "x86-interrupt" fn timer_handler(_stack_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    let waker = WAITER.lock().take();
    if let Some(waker) = waker {
        waker.wake();
    }
    unsafe { pic::end_of_interrupt(idt::TIMER) };
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    baddcafe_os::test_panic_handler(info)
}
