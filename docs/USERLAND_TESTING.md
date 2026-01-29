# Userland Testing Guide

This document explains how to test `no_std` userland libraries (scarlet_std and scarlet-ui) in Scarlet.

## Overview

Userland library tests are compiled as standalone ELF executables that run on the Scarlet kernel in QEMU. The test framework provides a similar experience to standard Rust testing but works in a `no_std` environment.

## Running Tests

### Quick Start

Run all tests (kernel + userland):
```bash
cargo make test-riscv64
```

Run only userland library tests:
```bash
# Test scarlet_std library
cargo make test-userland-std-riscv64

# Test scarlet-ui library (when implemented)
cargo make test-userland-ui-riscv64
```

### Using the Test Script Directly

For more control, use the test runner script:

```bash
# Test scarlet_std on RISC-V
./tools/test-userland.sh --arch riscv64 --lib std

# Test scarlet_std on AArch64
./tools/test-userland.sh --arch aarch64 --lib std

# Debug mode (starts QEMU with GDB server)
./tools/test-userland.sh --arch riscv64 --lib std --debug
```

## How It Works

1. **Build Phase**: The test binary is compiled using `cargo test --no-run` with the `test_harness` feature
2. **Initramfs Creation**: The test binary is copied as `/system/scarlet/bin/init` in a test initramfs
3. **QEMU Execution**: Kernel boots in QEMU, automatically runs the init binary (test runner)
4. **Result Collection**: Script monitors output for test results and reports pass/fail

## Test Flow

```
cargo test --no-run → Test Binary (ELF)
                          ↓
                    Copy to initramfs as 'init'
                          ↓
                    QEMU + Kernel Boot
                          ↓
                    Kernel runs init (test runner)
                          ↓
                    Test execution & output
                          ↓
                    Parse results & exit
```

## Writing Tests

Tests are written in `tests/runner.rs` for each library. Example:

```rust
fn test_vec_push() {
    use std::vec::Vec;
    let mut v = Vec::new();
    v.push(1);
    v.push(2);
    v.push(3);
    
    assert_eq!(v.len(), 3);
    assert_eq!(v[0], 1);
}
```

Add test functions to the `main()` function in `tests/runner.rs` to include them in the test suite.

## Architecture

### Test Target JSON

Separate target JSON files are used for tests:
- `riscv64gc-unknown-scarlet-elf-test.json`
- `aarch64-unknown-scarlet-elf-test.json`

These targets omit the linker script from `pre-link-args` to avoid conflicts. The linker script is provided via `.cargo/config.toml` rustflags instead.

### Symbol Conflicts

To avoid symbol conflicts between the library and test binary:
- `test_harness` feature flag controls `_entry` and `_start` definitions
- Library's `_entry` is conditionally compiled: `#[cfg(not(feature = "test_harness"))]`
- Test binary provides its own `_entry` and panic handler

### Test Module

Each library has a `test` module that provides:
- `TestableFn` trait for test functions
- `test_runner` function that executes tests and reports results
- Integration with library-specific output (println, task::exit)

## Limitations

- Tests must be manually added to `tests/runner.rs` (no automatic test discovery)
- Requires QEMU and kernel to be built
- Test execution is slower than native tests due to QEMU overhead
- Limited to unit tests that don't require complex kernel interactions

## Troubleshooting

### Test binary not found
Make sure the test compiles successfully:
```bash
cd user/lib/std
cargo test --no-run --features test_harness --target ../../targets/riscv64gc-unknown-scarlet-elf-test.json
```

### QEMU doesn't start
Ensure kernel is built:
```bash
cargo make build-kernel-debug-riscv64
```

### Tests hang
Use debug mode to attach GDB and investigate:
```bash
./tools/test-userland.sh --arch riscv64 --lib std --debug
# In another terminal:
gdb kernel/target/riscv64gc-unknown-none-elf/debug/kernel -ex 'target remote :12345'
```

## Future Improvements

- Automatic test discovery using `#[test_case]` attribute
- Parallel test execution
- Test filtering and selection
- Coverage reporting
- Integration with CI/CD pipelines
