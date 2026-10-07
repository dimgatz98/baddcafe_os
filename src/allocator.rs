use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use spin::{Mutex, MutexGuard};

use crate::idt;
use crate::memory::{self, MapError, NO_EXECUTE, PAGE_SIZE, WRITABLE};

/// An otherwise unused PML4 slot, far from the kernel image, the bootloader's stack
/// and its physical-memory mapping. `init_heap` refuses to map over anything already
/// there, so a collision is an error at boot rather than silent corruption.
pub const HEAP_START: u64 = 0x4444_4444_0000;
pub const HEAP_SIZE: u64 = 1024 * 1024;

#[global_allocator]
static ALLOCATOR: Locked<BumpAllocator> = Locked::new(BumpAllocator::new());

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

/// Hands out memory by moving a pointer forward and never reuses a freed block
/// individually: the whole heap is reclaimed only once every allocation is freed.
pub struct BumpAllocator {
    heap_start: usize,
    heap_end: usize,
    next: usize,
    allocations: usize,
}

impl BumpAllocator {
    /// An empty allocator: every request fails until `init`.
    pub const fn new() -> Self {
        BumpAllocator {
            heap_start: 0,
            heap_end: 0,
            next: 0,
            allocations: 0,
        }
    }

    /// # Safety
    ///
    /// `[heap_start, heap_start + heap_size)` must be mapped, writable, and used by
    /// nothing else for as long as this allocator hands out pointers into it.
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        self.heap_start = heap_start;
        self.heap_end = heap_start
            .checked_add(heap_size)
            .expect("heap range wraps around the address space");
        self.next = heap_start;
        self.allocations = 0;
    }

    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let Some(start) = align_up(self.next, layout.align()) else {
            return ptr::null_mut();
        };
        let Some(end) = start.checked_add(layout.size()) else {
            return ptr::null_mut();
        };
        if end > self.heap_end {
            return ptr::null_mut();
        }
        self.next = end;
        self.allocations += 1;
        start as *mut u8
    }

    pub fn dealloc(&mut self) {
        self.allocations = self
            .allocations
            .checked_sub(1)
            .expect("dealloc without a matching alloc");
        if self.allocations == 0 {
            self.next = self.heap_start;
        }
    }
}

impl Default for BumpAllocator {
    fn default() -> Self {
        Self::new()
    }
}

/// `None` if rounding up would overflow. `align` is a power of two, which `Layout`
/// guarantees, so the mask trick is exact.
fn align_up(addr: usize, align: usize) -> Option<usize> {
    let mask = align - 1;
    Some(addr.checked_add(mask)? & !mask)
}

// The lock is taken with interrupts off: an interrupt handler that allocated while
// the interrupted code held the lock would spin on it forever on a single core.
unsafe impl GlobalAlloc for Locked<BumpAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        idt::without_interrupts(|| self.lock().alloc(layout))
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        idt::without_interrupts(|| self.lock().dealloc())
    }
}
