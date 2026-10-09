pub mod bump;
pub mod fixed_size_block;
pub mod linked_list;

use spin::{Mutex, MutexGuard};

use crate::memory::{self, MapError, NO_EXECUTE, PAGE_SIZE, WRITABLE};
use fixed_size_block::FixedSizeBlockAllocator;

/// An otherwise unused PML4 slot, far from the kernel image, the bootloader's stack
/// and its physical-memory mapping. `init_heap` refuses to map over anything already
/// there, so a collision is an error at boot rather than silent corruption.
pub const HEAP_START: u64 = 0x4444_4444_0000;
pub const HEAP_SIZE: u64 = 1024 * 1024;

#[global_allocator]
static ALLOCATOR: Locked<FixedSizeBlockAllocator> = Locked::new(FixedSizeBlockAllocator::new());

/// Maps every page of the heap range to a fresh frame, writable and non-executable,
/// and hands the range to the global allocator. Must run once, after `memory::init`.
pub fn init_heap() -> Result<(), MapError> {
    let mut frames = memory::frame_allocator();
    for page in (HEAP_START..HEAP_START + HEAP_SIZE).step_by(PAGE_SIZE as usize) {
        let frame = frames.allocate_frame().ok_or(MapError::OutOfFrames)?;
        unsafe { memory::create_mapping(page, frame, WRITABLE | NO_EXECUTE, &mut frames)? };
    }
    // Mapping succeeded for every page, and a second call fails above with
    // `AlreadyMapped`, so the range is handed over exactly once.
    unsafe { ALLOCATOR.lock().init(HEAP_START as usize, HEAP_SIZE as usize) };
    Ok(())
}

/// `GlobalAlloc` takes `&self`, so the allocator needs interior mutability; and
/// because `GlobalAlloc` is a foreign trait, it can only be implemented on a type of
/// ours wrapping the lock, not on `spin::Mutex` directly.
pub struct Locked<A> {
    inner: Mutex<A>,
}

impl<A> Locked<A> {
    pub const fn new(inner: A) -> Self {
        Locked {
            inner: Mutex::new(inner),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, A> {
        self.inner.lock()
    }
}

/// `None` if rounding up would overflow. `align` is a power of two, which `Layout`
/// guarantees, so the mask trick is exact.
fn align_up(addr: usize, align: usize) -> Option<usize> {
    let mask = align - 1;
    Some(addr.checked_add(mask)? & !mask)
}
