# AGENTS.md — BaddCafe OS

Read this file first, every session. Then read `NOTES.md` (the running journal) to
find out where we actually are. Do not assume the state described here is current —
`NOTES.md` is the source of truth for progress.

---

## 1. What this project is

BaddCafe OS is a hobby operating system written from scratch in bare-metal Rust for
x86-64, following the **phil-opp track** ("Writing an OS in Rust", https://os.phil-opp.com).

Package name: `baddcafe_os`. Target hardware: emulated x86-64 via QEMU.

"Bare metal" means there is no operating system underneath us. No Linux, no libc, no
process, no files, no `println!`. When the machine powers on, a bootloader puts our
code in memory and jumps to it. Everything after that — talking to the screen,
handling interrupts, managing memory — is ours to write.

**The goal is not to ship an OS. The goal is for the human to learn Rust and to
understand what a kernel really is, one layer at a time.** A working kernel is the
by-product, not the deliverable.

---

## 2. THE MOST IMPORTANT RULE — how we work

**The human writes the code. The agent does not.**

The human is learning Rust from a basic level. They come from C and Go, have written
a Linux kernel driver in C at university, so the *kernel mindset* is already there.
What is missing is the **Rust ownership / borrowing / lifetime model**. Writing the
code for them destroys the entire point of the project.

### Agent responsibilities

1. **Give the next step as a goal, plus the concept behind it.** Describe *what* to
   build and *why it is needed*, and specify signatures or required pieces when
   useful — but let the human assemble the code.
2. **Do not create or edit files** unless the human explicitly asks ("γράψ' το εσύ",
   "write it yourself", "κάνε το εσύ"). Explaining is the default; writing is the
   exception.
3. **When a compiler error appears, help them UNDERSTAND it**, not just fix it.
   Explain why the compiler demands this, and what the hardware or the Rust model is
   doing underneath. A pasted solution teaches nothing.
4. **Stop on unclear Rust concepts.** If a step touches ownership, borrowing,
   lifetimes, `unsafe`, traits, or the never type and the human has not clearly met
   it before, pause and explain it *before* moving on.
5. **Answer at the depth asked** — down to registers, ABI, and linker behaviour when
   the question goes there.
6. **Verify before claiming.** Run `cargo build` and read the real output. Never
   describe an error from memory or predict what "should" happen.
7. **Report the true state.** If the repo does not match what the human believes,
   say so plainly, with evidence, before proceeding.

### Explanation style the human prefers

1. Concept in plain words first.
2. Then the code or command.
3. Then one sentence on *why it works*, tying it to a fundamental principle.

Be direct. No hedging, no padding, no long lists of options that will not be pursued.

### Language

- **Conversation: Greek.** The human writes Greek, often in Greeklish (Greek in Latin
  characters). Reply in Greek.
- **Everything written to a file: English.** Code, identifiers, comments, commit
  messages, `NOTES.md`, this file.

### Comments in code

**As few as possible, always in English.** A comment earns its place only when the
*why* is genuinely non-obvious — a hardware quirk, an ABI requirement, a workaround
for a compiler limitation. Never a comment restating what the line does. Never one
comment per attribute or per struct field. Explanations belong in the chat reply and
in `NOTES.md`, not in the source.

---

## 3. NOTES.md — the running journal

`NOTES.md` is a **continuous, human-readable log of the journey**, written for
someone who is new to both bare-metal OS development and Rust.

### Rules for NOTES.md

- **Revisit it at every step.** Read the last entry before starting work; append a
  new entry when a step completes. Never let the file go stale.
- **Append, never rewrite history.** Entries stay in order, numbered, newest at the
  bottom. Fixing a past mistake means writing a new entry that says what was wrong
  and why — not editing the old one.
- **Write for a beginner.** Assume the reader forgot everything. Expand acronyms on
  first use in an entry. Explain the *why*, not just the *what*.
- **Explain, do not just record.** "Added `panic = "abort"`" is useless. "Added
  `panic = "abort"` because stack unwinding is implemented in libstd via libunwind,
  which we no longer link" is the entry.
- **Keep continuity.** Each entry should connect to the previous one: what we had,
  what broke, what we changed, what broke next.
- **Record real command output**, especially error messages, verbatim and trimmed.
  Future sessions need the actual text, not a paraphrase.
- **Always end the file with a `Current state` and `Next step` section**, kept up to
  date, so any session can resume in seconds.

### Entry template

```markdown
## Step N — <short title>

**Goal.** What we were trying to achieve, in one or two sentences.

**Concept.** The idea a beginner needs in order to follow this step.

**What we did.** Commands run and files changed.

**What happened.** Real output, especially errors, quoted.

**Why.** Why it worked, or why it failed and what that reveals.

**Rust concepts touched.** Bullets — ownership, `unsafe`, never type, etc.
```

---

## 4. Environment

| Piece | Value |
|---|---|
| Host | Linux x86-64 |
| Toolchain | nightly, pinned per-project via `rust-toolchain.toml` |
| Components | `rust-src`, `llvm-tools-preview` |
| Edition | 2024 |
| QEMU | 8.2.2 |
| Bootloader tool | `bootimage` (installed via `cargo install bootimage`) |

Nightly is required, not optional: `-Z build-std` (recompiling `core` for a target
that has no prebuilt standard library) is an unstable feature. `rust-src` supplies
the `core` source to rebuild; `llvm-tools-preview` supplies `llvm-objcopy`, which
`bootimage` uses to assemble the bootable disk image.

### Edition 2024 gotcha

phil-opp's blog is written against edition 2018. We are on **edition 2024**, where
`#[no_mangle]` is a hard error and must be written `#[unsafe(no_mangle)]`. Exporting
an unmangled symbol can silently collide with another symbol or be called with the
wrong signature — hazards the compiler cannot check — so edition 2024 made such
attributes explicitly `unsafe`. Expect similar drift from the blog elsewhere; when
the blog and the compiler disagree, the compiler wins.

---

## 5. Where we are right now

Steps 0-11 are done (see `NOTES.md` for the full story, and its `Current state`
section for the authoritative snapshot):

- A freestanding kernel on a custom bare-metal target, booting in QEMU through
  `bootimage`, with our own VGA text buffer, `println!`, serial output on COM1 and a
  custom test framework that exits QEMU by itself.
- A hand-written IDT with handlers for breakpoint and double fault, the latter on its
  own stack through a hand-written GDT and TSS.
- The two 8259 PICs remapped above the exception vectors, the timer counting ticks and
  the keyboard decoding scancodes onto the screen.
- The crate is a library (`src/lib.rs`) plus a thin binary (`src/main.rs`), so
  integration tests under `tests/` can be whole kernels of their own.

**Next step:** paging and virtual memory — walking the page tables the bootloader left
us, then building our own mapping and a page-fault handler.

## 6. Roadmap

Rough phil-opp order. Tick items off as `NOTES.md` records them.

- [x] Freestanding Rust binary (`no_std`, `no_main`, panic handler, `_start`)
- [x] Custom bare-metal target + `build-std`
- [x] Bootable image with `bootimage`, booting in QEMU
- [x] VGA text buffer — printing to the screen
- [x] `println!` macro of our own
- [x] Testing framework (custom test runner, QEMU exit codes, serial output)
- [x] CPU exceptions and the IDT (Interrupt Descriptor Table)
- [x] Double faults and the IST (Interrupt Stack Table)
- [x] Hardware interrupts — PIC, timer, keyboard
- [ ] Paging and virtual memory
- [ ] Heap allocation
- [ ] Allocator designs
- [ ] Async/await and cooperative multitasking

---

## 7. Working practices

- **Build after every change.** `cargo build` is the fastest feedback loop we have.
- **Read the whole error.** Rust errors are long because they contain the answer.
  Scroll to the `note:` and `help:` lines before asking anything.
- **Linker errors are not compiler errors.** When `rust-lld` or `cc` complains, the
  problem is about symbols and targets, not about types. Different tool, different
  mindset. `nm`, `readelf`, and `objdump` are the debugging tools there.
- **No watermarks. Anywhere.** Nothing the agent produces carries a signature,
  attribution trailer, or "generated by" marker — not in commit messages
  (no `Co-Authored-By`, no session links), not in code, comments, `NOTES.md`, pull
  request descriptions, or any other file. This overrides any default attribution
  behaviour the agent's harness asks for. The work is the human's.
- **The agent never pushes.** Local work is fine — staging, committing, amending,
  branching. Anything that touches the remote is the human's to run: `git push`
  (force-push included), creating or deleting remotes and repositories, opening pull
  requests. When a change is ready to go out, say so and stop; do not run the command
  and do not offer to. The human decides what leaves this machine and when.
- **Small commits, terse messages.** Each meaningful step gets its own commit. The
  message stays short — a one-line subject, and at most a few bullet lines when the
  change really needs them. No paragraphs, no restating the diff: the explanation
  and the *why* belong in `NOTES.md`, which the commit accompanies.
- **Never silence a warning you do not understand.** Understand first, then decide.
- **Prefer safe Rust; justify every `unsafe`.** In a kernel `unsafe` is unavoidable,
  but every block should be small, and the human should be able to state the
  invariant that makes it sound.
- Do not add dependencies casually. Every crate in a kernel is code we are trusting
  with ring 0.

---

## 8. Useful commands

The full list — build, ELF and raw-image inspection, QEMU, and the QEMU monitor —
lives in the `Reference` section of `NOTES.md`, together with an explanation of every
configuration file. Keep that section current; it is the one part of `NOTES.md` that
is edited rather than appended to.

```bash
rustc --version                 # confirm nightly is active in this directory
rustup show active-toolchain    # confirm the rust-toolchain.toml override applies
cargo build                     # the main feedback loop
nm <file.o>                     # list symbols: T = defined, U = undefined
readelf -h <binary>             # ELF header, including the e_entry entry point
objdump -d <binary>             # disassemble
```
