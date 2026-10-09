use core::alloc::{GlobalAlloc, Layout};
use core::mem;

use super::Locked;
use super::linked_list::LinkedListAllocator;
use crate::idt;

/// Each size is also the alignment of its blocks, so one block serves any request
/// whose size and alignment both fit in it. Larger requests go to the fallback.
const BLOCK_SIZES: &[usize] = &[8, 16, 32, 64, 128, 256, 512, 1024, 2048];

struct FreeBlock {
    next: Option<&'static mut FreeBlock>,
}

const _: () = assert!(BLOCK_SIZES[0] >= mem::size_of::<FreeBlock>());

/// One free list per block size: allocating and freeing pop and push a list head.
/// Blocks are carved from the fallback on first use and never given back to it.
pub struct FixedSizeBlockAllocator {
    list_heads: [Option<&'static mut FreeBlock>; BLOCK_SIZES.len()],
    fallback: LinkedListAllocator,
}

fn list_index(layout: &Layout) -> Option<usize> {
    let required = layout.size().max(layout.align());
    BLOCK_SIZES.iter().position(|&size| size >= required)
}

impl FixedSizeBlockAllocator {
    /// An empty allocator: every request fails until `init`.
    pub const fn new() -> Self {
        FixedSizeBlockAllocator {
            list_heads: [const { None }; BLOCK_SIZES.len()],
            fallback: LinkedListAllocator::new(),
        }
    }

    /// # Safety
    ///
    /// `[heap_start, heap_start + heap_size)` must be mapped, writable, and used by
    /// nothing else for as long as this allocator hands out pointers into it.
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        unsafe { self.fallback.init(heap_start, heap_size) }
    }

    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let Some(index) = list_index(&layout) else {
            return self.fallback.alloc(layout);
        };
        match self.list_heads[index].take() {
            Some(block) => {
                self.list_heads[index] = block.next.take();
                block as *mut FreeBlock as *mut u8
            }
            None => {
                let size = BLOCK_SIZES[index];
                self.fallback.alloc(Layout::from_size_align(size, size).unwrap())
            }
        }
    }

    /// # Safety
    ///
    /// `ptr` must come from `alloc` on this allocator with the same `layout`, and must
    /// not be used afterwards.
    pub unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let Some(index) = list_index(&layout) else {
            return unsafe { self.fallback.dealloc(ptr, layout) };
        };
        let block = FreeBlock {
            next: self.list_heads[index].take(),
        };
        // `ptr` is a whole block of `BLOCK_SIZES[index]` bytes, aligned to that size.
        let block_ptr = ptr as *mut FreeBlock;
        unsafe {
            block_ptr.write(block);
            self.list_heads[index] = Some(&mut *block_ptr);
        }
    }
}

impl Default for FixedSizeBlockAllocator {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl GlobalAlloc for Locked<FixedSizeBlockAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        idt::without_interrupts(|| self.lock().alloc(layout))
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        idt::without_interrupts(|| unsafe { self.lock().dealloc(ptr, layout) })
    }
}
