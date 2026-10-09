use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use super::{Locked, align_up};
use crate::idt;

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

unsafe impl GlobalAlloc for Locked<BumpAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        idt::without_interrupts(|| self.lock().alloc(layout))
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        idt::without_interrupts(|| self.lock().dealloc())
    }
}
