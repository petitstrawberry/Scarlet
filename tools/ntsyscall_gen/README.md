# ntsyscall_gen

`ntsyscall_gen` is a standalone Rust CLI tool that parses an ARM64 Windows `ntdll.dll` and generates Scarlet's NT syscall mapping source file.

Output target:

- `kernel/src/abi/windows/syscall_table.rs`

It also emits a verification-friendly reference file next to it:

- `kernel/src/abi/windows/syscall_table.reference.md`

## Input requirements

- ARM64 `ntdll.dll` only (`Machine = 0xAA64`)
- PE32+ image with an export directory

## How to obtain `ntdll.dll` (ARM64)

Use an ARM64 Windows installation source or running system. Typical path on Windows ARM64:

- `C:\Windows\System32\ntdll.dll`

If using WSL or a mounted image, provide the mounted file path to the tool.

## Usage

From the repository root:

```bash
cargo run --release --manifest-path tools/ntsyscall_gen/Cargo.toml -- /path/to/ntdll.dll
```

Example:

```bash
cargo run --release --manifest-path tools/ntsyscall_gen/Cargo.toml -- /mnt/c/Windows/System32/ntdll.dll
```

## Regeneration

Whenever the NT syscall table needs refresh (e.g. new Windows build), rerun the command above with the target `ntdll.dll`.

The generated Rust file includes a regeneration hint in its header.
