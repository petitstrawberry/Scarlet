# Rust std on Scarlet Native: M0 Extraction Notes

Issue #449 tracks eventual Rust `std` support for Scarlet Native targets. This
note records the first extraction boundary from the existing `scarlet_std`
crate.

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

`scarlet-std`:

- Remains the existing compatibility facade for current userland programs.
- Re-exports `scarlet_abi::Syscall` and `scarlet_sys::syscallN` through
  `scarlet_std::syscall` to preserve existing Rust crate imports.
- Still owns runtime entry, allocator glue, and safe-ish wrappers for now.

## Follow-up Boundaries

Likely `scarlet-rt` content:

- `_entry` / `_start`.
- argc/argv/env initialization handoff.
- TLS setup glue.
- allocator and panic/abort integration points.

Likely `scarlet-os` content:

- Owning `Handle` and capability views.
- `File`, `Socket`, `SharedMemory`, `Task`, `Thread`, and event wrappers.
- Scarlet-specific IPC/control-plane APIs that do not map cleanly to Rust
  `std`.

Kernel sharing is intentionally deferred. The kernel still has its syscall table
definition in `kernel/src/syscall/mod.rs`; moving both sides to `scarlet-abi`
should be a separate patch so the kernel build graph and feature gates can be
checked independently.
