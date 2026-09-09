use core::arch::asm;
use core::cell::UnsafeCell;
use core::mem::size_of;

use spin::Once;

/// Index into the TSS interrupt stack table that the double-fault handler uses.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

const STACK_SIZE: usize = 4096 * 5;

/// Present, DPL 0, executable, 64-bit code segment.
const KERNEL_CODE: u64 = 0x00af_9b00_0000_ffff;

/// In 64-bit mode the task state segment no longer describes a task; it survives
/// only as a place to publish stack pointers the CPU may switch to on its own.
#[repr(C, packed(4))]
pub struct Tss {
    reserved_1: u32,
    privilege_stack_table: [u64; 3],
    reserved_2: u64,
    interrupt_stack_table: [u64; 7],
    reserved_3: u64,
    reserved_4: u16,
    iomap_base: u16,
}

impl Tss {
    const fn new() -> Tss {
        Tss {
            reserved_1: 0,
            privilege_stack_table: [0; 3],
            reserved_2: 0,
            interrupt_stack_table: [0; 7],
            reserved_3: 0,
            reserved_4: 0,
            iomap_base: size_of::<Tss>() as u16,
        }
    }
}

/// The CPU writes to this memory behind the compiler's back, so it cannot be a
/// plain `static` (that may land in a read-only section). `UnsafeCell` is how Rust
/// spells "shared, yet mutable"; `Sync` is the promise that only one CPU uses it.
#[repr(align(16))]
struct Stack(UnsafeCell<[u8; STACK_SIZE]>);

unsafe impl Sync for Stack {}

impl Stack {
    /// x86 stacks grow downwards, so the pointer the CPU wants is the far end.
    fn top(&self) -> u64 {
        self.0.get() as u64 + STACK_SIZE as u64
    }
}

static DOUBLE_FAULT_STACK: Stack = Stack(UnsafeCell::new([0; STACK_SIZE]));

pub struct Gdt {
    table: [u64; 8],
    len: usize,
}

impl Gdt {
    /// Entry 0 must be the null descriptor, so the table starts one slot in.
    const fn new() -> Gdt {
        Gdt {
            table: [0; 8],
            len: 1,
        }
    }

    fn push(&mut self, value: u64) -> usize {
        let index = self.len;
        self.table[index] = value;
        self.len += 1;
        index
    }

    /// A selector is a byte offset into the table, so the index is shifted by 3.
    fn add_code_segment(&mut self) -> u16 {
        (self.push(KERNEL_CODE) as u16) << 3
    }

    /// The TSS descriptor is 16 bytes, not 8: a 64-bit base address does not fit in
    /// the layout inherited from the 32-bit descriptor, so it spills into a second
    /// slot that holds nothing but the upper half of the address.
    fn add_tss(&mut self, tss: &'static Tss) -> u16 {
        let base = tss as *const Tss as u64;
        let mut low = 1 << 47;
        low |= (size_of::<Tss>() - 1) as u64;
        low |= (base & 0xff_ffff) << 16;
        low |= ((base >> 24) & 0xff) << 56;
        low |= 0b1001 << 40;

        let index = self.push(low);
        self.push(base >> 32);
        (index as u16) << 3
    }

    pub fn load(&'static self) {
        let pointer = DescriptorTablePointer {
            limit: (self.len * size_of::<u64>() - 1) as u16,
            base: self.table.as_ptr() as u64,
        };
        unsafe {
            asm!("lgdt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
        }
    }
}

#[repr(C, packed)]
pub struct DescriptorTablePointer {
    pub limit: u16,
    pub base: u64,
}

static TSS: Once<Tss> = Once::new();
static GDT: Once<(Gdt, Selectors)> = Once::new();

struct Selectors {
    code: u16,
    tss: u16,
}

pub fn init() {
    let tss = TSS.call_once(|| {
        let mut tss = Tss::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = DOUBLE_FAULT_STACK.top();
        tss
    });

    let (gdt, selectors) = GDT.call_once(|| {
        let mut gdt = Gdt::new();
        let code = gdt.add_code_segment();
        let tss = gdt.add_tss(tss);
        (gdt, Selectors { code, tss })
    });

    gdt.load();
    unsafe {
        set_code_segment(selectors.code);
        load_task_register(selectors.tss);
    }
}

/// `cs` cannot be assigned with `mov`; the only way to change it is to jump through
/// it. A far return pops the target address and the new selector in one go.
unsafe fn set_code_segment(selector: u16) {
    unsafe {
        asm!(
            "push {selector}",
            "lea {target}, [rip + 2f]",
            "push {target}",
            "retfq",
            "2:",
            selector = in(reg) selector as u64,
            target = lateout(reg) _,
        );
    }
}

unsafe fn load_task_register(selector: u16) {
    unsafe {
        asm!("ltr {0:x}", in(reg) selector, options(nomem, nostack, preserves_flags));
    }
}
