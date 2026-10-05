# BaddCafe OS

A hobby x86-64 kernel written from scratch in bare-metal Rust, following the
[Writing an OS in Rust](https://os.phil-opp.com) track. No Linux, no libc, no standard
library: the bootloader jumps into our code and everything after that is ours.

The goal is learning, not shipping. [`NOTES.md`](NOTES.md) is the running journal: one
numbered entry per step, with the real error output that each step ran into.

## Status

Done: freestanding binary, custom target, bootable image, VGA text output, a test
framework that exits QEMU by itself, CPU exceptions, double faults on their own stack,
hardware interrupts (timer, keyboard), page fault handling, and paging (a hand-written
page table walker and frame allocator).

Next: heap allocation.

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
```

## Layout

| Path | What it is |
|---|---|
| `src/main.rs` | The kernel entry point and banner |
| `src/lib.rs` | Shared init, the test runner and the library's own tests |
| `src/memory.rs` | Page table walker, frame allocator, mapping creation |
| `src/idt.rs`, `src/gdt.rs`, `src/pic.rs` | Exceptions, segmentation, interrupt controller |
| `src/vga_buffer.rs`, `src/serial.rs`, `src/keyboard.rs` | Screen, serial port, keyboard |
| `tests/` | Integration tests, each a separate kernel |
| `x86_64-baddcafe_os.json` | Our custom bare-metal target specification |
| `.cargo/config.toml` | Build settings: target, `build-std`, the runner |

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
