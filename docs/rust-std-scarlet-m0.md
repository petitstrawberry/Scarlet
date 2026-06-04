# Rust std on Scarlet Native: M0 Extraction Notes

Issue #449 tracks eventual Rust `std` support for Scarlet Native targets. This
note records the first extraction boundary from the existing `scarlet_std`
crate.

This is a historical M0 extraction note. The current cross-milestone execution
checklist lives in [`rust-std-scarlet-issue-449-todo.md`](rust-std-scarlet-issue-449-todo.md).
Some items that were deferred during M0, notably `scarlet-rt`, have since been
implemented as later milestone follow-up work.

## Current Split

`scarlet-abi`:

- `no_std`, dependency-free ABI definitions.
- Owns Scarlet Native syscall numbers.
- Owns raw ABI-facing primitive aliases such as `RawHandle`, `Pid`, and `Tid`.
- Does not contain syscall assembly, runtime entry code, allocator glue, or safe
  object wrappers.

`scarlet-sys`:

- `no_std` unsafe syscall binding layer.
- Depends on `scarlet-abi`.
- Owns architecture-specific syscall instruction wrappers for RISC-V and
  AArch64.
- Does not contain `_entry`, `_start`, argc/argv/env setup, allocator setup, or
  higher-level handle/socket/file abstractions.

`scarlet-os`:

- `no_std` safe-ish low-level wrapper entry point.
- Re-exports the Scarlet-specific OS wrappers that still live in `scarlet-std`.
- Contains `Handle`, capability views, IPC, shared memory, sockets, task/thread
  control, pty/tty, polling, and hypervisor APIs.
- Does not expose the std-like filesystem, I/O, environment, or collection
  facade APIs as its primary surface.

`scarlet-std`:

- Remains the existing compatibility facade for current userland programs.
- Re-exports `scarlet_abi::Syscall` and `scarlet_sys::syscallN` through
  `scarlet_std::syscall` to preserve existing Rust crate imports.
- Still owns runtime entry, allocator glue, and safe-ish wrappers for now.

## M0 Inventory

ABI definitions:

- Syscall numbers and raw IDs live in `scarlet-abi`.
- `RawHandle`, `Pid`, and `Tid` are the first shared raw aliases.
- Kernel-side adoption is deferred to a separate patch because the kernel's
  syscall table also wires handlers and feature gates.

Unsafe syscall binding:

- RISC-V `ecall` and AArch64 `svc #0` wrappers live in `scarlet-sys`.
- `scarlet-std::syscall` is now a compatibility re-export.

Runtime glue:

- `_entry`, `_start`, argc/argv/env initialization, allocator setup, and panic
  handling remain in `scarlet-std`.
- A separate `scarlet-rt` crate is not created in M0. Moving entry symbols out of
  `scarlet-std` changes link semantics for every user binary and should be done
  with focused runtime tests.

Safe low-level wrappers:

- `scarlet-os` is the new public landing point.
- Implementations are still backed by `scarlet-std` during M0 to avoid a broad
  source migration.
- Future patches can move modules into `scarlet-os` one at a time while
  preserving `scarlet-std` re-exports.

Compatibility facade:

- Existing applications keep importing `scarlet_std`.
- Cargo package names use hyphenated names (`scarlet-std`, `scarlet-os`,
  `scarlet-abi`, `scarlet-sys`, `sws-protocol`), while Rust crate names keep
  underscores where required by Rust import syntax.

## API Classification

Keep as Scarlet-specific APIs, exposed through `scarlet-os`:

- `Handle`, `RawHandle`, and handle capability views.
- `Socket` and native handle-transfer operations.
- `SharedMemory` and memory mapping capability APIs.
- Event delivery, event masking, and process-control event helpers.
- `task`, `thread`, `pty`, `tty`, `poll`, and hypervisor control APIs.
- SWS, IME, UI, and other Scarlet protocol crates remain separate consumers.

Prefer future Rust `std` PAL / `std` API coverage:

- `fs` path operations, `File`, directory iteration, metadata, cwd, and rename.
- `io` `Read`/`Write` style stream wrappers and stdio.
- `env` args, variables, and current directory.
- `thread::spawn`, join, sleep, yield, and TLS/runtime integration.
- `socket` pieces that map cleanly to `std::net` after the native socket API is
  stabilized.

Remain transitional in `scarlet-std`:

- `collections`, core/alloc facade exports, formatting macros, allocator glue,
  panic handler, and user binary entry.
- Std-like APIs stay available for existing programs until real Rust `std` can
  replace them.

## Follow-up Boundaries

Likely `scarlet-rt` content:

- `_entry` / `_start`.
- argc/argv/env initialization handoff.
- TLS setup glue.
- allocator and panic/abort integration points.

Kernel sharing is intentionally deferred. The kernel still has its syscall table
definition in `kernel/src/syscall/mod.rs`; moving both sides to `scarlet-abi`
should be a separate patch so the kernel build graph and feature gates can be
checked independently.

## M0 Completion Status

- Current `scarlet-std` responsibilities inventoried.
- ABI definitions, syscall binding, runtime glue, safe wrappers, and facade
  responsibilities classified.
- `scarlet-abi` created.
- `scarlet-sys` created.
- `scarlet-rt` explicitly deferred.
- `scarlet-os` created as the safe low-level wrapper landing point.
- Existing `scarlet-std` syscall layer moved onto `scarlet-abi` /
  `scarlet-sys`.
- Kernel/user ABI sharing boundary documented and deferred.
- Std-replacement APIs and Scarlet-specific APIs separated above.
