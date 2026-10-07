use core::arch::asm;

use bootloader::BootInfo;
use bootloader::bootinfo::{MemoryMap, MemoryRegionType};
use spin::{Mutex, MutexGuard, Once};

pub const PAGE_SIZE: u64 = 4096;

const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
const HUGE_PAGE: u64 = 1 << 7;
/// Honoured only because the bootloader sets EFER.NXE; with NXE clear, bit 63 is
/// reserved and setting it would page-fault on every access.
pub const NO_EXECUTE: u64 = 1 << 63;

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

/// One allocator for the whole kernel: two independent ones walking the same memory
/// map would hand out the same frames twice.
static FRAME_ALLOCATOR: Once<Mutex<FrameAllocator>> = Once::new();

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
    FRAME_ALLOCATOR.call_once(|| {
        Mutex::new(FrameAllocator {
            memory_map: &boot_info.memory_map,
            next: 0,
        })
    });
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

/// Not for use from interrupt handlers: one arriving while the lock is held would
/// spin on it forever.
pub fn frame_allocator() -> MutexGuard<'static, FrameAllocator> {
    FRAME_ALLOCATOR
        .get()
        .expect("memory::init was not called")
        .lock()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    Unaligned,
    AlreadyMapped,
    /// An intermediate entry on the way down is a huge page, so there is no table
    /// below it to put a 4 KiB entry in.
    InsideHugePage,
    OutOfFrames,
}

/// Creates a new 4 KiB mapping from `virt` to `frame` with `flags` (on top of
/// `PRESENT`), allocating whatever intermediate tables are missing from `allocator`
/// and zeroing each one before linking it in — an unzeroed table would hand the next
/// lookup 512 entries of garbage that happen to look present.
///
/// Intermediate entries get `WRITABLE` and never `NO_EXECUTE`: the CPU combines the
/// permissions of every level, so restrictions belong on the leaf only.
///
/// # Safety
///
/// `frame` must not be reachable through any other mapping and must not hold anything
/// still in use — exactly what `allocator` promises for the frames it hands out.
pub unsafe fn create_mapping(
    virt: u64,
    frame: u64,
    flags: u64,
    allocator: &mut FrameAllocator,
) -> Result<(), MapError> {
    if !virt.is_multiple_of(PAGE_SIZE) || !frame.is_multiple_of(PAGE_SIZE) {
        return Err(MapError::Unaligned);
    }

    let mut table_phys = read_cr3();

    for level in (1..=3).rev() {
        let table = table_at_mut(table_phys);
        let idx = index(virt, level);
        if !table.entries[idx].is_present() {
            let new_table = allocator.allocate_frame().ok_or(MapError::OutOfFrames)?;
            table_at_mut(new_table).zero();
            table.entries[idx].set(new_table, WRITABLE);
        } else if table.entries[idx].is_huge_page() {
            return Err(MapError::InsideHugePage);
        }
        table_phys = table.entries[idx].addr();
    }

    let leaf = &mut table_at_mut(table_phys).entries[index(virt, 0)];
    if leaf.is_present() {
        return Err(MapError::AlreadyMapped);
    }
    leaf.set(frame, flags);
    flush_tlb(virt);
    Ok(())
}
