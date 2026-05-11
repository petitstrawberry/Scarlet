# Darwin Test Binaries

Scarlet uses real macOS binaries for Darwin ABI testing. These are NOT included in the repository for copyright reasons.

## Directory Structure

```
kernel/src/abi/darwin/aarch64/test_binaries/
├── README.md           # This file
├── .gitignore          # Ignore actual binaries
├── static/             # Statically linked binaries (Phase 1-3)
└── dynamic/            # Dynamically linked binaries (Phase 4+)
```

## How to Obtain Test Binaries

### From a macOS System (Recommended)

If you have access to a macOS machine (physical or VM):

```bash
# Copy basic utilities
scp user@mac-host:/bin/echo ./dynamic/
scp user@mac-host:/bin/cat ./dynamic/
scp user@mac-host:/bin/ls ./dynamic/
scp user@mac-host:/usr/bin/uname ./dynamic/

# Copy dyld (dynamic linker) - needed for Phase 4
scp user@mac-host:/usr/lib/dyld ./dynamic/

# Copy libSystem
scp user@mac-host:/usr/lib/libSystem.dylib ./dynamic/
```

### From macOS VM

You can create a macOS VM using:
- **UTM** (free, Apple Silicon Mac)
- **VMware Fusion** (paid)
- **VirtualBox** (free, limited macOS support)

Install macOS, then copy binaries from the VM.

### From macOS Recovery / Installer

Download macOS installer and extract binaries from the installer package.

## Current Limitations

**IMPORTANT**: As of now (Phase 1-3), Scarlet only supports statically linked binaries.

Real macOS binaries are **dynamically linked** and require:
- `dyld` (dynamic linker) loaded into memory
- `libSystem.dylib` and other libraries resolved
- Proper Mach-O dynamic linking support

This is planned for **Phase 4**. Until then, you can:
1. Use the binaries to test Mach-O parsing (header, load commands, segments)
2. Verify our syscall numbers against real binaries
3. Prepare for full dynamic linking support

## Testing with Real Binaries

### Test 1: Mach-O Header Parsing

Verify Scarlet can parse the Mach-O header correctly:

```rust
#[test]
fn test_parse_real_binary() {
    let binary_data = std::fs::read("test_binaries/dynamic/echo").unwrap();
    // Parse header, verify magic, cpu type, etc.
}
```

### Test 2: Load Command Verification

Check that load commands are parsed correctly:

```rust
#[test]
fn test_load_commands_real_binary() {
    // Verify LC_SEGMENT_64, LC_LOAD_DYLINKER, LC_LOAD_DYLIB, etc.
}
```

### Test 3: Static Binary Execution (Phase 1)

For a truly static binary (rare on macOS, but can be built with special flags):

```bash
# On macOS, build a static binary
clang -static -o hello_static hello.c
# Note: This may still link libSystem
```

## Cross-Checking Syscall Numbers

Real binaries are useful for verifying our syscall table:

```bash
# Disassemble a binary to see actual syscall numbers
otool -tv /bin/echo | grep -A2 svc

# Or use our darwin_syscall_gen tool with libSystem_kernel.dylib
cargo run --manifest-path tools/darwin_syscall_gen/Cargo.toml -- \
  --syscalls-master /path/to/xnu/bsd/kern/syscalls.master \
  --mach-traps /path/to/xnu/osfmk/kern/syscall_sw.c \
  --dylib /path/to/libSystem_kernel.dylib
```

## Legal Note

macOS binaries are copyrighted by Apple Inc. Do NOT redistribute them.
This directory is gitignored to prevent accidental commits.

For CI/testing, consider:
1. Hand-assembled minimal binaries (see macho_loader.rs tests)
2. Building your own test programs on macOS
3. Using XNU open-source components where possible

## Future Work

- Phase 4: Load and execute real dynamically-linked binaries
- Phase 5: Run complex programs like `curl`, `python`, etc.
