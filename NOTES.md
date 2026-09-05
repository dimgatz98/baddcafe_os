# BaddCafe OS — Journal

A continuous log of building an x86-64 kernel from nothing, written for someone new
to both bare-metal OS work and Rust. Read the last entry to know where we are; read
from the top to understand how we got here.

Rules for this file live in `AGENTS.md` section 3. Short version: append only, newest
at the bottom, explain the *why*, quote real output, and keep the two sections at the
end current.

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

## Current state

- Branch `master`, **no commits yet**. Untracked: `.gitignore`, `.cargo/`,
  `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `x86_64-baddcafe_os.json`,
  `src/`, `AGENTS.md`, `NOTES.md`.
- Toolchain: nightly `1.100.0-nightly (a69a63265 2026-09-03)`, pinned by
  `rust-toolchain.toml`, with `rust-src` and `llvm-tools-preview` installed.
- Edition 2024. `panic = "abort"` on both profiles.
- `src/main.rs` is unchanged since Step 2: `no_std` / `no_main`, our `_start`, our
  `#[panic_handler]`.
- **`cargo build` succeeds**, producing
  `target/x86_64-baddcafe_os/debug/baddcafe_os` — a static ELF with entry point
  `0x201120` (our `_start`) and zero undefined symbols.
- Two warnings we are knowingly carrying: cargo's kebab-case binary-name suggestion,
  and a future-incompatibility report about nightly's own `core` sources.
- QEMU 8.2.2 and `bootimage` are installed but not used yet.

## Next step

Turn this ELF into something a machine can actually **boot**, and run it in QEMU.

**The gap.** We have a kernel binary, but nothing puts it in memory. An x86-64 PC
powers on in **real mode** (16-bit, 1 MiB of addressable memory, no paging) and the
firmware loads 512 bytes from the disk — the **boot sector** — at address `0x7c00`.
Getting from there to our 64-bit `_start` means switching the CPU through real mode →
protected mode (32-bit) → long mode (64-bit), setting up a **GDT** (Global Descriptor
Table) and initial page tables, and loading the kernel off the disk. That is a
project of its own, so we borrow it: the `bootloader` crate does exactly that, and
`bootimage` glues its output together with our kernel into one bootable disk image.

1. Add the `bootloader` crate as a dependency (phil-opp's track uses the `0.9.x`
   series with `bootimage`; newer `0.11` has a different, no-`bootimage` workflow —
   pick one and stay on it).
2. `cargo bootimage` to produce `target/x86_64-baddcafe_os/debug/bootimage-baddcafe_os.bin`.
   `bootimage` compiles the bootloader, then uses `llvm-objcopy` (from
   `llvm-tools-preview`) to concatenate bootloader and kernel into a disk image —
   which is why that component was installed in Step 1.
3. Boot it: `qemu-system-x86_64 -drive format=raw,file=<that .bin>`.
4. Optionally add a `[target.'cfg(target_os = "none")'] runner = "bootimage runner"`
   entry to `.cargo/config.toml` so `cargo run` boots QEMU directly.

Expect a black QEMU window: `_start` still only spins in `loop {}`. The proof of life
comes in the step after, when we write to the VGA text buffer at `0xb8000`.
