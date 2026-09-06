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
   Compiling baddcafe_os v0.1.0 (/space/dgkatzio/baddcafe_os)
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
   Compiling bootloader v0.9.35 (/home/dgkatzio/.cargo/registry/src/.../bootloader-0.9.35)
    Finished `release` profile [optimized + debuginfo] target(s) in 41.06s
Created bootimage for `baddcafe_os` at
`/space/dgkatzio/baddcafe_os/target/x86_64-baddcafe_os/debug/bootimage-baddcafe_os.bin`
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

[dependencies]
bootloader = "0.9.35"

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

#### `.cargo/config.toml` — how cargo behaves in this directory

```toml
[build]
target = "x86_64-baddcafe_os.json"

[unstable]
build-std = ["core", "compiler_builtins"]
build-std-features = ["compiler-builtins-mem"]
json-target-spec = true

[target.'cfg(target_os = "none")']
runner = "bootimage runner"
```

- `build.target` — always cross-compile for our own target, so plain `cargo build` does
  the right thing without flags.
- `build-std` — for a target rustc has never heard of, no precompiled `core` exists, so
  cargo compiles `core` and `compiler_builtins` from source alongside our crate.
- `build-std-features = ["compiler-builtins-mem"]` — the compiler emits calls to
  `memcpy`, `memset` and `memcmp` for ordinary things like assigning a struct. Normally
  libc provides them. There is no libc here, so `compiler_builtins` must provide them.
- `json-target-spec = true` — custom `.json` targets are gated behind their own
  unstable flag on current nightly. Without it: *"`.json` target specs require
  -Zjson-target-spec"*.
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
```

Artifacts:

```
target/x86_64-baddcafe_os/debug/baddcafe_os                  # the kernel ELF
target/x86_64-baddcafe_os/debug/bootimage-baddcafe_os.bin    # the bootable disk image
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
(examine physical memory), `quit`.

This is currently our **only** way to see inside the kernel. Until we can write to the
screen, the kernel cannot tell us anything itself.

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

- Branch `master`. Steps 0-3 are committed as `75d458e` ("Freestanding no_std kernel on
  a custom bare-metal target"). Step 4 is not committed yet.
  (An earlier version of this section claimed the repo had zero commits; that was true
  when it was written and is no longer.)
- Toolchain: nightly `1.100.0-nightly (a69a63265 2026-09-03)`, pinned by
  `rust-toolchain.toml`, with `rust-src` and `llvm-tools-preview`.
- Edition 2024. `panic = "abort"` on both profiles.
- `src/main.rs` is still exactly what Step 2 produced: `no_std` / `no_main`, our
  `_start`, our `#[panic_handler]`. It has not changed since.
- `bootloader = "0.9.35"` in `[dependencies]`; `bootimage 0.10.5` installed;
  `runner = "bootimage runner"` in `.cargo/config.toml`.
- **`cargo run` boots the kernel in QEMU 8.2.2.** The window is black, and the CPU is
  verified to be sitting in our `loop {}` at `RIP=0x201122` in long mode with paging on.
- Warnings we are knowingly carrying: cargo's kebab-case binary-name suggestion, the
  future-incompatibility report about nightly's own `core` sources, and
  `unused dependency 'bootloader'` (false alarm - `bootimage` reads it).

## Next step

Write something to the screen: the **VGA text buffer**.

**The gap.** We can boot, but we are blind. Every observation so far has come from
outside the machine - `readelf`, `objdump`, the QEMU monitor. We need the kernel itself
to tell us something.

**Concept.** On a BIOS-booted PC in text mode, the screen is memory. Physical address
`0xb8000` is the start of a 25-row by 80-column grid, two bytes per character cell: the
first byte is the code point in **code page 437**, the second is the colour attribute
(low nibble foreground, high nibble background). Writing `0x0f48` to `0xb8000` puts a
white-on-black `H` in the top-left corner. There is no driver, no system call, no
device - just a store instruction to an address the graphics hardware happens to watch.

1. Write a byte pair to `0xb8000` from `_start` and see a character appear. This
   requires `unsafe`: dereferencing a raw pointer, because the compiler cannot know
   that this integer is a valid address.
2. Understand why the naive version can be optimised away, and what `Volatile` is for:
   the compiler sees a write to memory that is never read back and is entitled to
   delete it. `volatile` marks the write as having an effect the compiler cannot see.
3. Then build it into something civilised: a `Writer` struct that owns the buffer,
   tracks a column position, handles `\n`, scrolls when the screen fills, and
   implements `core::fmt::Write` so that formatting machinery works on it.

**Rust concepts about to appear.** Raw pointers and `unsafe`; `repr(transparent)` and
`repr(C)` for laying out structs the way hardware expects; enums with explicit
discriminants; `&mut` and why exactly one mutable reference to the buffer is allowed;
traits (`core::fmt::Write`) as the bridge to `write!`. Pause on any of these before
moving past them.
