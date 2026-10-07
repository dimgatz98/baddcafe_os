#![no_std]
#![cfg_attr(test, no_main)]
#![feature(abi_x86_interrupt)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

pub mod allocator;
pub mod gdt;
pub mod idt;
pub mod keyboard;
pub mod memory;
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
pub extern "C" fn _start(boot_info: &'static bootloader::BootInfo) -> ! {
    init();
    memory::init(boot_info);
    allocator::init_heap().expect("heap initialization failed");
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

/// The VGA text buffer is memory-mapped I/O fixed by the hardware at physical
/// 0xb8000, not ordinary RAM. We have written straight to virtual 0xb8000 since
/// Step 5 and seen correct output on screen, which is only possible if that virtual
/// address really resolves to the same physical one.
#[test_case]
fn vga_buffer_is_identity_mapped() {
    assert_eq!(memory::translate(0xb8000), Some(0xb8000));
}

#[test_case]
fn kernel_code_is_mapped_but_not_identity() {
    let virt = hlt_loop as *const () as u64;
    let phys = memory::translate(virt).expect("kernel code must be mapped to run at all");
    assert_ne!(phys, virt);
    // The page offset is never translated; only the frame above it changes.
    assert_eq!(phys & 0xfff, virt & 0xfff);
}

#[test_case]
fn a_freshly_created_mapping_is_readable_and_writable() {
    const PROBE_VIRT: u64 = 0x1000_0000_0000;
    assert_eq!(
        memory::translate(PROBE_VIRT),
        None,
        "probe address is already mapped to something; pick a different one"
    );

    let mut allocator = memory::frame_allocator();
    let frame = allocator.allocate_frame().expect("no usable frames left");
    unsafe { memory::create_mapping(PROBE_VIRT, frame, memory::WRITABLE, &mut allocator) }
        .expect("mapping the probe page failed");

    assert_eq!(memory::translate(PROBE_VIRT), Some(frame));

    let ptr = PROBE_VIRT as *mut u64;
    unsafe {
        ptr.write_volatile(0xcafe_babe_dead_beef);
        assert_eq!(ptr.read_volatile(), 0xcafe_babe_dead_beef);
    }
}

#[test_case]
fn mapping_the_same_page_twice_is_refused() {
    let mut allocator = memory::frame_allocator();
    let frame = allocator.allocate_frame().expect("no usable frames left");
    let result = unsafe {
        memory::create_mapping(allocator::HEAP_START, frame, memory::WRITABLE, &mut allocator)
    };
    assert_eq!(result, Err(memory::MapError::AlreadyMapped));
}

#[test_case]
fn bump_allocator_aligns_and_reclaims() {
    use allocator::BumpAllocator;
    use core::alloc::Layout;

    // Addresses only: the bump logic never touches the memory it hands out.
    let mut bump = BumpAllocator::new();
    unsafe { bump.init(0x1000, 0x100) };

    let a = bump.alloc(Layout::from_size_align(1, 1).unwrap());
    let b = bump.alloc(Layout::from_size_align(8, 64).unwrap());
    assert_eq!(a as usize, 0x1000);
    assert_eq!(b as usize, 0x1040);

    bump.dealloc();
    bump.dealloc();
    let c = bump.alloc(Layout::from_size_align(1, 1).unwrap());
    assert_eq!(c as usize, 0x1000, "heap not reclaimed after the last dealloc");
}

#[test_case]
fn bump_allocator_refuses_what_does_not_fit() {
    use allocator::BumpAllocator;
    use core::alloc::Layout;

    let mut bump = BumpAllocator::new();
    assert!(bump.alloc(Layout::from_size_align(1, 1).unwrap()).is_null());

    unsafe { bump.init(0x1000, 0x100) };
    assert!(bump.alloc(Layout::from_size_align(0x101, 1).unwrap()).is_null());
    assert!(!bump.alloc(Layout::from_size_align(0x100, 1).unwrap()).is_null());
    assert!(bump.alloc(Layout::from_size_align(1, 1).unwrap()).is_null());
}

#[test_case]
fn bump_allocator_never_wraps_around() {
    use allocator::BumpAllocator;
    use core::alloc::Layout;

    let mut bump = BumpAllocator::new();
    unsafe { bump.init(usize::MAX - 0x20, 0x20) };
    assert!(bump.alloc(Layout::from_size_align(1, 0x40).unwrap()).is_null());
    assert!(bump.alloc(Layout::from_size_align(usize::MAX / 2, 1).unwrap()).is_null());
}
