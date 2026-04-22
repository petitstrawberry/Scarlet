# darwin_syscall_gen

`darwin_syscall_gen` is a standalone Rust CLI tool that parses XNU source files (`syscalls.master` and `syscall_sw.c`) and generates Scarlet's Darwin syscall mapping source file.

## Output target

- `kernel/src/abi/darwin/aarch64/syscall_table.rs`

## Input requirements

- XNU source tree with:
  - `bsd/kern/syscalls.master` (BSD syscall definitions)
  - `osfmk/kern/syscall_sw.c` (Mach trap table)

## How to obtain XNU sources

Clone Apple's open-source XNU repository:

```bash
git clone https://github.com/apple-oss-distributions/xnu.git
```

Or download a specific release tarball from [Apple Open Source](https://opensource.apple.com/source/xnu/).

## Usage

From the repository root:

```bash
cargo run --release --manifest-path tools/darwin_syscall_gen/Cargo.toml -- \
  --syscalls-master /path/to/xnu/bsd/kern/syscalls.master \
  --mach-traps /path/to/xnu/osfmk/kern/syscall_sw.c
```

Example:

```bash
cargo run --release --manifest-path tools/darwin_syscall_gen/Cargo.toml -- \
  --syscalls-master ~/xnu/bsd/kern/syscalls.master \
  --mach-traps ~/xnu/osfmk/kern/syscall_sw.c \
  --version "15.0.0"
```

## Optional: Verify against libSystem_kernel.dylib

If you have access to a macOS system's `libSystem_kernel.dylib`, you can verify the generated syscall numbers match the actual binary:

```bash
cargo run --release --manifest-path tools/darwin_syscall_gen/Cargo.toml -- \
  --syscalls-master ~/xnu/bsd/kern/syscalls.master \
  --mach-traps ~/xnu/osfmk/kern/syscall_sw.c \
  --dylib /path/to/libSystem_kernel.dylib
```

## Regeneration

Whenever the Darwin syscall table needs refresh (e.g. new macOS version), rerun the command above with the target XNU sources.

The generated Rust file includes a regeneration hint in its header.

## Architecture

This tool follows the same pattern as `tools/ntsyscall_gen/` for Windows ABI:
- Primary input: authoritative source files (XNU sources)
- Output: checked-in generated Rust file
- Manual regeneration when needed
