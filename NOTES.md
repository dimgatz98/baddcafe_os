# BaddCafe OS — Journal

A continuous log of building an x86-64 kernel from nothing, written for someone new
to both bare-metal OS work and Rust. Read the last entry to know where we are; read
from the top to understand how we got here.

Rules for this file live in `AGENTS.md` section 3. Short version: append only, newest
at the bottom, explain the *why*, quote real output, and keep the two sections at the
end current.

Near the end, before `Current state`, there is a **Reference** section: the
configuration files and what each setting is for, every build / inspection / QEMU
command worth remembering, the boot sector decoded byte by byte, and how to read a
register dump. Unlike the numbered steps, that section is kept up to date rather than
appended to.

---

## Step 0 — What "bare metal" actually means

**Goal.** Understand what we are signing up for before writing a line of code.

**Concept.** A normal program never runs alone. When you launch it on Linux, a long
chain of machinery runs first:

```
kernel execve()
  -> jumps to the address in the ELF header field `e_entry`
       -> _start            (assembly, supplied by glibc, not by you)
            -> __libc_start_main   (glibc: sets up TLS, stdio, env, constructors, atexit)
                 -> main             (C ABI)
                      -> lang_start   (Rust's libstd: stack guard page, panic hook, thread info)
                           -> your fn main()
```

Vocabulary, since all of it shows up in error messages later:

- **ELF** — Executable and Linkable Format, the file format of Linux binaries. Its
  header contains **`e_entry`**, the virtual address where execution begins. The
  kernel's loader maps the file into memory and sets the instruction pointer to it.
- **`_start`** — just a *name*, a convention. It is the first function to run. In
  userspace glibc provides it inside an object file called `Scrt1.o`, which the
  linker adds to every Linux binary automatically.
- **`__libc_start_main`** — glibc's real setup routine. `_start` is only about ten
  instructions; it collects `argc`/`argv`/`envp` off the stack and calls this, which
  does the heavy lifting and then calls `main`. When `main` returns, this calls
  `exit()`. So `return 0` from `main` is not a return at all.
- **C ABI** — an *API* says "takes two ints, returns an int". An **ABI** (Application
  Binary Interface) says the same thing at machine level: first argument in register
  `rdi`, second in `rsi`, then `rdx, rcx, r8, r9`; return value in `rax`; stack
  16-byte aligned at the `call`. On Linux x86-64 this is the **System V AMD64 ABI**.
  It is stable and public, which is why different languages can call each other.
  Rust's own ABI is deliberately *unstable* so the compiler can optimise freely.
- **`lang_start`** — Rust's smaller equivalent of `__libc_start_main`, living in
  libstd. It installs the stack-overflow guard page, the panic hook (the thing that
  prints `thread 'main' panicked at ...`), the main thread object, and command-line
  argument storage, then calls your `main`.
- **C-ABI `main` shim** — your `fn main()` is *not* exported as C `main`. `rustc`
  quietly generates a small C-ABI `main` that calls `lang_start`, which calls yours.

On bare metal none of this exists. The bootloader jumps straight to `_start`, and
that is the entire runtime.

**Why it matters.** Every error in Steps 1 and 2 is a piece of this chain complaining
that we removed it.

---

## Step 1 — Freestanding binary, and the entry-point error

**Goal.** Turn the default `cargo new` hello-world into a program that depends on no
operating system, and read the resulting error properly instead of guessing.

**Concept.** Two crate-level attributes cut the chain above at two different points:

- `#![no_std]` drops **libstd**, leaving only **`core`** — the language without an OS:
  types, traits, iterators, but no heap, no I/O, no threads. `lang_start` lives in
  libstd, so it goes too.
- `#![no_main]` tells `rustc` not to emit the `main` shim and not to look for
  `fn main`.

Without libstd there is no default behaviour for `panic!`, so the compiler requires
us to supply one via `#[panic_handler]`.

**What we did.**

```bash
cat > rust-toolchain.toml <<'TOML'
[toolchain]
channel = "nightly"
components = ["rust-src", "llvm-tools-preview"]
TOML

rustup component add rust-src llvm-tools-preview --toolchain nightly
rustc --version                  # rustc 1.100.0-nightly (a69a63265 2026-09-03)
rustup show active-toolchain     # nightly ... (overridden by '.../rust-toolchain.toml')
```

Nightly is pinned per-project, not globally, so this repo cannot accidentally build
with stable. It is required because `-Z build-std` (Step 3) is unstable.

Then `src/main.rs` became `#![no_std]` + `#![no_main]` + a `#[panic_handler]`, with
no `_start` yet — deliberately, to see the error clean.

**What happened (first failure).**

```
error: unwinding panics are not supported without std
  = help: using nightly cargo, use -Zbuild-std with panic="abort" to avoid unwinding
```

Fixed by adding to `Cargo.toml`:

```toml
[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"
```

**Why.** A Rust `panic!` normally *unwinds*: it walks back up the stack running
destructors. That machinery lives in libstd and libunwind, which we no longer link.
`abort` means a panic simply stops the program instead.

**What happened (second failure — the real entry-point error).**

```
error: linking with `cc` failed: exit status: 1
  = note: rust-lld: error: undefined symbol: main
          >>> referenced by Scrt1.o:(_start)
          rust-lld: error: undefined symbol: __libc_start_main
          >>> referenced by Scrt1.o:(_start)
```

To see exactly why, we dumped glibc's startup object:

```bash
$ nm /usr/lib/x86_64-linux-gnu/Scrt1.o
0000000000000000 T _start              # T = defined here
                 U __libc_start_main   # U = undefined, expected from libc.so
                 U main                # U = undefined, expected from YOU
```

And the ELF entry point of any normal binary:

```bash
$ readelf -h /bin/ls
  Entry point address:   0x6d30        # this is e_entry; the linker points it at _start
```

**Why.** `Scrt1.o` is glibc's, not ours, and the linker adds it to every Linux
binary. It defines `_start` and demands two symbols in return: `__libc_start_main`
(gone, because `no_std` means we do not link libc) and `main` (gone, because
`no_main` means `rustc` no longer emits the shim). Those two `U` lines are exactly
our two errors. The error was never about our code — it was glibc's startup code
looking for a runtime we deleted.

**Rust concepts touched.**
- `#![...]` is a *crate-level* attribute (applies to the whole crate); `#[...]`
  applies to the item that follows.
- `core` vs `std`: `std` is `core` plus an OS.
- `panic = "abort"` vs unwinding as a compile-time strategy, not a runtime flag.

---

## Step 2 — Our own `_start`, and the duplicate-symbol wall

**Goal.** Define our own entry point, the function the bootloader will eventually
jump to.

**Concept.** Three things must be true of that function:

1. **It must never return.** There is no caller. In a normal function, `return` pops
   a saved address off the stack into `rip`; here that slot holds garbage and we
   would jump into random memory. Rust expresses this with the **never type**,
   `-> !`, and the compiler *verifies* the function really cannot finish. An infinite
   `loop {}` satisfies it.
2. **It must use the C calling convention.** `extern "C"` says "follow the System V
   AMD64 ABI here". The bootloader is assembly and knows only that convention; Rust's
   own ABI is unstable and could change between compiler versions.
3. **Its symbol name must survive compilation literally.** Rust *mangles* names — it
   encodes the module path, generics, and a hash, so a function ends up in the binary
   as something like `_ZN11baddcafe_os6_start17h4c8f2e1a9d3b7f60E`. Two functions
   with the same name in different modules must not collide. But the bootloader
   searches for the exact string `_start`, so we disable mangling.

On **edition 2024** that attribute is written `#[unsafe(no_mangle)]`, not
`#[no_mangle]`. The `unsafe(...)` wrapper exists because exporting an unmangled
symbol is genuinely hazardous in a way the compiler cannot check: it can silently
collide with another symbol, or be called through a mismatched signature so that
arguments are read from the wrong registers. Edition 2024 made you say out loud that
you accept that. (The collision risk is not theoretical — it is literally what
happens next.)

**What we did.** `src/main.rs`:

```rust
#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
```

**What happened.**

```
error: linking with `cc` failed: exit status: 1
  = note: rust-lld: error: duplicate symbol: _start
```

**Why.** We now have two `_start` symbols: ours, and the one in glibc's `Scrt1.o`.
The linker cannot choose. The root cause is the **target triple** we are building
for:

```
x86_64  -  unknown  -  linux  -  gnu
  ^          ^          ^        ^
 arch     vendor       OS      ABI / libc
```

That trailing `linux-gnu` is what tells the linker "this is a Linux userspace
program", which is why it drags in `Scrt1.o` unasked. No amount of better Rust fixes
this. We need a target that says *there is no OS underneath*.

**Rust concepts touched.**
- **The never type `!`** — a type with no values, meaning "this never produces a
  result". Satisfied only by diverging forever or calling something else that
  diverges.
- **`&PanicInfo` — shared references and borrowing.** `&T` is a pointer, but a
  *non-owning* one. The panic machinery keeps ownership of the `PanicInfo`; our
  handler gets temporary read access, guaranteed valid for the duration of the call.
  We do not free it and must not keep it afterwards. This is the heart of Rust's
  model: who *owns* memory and who has *borrowed* access to it, checked at compile
  time instead of being a convention in a comment, as in C.
- **`PanicInfo` itself** — a struct in `core::panic` describing a panic: the message
  and the source location (file, line, column). When `panic!("x = {}", x)` runs, the
  compiler builds one and calls our `#[panic_handler]` with a reference to it.
- **Name mangling and `unsafe` attributes** in edition 2024.
- `_info`: the leading underscore marks a parameter as intentionally unused and
  silences the warning.

---

## Step 3 — A target of our own, and the death of the C runtime

**Goal.** Stop building for Linux userspace. Describe a machine with no operating
system underneath, so the linker has no reason to add glibc's startup code, and the
duplicate `_start` disappears.

**Concept.** For `rustc`, a *target* is not just "an architecture". It is a set of
answers to questions the compiler cannot guess on its own: how wide is a pointer, how
are values laid out in memory, which linker to invoke, which CPU features are allowed,
what a `panic!` does. Built-in targets carry those answers inside the compiler. Ours
did not exist, so we wrote them down in a JSON file.

The single most important line is `"os": "none"`. That is what stops the linker from
pulling in `Scrt1.o` — everything else in Step 2 was a consequence of the target
saying `linux-gnu`.

**What we did.** First we asked the compiler to describe a bare-metal target it
already knows, as a reference for field names (the blog is written against a much
older `rustc`):

```bash
rustc +nightly -Z unstable-options --print target-spec-json --target x86_64-unknown-none
```

Then wrote `x86_64-baddcafe_os.json`:

```json
{
  "llvm-target": "x86_64-unknown-none",
  "data-layout": "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128",
  "arch": "x86_64",
  "target-endian": "little",
  "target-pointer-width": 64,
  "target-c-int-width": 32,
  "os": "none",
  "executables": true,
  "linker-flavor": "ld.lld",
  "linker": "rust-lld",
  "panic-strategy": "abort",
  "disable-redzone": true,
  "features": "-mmx,-sse,+soft-float",
  "rustc-abi": "softfloat"
}
```

Field by field, the ones that carry weight:

- `data-layout` — a contract with LLVM describing endianness, pointer sizes per
  address space, and alignment of each integer and float width. Copied verbatim from
  the built-in target. One wrong character here silently produces wrong code.
- `os: "none"` — no C runtime, no `Scrt1.o`, no libc.
- `linker: "rust-lld"` with `linker-flavor: "ld.lld"` — use LLVM's own linker
  directly instead of the host `cc`, which would keep assuming Linux conventions.
- `panic-strategy: "abort"` — same reason as Step 1: unwinding lives in libstd.
- `disable-redzone: true` — the red zone is 128 bytes below `rsp` that the System V
  ABI lets a leaf function scribble in without moving the stack pointer. Harmless in
  userspace. Fatal in a kernel: when an interrupt fires, the CPU pushes its own frame
  exactly there and destroys whatever was living in it.
- `features: "-mmx,-sse,+soft-float"` — no SIMD. Those wide registers would have to
  be saved and restored on every interrupt, and enabling SSE also changes how floats
  are passed in the ABI.

And `.cargo/config.toml`:

```toml
[build]
target = "x86_64-baddcafe_os.json"

[unstable]
build-std = ["core", "compiler_builtins"]
build-std-features = ["compiler-builtins-mem"]
json-target-spec = true
```

`build-std` is why we needed nightly and `rust-src`. Rust ships a precompiled `core`
only for targets it already knows; ours was invented five minutes ago, so `core` has
to be compiled from source for it. `compiler-builtins-mem` additionally provides
`memcpy`, `memset`, `memcmp` and friends — the compiler emits calls to them for
things like struct assignment, and normally libc supplies them. There is no libc here.

**What happened (two failures on the way).**

```
error: `.json` target specs require -Zjson-target-spec to be added to the cargo invocation
```

This one is drift from the blog: custom JSON target specs are now gated behind their
own unstable flag. Fixed by adding `json-target-spec = true` to the `[unstable]`
section of `.cargo/config.toml`.

```
error: error loading target specification: target feature `soft-float` is
       incompatible with the ABI but gets enabled in target spec
```

Also drift. Turning off SSE on x86-64 changes how floating-point arguments are passed
— the System V ABI normally passes them in `xmm` registers, which we just disabled.
Modern `rustc` refuses to let the *feature* and the *ABI* disagree silently, so the
target must declare the ABI too: `"rustc-abi": "softfloat"`. The built-in
`x86_64-unknown-none` sets exactly the same field, which is how we knew what to add.

**What happened (success).**

```
   Compiling compiler_builtins v0.1.160
   Compiling core v0.0.0
   Compiling baddcafe_os v0.1.0 (<project>)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 28.91s
```

`core` and `compiler_builtins` being compiled from source is `build-std` doing its
job. Inspecting the result:

```bash
$ readelf -h target/x86_64-baddcafe_os/debug/baddcafe_os
  Type:                              EXEC (Executable file)
  Entry point address:               0x201120

$ nm target/x86_64-baddcafe_os/debug/baddcafe_os | grep ' _start'
0000000000201120 T _start          # one definition now, and it is ours

$ nm -u target/x86_64-baddcafe_os/debug/baddcafe_os
                                   # empty: no undefined symbols at all

$ objdump -d --disassemble=_start target/x86_64-baddcafe_os/debug/baddcafe_os
0000000000201120 <_start>:
  201120:  eb 00     jmp 201122 <_start+0x2>
  201122:  eb fe     jmp 201122 <_start+0x2>
```

`e_entry` (`0x201120`) is exactly the address of our `_start`. The empty `nm -u`
output is the real proof of "freestanding": the binary asks nothing of any library.
And `eb fe` is `loop {}` in two bytes — a jump whose offset is `-2`, i.e. to itself.

**Why.** The whole Step 2 error was the target lying about the machine. Once the
target says there is no OS, glibc's startup object is never linked, so there is only
one `_start` and the linker is happy. Nothing about our Rust code changed.

**Rust concepts touched.**
- **Targets and cross-compilation** — `rustc` is a cross-compiler by default; the
  target is data, not a build of the compiler.
- **`build-std`** — the standard library is just crates; for an unknown target they
  are compiled like any dependency.
- **`core` vs `compiler_builtins`** — `core` is the language; `compiler_builtins`
  supplies the low-level routines (`memcpy`, 128-bit arithmetic, soft float) that
  codegen emits calls to and that libc would otherwise provide.
- **Warning we are carrying:** cargo reports that `core` "contains code that will be
  rejected by a future version of Rust". That is upstream's problem, in the nightly's
  own sources, not ours. Noted, not silenced.

---


## Step 4 — A bootable disk image, and the first real boot

**Goal.** Turn the freestanding ELF from Step 3 into something a machine can actually
start, and watch a real (emulated) CPU execute our `_start`.

**Concept.** The ELF we built is a file, not a bootable disk. Nothing in it puts it
into memory. An x86-64 PC powers on in **real mode**: 16-bit registers, 1 MiB of
addressable memory, no paging, no protection. The firmware reads the first 512 bytes
of the disk — the **boot sector** — to physical address `0x7c00`, checks that its last
two bytes are `0x55 0xAA`, and jumps there. Getting from that to our 64-bit `_start`
means walking the CPU through real mode -> protected mode (32-bit) -> long mode
(64-bit), building a **GDT** (Global Descriptor Table, which describes memory segments
and, in long mode, mainly the code/data privilege levels) and initial **page tables**,
enabling **PAE** (Physical Address Extension) and paging, then parsing our ELF and
loading its segments to the addresses it asks for.

That is a whole project on its own, so we borrow it. The `bootloader` crate is exactly
that code; `bootimage` is a small tool that builds the bootloader, builds our kernel,
and concatenates the two into one raw disk image with `llvm-objcopy`.

Two incompatible generations exist. `bootloader` 0.9.x is what phil-opp's blog uses,
driven by `bootimage`. `bootloader` 0.11.x is a different design: the kernel depends on
`bootloader_api` and declares its entry with an `entry_point!` macro instead of writing
its own `_start`, while a separate *host-side* crate (`BiosBoot` / `UefiBoot` /
`DiskImageBuilder`) builds the image. 0.11 also boots UEFI and hands the kernel a
**framebuffer** rather than a VGA text buffer, which would delete the whole next step
of this project. We chose **0.9.35** deliberately: it keeps our own `_start`, it keeps
the blog aligned with the repo, and moving to 0.11 later is a self-contained exercise
worth doing once we know what a bootloader actually does.

**What we did.**

```toml
# Cargo.toml
[dependencies]
bootloader = "0.9.35"
```

```toml
# .cargo/config.toml
[target.'cfg(target_os = "none")']
runner = "bootimage runner"
```

```bash
cargo build
cargo bootimage
qemu-system-x86_64 -drive format=raw,file=target/x86_64-baddcafe_os/debug/bootimage-baddcafe_os.bin
```

**What happened.** No drift this time — it worked on the first attempt.

```
   Compiling bootloader v0.9.35 (~/.cargo/registry/src/.../bootloader-0.9.35)
    Finished `release` profile [optimized + debuginfo] target(s) in 41.06s
Created bootimage for `baddcafe_os` at
`<project>/target/x86_64-baddcafe_os/debug/bootimage-baddcafe_os.bin`
```

One new warning, and it is a false alarm:

```
warning: unused dependency `bootloader`
```

Our kernel source never mentions the crate, so cargo thinks it is dead weight. It is
not: `bootimage` reads the dependency from `Cargo.toml` to learn which bootloader
version to build. Noted, not silenced.

The image is 50176 bytes. Inspecting its first sector:

```bash
$ xxd -l 16 bootimage-baddcafe_os.bin
00000000: 31c0 8ed8 8ec0 8ed0 8ee0 8ee8 fcbc 007c  1..............|

$ xxd -s 510 -l 2 bootimage-baddcafe_os.bin
000001fe: 55aa                                     U.
```

`31 c0` is `xor ax, ax`; `8e d8` is `mov ds, ax`; `bc 00 7c` is `mov sp, 0x7c00` — the
bootloader zeroing the segment registers and putting its stack just below where it was
loaded. This is 16-bit code, the last 16-bit code this machine will run. `55 aa` at
offset 510 is the signature the firmware looks for.

Booting it headless, with resets logged:

```bash
$ timeout 8 qemu-system-x86_64 -drive format=raw,file=<img> \
      -display none -no-reboot -d cpu_reset -D qemu.log
exit code: 124        # 124 = timeout killed it; QEMU was still alive
```

`qemu.log` contains exactly two `CPU Reset` entries, both from power-on: one with all
registers zero, then the architectural reset state (`CS=f000`, `EIP=0000fff0` — the
reset vector, 16 bytes below the top of the first megabyte). No third reset means no
**triple fault**, which is how a broken kernel announces itself: a fault, then a fault
while handling it, then a fault while handling *that*, and the CPU gives up and reboots.

Then the decisive check — asking the QEMU monitor where the CPU actually is:

```
RAX=0000000000201120  RIP=0000000000201122
CS =0008 ... DPL=0 CS64 [-R-]
CR0=80010011  CR4=00000020  EFER=0000000000000d00
```

**Why.** Every field there is a piece of the story:

- `RIP = 0x201122` is the second instruction of `_start`, the `eb fe` from Step 3 that
  jumps to itself. The CPU is spinning inside our `loop {}`.
- `RAX = 0x201120` is the entry address the bootloader jumped to — the `e_entry` we
  read out of the ELF header in Step 3, unchanged.
- `CS64` means the code segment is 64-bit: we are in **long mode**.
- `EFER = 0xd00` has bit 8 (LME, Long Mode Enable), bit 10 (LMA, Long Mode Active) and
  bit 11 (NXE, No-Execute Enable) set.
- `CR0 = 0x80010011` has bit 31 (PG, paging) and bit 0 (PE, protected mode) set;
  `CR4 = 0x20` is bit 5, PAE. Paging is on and virtual memory exists.

So the borrowed bootloader did all of it: 16-bit real mode -> 32-bit protected mode ->
64-bit long mode, GDT, PAE, page tables, ELF loading, and a jump to us. The screen is
black because `_start` still does nothing but spin. That is the expected result, and
the register dump is the proof of life that the screen cannot give us yet.

Finally, `cargo run` now does all of it in one command:

```
     Running `bootimage runner target/x86_64-baddcafe_os/debug/baddcafe_os`
Running: `qemu-system-x86_64 -drive format=raw,file=.../bootimage-baddcafe_os.bin`
```

Cargo's `runner` key says "instead of executing the built binary, pass it to this
program". Our binary cannot run on Linux at all, so the runner turns it into a disk
image and hands it to QEMU.

**Rust concepts touched.**
- **Dependencies that are not linked.** `bootloader` is in `[dependencies]` but never
  `use`d; it exists so a *build tool* can find it. Cargo's dependency graph and the
  final link graph are not the same graph.
- **Cargo `runner`** — build configuration can redefine what "run" means for a target,
  which is how cross-compiled and bare-metal binaries get tested at all.
- **Nothing in `src/main.rs` changed.** The entire step happened in configuration and
  tooling. Worth remembering when a build breaks: the code is not always the suspect.

---

## Step 5 — The screen is memory

**Goal.** Make the kernel say something on its own. Until now every observation came
from outside the machine: `readelf`, `objdump`, the QEMU monitor. The kernel itself had
never produced a single byte of output.

**Concept — memory-mapped I/O.** In a normal program, printing means calling a library,
which calls the operating system, which talks to a driver. None of that exists here.
What does exist is far more direct: **the graphics hardware watches a region of memory**.
Whatever is written there appears on screen. No call, no device, no protocol — one
`mov` to an address.

That address is `0xb8000`. From it stretches a grid of **25 rows by 80 columns**, two
bytes per cell:

| Byte | Meaning |
|---|---|
| first | The character, in **code page 437** (the IBM PC character set). For letters and digits it coincides with ASCII. |
| second | The colour attribute. Bits 0-3 foreground, bits 4-6 background, bit 7 blink. `0x0f` is white on black. |

Cells are consecutive: cell 0 is the top-left corner at `0xb8000`, cell 1 at `0xb8002`,
and row 1 begins at cell 80.

**What we did.** Created `src/vga_buffer.rs` and rewrote `_start` to use it.

The module defines four types, and each `#[repr(...)]` attribute is load-bearing:

```rust
#[repr(u8)]
pub enum Color { Black = 0, Blue = 1, /* ... */ White = 15 }

#[repr(transparent)]
pub struct ColorCode(u8);

#[repr(C)]
struct ScreenChar {
    ascii_character: u8,
    color_code: ColorCode,
}
```

- `#[repr(u8)]` pins each enum variant to a single byte with the numeric value we wrote.
  Without it, the compiler is free to choose any layout it likes.
- `#[repr(transparent)]` guarantees `ColorCode` is laid out *exactly* like the `u8`
  inside it — no wrapper overhead. The type exists for us, not for the machine.
- `#[repr(C)]` is the important one. Rust normally reserves the right to reorder struct
  fields for better packing. The hardware does not negotiate: character byte first,
  attribute byte second. `repr(C)` means "lay this out the way C would", which is to say
  in declaration order.

The buffer itself is reached through a raw pointer constant:

```rust
const BUFFER_ADDR: *mut ScreenChar = 0xb8000 as *mut ScreenChar;
```

**Why `unsafe` is unavoidable here.** Rust guarantees that safe code cannot cause
undefined behaviour, and it proves this with the ownership and borrowing rules. Handing
it the integer `0xb8000` and calling it an address defeats that machinery completely:
the compiler has no way to know whether that memory exists, whether it is ours, or
whether something else is writing it concurrently. It cannot prove anything.

`unsafe` does **not** switch the checks off — that is the common misreading. It unlocks
a small set of operations the compiler cannot verify (dereferencing raw pointers among
them) and moves the burden of proof onto us. It is a signature, not a switch. Our
invariant here: the VGA text buffer is mapped at this physical address by the firmware,
the bootloader identity-maps it, and nothing else in this kernel touches those cells.

**Why `volatile`.** Every write goes through `core::ptr::write_volatile`, not a plain
store. The reason is that our program writes to those cells and never reads them back.
An optimising compiler sees stores whose results are never observed, concludes they have
no effect, and deletes them. It is entirely right, given what it knows: it cannot see
that a graphics chip is reading that memory. `volatile` means "this access has an effect
you cannot see — perform it exactly as written, do not remove it, do not reorder it".

Note that we used `core::ptr::write_volatile` rather than adding phil-opp's `volatile`
crate. A raw pointer plus the two `ptr` functions covers this case completely, and every
crate in a kernel is code trusted with ring 0.

**The `Writer`.** A small struct holding a column position and the current colour. It
always writes to the **bottom** row, and `new_line()` copies every row up by one and
clears the last. That way the writer never has to track which row it is on, and
scrolling falls out for free. `write_string` filters bytes: anything outside printable
ASCII becomes `0xfe`, a solid block — because a `&str` in Rust is UTF-8 and may contain
bytes this hardware has no glyph for.

Finally:

```rust
impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result { ... }
}
```

`core::fmt::Write` is a trait: a contract saying "this type can accept string slices".
Implementing that one method is enough for the entire formatting machinery — `write!`
and every `{}` placeholder — to start working on our type. This is the first place in
the project where a **trait** buys us something real: `core` already contains all the
formatting code, and it is written against the trait rather than against any concrete
type, so it works on hardware `core` has never heard of.

**What happened.** `cargo build` clean, then a screenshot taken from inside the emulator:

```bash
{ timeout 5 tail -f /dev/null; echo "screendump screen.ppm"; echo "quit"; } \
  | qemu-system-x86_64 -drive format=raw,file=<img> -display none -monitor stdio
```

The image showed, in the bottom two rows, `BaddCafe OS` in light grey and
`VGA text buffer at 0xb8000, 2 bytes per cell` in green. Both `write_string` and
`write!` formatting worked on the first attempt.

**Rust concepts touched.**
- **Raw pointers** (`*mut T`) — pointers with no guarantees, the equivalent of C's
  `unsigned char *`. Creating one is safe; dereferencing one is not.
- **`unsafe`** — unlocks specific unverifiable operations and transfers the proof
  obligation to the programmer.
- **`#[repr(...)]`** — the bridge between Rust's freedom to lay out types however it
  likes and hardware that demands an exact layout.
- **Traits** — `core::fmt::Write` as the seam that lets `core`'s formatting code drive
  a type written years later for a machine with no operating system.
- **Volatile access** — telling the optimiser that a memory access has effects it cannot
  observe.

---

## Step 6 — `println!` of our own

**Goal.** Print from anywhere in the kernel, without threading a `Writer` through every
function that wants to say something.

**Concept — the problem with a global.** `println!` works in normal Rust because there
is one standard output that any code can reach. We want the same: a single global
`Writer`. But a mutable global is exactly what Rust refuses to allow casually, and for
good reason — if two pieces of code write to it at once, the state is garbage. In a
kernel this is not hypothetical: once hardware interrupts are enabled, an interrupt can
fire *in the middle* of a `println!` and try to print from the handler.

The type that expresses "shared, mutable, but only by one at a time" is a **mutex**. The
usual mutex asks the operating system to put a waiting thread to sleep; there is no
operating system here and no threads to sleep. So we use a **spinlock**: a lock that,
when contended, simply loops until the lock is free. Wasteful in general, correct and
minimal here.

**What we did.**

```toml
spin = "0.12.3"
```

```rust
pub static WRITER: Mutex<Writer> =
    Mutex::new(Writer::new(ColorCode::new(Color::LightGray, Color::Black)));
```

A dependency in a kernel deserves justification, and this one has it: `spin` is
maintained by rust-osdev, the same group behind the `bootloader` crate, and a correct
spinlock is smaller than it looks but has real soundness subtleties (`Sync` impls,
atomic orderings) that we are not ready to reason about yet. Writing our own is a good
exercise for later, once atomics have been met properly.

Note that `Mutex::new(...)` appears directly in a `static`. That works because
`Writer::new` and `ColorCode::new` are **`const fn`** — functions the compiler can
evaluate at compile time. This is why the `Writer` holds no pointer field: the buffer
address is a module-level constant instead, so nothing in the struct needs a raw pointer
dereference to construct. phil-opp reaches for the `lazy_static` crate at this point; we
avoided it by making the type const-constructible, and saved a dependency.

Then the macros:

```rust
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::vga_buffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

pub fn _print(args: fmt::Arguments) {
    WRITER.lock().write_fmt(args).unwrap();
}
```

`macro_rules!` is Rust's pattern-matching macro system: `$($arg:tt)*` captures any
sequence of token trees and passes it through untouched. `format_args!` is the built-in
that turns `"{} + {}", 2, 2` into a `fmt::Arguments` value — a description of the work,
carrying references to the arguments, with **no allocation anywhere**. That matters: we
have no heap. `#[macro_export]` puts the macro at the crate root, which is why
`println!` is usable in `main.rs` with no import.

We also gave the panic handler a voice:

```rust
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    WRITER.lock().set_color(ColorCode::new(Color::White, Color::Red));
    println!("{}", info);
    loop {}
}
```

**What happened.** Two screenshots, both taken with `screendump` from the monitor.

Normal boot, bottom of the screen:

```
BaddCafe OS

Booted by bootloader 0.9.35, long mode, paging on.
Screen is memory: 2000 cells of 2 bytes at 0xb8000.
println! works, and it formats: 2 + 2 = 4          <- green
```

Then, with a temporary `panic!("temporary panic test, index {} out of range", 42)`
inserted at the end of `_start` and removed afterwards:

```
panicked at src/main.rs:23:5:                       <- white on red
temporary panic test, index 42 out of range
```

Both the formatting machinery and the failure path work. Untested error handling in a
kernel is a lie, so the panic path was exercised deliberately rather than assumed.

**A known deadlock, recorded rather than fixed.** `_print` is
`WRITER.lock().write_fmt(args).unwrap()`. The temporary lock guard lives until the end
of that statement, so if `unwrap()` ever panicked, the panic handler would try to lock a
mutex that is still held — and spin forever. Today it cannot happen, because our
`write_str` always returns `Ok`. The same shape becomes a genuine hazard once interrupt
handlers can print, and the fix then is to disable interrupts around the lock. Noted
now so it is not discovered the hard way later.

**Rust concepts touched.**
- **`static` and interior mutability** — why a mutable global is refused, and what a
  mutex actually provides.
- **`const fn`** — functions evaluable at compile time, which is what allows a `static`
  to be initialised without lazy initialisation tricks.
- **Declarative macros** (`macro_rules!`) — token-tree capture, multiple match arms,
  and `$crate` for hygiene.
- **`format_args!`** — formatting without allocation, which is the only kind available
  to us.
- **RAII lock guards** — the lock is released when the guard is dropped, and *when*
  that happens is a decision with consequences.

---

## Step 7 — A test framework of our own

**Goal.** Make `cargo test` build and boot a kernel that runs our own tests and reports
them, instead of a human squinting at a screenshot.

**Concept.** `cargo test` normally builds a hidden `main` that lives in `std`: it
collects every `#[test]` function, runs them on threads, catches panics via unwinding,
and returns a process exit code. Every one of those four things is missing here. There
is no `std`, no threads, no unwinder, and nothing above us to receive an exit code.

Nightly offers a hook for exactly this situation: `#![feature(custom_test_frameworks)]`.
It keeps the *collection* half of the machinery — the compiler still gathers annotated
functions across the crate — and hands the result to a function we name. Everything
after that is ours.

Three attributes wire it up:

```rust
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]
```

The collected functions are marked `#[test_case]`, not `#[test]` — `#[test]` belongs to
the std harness. The generated entry point would normally be called `main`; since we are
`#![no_main]` nobody would ever call it, so `reexport_test_harness_main` renames it to
`test_main` and we call it ourselves from `_start`.

**What we did.** Rewrote `src/main.rs`:

- the three attributes above,
- `fn test_runner(tests: &[&dyn Fn()])`, which prints the count and calls each test,
- one `#[test_case] fn trivial_assertion()`,
- split `_start`: `#[cfg(test)] test_main();` versus `#[cfg(not(test))] banner();`,
  where `banner()` holds the four demo lines Step 6 left in `_start`.

**What happened.** `cargo build` was clean. `cargo test` was not:

```
error[E0152]: duplicate lang item in crate `core`: `sized`
  = note: the lang item is first defined in crate `core` (which `scopeguard` depends on)
  = note: first definition in `core` loaded from  .../build/core/31f1e492433e93d4/out/libcore-....rmeta
  = note: second definition in `core` loaded from .../build/core/ac70bd9dbb04c9ea/out/libcore-....rmeta
error: could not compile `scopeguard` (lib) due to 1 previous error
error: could not compile `bootloader` (lib) due to 1 previous error
```

Two copies of `core` in one build. A **lang item** is a definition the compiler itself
depends on — `sized` is the `Sized` trait, which every generic bound mentions. Exactly
one crate in a build may define it. `cargo test -v` showed the contradiction inside a
single `rustc` invocation:

```
-L dependency=.../build/core/ac70bd9dbb04c9ea/out
--extern noprelude,nounused:core=.../build/core/31f1e492433e93d4/out/libcore-31f1e492433e93d4.rmeta
```

One `core` named explicitly, a *different* `core` reachable on the library search path.

Diffing the two fingerprints
(`target/x86_64-baddcafe_os/debug/build/core/<hash>/fingerprint/lib-core.json`) isolated
the cause: identical `rustc`, `features`, `target`, `path`, `config` — only `profile`
differed. Cargo builds test targets under the `test` profile, and by default it forces
`panic = "unwind"` there, because the std harness needs to catch a panicking test.
Different panic strategy, different profile hash, second build of `core`.

The fix is one unstable cargo flag in `.cargo/config.toml`:

```toml
[unstable]
panic-abort-tests = true
```

It tells cargo that test targets may keep `panic = "abort"`, which is the only strategy
this target has. With it, one `core`, and the build succeeds.

Verified headless by dumping the text buffer straight out of guest memory:

```
22| Running 1 tests
23| trivial assertion... [ok]
```

**Why.** The duplicate `core` was not a bug in our code at all — it was two build
profiles disagreeing about a property (`panic`) that is baked into compiled code. This
is the recurring shape of `build-std` problems: because the standard library is compiled
*per build*, anything that changes compilation flags can silently fork it in two.
Reading the fingerprint JSONs is the general technique for these — they are cargo
showing its work about why two units are considered different.

The `#[cfg(test)]` split matters for a subtler reason. `test_main` is generated *only*
in a test build; without the `cfg` the normal build would fail on an undefined function.
`banner()` gets `#[cfg(not(test))]` for the mirror reason: unused in a test build, it
would warn as dead code.

Running `cargo test` for real hangs, on purpose. The tests print and then fall into
`loop {}`, because the kernel has no way to say "done" to anything outside itself. That
is the next step.

**Rust concepts touched.**

- `#![feature(...)]` — opting into an unstable language feature, crate-wide.
- `#[cfg(test)]` / `#[cfg(not(test))]` — conditional compilation. The item is not
  compiled-then-skipped; it never reaches the compiler at all.
- `&[&dyn Fn()]` — a slice of **trait objects**. Every function item and every closure
  has its own unique, unnameable type, so a homogeneous array of them is impossible.
  `dyn Fn()` erases the type: the reference becomes a *fat pointer*, two machine words —
  the data, plus a vtable holding the address of `call`. The same idea as a Go
  interface value, but visible in the type.
- `for test in tests` over a `&[&dyn Fn()]` yields `&&dyn Fn()`; calling `test()`
  works anyway because auto-deref applies through both references.

---

## Step 8 — Port-mapped I/O: the test run ends by itself, and speaks in text

**Goal.** Two things Step 7 left broken: `cargo test` never terminated, and its output
was only visible by dumping guest memory. Fix both. They turn out to be the same
mechanism twice.

**Concept.** x86 has **two** address spaces, not one.

- **Memory-mapped I/O** is what we already use. The device's registers sit at ordinary
  memory addresses (`0xb8000`) and a plain `mov` reaches them.
- **Port-mapped I/O** is a *separate* 16-bit space — 65536 "ports" — reachable only
  through the `in` and `out` instructions. No pointer can name it. It does not go
  through the MMU and it is not cached. We had already read this pattern in the
  bootloader's A20 code (R3): `in al, 0x92` / `out 0x92, al`.

Both devices in this step live in that second space: QEMU's `isa-debug-exit` at port
`0xf4`, and the 16550 UART (the serial port) at `0x3f8`.

**A kernel has no parent process**, so it has no exit code to return. `cargo test`
decides pass/fail purely from an exit code. `isa-debug-exit` is a fake device that
bridges the gap: write a `u32` to its port and QEMU terminates with process status
`(value << 1) | 1`. The `| 1` makes the status always odd, therefore never `0`,
therefore always "failure" to cargo — unless we tell cargo which odd number means
success.

**What we did.**

`src/port.rs` — the primitives, three functions around `core::arch::asm!`:

```rust
pub unsafe fn outb(port: u16, value: u8)
pub unsafe fn outl(port: u16, value: u32)
pub unsafe fn inb(port: u16) -> u8
```

Each is one instruction with explicit registers, e.g.

```rust
asm!("out dx, al", in("dx") port, in("al") value,
     options(nomem, nostack, preserves_flags));
```

The `in("dx")`/`in("al")` form pins the operand to a specific register, because `out`
takes no others: the port number must be in `dx`, the payload in `al`/`ax`/`eax`. The
`options` are promises to the optimiser — this asm touches no memory, does not use the
stack, and leaves the flags register alone — so it may be moved and cached freely.
`unsafe` because writing an arbitrary I/O port can do arbitrary things to the machine,
and no type system can check which port is safe.

`src/main.rs` — the exit path, test-only for now:

```rust
#[repr(u32)]
pub enum QemuExitCode { Success = 0x10, Failed = 0x11 }

pub fn exit_qemu(code: QemuExitCode) -> !
```

called at the end of `test_runner` (`Success`) and from the test build's panic handler
(`Failed`).

`src/serial.rs` — the UART. Six `out`s to configure it, then one poll-and-write per
byte. Its register map is the interesting part: the chip has more registers than the
eight ports it was given, so bit 7 of the line-control register (**DLAB**, Divisor
Latch Access Bit) *changes what the first two ports mean*. Bank switching, 1980-style.

| Offset | Role | With DLAB set |
|---|---|---|
| +0 | data in / out | divisor, low byte |
| +1 | interrupt enable | divisor, high byte |
| +2 | FIFO control | |
| +3 | line control (bit 7 = DLAB) | |
| +4 | modem control | |
| +5 | line status (bit 5 = transmitter empty) | |

The init sequence: interrupts off (`0x00`) — we have no IDT yet; DLAB on (`0x80`);
divisor `3`, giving 115200/3 = 38400 baud; `0x03` = 8 data bits, no parity, 1 stop
bit, and DLAB back off; `0xc7` = FIFOs on and cleared; `0x0b` = DTR, RTS and OUT2
asserted. Sending a byte means spinning on line-status bit 5 until the transmit holding
register is empty, then writing the byte. Busy-wait, because interrupts do not exist
yet.

On top of it, the exact shape of `vga_buffer.rs`: an `impl fmt::Write`, a global
`static SERIAL: Mutex<Serial>`, and `serial_print!` / `serial_println!`.

`Cargo.toml` — the flags `bootimage` passes to QEMU:

```toml
[package.metadata.bootimage]
run-args = ["-serial", "stdio"]
test-args = [
    "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    "-serial", "stdio",
    "-display", "none",
]
test-success-exit-code = 33
```

`33` is `(0x10 << 1) | 1`. It is written as a literal in a different file from the enum
that produces it — change one without the other and the build breaks silently.

Finally the runner learned to name its tests. A trait with a blanket implementation:

```rust
pub trait Testable { fn run(&self); }

impl<T: Fn()> Testable for T {
    fn run(&self) {
        serial_print!("{}... ", core::any::type_name::<T>());
        self();
        serial_println!("[ok]");
    }
}
```

and `test_runner(tests: &[&dyn Testable])`. Each test now prints its own name, so the
test cases themselves shrank to just their assertion.

**What happened.** `cargo test`:

```
Running: `qemu-system-x86_64 -drive format=raw,file=...bootimage-baddcafe_os-....bin
  -no-reboot -device isa-debug-exit,iobase=0xf4,iosize=0x04 -serial stdio -display none`
Running 1 tests
baddcafe_os::trivial_assertion... [ok]
$ echo $?
0
```

It terminates on its own, and the output is ordinary text on stdout. Breaking the
assertion on purpose (`assert_eq!(1 + 1, 3)`):

```
[failed]
panicked at src/main.rs:106:5:
assertion `left == right` failed
  left: 2
 right: 3
$ echo $?
35
```

`35` is `(0x11 << 1) | 1`, which is not the declared success code, so cargo reports
failure. The normal build talks too:

```
$ timeout 6 qemu-system-x86_64 -drive format=raw,file=<img> -display none \
      -serial file:/tmp/ser.txt
$ cat /tmp/ser.txt
BaddCafe OS: serial is alive on COM1
```

One warning appeared and was dealt with deliberately: `outl` is dead code in a non-test
build, because the only caller is `exit_qemu`, which is `#[cfg(test)]`. It carries an
`#[allow(dead_code)]` — the same treatment `Color` already has — rather than being
gated on `test`, because it is a hardware primitive the PIC and the timer will use in a
few steps.

**Why.** The exit device is not a real machine feature; it is a debugging hook QEMU
provides precisely because the guest cannot otherwise report a result. The serial port
*is* real, and it is the oldest and simplest output a PC has: no framebuffer layout, no
scrolling, no memory mapping — a byte goes in one end and comes out on the host's
terminal. That property makes it the right channel for machine-readable output and,
later, for panic messages that must survive a broken screen.

The `[failed]` line is printed by the panic handler, not by the runner: once a test
panics, control never returns to `Testable::run`, so nothing else can report it.

**Rust concepts touched.**

- `core::arch::asm!` — operands are `in(...)`/`out(...)` bound to a register *class* or,
  as here, to a named register. `options(...)` are guarantees the compiler cannot verify
  and must be told.
- Edition 2024: the body of an `unsafe fn` is no longer implicitly an unsafe block
  (`unsafe_op_in_unsafe_fn` is deny-by-default), so each `asm!` sits in its own explicit
  `unsafe { }`.
- `#[repr(u32)]` on an enum pins the discriminant's size. Without it the compiler picks
  the smallest type that fits — here `u8` — and the value's layout would be its private
  business. Once a value crosses into hardware, layout stops being private.
- A **blanket impl** — `impl<T: Fn()> Testable for T` — implements our trait for every
  type that is callable, closures included. It is allowed because the trait is ours; the
  orphan rule only forbids implementing a foreign trait for a foreign type.
- `core::any::type_name::<T>()` returns a compile-time string. Every function has its own
  unnameable type, so the generic parameter *is* the identity of the test, and its name
  is available with no runtime reflection and no allocation — which is why it works in
  `no_std`.
- `-> !` on `exit_qemu`: the compiler cannot know the `out` instruction kills the
  machine, so the function still needs a `loop {}` to satisfy the never type.

## Step 9 — The IDT, and the first exception we asked for

**Goal.** Teach the CPU where to jump when an instruction cannot be completed, and
prove it by triggering an exception on purpose and surviving it.

**Concept.** Until now the CPU ran our code straight through. An **exception** is the
CPU interrupting *itself*: it reaches an instruction it cannot finish — a division by
zero, a memory access to an unmapped page, an invalid opcode — and instead of
continuing it looks up a handler and jumps there. Vectors 0-31 are reserved for these;
32-255 are for hardware and software interrupts.

The table it looks in is the **IDT** (Interrupt Descriptor Table): 256 entries of 16
bytes each, living anywhere in memory, with its address handed to the CPU by the
`lidt` instruction. If the entry for a vector is not marked *present*, the CPU faults
while trying to handle the fault — and a fault while handling *that* is a **triple
fault**, which resets the machine. That is the reboot loop `-no-reboot` exists to stop.

An entry is a strange shape, and the strangeness is historical:

| Bytes | Field |
|---|---|
| 0-1 | handler address, bits 0-15 |
| 2-3 | code segment selector to run the handler under |
| 4-5 | options |
| 6-7 | handler address, bits 16-31 |
| 8-11 | handler address, bits 32-63 |
| 12-15 | reserved |

The address is split into three pieces because 64-bit mode widened a descriptor that
already existed in 32-bit mode, and could not move the fields that were already there.
The options word we write is `0x8e00`: bit 15 = present, bits 13-14 = DPL 0 (only ring
0 may invoke it with `int`), bits 8-11 = type `0xe`, a 64-bit **interrupt gate**. Type
`0xf` would be a *trap* gate; the only difference is that an interrupt gate clears the
interrupt flag on entry, so the handler is not itself interrupted.

**The calling convention is the real lesson.** A handler is not *called* — it is jumped
to by hardware, at an arbitrary instruction boundary in unrelated code. Two consequences
the normal C ABI gets wrong:

1. **It must return with `iretq`, not `ret`.** The CPU pushed five values (instruction
   pointer, code segment, flags, stack pointer, stack segment) and `iretq` pops exactly
   those and restores the flags. `ret` would pop one value and leave the rest as
   garbage.
2. **It must preserve every register.** The System V ABI splits registers into
   callee-saved and caller-saved; the caller-saved ones may be destroyed by a call
   because the caller knew a call was coming. Here the "caller" is a random instruction
   that knew nothing. So *all* registers must be restored.

Rust cannot express this with `extern "C"`, so there is a dedicated, still-unstable
convention: `extern "x86-interrupt"`, behind `#![feature(abi_x86_interrupt)]`. It also
knows about the pushed frame — the handler takes an `InterruptStackFrame` argument —
and about the exceptions that push an **error code** on top, which take a second `u64`.

**What we did.** A new `src/idt.rs`, roughly 130 lines and no new dependency:

```rust
pub type HandlerFunc = extern "x86-interrupt" fn(InterruptStackFrame);

#[repr(C)] struct Entry { offset_low, selector, options, offset_middle, offset_high, reserved }
#[repr(C, align(16))] pub struct Idt { entries: [Entry; 256] }

impl Idt {
    pub const fn new() -> Idt;
    pub fn set_handler(&mut self, vector: u8, handler: HandlerFunc);
    pub fn load(&'static self);
}
```

Three decisions worth naming:

- **`load(&'static self)`.** `lidt` hands the CPU a raw address and the CPU keeps
  reading that memory forever afterwards, long after `load` has returned. `&'static` is
  how we state that in the type system: this table must outlive everything. A stack-local
  IDT would compile without the bound and produce a machine that dies at the first
  exception.
- **The selector is *read*, not hardcoded.** `set_handler` executes `mov {}, cs` to
  learn which code segment we are currently running under, rather than assuming the
  bootloader's `0x8`. The value happens to be `0x8`, but the assumption would be
  invisible the day we install our own GDT.
- **`spin::Once<Idt>` for the global.** The table must be built at runtime (handler
  addresses are not known at compile time) yet live in a `static`. `static mut` would
  need `unsafe` at every touch and is lint-denied in edition 2024;
  `core::cell::OnceCell` is not `Sync`; there is no `OnceLock` in `core`. `Once`
  initialises exactly once behind a spinlock and hands back a `&'static Idt` — the
  precise lifetime `load` demands.

Then `init()` registers one handler on vector 3 and loads the table, `_start` calls it
right after the serial port comes up, `banner()` executes `int3` on purpose, and a
`#[test_case]` does the same and passes if execution simply continues.

**What happened.** First build, clean. `cargo test`:

```
Running 2 tests
baddcafe_os::breakpoint_exception_returns... EXCEPTION: BREAKPOINT
InterruptStackFrame {
    instruction_pointer: 2118217,
    code_segment: 8,
    cpu_flags: 6,
    stack_pointer: 1099513732680,
    stack_segment: 0,
}
[ok]
baddcafe_os::trivial_assertion... [ok]
```

Correct, but decimal — a `#[derive(Debug)]` on a struct full of addresses. Replaced with
a hand-written `impl fmt::Debug` that wraps each field in `format_args!("{:#x}", ...)`,
the standard trick for changing a field's formatting inside `debug_struct`. `Formatter`
stores an `Arguments` and prints it verbatim, so the field comes out in hex:

```
instruction_pointer: 0x2051c9,
code_segment: 0x8,
cpu_flags: 0x6,
stack_pointer: 0x10000201e48,
stack_segment: 0x0,
```

And the normal boot, read off the screen with `pmemsave`:

```
15| EXCEPTION: BREAKPOINT
16| InterruptStackFrame {
...
22| }
23| ...and we came back from the exception.
```

That last line is the whole point: the CPU left our code, ran a function we registered,
and `iretq` put it back exactly where it was.

**Why.** `cpu_flags: 0x2` on the real boot is worth a look — bit 1 of RFLAGS is a
reserved bit that is always 1, so `0x2` means "no flags set at all". In the test run it
was `0x6`, which adds bit 2, the parity flag, left over from the `assert_eq!` that ran
just before. The frame is a snapshot of the interrupted code, and it shows.

`instruction_pointer` points at the instruction *after* `int3`, because a breakpoint is
a **trap**: it reports something that already happened, so execution resumes past it.
Faults (page fault, general protection fault) report something that has *not* happened
and push the address of the offending instruction itself, so that a handler can fix the
situation and retry. Aborts (double fault) do not promise anything useful at all.

**Rust concepts touched.**

- `extern "x86-interrupt"` — a calling convention as a language feature, because the
  return instruction and the register-saving rules are not something a library can add.
- `#![feature(...)]` again: `abi_x86_interrupt` is unstable, which is one more reason
  this project is pinned to nightly.
- `#[repr(C, align(16))]` — layout dictated by hardware, not by the compiler's
  preferences. `#[repr(C, packed)]` on the `lidt` operand for the opposite reason: the
  6-byte pointer has a `u16` followed by a `u64` with no padding between them, which no
  compiler would choose on its own.
- `&'static self` as an API constraint. The lifetime is not documentation here; it is
  the thing that makes a whole class of bug impossible.
- Function pointers as values: `handler as u64` is the address of the code, and a
  function item coerces to a function pointer on the way.
- `format_args!` inside `debug_struct().field(...)` — `Arguments` implements `Debug` by
  printing itself, so it is the way to override formatting of one field.
- `spin::Once<T>` vs `static mut`: deferred initialisation of a global without `unsafe`,
  and why `&'static` falls out of it for free.

---

## Step 10 — A stack the double-fault handler can trust (GDT, TSS, IST)

**Goal.** Survive the exception that has no handler. Register a handler for vector 8,
the double fault, and — the harder half — make sure it can run even when the fault was
caused by a stack that no longer works.

**Concept.** A **double fault** is not "two faults at once". It is what the CPU raises
when it fails to *deliver* an exception: a page fault happens, the CPU goes to look up
vector 14 in the IDT, finds no present entry, and now has a second problem while still
handling the first. That second problem is vector 8. If the delivery of vector 8 also
fails, there is no third chance — the CPU gives up and **triple-faults**, which on real
hardware and in QEMU means an immediate reset. Every silent reboot we have had so far
was a triple fault.

So a double-fault handler is the kernel's safety net: it converts "the machine
mysteriously restarted" into a message we can read.

But there is a case where merely registering the handler is not enough. Suppose the
first fault was a **stack overflow**: the program recursed until `rsp` walked off the
bottom of the stack onto a guard page, and the write that touched the guard page raised
a page fault. To call a handler, the CPU must first *push* a five-word interrupt stack
frame — onto the same broken stack. That push faults too. Now the double-fault handler
must be invoked, and the CPU tries to push *its* frame onto the same broken stack, and
fails again. Triple fault. The handler is never entered.

The fix is not software. x86-64 provides the **IST** (Interrupt Stack Table): seven
addresses of known-good stacks, published in a structure called the **TSS** (Task State
Segment). An IDT entry has a 3-bit field naming one of them. If that field is non-zero,
the CPU switches `rsp` to that stack *before pushing anything*, unconditionally,
whatever the old stack pointer was. It cannot fail on a broken stack, because it does
not use the broken stack.

The TSS is a leftover from 32-bit hardware task switching, a feature 64-bit mode
dropped. What remains of it is a table of stack pointers: three for privilege-level
changes (`rsp0`-`rsp2`, unused so far, since we never leave ring 0) and seven for the
IST. And the TSS can only be pointed at through a descriptor in the **GDT** (Global
Descriptor Table) — the table that in 32-bit days described memory segments and today
mostly exists so that `cs` has something to point at. The bootloader set up a GDT for
its own use; we cannot extend a table we do not own, so we build our own.

That is the chain, and it explains why so much machinery appears for one handler:

```
IDT entry (vector 8)  --IST index-->  TSS.interrupt_stack_table[0]  --> our stack
        ^                                     ^
        | loaded by lidt                      | reachable only via a GDT descriptor,
        |                                     | selected by ltr
```

**What we did.**

*First, a library.* A stack-overflow test can only pass by *not* returning: the double
fault handler must end the QEMU session itself. That cannot share a kernel with other
tests, so it needs a kernel of its own — and a second binary can only reuse our code if
that code lives in a library. `src/main.rs` was split:

- `src/lib.rs` — a `#![no_std]` library holding the modules, `init()`, `hlt_loop()`,
  `QemuExitCode`, `exit_qemu`, `Testable`, `test_runner`, `test_panic_handler`, and
  (under `#[cfg(test)]`) its own `_start` and panic handler, so `cargo test` can run
  unit tests against the library itself.
- `src/main.rs` — now a thin binary: `_start` calls `baddcafe_os::init()`, clears the
  screen, prints the banner, then parks in `hlt_loop()`.
- `tests/stack_overflow.rs` — an integration test that is a whole separate kernel, with
  `harness = false` in `Cargo.toml` because it is one test, not a list of them.

*Then the tables.* `src/gdt.rs`:

- `Tss`, `#[repr(C, packed(4))]`, 104 bytes, field order dictated by the manual.
- A 20 KiB stack for the handler. It is a `static`, because there is no allocator and
  no heap; a plain immutable `static` could be placed in a read-only section, and the
  CPU is going to *write* to this memory, so it is wrapped in `UnsafeCell` with a
  hand-written `unsafe impl Sync`.
- `Gdt` — eight `u64` slots, index 0 always the null descriptor. `add_code_segment`
  pushes a 64-bit ring-0 code descriptor; `add_tss` pushes a **16-byte** system
  descriptor, twice the size of a normal one, because a 64-bit base address does not
  fit in a layout designed when addresses were 32 bits.
- `init()` loads the table with `lgdt`, reloads `cs` and loads the task register.

Reloading `cs` deserves a note: `mov cs, ax` does not exist. The only way to change the
code segment is to jump through it, so we push the new selector and a return address
and execute `retfq` — a far return to the very next instruction, which happens to
reload `cs` on the way.

*Then the IDT.* `set_handler` now returns `&mut Entry`, and `Entry::set_stack_index`
writes the low three bits of the options word. Vector 8 needs a second handler shape,
`extern "x86-interrupt" fn(InterruptStackFrame, u64) -> !`: it takes an error code
(always zero for a double fault) and never returns, because the CPU does not record
enough state to resume the faulting instruction.

**What happened.** `cargo test` now builds three kernels and boots each one:

```
Running 4 tests
baddcafe_os::breakpoint_exception_returns... [ok]
baddcafe_os::keyboard_decodes_a_shifted_letter... [ok]
baddcafe_os::timer_interrupt_fires... [ok]
baddcafe_os::trivial_assertion... [ok]
...
stack_overflow::double_fault_survives_stack_overflow... [ok]
```

The last line is the whole point: a kernel deliberately recursed until its stack ran
out, and instead of resetting the machine it reached a handler and exited cleanly.

Two errors on the way there are worth keeping:

```
error[E0133]: use of mutable static is unsafe and requires unsafe block
   --> src/gdt.rs:116:33
    |
116 |         let bottom = &raw const DOUBLE_FAULT_STACK.0 as *const u8 as u64;
    |                                 ^^^^^^^^^^^^^^^^^^ use of mutable static
```

Taking `&raw const SOME_STATIC` is allowed without `unsafe`, but reaching *into* a
mutable static for a field is not. That error is what pushed us to `UnsafeCell`, which
is the better answer anyway: `static mut` is on its way out of the language, and
`UnsafeCell` states the same thing precisely — shared, yet mutable, and we take
responsibility for the synchronisation.

```
error[E0658]: the extern "x86-interrupt" ABI is experimental and subject to change
```

An integration test is its own crate, so `#![feature(abi_x86_interrupt)]` has to be
declared again at the top of `tests/stack_overflow.rs`. Crate attributes do not travel
with a `use`.

**Why.** The `x86-interrupt` ABI matters here more than usual. A normal function may
clobber the caller-saved registers; an interrupt handler must leave *every* register
exactly as it found it, because the interrupted code did not agree to be called. It
also has to return with `iretq` rather than `ret`, since the CPU pushed a five-word
frame instead of a plain return address. That is what the ABI generates for us, and it
is why handlers cannot be ordinary `extern "C"` functions.

**Rust concepts touched.**

- Splitting a crate into a library plus a binary, and what changes: the binary now says
  `baddcafe_os::…`, and `#[macro_export]` macros are imported from the crate root.
- `UnsafeCell` and `unsafe impl Sync` — the two halves of "I promise this shared,
  mutable memory is safe", which the compiler cannot verify and we must.
- `&raw const` as the modern way to take an address without ever creating a reference.
- Returning `&mut self.entries[i]` from a setter to allow `set_handler(..).set_stack_index(..)`.
- The never type `!` as a return type expressing "this function does not come back".
- `#[repr(C, packed(4))]` — layout dictated by hardware, not by the compiler.

---

## Step 11 — The world interrupts us: PIC, timer, keyboard

**Goal.** Stop asking for exceptions and start receiving events. Wire up the interrupt
controller, take the timer tick, and turn keypresses into characters on the screen.

**Concept.** Everything the CPU has done for us so far was **synchronous**: `int3` was
an instruction we executed, a page fault is caused by a memory access we made. A
keypress is different — it happens because a human moved a finger, at a moment
unrelated to any instruction. The hardware needs a way to say "now", and the CPU needs
a way to be told without being asked.

That path is a wire. Devices raise an IRQ line; a chip called the **PIC** (Programmable
Interrupt Controller, the Intel 8259) collects those lines, decides which one wins, and
signals the CPU with a vector number. The CPU finishes the current instruction, then
jumps through the IDT exactly as it does for an exception. From the kernel's side an
interrupt and an exception look identical; only the cause differs.

A PC has two 8259s, because one chip has eight inputs and eight was not enough by 1983.
The second is *chained* into line 2 of the first, giving 15 usable IRQs:

```
                     ____________                          ____________
 real time clock --> |          |   timer      ---------> |          |
 ACPI -------------> |          |   keyboard   ---------> |          |      _____
 available --------> | slave    |_____________            | master   |     |     |
 available --------> | 8259A    |             |           | 8259A    |---> | CPU |
 mouse ------------> |          |             |---------> |          |     |_____|
 co-processor -----> |          |                         |          |
 primary ATA ------> |          |                         |          |
 secondary ATA ----> |__________|                         |__________|
```

Two things must be fixed before enabling any of this.

**The vectors collide.** By default the master PIC delivers IRQ0-7 as vectors 8-15. On
a 64-bit CPU, vector 8 is the double fault and 13 is the general protection fault. Left
alone, the first timer tick would arrive at our double-fault handler. So the chips must
be **remapped**, and 32 is the first vector the CPU architecture leaves free: exceptions
own 0-31. Ours land at 32 (timer) and 33 (keyboard).

**Locks become deadlocks.** `print!` takes the `WRITER` spinlock. If an interrupt fires
while the main code holds that lock, and the handler prints, the handler spins forever
waiting for a lock that will only be released by the code it interrupted — which will
never run again. This was written down as a hazard in Step 9 and became real the moment
interrupts were enabled. The fix is to disable interrupts for the duration of the lock:
`without_interrupts` reads the flags register, clears IF with `cli`, runs the closure,
and restores IF only if it had been set.

**What we did.**

`src/pic.rs` — initialisation is a fixed four-word conversation with each chip, sent
through its data port: ICW1 says "a new initialisation begins", ICW2 carries the vector
offset, ICW3 describes the chaining, ICW4 asks for 8086 mode. Each write is followed by
`io_wait()`, a read from unused port 0x80 — the 8259 is much older than the bus and
needs the delay to latch the previous byte. Then a mask byte per chip: everything
masked except IRQ0, IRQ1 and IRQ2 (the cascade line, which must stay open or the second
chip is unreachable). `end_of_interrupt` acknowledges a vector; without it the
controller keeps the line busy and never sends another event — a keyboard that answers
exactly one keypress.

`src/idt.rs` — two more handlers plus the interrupt-flag helpers:

- `timer_handler` bumps an `AtomicU64` and sends EOI. It prints nothing; a tick is 55
  times a second and the screen would be useless.
- `keyboard_handler` reads port 0x60, decodes it, prints the character, sends EOI.
- `enable`, `disable`, `are_enabled`, `without_interrupts`.

`src/keyboard.rs` — the controller hands us a **scancode**, not a character. It is a
position on the keyboard, not a letter: the same physical key sends the same code on a
Greek or a Dvorak layout. Set 1 encodes a press as a *make code* and the matching
release as the same code with bit 7 set, which is why two 0x40-entry tables cover the
whole main block. Shift and caps lock are state we keep ourselves, because the hardware
does not: shift is "is it currently held", caps lock toggles on press and inverts the
case of letters only. Codes prefixed with 0xE0 (arrows, right control, the keypad) are
recognised and dropped for now.

`src/vga_buffer.rs` — typing needs two things printing did not: backspace (0x08 moves
the column back and blanks the cell) and a cursor. The blinking cursor is not a
character in the buffer; it is a position stored in two CRT controller registers, so
`update_cursor` writes the cell index to ports 0x3D4/0x3D5 after every change.

`src/lib.rs` — `init()` now does the whole sequence in the one order that works: serial,
GDT, IDT, PIC, `sti`. GDT before IDT, because each IDT entry stores the code-segment
selector that is current when it is written. `sti` last, because an interrupt arriving
before the IDT is loaded is a triple fault.

And `loop {}` became `hlt_loop()`. A bare `loop {}` keeps the CPU at 100% doing nothing;
`hlt` stops the core until the next interrupt arrives. With interrupts on it wakes for
every tick and every keypress, so it costs nothing and idles honestly.

**What happened.** The kernel boots, prints the banner, and typing on the QEMU window
puts characters on the screen. Verified without a human at the keyboard by driving
QEMU's monitor: `sendkey h`, `sendkey i`, `sendkey spc`, `sendkey shift-r`, … then
reading the VGA buffer straight out of physical memory with `xp /160xb 0xb80e0`:

```
row 22: 'Timer and keyboard interrupts are on. Type:'
row 23: 'hi Rust'
row 24: '12'
```

Row 23 is shift working (capital R), row 24 is `1`, `2`, `3` followed by a backspace.
`cargo test` reports five green tests across three kernels, one of which waits for the
timer tick counter to move and would hang forever if the PIC were misconfigured.

**Why.** The remapping is the interesting part of the design. Nothing in the 8259 knows
what a double fault is; it simply sends "vector = offset + line". The collision exists
because IBM chose the offsets in 1981 for a CPU whose exception range stopped at 5, and
Intel later grew the exception range into that space. Backward compatibility means the
default is still wrong, and every x86 kernel written since has had to fix it in the
same eight port writes.

**Rust concepts touched.**

- `AtomicU64` with `Ordering::Relaxed` — shared mutable state that needs no lock, and
  therefore cannot deadlock inside a handler.
- Closures as arguments: `without_interrupts<F: FnOnce() -> R, R>(f: F) -> R`, generic
  over both the closure and its return type. `FnOnce` because calling it once is all we
  need, and it is the least demanding bound the caller can satisfy.
- `const` lookup tables of `char`, indexed by a hardware code — data instead of a
  hundred-arm `match`.
- Method calls on a `Mutex`-guarded struct inside an interrupt handler, and why that is
  only safe because every other user of the same lock disables interrupts first.

---

## Step 12 — The page fault handler, and what the CPU tells us

**Goal.** Fill vector 14, so that a failed memory access produces the faulting address
and the reason, instead of being paid for with a double fault.

**Concept.** Until now vector 14 was **missing** from the IDT. A bad memory access
therefore went: page fault → the CPU looks up entry 14 → `present = 0` → double fault.
Our double-fault handler printed "double fault" and panicked, which is correct and
useless: the one piece of information worth having — *which address, and why* — was
gone by then.

This matters more from here on than it did behind us. The moment we start touching page
tables, **every** mistake will arrive as a page fault. Without this handler we would be
staring at "double fault" and guessing.

The CPU hands us three things:

1. **`CR2`** — a control register the CPU fills with the address that failed. It is the
   only place that information exists, and the next page fault overwrites it. So a
   handler must read it before doing anything else that could itself fault.
2. **An error code**, pushed on the stack, which is a bitfield:

   | bit | 0 means | 1 means |
   |---|---|---|
   | 0 | nothing is mapped there | it is mapped, but the access was not allowed |
   | 1 | it was a read | it was a write |
   | 2 | ring 0 | ring 3 |
   | 3 | — | a reserved bit in a page table entry was not zero |
   | 4 | — | the CPU was trying to *execute* there |

3. **The interrupt stack frame**, whose `instruction_pointer` is the instruction that
   faulted — not the one after it, as with `int3`.

The deep point is in the handler's signature. A page-fault handler **may return**: the
faulting instruction is restartable. That is not a quirk, it is the entire mechanism —
a real kernel fixes the mapping first (fetches the page from disk, hands a fresh frame
to a `malloc` that just touched it, resolves a copy-on-write) and *then* returns, and
the instruction succeeds. A page fault is not an error; it is how virtual memory is
built. We have nothing to fix yet, so ours stops the machine instead of letting the
same instruction fault forever.

**What we did.** `src/idt.rs`:

- `const PAGE_FAULT: u8 = 14;` and a third handler type,
  `pub type HandlerFuncWithErrorCode = extern "x86-interrupt" fn(InterruptStackFrame, u64)`
  — the diverging one we already had, minus the `-> !`.
- The three setters had become near-copies, so the body moved into one private
  `fn set(&mut self, vector: u8, address: u64) -> &mut Entry` and the three public ones
  are now one line each. They cannot be collapsed further: a function's signature is
  part of its type in Rust, so a single generic entry point would have to erase exactly
  the check that makes these calls safe.
- `pub fn read_cr2() -> u64`, one `mov {}, cr2`.
- A private `PageFaultErrorCode(u64)` newtype whose only job is a `Debug` impl that
  prints the raw bits *and* the names of the bits that are set.
- `page_fault_handler`, which prints address, error code and stack frame to both the
  screen and the serial port, then calls `hlt_loop()`.

Two new integration tests, each a whole kernel with its own IDT (the handler ends the
QEMU session, so it cannot share one with other tests):

- `tests/page_fault.rs` — writes to `0xdeadbeaf`, and the handler asserts that `CR2`
  holds exactly that address and that the error code is `0b10`: not present, caused by
  a write.
- `tests/page_fault_protection.rs` — writes to `_start`, i.e. to our own code, and
  asserts the error code is `0b11`: mapped, and a write is what was refused.

**What happened.** The first attempt did not reach the hardware at all:

```
page_fault::write_to_unmapped_page... [failed]
panicked at tests/page_fault.rs:29:14:
misaligned pointer dereference: address must be a multiple of 0x8 but is 0xdeadbeaf
```

A debug build inserts an alignment check *before* the access, and `0xdeadbeaf` is not a
multiple of 8, so the dereference of a `*mut u64` panicked before the CPU ever saw it.
Writing through a `*mut u8` — alignment 1 — fixed it. Worth remembering: a debug build
can intercept undefined behaviour before the machine does, which is helpful everywhere
except when the machine's reaction is the thing you are testing.

With that fixed, all five tests pass, and the handler on a live kernel prints:

```
EXCEPTION: PAGE FAULT
accessed address: 0xdeadbeaf
error code: 0b00010 (CAUSED_BY_WRITE)
InterruptStackFrame {
    instruction_pointer: 0x2048e2,
    code_segment: 0x8,
    cpu_flags: 0x202,
    stack_pointer: 0x10000201e90,
    stack_segment: 0x0,
}
```

**Why.** The protection test is the more interesting of the two, because it passes
*only* because someone else enforced something we never asked for. Writing to our own
`.text` produces bit 0 = 1 (the page is there) and bit 1 = 1 (a write was refused),
which proves the bootloader mapped our code read-only and that the hardware, not the
compiler, is what stops the write. Permissions live in the page table entries. The
`const`/`mut` distinction in Rust is a compile-time story about aliasing; this is a
different mechanism entirely, enforced on every single access, and it is the one that
still applies to code that lies.

**Rust concepts touched.**

- Raw pointer dereference as the rawest form of `unsafe`: `*(0xdeadbeaf as *mut u8) = 42`
  compiles because the compiler has no way whatsoever to know whether that address means
  anything. Nothing here is checkable; the `unsafe` is a pure assertion by us.
- Debug-build UB checks (`misaligned pointer dereference`) and when they get in the way.
- A newtype (`PageFaultErrorCode`) with a hand-written `Debug` instead of a
  bitflags-style dependency: `write!` for the raw value, then a running separator over a
  `const` array of (bit, name) pairs.
- Why three setters are not reducible to one generic: function types, and the fact that
  erasing them is exactly what we do *not* want.
- A non-diverging handler whose body ends in a `-> !` call — `hlt_loop()` coerces to
  `()`, so the signature stays honest about what the hardware allows.

---

## Step 13 — Reading the page tables the bootloader left us

**Goal.** Stop guessing at mappings from the outside. Walk the same four-level
hierarchy the CPU itself walks on every memory access, from inside the kernel, and
prove the walk is correct against addresses whose mapping we already know by other
means.

**Concept.** `CR3` holds the *physical* address of the top-level (PML4) table. Every
memory access the kernel issues is itself a *virtual* address, translated through
that same hierarchy before it ever reaches a real address line. To read the tables as
data, we need a virtual address that resolves to the physical address in `CR3` — and
by construction, none of our existing mappings do that. This is the chicken-and-egg
problem flagged at the end of Step 12.

Three standard ways out exist: identity-map all of physical RAM, recursively map the
PML4 into itself, or have something map *all* physical RAM once at a fixed virtual
offset. We took the third, because the `bootloader` crate can build it for us: behind
the Cargo feature `map_physical_memory`, it maps every physical frame at
`physical_memory_offset + frame`, and publishes that offset through a `BootInfo`
struct handed to `_start` as `&'static BootInfo`. (Real production kernels do exactly
this — Linux calls it the direct-mapped region of physical memory, reachable at a
fixed `PAGE_OFFSET`.)

**What we did.**

```toml
# Cargo.toml
[dependencies]
bootloader = { version = "0.9.35", features = ["map_physical_memory"] }
```

`_start`'s signature changed everywhere it is defined by hand — `src/main.rs` and the
`#[cfg(test)]` entry point in `src/lib.rs` — from `extern "C" fn _start() -> !` to
`extern "C" fn _start(boot_info: &'static bootloader::BootInfo) -> !`. We did not reach
for the crate's `entry_point!` macro: it only buys compile-time type-checking of the
signature, and this project hand-rolls `_start` on purpose, the same way it hand-rolled
the IDT and GDT instead of using the `x86_64` crate. The three existing integration
tests (`stack_overflow.rs`, `page_fault.rs`, `page_fault_protection.rs`) were left
exactly as they are: their `_start` still takes no parameters, and the unused
`boot_info` argument the bootloader passes in a register is simply never read — which
is also, in hindsight, why a zero-argument `_start` has worked since Step 2 without
anyone ever having to tell the bootloader to stop passing it.

New module, `src/memory.rs`, hand-rolled like everything else in this kernel:

- `Entry` — a newtype over `u64`. `is_present`, `is_huge_page`, `addr` (masked to bits
  12-51, the physical address width) and `set`.
- `Table` — `#[repr(C, align(4096))]`, 512 entries, matching the hardware's own layout
  exactly, the same reasoning as `Idt`'s `repr(C, align(16))` in Step 9.
- `table_at` / `table_at_mut` — the one load-bearing idea in the module: given a
  *physical* address, add `physical_memory_offset` and dereference the result as a
  `Table`. This is the only place physical addresses ever get turned into something
  the CPU can actually touch.
- `translate(virt) -> Option<u64>` — walks PML4 -> PDPT -> PD -> PT by hand, exactly
  mirroring the hardware, through `table_at` instead of silicon. Handles huge-page
  entries (2 MiB at the PD level, 1 GiB at the PDPT level) by widening the untranslated
  offset instead of assuming every entry bottoms out at a 4 KiB `PT` — necessary
  because the bootloader's own physical-memory mapping covers all of RAM using 2 MiB
  pages, confirmed by reading `bootloader-0.9.35/src/main.rs` directly.
- `FrameAllocator` — walks `BootInfo`'s `MemoryMap`, filters frames marked `Usable`,
  hands them out once each. Re-walks the map from scratch on every call: wasteful, but
  the same trade-off phil-opp's own first allocator makes, and explicitly a problem for
  the next roadmap step (Allocator Designs) to improve, not this one.
- `create_mapping(virt, frame, allocator)` — the write side: walks the same four
  levels, and where an intermediate table is missing, allocates a fresh frame for it,
  zeroes it (an unzeroed table hands the next lookup 512 entries of garbage that can
  look "present" by accident), and links it in. Finishes with `invlpg` so the CPU's TLB
  does not keep serving a stale "not present" for an address it cached before the
  mapping existed.

Three new `#[test_case]`s in `src/lib.rs`, chosen so each proves something we could not
already have assumed:

```rust
#[test_case]
fn vga_buffer_is_identity_mapped() {
    assert_eq!(memory::translate(0xb8000), Some(0xb8000));
}
```

The VGA text buffer is memory-mapped I/O fixed by the graphics hardware at physical
`0xb8000` — not negotiable, not something the bootloader chooses. We have written to
virtual `0xb8000` since Step 5 and seen correct output on a real screen, which is only
possible if that virtual address really does resolve to the same physical one. This
test turns an assumption we have been quietly relying on for eight steps into a
checked fact.

```rust
#[test_case]
fn kernel_code_is_mapped_but_not_identity() {
    let virt = hlt_loop as *const () as u64;
    let phys = memory::translate(virt).expect("kernel code must be mapped to run at all");
    assert_ne!(phys, virt);
    assert_eq!(phys & 0xfff, virt & 0xfff);
}
```

Confirms the opposite case: our own code is *not* identity-mapped (matching what
`gva2gpa` already showed us from outside the machine, back in Step 12's notes), while
the low 12 bits — the in-page offset, which no level of the walk ever touches — must
still agree between virtual and physical.

```rust
#[test_case]
fn a_freshly_created_mapping_is_readable_and_writable() {
    const PROBE_VIRT: u64 = 0x1000_0000_0000;
    assert_eq!(memory::translate(PROBE_VIRT), None, "probe address already in use");

    let mut allocator = memory::frame_allocator();
    let frame = allocator.allocate_frame().expect("no usable frames left");
    unsafe { memory::create_mapping(PROBE_VIRT, frame, &mut allocator) };

    assert_eq!(memory::translate(PROBE_VIRT), Some(frame));

    let ptr = PROBE_VIRT as *mut u64;
    unsafe {
        ptr.write_volatile(0xcafe_babe_dead_beef);
        assert_eq!(ptr.read_volatile(), 0xcafe_babe_dead_beef);
    }
}
```

The strongest of the three: it does not just read the existing tables, it *writes* a
brand new mapping and proves the hardware actually honours it, round-tripping a
sentinel value through a virtual address that did not resolve to anything a moment
earlier. The `None` assertion first is not decoration — it is what makes the test
trustworthy regardless of where the bootloader happened to place the kernel, its stack,
or the physical-memory mapping on a given run.

A quirk worth recording: the first version of the second test tried to take the
address of another `#[test_case]`-annotated function directly —

```
error[E0425]: cannot find value `keyboard_decodes_a_shifted_letter` in this scope
   |
   | you might have meant to refer to this function
```

— which rustc reported as both "this is a function" and "not found" in the same
breath. Whatever `custom_test_frameworks` does internally to collect `#[test_case]`
items on this nightly, it makes the original item unusable as an ordinary value from
elsewhere in the same module. Fixed by pointing at an ordinary function
(`hlt_loop`, not `#[test_case]`-tagged) instead — not investigated further, since the
test's point survives perfectly well either way, but filed here so it is not
rediscovered the hard way.

A second, unrelated nightly complaint:

```
warning: direct cast of function item into an integer
help: first cast to a pointer `as *const ()`
```

Fixed by casting through `*const ()` first. The warning is right to exist: a bare
function item is zero-sized and has its own unique type; `as u64` on it is really two
casts in one (function item -> function pointer -> integer), and spelling both out
removes the ambiguity about which address is meant if the function is ever generic.

**Environment note.** This session ran on a different machine than the one
The `/space/...` and `/home/...` paths earlier in this file came from — a fresh Windows checkout
with neither QEMU nor `bootimage` installed, which blocked `cargo test`/`cargo run`
entirely at first. Installed `qemu` via `scoop install qemu` and `bootimage` via
`cargo install bootimage`; both work, but `qemu-system-x86_64` did not end up on `PATH`
through a scoop shim and has to be added manually
(`$HOME/scoop/apps/qemu/current`), and its monitor's `stdio` mode echoes raw input
characters when driven through a non-interactive pipe on Windows, which broke the
`pmemsave`-based screen-reading recipe from the Reference section. Serial output was
unaffected and remains the reliable channel on this machine.

**What happened.**

```
Running 7 tests
baddcafe_os::a_freshly_created_mapping_is_readable_and_writable... [ok]
baddcafe_os::breakpoint_exception_returns... EXCEPTION: BREAKPOINT
InterruptStackFrame { ... }
[ok]
baddcafe_os::kernel_code_is_mapped_but_not_identity... [ok]
baddcafe_os::keyboard_decodes_a_shifted_letter... [ok]
baddcafe_os::timer_interrupt_fires... [ok]
baddcafe_os::trivial_assertion... [ok]
baddcafe_os::vga_buffer_is_identity_mapped... [ok]
```

All three existing integration kernels (`stack_overflow`, `page_fault`,
`page_fault_protection`) still pass unchanged. The production kernel, booted headless
with serial captured to a file:

```
BaddCafe OS: serial is alive on COM1
EXCEPTION: BREAKPOINT
InterruptStackFrame {
    instruction_pointer: 0x206269,
    code_segment: 0x8,
    cpu_flags: 0x206,
    stack_pointer: 0x10000201e78,
    stack_segment: 0x0,
}
```

No page fault, no double fault, no reset in the run — `init()` followed by
`memory::init(boot_info)` followed by the full banner path, including the deliberate
`int3`, completed cleanly. The `unused dependency: bootloader` warning that has been
carried since Step 4 is also gone on its own: the crate is finally `use`d for real.

**Why.** The whole step rests on one fact we did not have to build, only trust: the
bootloader's own doc comment on `physical_memory_offset` says plainly that regions not
marked `Usable` (including, per `MemoryRegionType`, anything the bootloader used for
its own page tables) must not be treated as free. Our `FrameAllocator` only ever hands
out `Usable` frames, which is exactly what keeps a freshly allocated table frame from
ever colliding with a table frame the bootloader is still using to keep *us* running.

**Rust concepts touched.**

- `&'static BootInfo` as a parameter, not a return type — the lifetime says the
  pointee outlives the whole kernel, which is true here because the bootloader placed
  it in memory that is never reclaimed, the same reasoning `Idt::load(&'static self)`
  relied on in Step 9.
- A manual 4-level pointer-chasing loop as the realistic alternative to the `x86_64`
  crate's `Mapper`/`PageTable` abstractions — the trade-off this project keeps making:
  more code, in exchange for nothing hidden.
- `impl Iterator<Item = u64>` as a return type (`usable_frames`) — a method that
  returns "some iterator, whose concrete type is this function's own business," built
  from `filter`/`flat_map`/`map` over the memory map without allocating anything, which
  matters here as much as it did for `format_args!` in Step 6: there is still no heap.
- `#[repr(transparent)]` vs `#[repr(C)]` revisited: `Entry` is the former (one `u64`,
  nothing more, purely for our own ergonomics), `Table` is the latter (hardware layout,
  non-negotiable) — the same distinction `ColorCode` vs `ScreenChar` drew in Step 5.

---

## Step 14 — A heap: `Box`, `Vec` and `String` on bare metal

**Goal.** Give the kernel dynamic memory: a region of virtual addresses backed by real
frames, and a global allocator over it, so `alloc::boxed::Box`, `Vec`, `String` and
`format!` work exactly as they do in ordinary Rust.

**Concept.** Until now every byte the kernel used had a size known at compile time: a
`static`, or a local on the stack. A *heap* is memory whose size and lifetime are
decided at run time — "give me 4000 bytes, I will tell you when I am done". In
ordinary Rust, `std` gets that memory from the operating system (`mmap`/`brk` on
Linux, through libc's `malloc`). We are the operating system, so three pieces have to
exist before a single `Box::new` can work:

1. **The `alloc` crate itself.** Rust's library is split in three layers: `core`
   (needs nothing — no memory, no OS), `alloc` (needs only an allocator: `Box`, `Vec`,
   `String`, `Rc`, `BTreeMap`), and `std` (needs a whole OS: files, threads, sockets).
   We have been on `core` alone since Step 1. `alloc` is the layer we can now afford.
   It has no prebuilt copy for our custom target, so `build-std` must compile it, just
   as it compiles `core`.
2. **Memory behind the addresses.** A virtual address range is useless until every
   page in it is mapped to a frame. Step 13 built exactly the two tools for this:
   `frame_allocator()` hands out unused physical frames, `create_mapping` wires a page
   to one.
3. **An allocator — the bookkeeping.** Something has to answer "where do these N bytes,
   aligned to A, go?" and "this block is free again". Rust's contract for that is the
   `GlobalAlloc` trait: `alloc(Layout) -> *mut u8` and `dealloc(*mut u8, Layout)`. A
   `Layout` is a size plus an alignment (a power of two). Returning a null pointer
   means "out of memory"; Rust then calls the allocation-error handler, which in
   `no_std` defaults to a panic (`memory allocation of N bytes failed`). The
   `#[global_allocator]` attribute on one `static` tells the compiler "every `Box`,
   `Vec` and `String` in the program goes through this one".

The simplest correct design is a **bump allocator**: keep a pointer `next` to the
first free byte; to allocate, round `next` up to the requested alignment, hand that
address out, and move `next` past the block. Freeing a single block does nothing,
because there is no record of where blocks are. A counter of live allocations lets one
case still work: when it drops to zero, *nothing* is in use, so `next` can jump back
to the start. Allocation is a handful of instructions; the price is that one
long-lived allocation pins the whole heap forever. Better designs are the next step.

**What we did.**

*Build configuration.* `.cargo/config.toml`:

```toml
build-std = ["core", "compiler_builtins", "alloc"]
```

*A bug in Step 13, found before it could bite.* `memory::frame_allocator()` built a
brand new `FrameAllocator { next: 0, .. }` on every call. Two callers — the heap
setup and the probe-mapping test — would each have started from frame 0 of the
memory map and received **the same physical frames**: two virtual pages silently
aliasing one piece of RAM, each overwriting the other. It never showed up because only
one caller existed. Now there is exactly one allocator for the whole kernel, built in
`memory::init` and kept behind a lock:

```rust
static FRAME_ALLOCATOR: Once<Mutex<FrameAllocator>> = Once::new();

pub fn frame_allocator() -> MutexGuard<'static, FrameAllocator> { .. }
```

Callers hold the guard while they map, and pass `&mut guard` where a
`&mut FrameAllocator` is expected — the compiler turns one into the other through
`DerefMut` (*deref coercion*), so the call sites barely changed.

*`create_mapping` made stricter.* It now takes the leaf flags as a parameter and
returns `Result<(), MapError>` instead of panicking or silently overwriting:

| `MapError` | When |
|---|---|
| `Unaligned` | `virt` or `frame` not a multiple of 4096 |
| `AlreadyMapped` | the leaf entry is already present — the old version would have overwritten it, leaking or aliasing whatever was there |
| `InsideHugePage` | an intermediate entry is a 2 MiB/1 GiB page, so there is no table below it; the old version would have read the huge frame's *data* as if it were a page table |
| `OutOfFrames` | no frame left for a missing intermediate table |

A new flag, `NO_EXECUTE` (bit 63 of an entry). The bootloader turns on `EFER.NXE`
(Extended Feature Enable Register, No-eXecute Enable — read in
`bootloader-0.9.35/src/main.rs`, `enable_nxe_bit()`); without NXE, bit 63 is
*reserved* and setting it makes every access to that page fault. Intermediate tables
never get it: the CPU combines the permissions of all four levels, so an NX on a PML4
entry would forbid execution for 512 GiB at once.

*`src/allocator.rs`, new:*

- `HEAP_START = 0x4444_4444_0000`, `HEAP_SIZE = 1 MiB` — an arbitrary, recognisable
  address in an otherwise empty part of the address space. Because `create_mapping`
  now refuses to map over anything already present, a collision would be an error at
  boot, not quiet corruption.
- `init_heap()` maps the 256 pages, each `WRITABLE | NO_EXECUTE`, then hands the range
  to the allocator. A second call fails with `AlreadyMapped` before touching the
  allocator, so the range can only ever be handed over once.
- `BumpAllocator { heap_start, heap_end, next, allocations }`. All arithmetic is
  checked: `align_up` and `start + size` return `None` (→ null pointer) on overflow
  instead of wrapping around to a small address that would land on top of the kernel.
  An allocator that was never `init`ed has `heap_end == 0` and refuses everything —
  a clean panic, not a write to address 0.
- `Locked<A>`, a wrapper around `spin::Mutex<A>`, and
  `unsafe impl GlobalAlloc for Locked<BumpAllocator>`. Every lock is taken inside
  `idt::without_interrupts`.

*Callers.* `_start` in `src/main.rs`, in the library's test kernel, and in the new
integration tests: `init()` → `memory::init(boot_info)` →
`allocator::init_heap().expect(..)`. The banner now prints a `Vec` and a `Box`.

*Tests.* Four new library cases — `mapping_the_same_page_twice_is_refused`, and three
pure-logic tests of `BumpAllocator` (alignment and reclaim, refusing what does not fit,
never wrapping around) that run it over made-up addresses, since the bump logic never
touches the memory it hands out. Two new integration kernels:

- `tests/heap_allocation.rs` — the first integration test using our test *runner*
  (`custom_test_frameworks`) instead of `harness = false`, seven cases: the range is
  mapped end to end and not one byte further, a second `init_heap` is refused, `Box`
  and a 1000-element `Vec` land inside the heap range, `format!`/`String` work, a
  `#[repr(align(4096))]` value really comes back page-aligned, and 2 MiB worth of
  short-lived boxes fit in a 1 MiB heap (only possible if freeing rewinds `next`).
- `tests/heap_no_execute.rs` — writes a single `ret` instruction (`0xc3`) into a
  `Box`, turns the pointer into a function pointer, and calls it. The handler asserts
  that `CR2` is the box's address and that the error code has bit 0 (present) and
  bit 4 (instruction fetch) set and bit 1 (write) clear.

**What happened.**

The first build failed on the banner, through a mistake in formatting, not in memory:

```
error[E0277]: the trait bound `Box<u64>: LowerHex` is not satisfied
  --> src/main.rs:60:52
   |
60 |     println!("Heap: {:?}, {:#x} at {:p}", squares, boxed, boxed);
   |                           -----                    ^^^^^ the trait `LowerHex` is not implemented for `Box<u64>`
```

`Box<T>` forwards `Display` and `Debug` to `T`, but not `LowerHex`; `{:#x}` needs the
`u64` itself, so it became `*boxed`. `{:p}` on the `Box` itself prints the address.

Clippy flagged one new line, with a nightly-era suggestion:

```
warning: manual implementation of `.is_multiple_of()`
208 |     if virt % PAGE_SIZE != 0 || frame % PAGE_SIZE != 0 {
    |        ^^^^^^^^^^^^^^^^^^^^^ help: replace with: `!virt.is_multiple_of(PAGE_SIZE)`
```

Taken. Every other clippy warning in the crate predates this step.

`cargo test`, all five kernels:

```
Running 11 tests
baddcafe_os::a_freshly_created_mapping_is_readable_and_writable... [ok]
...
baddcafe_os::bump_allocator_aligns_and_reclaims... [ok]
baddcafe_os::bump_allocator_never_wraps_around... [ok]
baddcafe_os::bump_allocator_refuses_what_does_not_fit... [ok]
...
baddcafe_os::mapping_the_same_page_twice_is_refused... [ok]
...
Running 7 tests
heap_allocation::box_lives_on_the_heap... [ok]
heap_allocation::freed_memory_is_reused... [ok]
heap_allocation::heap_is_mapped_end_to_end... [ok]
heap_allocation::large_vec_grows_and_keeps_its_contents... [ok]
heap_allocation::over_aligned_allocation_is_honoured... [ok]
heap_allocation::second_init_is_refused... [ok]
heap_allocation::string_formatting_allocates... [ok]
heap_no_execute::jump_into_heap... [ok]
page_fault::write_to_unmapped_page... [ok]
page_fault_protection::write_to_executable_page... [ok]
stack_overflow::double_fault_survives_stack_overflow... [ok]
```

22 tests, exit 0, in both `cargo test` and `cargo test --release`.

A passing test proves nothing until it has been seen to fail. With `NO_EXECUTE`
temporarily removed from `init_heap`, the NX test fails exactly as it should — the CPU
happily fetched and ran the `ret` from the heap and returned:

```
heap_no_execute::jump_into_heap... [failed]
panicked at tests/heap_no_execute.rs:45:5:
executed an instruction fetched from the heap
```

The production kernel, booted headless and its screen read back with `pmemsave`:

```
...and we came back from the exception.
Heap: [1, 4, 9, 16, 25, 36, 49, 64], 0xbaddcafe at 0x444444440040
Timer and keyboard interrupts are on. Type:
```

`0x444444440040` is the bump allocator showing its work: the `Vec` of eight `u64`s
took the first 64 (`0x40`) bytes of the heap, and the `Box` landed directly after it.

**Why.** `Box::new` compiles to a call to `__rust_alloc`, which `#[global_allocator]`
resolves to our `ALLOCATOR.alloc`. That returns an address in the heap range; the
first write through it goes to the MMU (Memory Management Unit), which walks the four
tables `init_heap` built and finds a present, writable leaf. Nothing about `Box` is
special to the kernel — it is just memory, and every piece of the path from a type
in `alloc` to an address line is now code we wrote.

Why interrupts are off while the allocator lock is held: we have one CPU and a spin
lock. If the kernel holds the lock inside `alloc` and the timer fires, and the
handler allocates, the handler spins waiting for a lock that only the interrupted code
can release — and that code cannot run until the handler returns. A deadlock with no
second party. No handler allocates today; turning interrupts off makes that a
non-question rather than a rule someone has to remember. The frame allocator's lock
has the same hazard and is documented as off-limits to handlers instead, because it
is only used at boot.

**Rust concepts touched.**

- **The `core` / `alloc` / `std` split** — `extern crate alloc;` is still required in
  edition 2024 for crates the compiler does not link implicitly, which in `no_std` is
  everything except `core`.
- **`unsafe trait`** — `GlobalAlloc` is declared `unsafe trait`, so implementing it is
  `unsafe impl`: the *implementer* promises the compiler something it cannot check
  (every pointer returned is valid for the `Layout` and not handed out twice). That is
  the opposite direction from an `unsafe fn`, where the *caller* makes the promise.
  `BumpAllocator::init` is the latter: its caller promises the range is mapped and
  unused.
- **Interior mutability through a lock** — `GlobalAlloc::alloc` takes `&self`, because
  the global allocator is a shared `static`; mutation goes through the `Mutex`.
- **The orphan rule** — a trait can only be implemented for a type if the trait or the
  type is defined in our crate. `GlobalAlloc` (in `core`) and `spin::Mutex` (in
  `spin`) are both foreign, which is why `Locked<A>` exists at all.
- **Deref coercion** — `&mut MutexGuard<FrameAllocator>` passing as
  `&mut FrameAllocator`.
- **`Result` and `?` replacing panics** — `create_mapping` errors are values the caller
  decides about; `init_heap` forwards them with `?`, and only `_start` chooses to
  `expect`.
- **`let … else`** — `let Some(start) = align_up(..) else { return null };` binds on
  success and forces the failure branch to diverge.
- **Checked arithmetic** — `checked_add`/`checked_sub` return `Option`, the honest
  type for "this might not fit in 64 bits"; plain `+` panics on overflow in debug
  builds and silently wraps in release builds.

---

## Step 15 — A linked-list allocator: reusing any freed block

**Goal.** Replace the bump allocator with one that can reuse an individual freed block,
so that one long-lived allocation no longer pins the whole heap. Prove the bump
allocator's weakness with a test first, then make that test pass.

**Concept.** An allocator needs to remember which parts of the heap are free. The
obvious place for that record would be a data structure on the heap — but we *are*
the heap, so there is nowhere to put it. The trick is that free memory is, by
definition, memory nobody is using: the allocator may write into it. So every free
region carries its own header in its first bytes:

```rust
struct ListNode {
    size: usize,
    next: Option<&'static mut ListNode>,
}
```

16 bytes on x86-64 (`Option<&mut T>` is a single pointer, `None` being the null
address — the *niche optimisation*). The free regions form a singly linked list
threaded through the free memory itself; the allocator stores only the head.

- **Allocate** (*first fit*): walk the list, take the first region the request fits
  in, unlink it, and put back whatever is left over before and after the block.
- **Free**: write a new `ListNode` at the freed address and push it on the front of
  the list.

Two consequences follow from the header living inside the block:

1. Every block must be able to hold a `ListNode` once it is freed, so each request is
   rounded up to at least 16 bytes and to a multiple of 8 (the node's alignment).
2. A leftover piece smaller than 16 bytes can never go back on the list. The allocator
   must refuse a placement that would leave one, or those bytes leak forever.

What it does *not* do is merge neighbouring free regions back together (*coalescing*).
Free A then its neighbour B and the list holds two small regions, not one large one:
over time the heap fragments into pieces too small for big requests.

**What we did.**

*A correction to the journal first.* Step 14's `Current state` said "Step 14 is
uncommitted" and listed housekeeping commits `703dce8` and `f565da1`. Neither hash
exists any more: the history was rewritten, the README/license and `.gitignore` work
became `a6ea869`, and Step 14 itself was committed as `64697c0`. The `Current state`
section now lists the real hashes.

*The failing test first.* `tests/heap_allocation.rs`, `long_lived_box_does_not_pin_the_heap`:
one `Box` kept alive while 2 MiB worth of short-lived boxes are created and dropped.
Against the bump allocator:

```
heap_allocation::long_lived_box_does_not_pin_the_heap... [failed]
panicked at ...\library\alloc\src\alloc.rs:699:9:
memory allocation of 8 bytes failed
```

The live-allocation counter never reaches zero, so `next` never rewinds and the 1 MiB
runs out half-way through. `alloc` returned null; `Box::new` answered with
`handle_alloc_error`, which in `no_std` panics.

*`src/allocator.rs` split into a module directory.* The root keeps `HEAP_START`,
`HEAP_SIZE`, `init_heap`, `Locked<A>`, `align_up` and the `#[global_allocator]`
static. Each design gets a file of its own with its own
`unsafe impl GlobalAlloc for Locked<…>`: `src/allocator/bump.rs` (moved unchanged),
`src/allocator/linked_list.rs` (new). Swapping designs is a one-line change to the
type of `ALLOCATOR`. Child modules can see the parent's private `align_up` — privacy in
Rust stops at the module boundary going *out*, not going *in*.

*`LinkedListAllocator`*, in `src/allocator/linked_list.rs`:

- `head: ListNode` — a dummy node of size 0 whose `next` is the real list. A dummy
  head means removing the first real region is no special case.
- `add_free_region(addr, size)` — asserts alignment and minimum size, writes a node
  with `ptr.write`, links it in front.
- `find_region(size, align)` — walks with a `current: &mut ListNode` cursor; on a fit,
  unlinks `current.next` and returns it as `&'static mut ListNode` plus the start
  address.
- `alloc_from_region` — computes the start address and rejects any placement that
  would leave a gap of 1-15 bytes in front of the block or behind it. When the gap in
  front would be too small, it moves to the next aligned address instead of giving up.
- `alloc` — gives *both* gaps back to the list. phil-opp's version returns only the
  piece behind the block and leaks the alignment padding in front; a page-aligned
  request could waste up to 4 KiB each time.

*Unit tests* in `src/lib.rs`. Unlike the bump allocator, this one writes into the
memory it manages, so made-up addresses will not do. `leaked_arena(bytes)` takes a
`Vec<u64>` from the real kernel heap and leaks it with `Vec::leak`, which returns a
`&'static mut [u64]`: memory that really does live forever, so the `'static` lists
pointing into it are not a lie. Four cases: a freed block is handed out again, a
request that would leave an 8-byte tail is refused, the padding before a 256-aligned
block is reused, and a block moves past a too-small gap.

**What happened.**

With `ALLOCATOR` switched to `Locked<LinkedListAllocator>`, it compiled first time,
and the integration test that had failed passed:

```
heap_allocation::long_lived_box_does_not_pin_the_heap... [ok]
```

The new unit tests then found a real bug in my own `size_align`:

```
baddcafe_os::linked_list_keeps_the_alignment_padding... [failed]
panicked at src\lib.rs:279:5:
assertion `left == right` failed
```

The first version followed phil-opp, `layout.align_to(8)?.pad_to_align()`.
`pad_to_align` rounds the *size* up to a multiple of the *requested alignment*: a
16-byte request aligned to 256 became a 256-byte request and no longer fit in the
264-byte test region. In the kernel it would not crash, only waste — a 16-byte,
page-aligned allocation would eat a whole page. The header only needs the size to be
a multiple of 8, so the fix rounds to that:

```rust
let size = align_up(layout.size(), NODE_ALIGN)?.max(NODE_SIZE);
Some((size, layout.align().max(NODE_ALIGN)))
```

After it, all cases passed.

A prediction that turned out wrong, recorded because the reason is instructive: I
also wrote `growing_allocations_do_not_fragment_the_heap`, 2000 `Vec`s of capacity
1, 2, …, 2000, about 2 MiB in total, expecting a list that never merges to run out
of memory. It passed. Every size is rounded up to a multiple of 8, so the 2000
requests are only ~250 distinct sizes; same-size requests reuse the block just freed,
and the ~250 distinct ones together need about 250 KiB. A test that does not check
what its name says was deleted.

**Why.** `dealloc` receives the same `Layout` that `alloc` did — that is the
`GlobalAlloc` contract — so it can recompute the block's size with the same
`size_align` and does not need to store it anywhere. The size lives in the `ListNode`
only while the block is free; while it is in use, the user owns every byte.

**Rust concepts touched.**

- **`Option<&'static mut ListNode>`** — a borrow that never ends, pointing at memory
  that has no owner in the `Box` sense. It is created out of a raw address with
  `&mut *ptr` inside `unsafe`; the invariant is that the region is mapped, unused and
  on the list exactly once, so no other `&mut` to it exists.
- **`Option::take()`** — moves the value out and leaves `None` behind. This is how a
  `&mut` is unlinked from a list without ever having two live `&mut`s to the same node:
  ownership of the reference moves, nothing is copied.
- **The cursor pattern and non-lexical lifetimes** — in `find_region`, `region`
  borrows `current.next`; the borrow checker accepts `current.next.take()` right after
  because `region` is not used again (NLL ends a borrow at its last use, not at the end
  of the block).
- **Deref coercion on assignment** — `current = current.next.as_mut().unwrap()`
  assigns a `&mut &'static mut ListNode` to a `&mut ListNode`.
- **`ptr.write(value)`** versus `*ptr = value` — assignment would first *drop* the old
  value at that address, which is garbage; `write` does not read or drop anything.
- **`Vec::leak`** — turning an owned allocation into a `&'static mut` by giving up the
  ability to free it.
- **Module privacy** — child modules see the parent's private items.

---

## Step 16 — A fixed-size block allocator, and the end of the list walk

**Goal.** Make the common case — small allocations — constant-time, and make the
global allocator a fixed-size block allocator with the linked list behind it as
fallback.

**Concept.** The linked list walks the free list on every allocation, and that list
grows with fragmentation. Most kernel allocations, however, are small and come in a
few sizes. A *fixed-size block allocator* rounds every request up to one of a few
size classes — 8, 16, 32, …, 2048 bytes — and keeps one free list per class. All blocks
on a list are interchangeable, so:

- **Allocate**: pop the head of the class's list. If the list is empty, carve a new
  block from the fallback allocator.
- **Free**: push the block onto its class's list. Every block of a class is big
  enough, so no search and no splitting.

Each block size is also used as its alignment (a 64-byte block is 64-aligned). A
request then fits a class if both its size and its alignment are at most the block
size. Anything above 2048 bytes goes straight to the fallback.

The price: internal waste (a 33-byte request gets a 64-byte block), and blocks never
return to the fallback. Once memory has been cut into 16-byte blocks, it serves only
16-byte requests from then on.

**What we did.**

*`src/allocator/fixed_size_block.rs`, new:*

```rust
const BLOCK_SIZES: &[usize] = &[8, 16, 32, 64, 128, 256, 512, 1024, 2048];

struct FreeBlock {
    next: Option<&'static mut FreeBlock>,
}

pub struct FixedSizeBlockAllocator {
    list_heads: [Option<&'static mut FreeBlock>; BLOCK_SIZES.len()],
    fallback: LinkedListAllocator,
}
```

- `FreeBlock` holds only `next`. The size is implied by which list the block is on.
- `const _: () = assert!(BLOCK_SIZES[0] >= mem::size_of::<FreeBlock>());` — a
  compile-time check that the smallest block can hold its own header. If someone ever
  adds a 4-byte class, the build fails instead of the heap.
- `list_index(layout)` — the first class `>= max(size, align)`.
- A new block is carved from the fallback with `Layout::from_size_align(size, size)`,
  which is what makes every block aligned to its own size.

`ALLOCATOR` became `Locked<FixedSizeBlockAllocator>`; `init_heap` is unchanged, since
`init` simply hands the range to the fallback.

*Tests.* One library case, `fixed_size_blocks_are_recycled_per_size_class`: a 24-byte
request gets a 32-aligned block; once it is freed, a 20-byte request (same class) gets
the *same* block back, an 8-byte request (another class) does not; a 4096-byte request
goes to the fallback and is reused there too.

**What happened.** It compiled on the first try. `cargo test`, every kernel:

```
Running 16 tests
...
baddcafe_os::fixed_size_blocks_are_recycled_per_size_class... [ok]
...
baddcafe_os::linked_list_keeps_the_alignment_padding... [ok]
baddcafe_os::linked_list_never_leaves_a_gap_too_small_for_a_node... [ok]
baddcafe_os::linked_list_refuses_what_does_not_fit... [ok]
baddcafe_os::linked_list_reuses_a_freed_block... [ok]
...
Running 8 tests
heap_allocation::box_lives_on_the_heap... [ok]
heap_allocation::freed_memory_is_reused... [ok]
heap_allocation::heap_is_mapped_end_to_end... [ok]
heap_allocation::large_vec_grows_and_keeps_its_contents... [ok]
heap_allocation::long_lived_box_does_not_pin_the_heap... [ok]
heap_allocation::over_aligned_allocation_is_honoured... [ok]
heap_allocation::second_init_is_refused... [ok]
heap_allocation::string_formatting_allocates... [ok]
heap_no_execute::jump_into_heap... [ok]
page_fault::write_to_unmapped_page... [ok]
page_fault_protection::write_to_executable_page... [ok]
stack_overflow::double_fault_survives_stack_overflow... [ok]
```

28 tests, exit 0, in both `cargo test` and `cargo test --release`. Clippy reports
nothing in the allocator files.

The production kernel, booted headless on Windows and its screen read through a TCP
monitor (the recipe is now in the Reference section):

```
...and we came back from the exception.
Heap: [1, 4, 9, 16, 25, 36, 49, 64], 0xbaddcafe at 0x444444440040
```

The same address as under the bump allocator, for a different reason: the `Vec` of
eight `u64`s (64 bytes) took a 64-byte block carved at the very start of the heap, and
the `Box<u64>` took an 8-byte block carved right after it.

`over_aligned_allocation_is_honoured` exercises the boundary: a 16-byte value aligned
to 4096 needs `max(16, 4096) = 4096`, above the largest class, so it goes to the linked
list — which, after Step 15's fix, uses 16 bytes plus padding instead of 4096.

**Why.** Popping and pushing a list head is a few instructions whatever the heap's
state, which matters once interrupt-driven code allocates: the time spent with
interrupts off inside the allocator is now bounded for small requests. The linked list
still exists for what the classes cannot serve, so the two designs cover each other's
weakness.

**Rust concepts touched.**

- **`[const { None }; N]`** — an array repeat expression normally needs a `Copy`
  value, and `Option<&mut T>` is not `Copy` (copying it would create two `&mut`s). An
  inline `const` block is evaluated once per element instead, which is allowed.
- **`BLOCK_SIZES.len()` in a type** — array lengths must be known at compile time; the
  `len()` of a `const` slice is a `const fn`, so it can size `list_heads`.
- **Compile-time assertions** — `const _: () = assert!(..);` runs the check during
  compilation; a failure is a build error, not a panic.
- **`&mut T as *mut T as *mut u8`** — turning a reference back into a raw address to
  hand out. The `&'static mut FreeBlock` is consumed by the cast, so no reference to
  the block survives once the user owns it.
- **`let … else` with an `unsafe` return** — `dealloc` forwards to the fallback's
  `unsafe fn` inside the diverging branch.

---

## Step 17 — Futures, pinned tasks and a wake-driven executor

**Goal.** Use the heap from Steps 14-16 to hold asynchronous kernel tasks. The human
explicitly asked for implementation and verification first, with the explanations
saved here for later study; the usual teaching-first rule remains the default for
future work.

**Concept.** Calling an `async fn` builds a value representing unfinished work, a
`Future`. It does not execute the function's body yet. An executor asks that value
to make progress through `poll`: `Ready(())` means finished, `Pending` means it must
wait. The compiler turns locals and suspension points into a state machine, so the
task can return from `poll` without losing its place. `.await` suspends the enclosing
future only when the awaited future returns `Pending`.

The future can contain a reference to one of its own locals across an await. Moving
that state machine would invalidate the reference. `Pin<Box<dyn Future<Output = ()>>>`
solves this: the allocation stays put while the box pointer can move. `dyn Future`
lets different compiler-generated future types share one collection. `'static` on
the constructor means a task cannot borrow a temporary from its caller; it does not
mean the task leaks or lives forever. The executor owns and drops it on completion.

A `Waker` is the task's doorbell, not the task itself. Ringing it marks the task as
ready; only the executor calls `poll`. We implement the safe `alloc::task::Wake`
trait with an `Arc` (atomically reference-counted pointer), so cloned wakers keep
their flag alive without raw pointers or a hand-written `RawWaker` table.

**What we did.** Added `src/task.rs` and `src/task/executor.rs`, exported `task` from
the library, and added five library tests. `Task::new` pins its future. `spawn`
allocates a ready flag and caches a waker. `run_ready_tasks` visits each task once
per pass and polls only those whose ready flag was set. `yield_now().await` rings
the task's own doorbell and returns `Pending` once, giving peers their turn.

This deliberately differs from the tutorial's bounded ready queue: a flag per task
coalesces repeated wakes without any queue that can overflow. The tradeoff is an
O(number of tasks) scan per pass, reasonable for this small single-CPU kernel. The
executor does not busy-poll pending futures. No dependencies were added.

**What happened.** This session runs in Windows Subsystem for Linux (WSL), while
the installed Rust toolchain is on Windows. The first attempts exposed the boundary:

```text
zsh:1: command not found: cargo
<3>WSL (2 - ) ERROR: UtilBindVsockAnyPort:307: socket failed 1
```

Using the existing `cargo.exe` with approved WSL-to-Windows execution fixed the
environment. The baseline `cargo.exe build` and `cargo.exe test` passed all 28
existing cases. After this step, `cargo.exe build` succeeded and
`cargo.exe test --lib` reported:

```text
Running 21 tests
baddcafe_os::task::executor::tests::a_wake_before_sleep_keeps_the_executor_running... [ok]
baddcafe_os::task::executor::tests::completed_tasks_drop_and_stale_wakers_cannot_wake_new_tasks... [ok]
baddcafe_os::task::executor::tests::moving_task_storage_preserves_a_borrow_across_await... [ok]
baddcafe_os::task::executor::tests::pending_tasks_wait_for_a_wake_and_wakes_coalesce... [ok]
baddcafe_os::task::executor::tests::self_wakes_survive_poll_and_give_peers_a_turn... [ok]
```

The pre-existing future-incompatibility warning from nightly's `core` remains.
Also, the previous snapshot's statement that Steps 15-16 were uncommitted was
stale: they are committed as `f48128e`.

**Why.** The ready flag is cleared *before* polling with an acquire-release atomic
exchange. A wake during polling therefore survives for the next pass. A wake uses
a release store, paired with the executor's acquire operation. On completion the
entry is removed; old wakers keep only their old flag and cannot wake a new task.

Sleeping has another race: an interrupt could signal work after an idle check but
before `hlt`, leaving the processor asleep with work ready. The executor disables
interrupts, checks again, then executes `sti; hlt` in one assembly block. On x86,
`sti` delays interrupt delivery until after the following instruction. The assembly
also retains its compiler memory barrier. This assumes one active CPU (Central
Processing Unit); a remote CPU would need to send a wakeup interrupt too. Tasks must
return from `poll` and leave interrupts enabled; this is cooperative scheduling,
not preemptive scheduling that can forcibly take the CPU away from a task.

**Rust concepts touched.**

- `Future`, `Poll`, `Context`, compiler-generated async state machines and `.await`.
- `Pin`, `Box`, trait objects, ownership and a `'static` bound without a memory leak.
- `Arc`, `Wake`, `Waker` and acquire/release atomic ordering.
- `Rc` and interior mutability in tests: tasks themselves do not need to be `Send`.

Reading for the revisit: [the async chapter](https://os.phil-opp.com/async-await/),
[`Future`](https://doc.rust-lang.org/core/future/trait.Future.html),
[`Wake`](https://doc.rust-lang.org/alloc/task/trait.Wake.html) and
[`Waker`](https://doc.rust-lang.org/core/task/struct.Waker.html).

---

## Reference — files, commands, and how to read the machine

This section is not a journal entry. It is the lookup table: every configuration file
we have, every command worth remembering, and how to decode what the machine shows us.
Unlike the numbered steps, **this section may be edited and kept current**.

### R1. The configuration files, and why each one exists

Four files configure this project. None of them contain kernel logic; together they are
the reason `cargo build` produces something a bare machine can run.

#### `rust-toolchain.toml` — which compiler

```toml
[toolchain]
channel = "nightly"
components = ["rust-src", "llvm-tools-preview"]
```

`rustup` reads this whenever a cargo command runs inside this directory and switches to
the named toolchain automatically. Nobody has to remember `+nightly`.

Nightly is not a preference. Two things we depend on are unstable: `-Z build-std`
(recompiling the standard library for a target that has no prebuilt copy) and
`.json` target specifications. Stable `rustc` refuses both.

- `rust-src` ships the *source code* of `core`, so `build-std` has something to compile.
- `llvm-tools-preview` ships `llvm-objcopy`, the tool `bootimage` uses to glue the
  bootloader and the kernel into one disk image.

#### `Cargo.toml` — the package

```toml
[package]
name = "baddcafe_os"
version = "0.1.0"
edition = "2024"

[package.metadata.bootimage]
run-args = ["-serial", "stdio"]
test-args = [
    "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    "-serial", "stdio",
    "-display", "none",
]
test-success-exit-code = 33

[[test]]
name = "stack_overflow"
harness = false

[[test]]
name = "page_fault"
harness = false

[[test]]
name = "page_fault_protection"
harness = false

[dependencies]
bootloader = "0.9.35"
spin = "0.12.3"

[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"
```

- `edition = "2024"` — the newest Rust edition. It is why we write
  `#[unsafe(no_mangle)]` where the blog writes `#[no_mangle]`.
- `panic = "abort"` on **both** profiles — by default a Rust panic *unwinds* the stack,
  running destructors on the way out. Unwinding is implemented in libstd on top of
  libunwind, which is C library code we no longer link. With `abort`, a panic simply
  stops. Both profiles need it, because a build in either mode would otherwise ask for
  the missing unwinder.
- `bootloader = "0.9.35"` — a dependency our source never `use`s. It is here so that
  `bootimage`, an external tool, can read the manifest and learn which bootloader
  version to build. Cargo warns `unused dependency`; the warning is wrong for our case.
- The `[[test]]` tables — each is an integration test that is a whole kernel of its own. `harness = false` means cargo does not look for `#[test]`
  functions in it and simply runs its `_start`; the test either exits QEMU itself or
  fails. They exist because a test whose handler ends the session cannot return to a
  shared test runner.
- `spin = "0.12.3"` — a spinlock. There is no operating system to put a waiting thread
  to sleep, so a contended lock loops instead. Used for the global `WRITER` and `SERIAL`.
- `[package.metadata.bootimage]` — cargo ignores this table; `bootimage` reads it.
  `run-args` and `test-args` are appended to the QEMU command line for `cargo run` and
  `cargo test`. `test-success-exit-code = 33` is `(0x10 << 1) | 1`: the status QEMU
  produces when the kernel writes `QemuExitCode::Success` to the `isa-debug-exit` port.
  Without it every test run would look like a failure, because that device's status is
  always odd.

#### `.cargo/config.toml` — how cargo behaves in this directory

```toml
[build]
target = "x86_64-baddcafe_os.json"

[unstable]
build-std = ["core", "compiler_builtins", "alloc"]
build-std-features = ["compiler-builtins-mem"]
json-target-spec = true
panic-abort-tests = true

[target.'cfg(target_os = "none")']
runner = "bootimage runner"
```

- `build.target` — always cross-compile for our own target, so plain `cargo build` does
  the right thing without flags.
- `build-std` — for a target rustc has never heard of, no precompiled `core` exists, so
  cargo compiles `core` and `compiler_builtins` from source alongside our crate.
  `alloc` (`Box`, `Vec`, `String`) is in the list since Step 14, when the kernel got a
  heap; it is the layer between `core` and `std` that needs an allocator but no OS.
- `build-std-features = ["compiler-builtins-mem"]` — the compiler emits calls to
  `memcpy`, `memset` and `memcmp` for ordinary things like assigning a struct. Normally
  libc provides them. There is no libc here, so `compiler_builtins` must provide them.
- `json-target-spec = true` — custom `.json` targets are gated behind their own
  unstable flag on current nightly. Without it: *"`.json` target specs require
  -Zjson-target-spec"*.
- `panic-abort-tests = true` — cargo builds test targets under the `test` profile and
  by default forces `panic = "unwind"` there, because the std test harness catches a
  panicking test. That contradicts `panic = "abort"` everywhere else, and under
  `build-std` it makes cargo compile a **second** `core`, which then collides with the
  first over lang items. See Step 7.
- `runner` — cargo cannot execute our binary; it is not a Linux program. This key says
  "instead of running it, hand it to `bootimage runner`", which builds the disk image
  and launches QEMU. That is what makes `cargo run` work.

#### `x86_64-baddcafe_os.json` — the machine we are compiling for

A **target specification** is data describing the machine: pointer width, endianness,
which linker, what the operating system is. `rustc` is a cross-compiler by default; a
target is just a description it reads, not a separate build of the compiler.

| Field | Why |
|---|---|
| `"llvm-target": "x86_64-unknown-none"` | What LLVM is told to generate code for. `unknown` vendor, `none` OS. |
| `"data-layout"` | Sizes and alignments of every primitive type, in LLVM's notation. Copied verbatim from the built-in `x86_64-unknown-none`. |
| `"os": "none"` | **The critical one.** It is what stops the linker from adding the C runtime startup object. That object defines its own `_start`, which collided with ours in Step 2. |
| `"linker-flavor": "ld.lld"`, `"linker": "rust-lld"` | Use the LLVM linker shipped with Rust, instead of the host's `cc`. The host linker builds Linux programs; we are not building one. |
| `"panic-strategy": "abort"` | Same reason as in `Cargo.toml`: no unwinder exists. |
| `"disable-redzone": true` | The **red zone** is 128 bytes below the stack pointer that a function may use without reserving them, because on Linux nothing can touch that memory. In a kernel an interrupt can fire at any instruction and pushes onto the same stack — silently destroying the red zone. It must be off. |
| `"features": "-mmx,-sse,+soft-float"` | Disable the SIMD units. SIMD registers are large; saving and restoring them on every interrupt is expensive, and enabling SSE properly requires setup we have not done. `soft-float` makes the compiler emulate floating point in ordinary integer instructions. |
| `"rustc-abi": "softfloat"` | Turning off SSE changes how floats are *passed to functions* — the normal x86-64 ABI passes them in `xmm` registers, which we just disabled. Modern rustc refuses to let the feature and the ABI disagree silently, so the target must say so explicitly. |

### R2. Commands

#### Source layout

```
src/main.rs        the kernel's `_start`, the banner, the panic handlers
src/lib.rs         module list, init(), hlt_loop, exit_qemu, the test runner and
                   the library's own test kernel and test cases
src/vga_buffer.rs  the VGA text buffer: Color, ColorCode, ScreenChar, Writer,
                   the global WRITER, and the print!/println! macros
src/serial.rs      the 16550 UART on COM1 and serial_print!/serial_println!
src/port.rs        port-mapped I/O primitives: outb, outl, inb
src/gdt.rs         GDT, TSS and the double-fault stack
src/idt.rs         the IDT, the exception and interrupt handlers, TICKS,
                   without_interrupts
src/pic.rs         the two 8259 PICs
src/keyboard.rs    scancode set 1 decoding
src/memory.rs      the page-table walker: translate(), the global frame allocator,
                   create_mapping(), through the bootloader's physical-memory offset
src/allocator.rs   the heap range, init_heap(), Locked<A>, align_up and the
                   #[global_allocator]
src/allocator/     bump.rs, linked_list.rs, fixed_size_block.rs: one allocator
                   design each, each with its own GlobalAlloc impl
tests/*.rs         integration tests, each a whole kernel of its own
```

#### Toolchain sanity

```bash
rustc --version                 # confirm nightly is active in this directory
rustup show active-toolchain    # confirm rust-toolchain.toml is being honoured
```

#### Build and run

```bash
cargo build                     # the main feedback loop
cargo bootimage                 # build kernel + bootloader, produce the disk image
cargo run                       # build, make the image, boot it in QEMU
cargo test                      # build the test kernel, boot it, run the test cases
```

Artifacts:

```
target/x86_64-baddcafe_os/debug/baddcafe_os                  # the kernel ELF
target/x86_64-baddcafe_os/debug/bootimage-baddcafe_os.bin    # the bootable disk image
```

The test kernel is a separate binary with a hash in its name; ask cargo for the path
rather than guessing it:

```bash
cargo test --no-run --message-format=json | grep -o '"executable":"[^"]*"'
```

#### Inspecting the kernel ELF

```bash
readelf -h <binary>                       # ELF header; e_entry is the entry point
nm <binary>                               # symbols: T = defined here, U = undefined
nm -u <binary>                            # undefined only; empty output = freestanding
objdump -d <binary>                       # disassemble everything
objdump -d --disassemble=_start <binary>  # disassemble one function
```

`nm -u` printing nothing is the real proof that a binary is freestanding: it asks
nothing of any library.

#### Inspecting the raw disk image

A disk image is not an ELF file. It has no symbols and no sections — it is bytes that
the firmware copies into memory. So the ELF tools do not apply; these do.

```bash
xxd -l 32 <img>                # first 32 bytes: the start of the boot sector
xxd -s 510 -l 2 <img>          # bytes 510-511: must be 55 aa
ls -l <img>                    # size

# disassemble the boot sector as 16-bit code, shown at the address it will live at
head -c 512 <img> > boot.bin
objdump -D -b binary -m i8086 -M intel --adjust-vma=0x7c00 boot.bin
```

`-b binary` means "there is no file format, just bytes". `-m i8086` means "decode as
16-bit x86" — the same bytes decode into *different instructions* in 16-, 32- and
64-bit mode, so this flag is not optional. `--adjust-vma=0x7c00` labels the listing
with the addresses the code will actually occupy.

#### QEMU

```bash
# normal run: opens a window
qemu-system-x86_64 -drive format=raw,file=<img>

# headless, for scripted checks
qemu-system-x86_64 -drive format=raw,file=<img> -display none

# log every CPU reset, and stop instead of rebooting
qemu-system-x86_64 -drive format=raw,file=<img> \
    -display none -no-reboot -d cpu_reset -D qemu.log
```

`-no-reboot` is the important one while debugging. A kernel that faults badly enough
**triple faults** — a fault, then a fault while handling it, then a fault while
handling *that* — and the CPU gives up and restarts. Without `-no-reboot` you see an
endless reboot loop; with it, QEMU exits at the first one and the log shows the state.

Reading the log: two `CPU Reset` blocks at the very start are normal — they are
power-on (all registers zero, then the architectural reset state `CS=f000`,
`EIP=0000fff0`). A **third** one means we crashed.

#### The QEMU monitor — asking the machine where it is

The monitor is an interactive console into the emulator. `-monitor stdio` attaches it
to the terminal, and it can be driven from a script:

```bash
{ timeout 4 tail -f /dev/null; echo "info registers"; echo "quit"; } \
  | qemu-system-x86_64 -drive format=raw,file=<img> -display none -monitor stdio
```

The `timeout 4 tail -f /dev/null` is a four-second pause that produces no output: it
gives the machine time to boot before the monitor command is sent. Useful monitor
commands: `info registers`, `info mem` (page mappings), `info tlb`, `xp /16xb <addr>`
(examine physical memory), `screendump <file>.ppm`, `quit`.

**Screenshotting a headless run.** `screendump` writes the current framebuffer as a
PPM image, which is how the kernel's output can be checked without a window:

```bash
{ timeout 5 tail -f /dev/null; echo "screendump screen.ppm"; echo "quit"; } \
  | qemu-system-x86_64 -drive format=raw,file=<img> -display none -monitor stdio
convert screen.ppm screen.png     # ImageMagick, for a viewable file
```

**Reading the screen as text.** A PPM needs eyes. `pmemsave` copies guest physical
memory to a host file instead, and the text buffer decodes to plain characters — every
even byte is an ASCII code, every odd byte its colour:

```bash
{ sleep 4; echo 'pmemsave 0xb8000 4000 "/abs/path/vga.bin"'; sleep 2; echo quit; } \
  | qemu-system-x86_64 -drive format=raw,file=<img> -display none -monitor stdio

python3 -c 'd=open("/abs/path/vga.bin","rb").read()
for r in range(25):
    print("".join(chr(c) if 32<=c<127 else " " for c in d[r*160:(r+1)*160:2]).rstrip())'
```

The quotes around the filename are required: unquoted, the monitor tries to parse the
path as a numeric expression and fails with *"invalid char \'s\' in expression"*.

**On Windows**, piping into `-monitor stdio` does not work (see Step 13). A monitor on a
TCP socket does: QEMU listens, and PowerShell connects and types the commands.

```powershell
$q = Start-Process qemu-system-x86_64 -PassThru -ArgumentList `
  "-drive format=raw,file=`"$img`" -display none -monitor tcp:127.0.0.1:45454,server,nowait"
Start-Sleep 4
$c = New-Object Net.Sockets.TcpClient('127.0.0.1', 45454)
$w = New-Object IO.StreamWriter($c.GetStream()); $w.AutoFlush = $true
$w.WriteLine('pmemsave 0xb8000 4000 "C:/abs/path/vga.bin"'); Start-Sleep 2
$w.WriteLine('quit'); $c.Close()
$d = [IO.File]::ReadAllBytes('C:/abs/path/vga.bin')
0..24 | % { $r = $_; (-join (0..79 | % { $b = $d[$r*160 + $_*2]
  if ($b -ge 32 -and $b -lt 127) { [char]$b } else { ' ' } })).TrimEnd() }
```

To do the same for a **test** run, hand the test executable to `bootimage runner` and
append the QEMU flags; the tool needs `CARGO_MANIFEST_DIR` when invoked outside cargo:

```bash
CARGO_MANIFEST_DIR=$PWD bootimage runner <test-exe> -display none -monitor stdio
```

This was our only way to see inside the kernel before it could print. Since Step 8 the
serial port does the same job in plain text, so the memory dump is now only for
inspecting the screen itself.

#### Serial

```bash
# serial to the terminal (what run-args/test-args do)
qemu-system-x86_64 -drive format=raw,file=<img> -display none -serial stdio

# serial to a file, which survives a run killed by timeout
qemu-system-x86_64 -drive format=raw,file=<img> -display none -serial file:/abs/path/ser.txt
```

With `-serial stdio` a run terminated by `timeout` can lose its buffered output to the
pipe; `-serial file:` never does, so prefer it in scripts.

### R3. How a PC boots, decoded from our own image

The disassembly below is the real first sector of our image, produced with the command
in R2. Every line is explained because every line is an era of x86 history.

```
7c00:  31 c0        xor    ax, ax
7c02:  8e d8        mov    ds, ax
7c04:  8e c0        mov    es, ax
7c06:  8e d0        mov    ss, ax
7c08:  8e e0        mov    fs, ax
7c0a:  8e e8        mov    gs, ax
7c0c:  fc           cld
7c0d:  bc 00 7c     mov    sp, 0x7c00
7c19:  e4 92        in     al, 0x92
7c1b:  a8 02        test   al, 2
7c1d:  75 06        jne    0x7c25
7c1f:  0c 02        or     al, 2
7c21:  24 fe        and    al, 0xfe
7c23:  e6 92        out    0x92, al
7c25:  fa           cli
```

**Machine code is bytes.** `31 c0` *is* `xor ax, ax`; the hex and the mnemonic are the
same thing written for a machine and for a human. Disassembly is the translation back.

**`xor ax, ax` zeroes a register.** XOR yields 1 only where two bits differ, so a value
XORed with itself is always zero. It takes two bytes where `mov ax, 0` takes three — and
a boot sector has exactly 512 bytes to work with.

**Segment registers, and why they are cleared.** The 8086 was 16-bit: a register held
at most 65535, addressing 64 KB. To reach 1 MB, addresses were computed as
`segment × 16 + offset`, with the segment held in `DS` (data), `SS` (stack), `ES`, `FS`,
`GS`. Setting them all to zero means "no offsetting; addresses mean what they say". The
bootloader does not trust whatever the firmware left in them. `cld` is the same
instinct: it forces string/block instructions to move forward rather than backward.

**Why `0x7c00` twice.** The firmware loads the boot sector to physical address `0x7c00`
— an arbitrary choice IBM made in 1981 that nobody has changed since. `mov sp, 0x7c00`
puts the stack there too, which is not a collision: the x86 stack grows *downward*, so
it occupies memory below the code while the code extends upward. They never meet.

**The A20 gate.** In 1985 the 80286 could address beyond 1 MB, but existing programs
relied on addresses wrapping around at 1 MB. For compatibility, IBM added a switch that
forces the 21st address line (A20) low. Every PC still powers on with that switch off,
so every bootloader must turn it on. Port `0x92` bit 1 is that switch. The `and al,0xfe`
clears bit 0 first — on the same port, **bit 0 resets the CPU**. Setting it by accident
would reboot the machine instead of unlocking memory.

**`cli`** disables interrupts. From here on the bootloader is rebuilding the world
(descriptor tables, page tables) and cannot survive being interrupted mid-way.

**`55 aa`, the last two bytes.** Firmware has no way to know whether a disk is bootable,
so there is a convention: if bytes 510 and 511 of the first sector are `0x55` and
`0xAA`, boot it. In binary those are `01010101` and `10101010` — alternating bits,
unlikely to appear by accident. Such an agreed-upon marker is called a **magic number**.
Without it, the firmware skips the disk.

### R4. Reading the register dump

A **register** is a storage slot inside the CPU itself — a few dozen of them,
extraordinarily fast. Everything the processor does passes through them.

| Register | Meaning |
|---|---|
| `RIP` | Instruction Pointer: the address of the **next** instruction. Named `IP` at 16 bits, `EIP` at 32, `RIP` at 64 — one register widened twice. |
| `RAX`, `RBX`, … | General purpose (`AX` → `EAX` → `RAX`, same history). |
| `CS` | Code Segment selector. QEMU annotates it `CS64` when the descriptor says 64-bit code. |
| `CPL` | Current Privilege Level. `0` is ring 0, full control — kernel. |
| `CR0` | Control Register 0. Bit 0 `PE` = protected mode; bit 16 `WP` = write protect; **bit 31 `PG` = paging enabled**. |
| `CR3` | Physical address of the top-level page table. |
| `CR4` | Bit 5 `PAE` = Physical Address Extension, a prerequisite for 64-bit mode. |
| `EFER` | An MSR (Model Specific Register). Bit 8 `LME` = long mode enabled, bit 10 `LMA` = long mode **active**, bit 11 `NXE` = pages may be marked non-executable. |

**The three modes.** A modern x86-64 machine powers on pretending to be a 1978 8086.
Backward compatibility was never broken, so the bootloader has to climb the ladder by
hand:

```
real mode      16-bit, 1 MB, no protection, no paging     <- power-on
protected mode 32-bit, segmentation and privilege levels
long mode      64-bit, paging mandatory                   <- where our kernel runs
```

Our Step 4 dump — `CS64`, `EFER` with LME+LMA, `CR0` bit 31, `CR4` PAE — is the
evidence that all three rungs were climbed before control reached `_start`.

---

## Current state

- Branch `master`. Committed: `75d458e` (steps 0-3), `d40df33` (step 4), `3fa414d`
  (steps 5-6), `ae7a1a8` (step 7), `da2a262` (step 8), `fd8bae1` (step 9), `d17b0df`
  (steps 10-11), `905b84f` (step 12), `e75713c` (step 13), housekeeping `2693bb2` and
  `a6ea869` (README and license), `64697c0` (step 14), `f48128e` (steps 15-16).
  Step 17 adds the wake-driven executor; keyboard integration follows next.
- Toolchain: nightly, pinned by `rust-toolchain.toml`, with `rust-src` and
  `llvm-tools-preview`. Steps 15-16 ran on Windows on `1.101.0-nightly` (2026-10-04);
  Step 14 was built on Linux on `1.100.0-nightly`. The file pins a channel, not a
  date, so each machine keeps whatever snapshot it has.
- Edition 2024. `panic = "abort"` on both profiles, `panic-abort-tests = true`.
  `build-std = ["core", "compiler_builtins", "alloc"]`. Unstable features in use:
  `custom_test_frameworks`, `abi_x86_interrupt`.
- Dependencies: `bootloader = { version = "0.9.35", features = ["map_physical_memory"] }`
  and `spin = "0.12.3"` (`Mutex` behind `WRITER`/`SERIAL`/`KEYBOARD`, the frame
  allocator and the heap allocator; `Once` behind the `IDT`, the `GDT`, the `TSS`, each
  test kernel's own IDT, and `memory`'s globals).
- The crate is a library plus a binary:
  - `src/lib.rs` — modules, `extern crate alloc`, `init()` (serial → GDT → IDT → PIC →
    `sti`), `hlt_loop()`, `QemuExitCode`, `exit_qemu`, `Testable`, `test_runner`,
    `test_panic_handler`, and a `#[cfg(test)]` `_start` that also sets up the heap.
    Sixteen test cases live here, and `leaked_arena()` for the allocator tests;
    five more library cases live in `task::executor`.
  - `src/main.rs` — `_start(boot_info)`: `init()` → `memory::init` →
    `allocator::init_heap` → banner (which now prints a `Vec` and a `Box`).
  - `tests/stack_overflow.rs`, `tests/page_fault.rs`, `tests/page_fault_protection.rs`
    — `harness = false`, zero-argument `_start`, unchanged.
  - `tests/heap_allocation.rs` — eight cases through our own test runner, with a
    `BootInfo`-taking `_start` that sets up the heap.
  - `tests/heap_no_execute.rs` — `harness = false`; jumps into the heap and expects an
    instruction-fetch page fault.
- `src/gdt.rs`: `Tss` (`repr(C, packed(4))`), a 20 KiB `UnsafeCell` stack for the
  double-fault handler, an eight-slot `Gdt` with a null descriptor, a ring-0 code
  segment and a 16-byte TSS descriptor, `lgdt`, a `retfq` trampoline to reload `cs`,
  and `ltr`.
- `src/idt.rs`: the hand-written IDT. One private `set` behind three public setters
  (plain, with error code, diverging with error code), `Entry::set_stack_index`,
  handlers for breakpoint (3), double fault (8, on IST 0), page fault (14), timer (32)
  and keyboard (33), the `PageFaultErrorCode` newtype, `read_cr2`, a `TICKS` counter,
  and `enable`/`disable`/`are_enabled`/`without_interrupts`.
- `src/memory.rs`: the hand-written page-table walker — `Entry`, `Table`
  (`repr(C, align(4096))`), `translate()` (handles 2 MiB/1 GiB huge pages), a single
  global `FrameAllocator` behind `frame_allocator() -> MutexGuard<..>`, and
  `create_mapping(virt, frame, flags, allocator) -> Result<(), MapError>` with public
  `WRITABLE` and `NO_EXECUTE` flags. All reached through the bootloader's
  physical-memory-offset mapping. No `x86_64` crate.
- `src/allocator.rs`: `HEAP_START = 0x4444_4444_0000`, `HEAP_SIZE = 1 MiB`,
  `init_heap()`, `Locked<A>`, `align_up`, and the `#[global_allocator]`
  (`Locked<FixedSizeBlockAllocator>`), locked with interrupts off. Under
  `src/allocator/`: `bump.rs` (`BumpAllocator`, unchanged from Step 14),
  `linked_list.rs` (`LinkedListAllocator`, first fit, padding returned to the list, no
  coalescing) and `fixed_size_block.rs` (`FixedSizeBlockAllocator`, classes 8-2048
  bytes, the linked list as fallback). Each has its own `GlobalAlloc` impl.
- `src/pic.rs`: the two 8259s remapped to 32-47, all lines masked except timer,
  keyboard and the cascade, plus `end_of_interrupt` and `io_wait`.
- `src/keyboard.rs`: scancode set 1 decoded through two 0x40-entry `char` tables, with
  shift, caps lock and 0xE0-prefixed keys handled; a `Mutex<Keyboard>` global.
- `src/vga_buffer.rs`: the `Writer`, plus backspace, hardware cursor updates through
  ports 0x3D4/0x3D5, and `_print` wrapped in `without_interrupts`. `src/serial.rs`
  likewise.
- `src/task.rs` and `src/task/executor.rs`: pinned futures, safe `Wake` implementation,
  a ready flag per task, one poll per ready task per pass, and atomic idle sleep.
  The production entry point still uses `hlt_loop` until keyboard integration.
- Step 16 verified boot and 28 tests in debug/release on Windows. Step 17 reran the
  baseline successfully through Windows Cargo from WSL, then passed the expanded
  21-case library suite in debug. Production async integration is still pending.
- Warnings we are knowingly carrying: cargo's kebab-case binary-name suggestion, the
  future-incompatibility report about nightly's own `core` sources, and older clippy
  lints in `idt.rs`, `keyboard.rs`, `pic.rs` and `port.rs` (missing `Default`,
  function-pointer casts, missing `# Safety` docs) that predate Step 14. Nothing new.
- Known hazards:
  - The linked-list allocator never merges neighbouring free regions, so the heap
    fragments; and blocks the fixed-size allocator carves from it never return to it.
    Memory once cut into small blocks serves only small requests from then on.
  - The heap is a fixed 1 MiB and cannot grow.
  - Only vectors 3, 8, 14, 32 and 33 are present. Any other exception still arrives as
    a double fault, which panics with a readable message but tells us nothing about
    the original cause.
  - The double-fault stack has no guard page. An overflow inside the handler itself
    would silently corrupt whatever sits below it.
  - `keyboard_handler` prints directly from the interrupt handler. It cannot deadlock
    (every `WRITER` user disables interrupts), but the right design is a queue the
    handler pushes to and normal code drains. That arrives with async/await — and now
    that a heap exists, the queue can be an allocated one.
  - Extended (0xE0) scancodes — arrows, right control, keypad — are decoded and thrown
    away.
  - `FrameAllocator` re-walks the whole memory map on every `allocate_frame()` call,
    and freed frames are never returned.
  - The frame-allocator lock must not be taken from an interrupt handler; this is
    documented, not enforced.

## Next step

**Async keyboard input and end-to-end verification.**

The executor exists and its unit tests pass. Replace printing inside the keyboard
interrupt with a bounded scancode queue and a single async consumer, switch the
production entry point to the executor, and verify interrupt wakeups and keyboard
input in QEMU in addition to the full debug/release suite.
