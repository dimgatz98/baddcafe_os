use core::arch::asm;

use bootloader::BootInfo;
use bootloader::bootinfo::{MemoryMap, MemoryRegionType};
use spin::Once;

pub const PAGE_SIZE: u64 = 4096;

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const HUGE_PAGE: u64 = 1 << 7;

/// Bits 12-51: the hardware's physical address width. Bits 0-11 are flags, not
/// address, on every entry.
const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

#[derive(Clone, Copy)]
#[repr(transparent)]
struct Entry(u64);

impl Entry {
    fn is_present(&self) -> bool {
        self.0 & PRESENT != 0
    }

    fn is_huge_page(&self) -> bool {
        self.0 & HUGE_PAGE != 0
    }

    fn addr(&self) -> u64 {
        self.0 & ADDR_MASK
    }

    fn set(&mut self, addr: u64, extra_flags: u64) {
        self.0 = (addr & ADDR_MASK) | PRESENT | extra_flags;
    }
}

#[repr(C, align(4096))]
struct Table {
    entries: [Entry; 512],
}

impl Table {
    fn zero(&mut self) {
        for entry in self.entries.iter_mut() {
            entry.0 = 0;
        }
    }
}

/// Bits 12-20, 21-29, 30-38, 39-47 of a virtual address are the PT, PD, PDPT and
/// PML4 index respectively, so `level` 0 is the PT and 3 is the PML4.
fn index(virt: u64, level: u8) -> usize {
    ((virt >> (12 + 9 * level)) & 0x1ff) as usize
}

fn read_cr3() -> u64 {
    let value: u64;
    unsafe {
        asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value & ADDR_MASK
}

fn flush_tlb(virt: u64) {
    unsafe {
        asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags));
    }
}

/// Published by the bootloader through `BootInfo`: physical address `p` is reachable
/// at virtual address `offset + p`, because the bootloader mapped every usable frame
/// there before jumping to us. See `init`.
static PHYSICAL_MEMORY_OFFSET: Once<u64> = Once::new();
static MEMORY_MAP: Once<&'static MemoryMap> = Once::new();

fn offset() -> u64 {
    *PHYSICAL_MEMORY_OFFSET
        .get()
        .expect("memory::init was not called")
}

/// Reads a physical frame as a page table through the bootloader's physical-memory
/// mapping. Sound only because every physical address this module ever hands in here
/// came from CR3 or from a present page-table entry — exactly the addresses the CPU's
/// own page walk would dereference next.
fn table_at(phys_addr: u64) -> &'static Table {
    let virt = offset() + phys_addr;
    unsafe { &*(virt as *const Table) }
}

fn table_at_mut(phys_addr: u64) -> &'static mut Table {
    let virt = offset() + phys_addr;
    unsafe { &mut *(virt as *mut Table) }
}

/// Must run once, before `translate`, `frame_allocator` or `create_mapping`, with the
/// `BootInfo` the bootloader handed to `_start`.
pub fn init(boot_info: &'static BootInfo) {
    PHYSICAL_MEMORY_OFFSET.call_once(|| boot_info.physical_memory_offset);
    MEMORY_MAP.call_once(|| &boot_info.memory_map);
}

/// Walks the same four levels the CPU itself walks for `virt`, through the
/// physical-memory mapping instead of hardware, and returns the physical address it
/// resolves to. `None` means no entry along the way was present — the same condition
/// that would raise a page fault.
pub fn translate(virt: u64) -> Option<u64> {
    let mut table_phys = read_cr3();

    for level in (1..=3).rev() {
        let entry = table_at(table_phys).entries[index(virt, level)];
        if !entry.is_present() {
            return None;
        }
        if entry.is_huge_page() {
            // Level 1 -> a 2 MiB frame, level 2 -> a 1 GiB frame (the bootloader's
            // own physical-memory mapping uses exactly this to cover all of RAM
            // without a PT per 2 MiB). The untranslated low bits widen to match.
            let huge_bits = 12 + 9 * level;
            let frame_mask = (1u64 << huge_bits) - 1;
            return Some((entry.addr() & !frame_mask) | (virt & frame_mask));
        }
        table_phys = entry.addr();
    }

    let entry = table_at(table_phys).entries[index(virt, 0)];
    if !entry.is_present() {
        return None;
    }
    Some(entry.addr() | (virt & 0xfff))
}

/// Hands out physical frames the bootloader's memory map marked usable, one at a
/// time, never reusing one. Re-walks the map from the start on every call, which is
/// wasteful but correct — the same trade-off phil-opp's own first allocator makes,
/// and good enough until a real design replaces it.
pub struct FrameAllocator {
    memory_map: &'static MemoryMap,
    next: usize,
}

impl FrameAllocator {
    fn usable_frames(&self) -> impl Iterator<Item = u64> {
        self.memory_map
            .iter()
            .filter(|region| region.region_type == MemoryRegionType::Usable)
            .flat_map(|region| region.range.start_frame_number..region.range.end_frame_number)
            .map(|frame_number| frame_number * PAGE_SIZE)
    }

    pub fn allocate_frame(&mut self) -> Option<u64> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}

pub fn frame_allocator() -> FrameAllocator {
    FrameAllocator {
        memory_map: MEMORY_MAP.get().expect("memory::init was not called"),
        next: 0,
    }
}

/// Creates a new 4 KiB mapping from `virt` to `frame`, both already page-aligned,
/// allocating whatever intermediate tables are missing from `allocator` and zeroing
/// each one before linking it in — an unzeroed table would hand the next lookup 512
/// entries of garbage that happen to look present.
///
/// # Safety
///
/// `virt` must not already be in use for something else, and `frame` must not be
/// reachable through any other mapping — exactly what `allocator` promises as long as
/// it is only ever fed the bootloader's own memory map.
pub unsafe fn create_mapping(virt: u64, frame: u64, allocator: &mut FrameAllocator) {
    let mut table_phys = read_cr3();

    for level in (1..=3).rev() {
        let table = table_at_mut(table_phys);
        let idx = index(virt, level);
        if !table.entries[idx].is_present() {
            let new_table = allocator
                .allocate_frame()
                .expect("out of physical frames for a new page table");
            table_at_mut(new_table).zero();
            table.entries[idx].set(new_table, WRITABLE);
        }
        table_phys = table.entries[idx].addr();
    }

    table_at_mut(table_phys).entries[index(virt, 0)].set(frame, WRITABLE);
    flush_tlb(virt);
}
