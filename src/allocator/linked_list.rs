use core::alloc::{GlobalAlloc, Layout};
use core::{mem, ptr};

use super::{Locked, align_up};
use crate::idt;

/// The header of a free region, stored in the first bytes of the region itself.
struct ListNode {
    size: usize,
    next: Option<&'static mut ListNode>,
}

impl ListNode {
    const fn new(size: usize) -> Self {
        ListNode { size, next: None }
    }

    fn start_addr(&self) -> usize {
        self as *const Self as usize
    }

    fn end_addr(&self) -> usize {
        self.start_addr() + self.size
    }
}

const NODE_SIZE: usize = mem::size_of::<ListNode>();
const NODE_ALIGN: usize = mem::align_of::<ListNode>();

/// Keeps every free region in a singly linked list threaded through the free memory
/// itself, so any freed block can be reused. Adjacent free regions are never merged.
pub struct LinkedListAllocator {
    head: ListNode,
}

impl LinkedListAllocator {
    /// An empty allocator: every request fails until `init`.
    pub const fn new() -> Self {
        LinkedListAllocator {
            head: ListNode::new(0),
        }
    }

    /// # Safety
    ///
    /// `[heap_start, heap_start + heap_size)` must be mapped, writable, and used by
    /// nothing else for as long as this allocator hands out pointers into it.
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        unsafe { self.add_free_region(heap_start, heap_size) }
    }

    /// # Safety
    ///
    /// The region must be mapped, writable, and unused, and stay so while it is on
    /// the list.
    unsafe fn add_free_region(&mut self, addr: usize, size: usize) {
        assert_eq!(align_up(addr, NODE_ALIGN), Some(addr), "free region is misaligned");
        assert!(size >= NODE_SIZE, "free region cannot hold its own header");
        addr.checked_add(size)
            .expect("free region wraps around the address space");

        let mut node = ListNode::new(size);
        node.next = self.head.next.take();
        let node_ptr = addr as *mut ListNode;
        unsafe {
            node_ptr.write(node);
            self.head.next = Some(&mut *node_ptr);
        }
    }

    /// Unlinks the first region that can hold the request, and returns it with the
    /// address the allocation would start at.
    fn find_region(&mut self, size: usize, align: usize) -> Option<(&'static mut ListNode, usize)> {
        let mut current = &mut self.head;
        while let Some(ref mut region) = current.next {
            if let Some(start) = Self::alloc_from_region(region, size, align) {
                let next = region.next.take();
                let found = current.next.take().map(|region| (region, start));
                current.next = next;
                return found;
            }
            current = current.next.as_mut().unwrap();
        }
        None
    }

    /// Any gap left before or after the allocation must be able to hold a `ListNode`,
    /// or it could never go back on the list and would leak.
    fn alloc_from_region(region: &ListNode, size: usize, align: usize) -> Option<usize> {
        let region_start = region.start_addr();
        let mut start = align_up(region_start, align)?;
        if start != region_start && start - region_start < NODE_SIZE {
            start = align_up(region_start.checked_add(NODE_SIZE)?, align)?;
        }
        let end = start.checked_add(size)?;
        if end > region.end_addr() {
            return None;
        }
        let excess = region.end_addr() - end;
        if excess > 0 && excess < NODE_SIZE {
            return None;
        }
        Some(start)
    }

    /// Rounds a request up so that the block, once freed, can hold a `ListNode` and
    /// leaves whatever follows it aligned for the next one. The size is rounded to the
    /// node's alignment only: `Layout::pad_to_align` would round it to the requested
    /// one, turning a 16-byte, page-aligned request into a whole page.
    fn size_align(layout: Layout) -> Option<(usize, usize)> {
        let size = align_up(layout.size(), NODE_ALIGN)?.max(NODE_SIZE);
        Some((size, layout.align().max(NODE_ALIGN)))
    }

    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let Some((size, align)) = Self::size_align(layout) else {
            return ptr::null_mut();
        };
        let Some((region, start)) = self.find_region(size, align) else {
            return ptr::null_mut();
        };
        let (region_start, region_end) = (region.start_addr(), region.end_addr());
        let end = start + size;
        // The region is off the list, so the gaps around the allocation are unused
        // heap memory; `alloc_from_region` made sure each one fits a node.
        unsafe {
            if start > region_start {
                self.add_free_region(region_start, start - region_start);
            }
            if region_end > end {
                self.add_free_region(end, region_end - end);
            }
        }
        start as *mut u8
    }

    /// # Safety
    ///
    /// `ptr` must come from `alloc` on this allocator with the same `layout`, and must
    /// not be used afterwards.
    pub unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let (size, _) = Self::size_align(layout).expect("layout was accepted by alloc");
        unsafe { self.add_free_region(ptr as usize, size) }
    }
}

impl Default for LinkedListAllocator {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl GlobalAlloc for Locked<LinkedListAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        idt::without_interrupts(|| self.lock().alloc(layout))
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        idt::without_interrupts(|| unsafe { self.lock().dealloc(ptr, layout) })
    }
}
