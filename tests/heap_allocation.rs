#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(baddcafe_os::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::panic::PanicInfo;

use bootloader::BootInfo;

use baddcafe_os::allocator::{self, HEAP_SIZE, HEAP_START};
use baddcafe_os::memory::{self, MapError};

#[unsafe(no_mangle)]
pub extern "C" fn _start(boot_info: &'static BootInfo) -> ! {
    baddcafe_os::init();
    memory::init(boot_info);
    allocator::init_heap().expect("heap initialization failed");
    test_main();
    baddcafe_os::hlt_loop()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    baddcafe_os::test_panic_handler(info)
}

fn in_heap(addr: usize) -> bool {
    (HEAP_START..HEAP_START + HEAP_SIZE).contains(&(addr as u64))
}

#[test_case]
fn heap_is_mapped_end_to_end() {
    assert!(memory::translate(HEAP_START).is_some());
    assert!(memory::translate(HEAP_START + HEAP_SIZE - 1).is_some());
    assert_eq!(memory::translate(HEAP_START + HEAP_SIZE), None);
}

#[test_case]
fn second_init_is_refused() {
    assert_eq!(allocator::init_heap(), Err(MapError::AlreadyMapped));
}

#[test_case]
fn box_lives_on_the_heap() {
    let a = Box::new(41u64);
    let b = Box::new(13u64);
    assert_eq!(*a + 1, 42);
    assert_eq!(*b, 13);
    assert!(in_heap(&*a as *const u64 as usize));
}

#[test_case]
fn large_vec_grows_and_keeps_its_contents() {
    let n = 1000u64;
    let v: Vec<u64> = (0..n).collect();
    assert_eq!(v.iter().sum::<u64>(), (n - 1) * n / 2);
    assert!(in_heap(v.as_ptr() as usize));
}

#[test_case]
fn string_formatting_allocates() {
    let s = alloc::format!("{}-{:x}", "baddcafe", 0xbadd_cafe_u32);
    assert_eq!(s, "baddcafe-baddcafe");
    let mut t = String::new();
    t.push_str(&s);
    assert_eq!(t.len(), 17);
}

#[repr(align(4096))]
struct PageAligned([u8; 16]);

#[test_case]
fn over_aligned_allocation_is_honoured() {
    let _small = Box::new(1u8);
    let page = Box::new(PageAligned([0; 16]));
    assert_eq!(&*page as *const PageAligned as usize % 4096, 0);
    assert_eq!(page.0.len(), 16);
}

/// More bytes in total than the heap holds: passes only because each box is freed
/// before the next one, which lets the bump allocator rewind to the start.
#[test_case]
fn freed_memory_is_reused() {
    for i in 0..(HEAP_SIZE / 8 * 2) {
        let x = Box::new(i);
        assert_eq!(*x, i);
    }
}
