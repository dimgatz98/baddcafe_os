# BaddCafe OS

A hobby x86-64 kernel written from scratch in bare-metal Rust, following the
[Writing an OS in Rust](https://os.phil-opp.com) track. No Linux, no libc, no standard
library: the bootloader jumps into our code and everything after that is ours.

The goal is learning, not shipping. [`NOTES.md`](NOTES.md) is the running journal: one
numbered entry per step, with the real error output that each step ran into.

## Status

Done: freestanding binary, custom target, bootable image, VGA text output, a test
framework that exits QEMU by itself, CPU exceptions, double faults on their own stack,
hardware interrupts (timer, keyboard), page fault handling, paging (a hand-written
page table walker and frame allocator), and a heap, so `Box`, `Vec` and `String`
work. The heap has three hand-written allocators: bump, linked list, and the one in
use, a fixed-size block allocator that falls back to the linked list.

Async/await and cooperative multitasking are implemented: a wake-driven executor
runs pinned tasks, and a bounded keyboard queue moves decoding and printing out of
the interrupt handler. The CPU halts when no task is ready. This completes the
current learning roadmap; Steps 17-18 in `NOTES.md` explain the async implementation.

Verified: 42 tests in debug and release, plus production keyboard input and idle
sleep in QEMU. Scheduling uses one active CPU and requires tasks to yield; there
are no userspace processes or filesystem yet.

## Running it

You need:

- [rustup](https://rustup.rs). The nightly toolchain and its components are pinned in
  [`rust-toolchain.toml`](rust-toolchain.toml) and installed automatically.
- The `bootimage` tool: `cargo install bootimage`
- QEMU for x86-64, with `qemu-system-x86_64` on your `PATH`.

Then:

```bash
cargo run     # build the kernel, make a disk image, boot it in QEMU
cargo test    # build the test kernels, boot each one, exit with a pass/fail code
cargo test --release
```

On Windows, add `$HOME/scoop/apps/qemu/current` to the shell's `PATH` if QEMU was
installed through Scoop. From WSL with a Windows-only Rust installation, use
`cargo.exe` with working Windows interoperability.

An optional end-to-end check needs Python 3 and QEMU on `PATH`:

```bash
cargo bootimage
python3 scripts/check_keyboard.py target/x86_64-baddcafe_os/debug/bootimage-baddcafe_os.bin
```

It sends keyboard events, reads the VGA text buffer and checks idle sleep with the
timer interrupt masked. On Windows use `python` if that is your Python command.
For release, build with `cargo bootimage --release` and use the `release` image path.

## Layout

| Path | What it is |
|---|---|
| `src/main.rs` | Kernel entry point, banner and async task startup |
| `src/lib.rs` | Shared init, the test runner and the library's own tests |
| `src/memory.rs` | Page table walker, frame allocator, mapping creation |
| `src/allocator.rs`, `src/allocator/` | The kernel heap and its allocators |
| `src/idt.rs`, `src/gdt.rs`, `src/pic.rs` | Exceptions, segmentation, interrupt controller |
| `src/vga_buffer.rs`, `src/serial.rs`, `src/keyboard.rs` | Screen, serial port, keyboard |
| `src/task.rs`, `src/task/` | Pinned tasks, executor and async keyboard input |
| `tests/` | Integration tests, each a separate kernel |
| `scripts/check_keyboard.py` | Production keyboard and idle-sleep check through QEMU |
| `x86_64-baddcafe_os.json` | Our custom bare-metal target specification |
| `.cargo/config.toml` | Build settings: target, `build-std`, the runner |

## Acknowledgements

This kernel follows Philipp Oppermann's
[Writing an OS in Rust](https://os.phil-opp.com) series step by step. The order of
the steps, the overall design and several algorithms, the heap allocators among
them, come from that series and its code
([phil-opp/blog_os](https://github.com/phil-opp/blog_os), MIT / Apache-2.0).
Where this repo departs from it, for example its own page-table code instead of the
`x86_64` crate, `NOTES.md` says so.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
