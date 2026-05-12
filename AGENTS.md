# Scarlet Development Guide

## Build, Lint, and Test Commands

### Building

```bash
# Build all components (RISC-V, default)
cargo make build-riscv64

# Build all components (AArch64)
cargo make build-aarch64

# Individual components (RISC-V)
cargo make build-kernel-debug-riscv64      # Kernel only
cargo make build-userlib-debug-riscv64     # User space library
cargo make build-userbin-debug-riscv64     # User programs
cargo make build-initramfs-debug-riscv64   # Initial RAM filesystem

# Release builds
cargo make build-release-riscv64
cargo make build-release-aarch64

# Clean all build artifacts
cargo make clean
```

### Running and Debugging

```bash
# Run kernel in release mode
cargo make run-riscv64
cargo make run-aarch64

# Run kernel in debug mode
cargo make run-debug-riscv64
cargo make run-debug-aarch64

# Debug with GDB (RISC-V)
cargo make debug-riscv64
# Then in another terminal: gdb and connect to :1234
```

### Formatting

```bash
# Format all code
cargo make fmt

# Check formatting without changes (CI requirement)
cargo make fmt-check
```

### Linting

```bash
# Run clippy for all components (treats warnings as errors)
cargo make clippy-riscv64
cargo make clippy-aarch64

# Individual components
cargo make clippy-kernel
cargo make clippy-userlib
cargo make clippy-userbin
```

### Testing

```bash
# Run all kernel tests (RISC-V)
cargo make test-riscv64

# Run all kernel tests (AArch64)
cargo make test-aarch64

# Run specific test using cargo test filter
cd kernel && cargo test --target targets/riscv64gc-unknown-none-elf.json test_name

# Run user library tests
cd user/lib && cargo make test

# UI library tests
cd user/lib/scarlet-ui && cargo test

# Debug tests with GDB
cargo make debug-test-riscv64
```

## Code Style Guidelines

### Project Structure

- **Kernel**: `kernel/` - No std, bare-metal RISC-V/AArch64 kernel
- **User Library**: `user/lib/std/` - Scarlet standard library (no_std)
- **User Programs**: `user/bin/` - User space binaries
- **Build System**: Uses `cargo-make` (Makefile.toml)
- **Architectures**: RISC-V 64-bit (primary), AArch64 (experimental)
- **Hypervisor**: `kernel/src/hypervisor/` - SHV kernel subsystem; `user/bin/src/ushv/` - U-SHV userspace VMM

### Rust Edition

- Edition 2024 for all packages
- Nightly Rust toolchain (see `rust-toolchain`)

### Imports

- Group imports: `use alloc::...`, `use core::...`, `use crate::...`
- External crates first, then local modules
- Use `use crate::{...}` for multiple imports from same crate
- Avoid `*` imports; be explicit

Example:
```rust
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::any::Any;
use spin::{Mutex, Once};

use crate::device::char::CharDevice;
use crate::device::{Device, DeviceType};
use crate::object::capability::{ControlOps, MemoryMappingOps, Selectable};
```

### Formatting

- Standard `rustfmt` formatting (no custom config)
- 4-space indentation
- Run `cargo make fmt` before committing
- CI enforces `cargo make fmt-check`

### Types

- Prefer explicit types in public APIs
- Use `Result<T, E>` for fallible operations
- Kernel error type: `Result<T, &'static str>`
- Use `Option<T>` for optional values
- Lifetime annotations should be explicit when necessary

### Naming Conventions

- **Types**: `PascalCase` (e.g., `RandomManager`, `VirtQueue`)
- **Functions**: `snake_case` (e.g., `get_random_bytes`, `alloc_desc`)
- **Constants**: `SCREAMING_SNAKE_CASE` (e.g., `RANDOM_POOL_SIZE`)
- **Private fields**: `snake_case` (e.g., `sources`, `pool`)
- **Modules**: `snake_case` (e.g., `random`, `ipc`)
- **Acronyms in types**: Keep capitalized (e.g., `IPC`, `VFS`, `ABI`)

### Documentation

- Module-level docs with `//!` at top of file
- Public functions/structs use `///` doc comments
- Use Rustdoc format with sections:
  - Brief description
  - `# Arguments` for parameters
  - `# Returns` for return values
  - `# Safety` for unsafe functions
  - Example code blocks with ` ```rust ```

Example:
```rust
/// Get random bytes from the pool
///
/// # Arguments
///
/// * `buffer` - Buffer to fill with random data
///
/// # Returns
///
/// Number of bytes actually written
pub fn get_random_bytes(buffer: &mut [u8]) -> usize { ... }
```

### Error Handling

- Use `Result<T, &'static str>` for kernel errors
- String errors describe the issue (e.g., `"No entropy sources available"`)
- Use `?` operator for early returns
- Avoid unwrap() in production code; use expect() with messages if unavoidable
- For critical failures, consider panic with `early_println!` in kernel

### Unsafe Code

- Mark all unsafe blocks with `// SAFETY:` comments
- Document safety invariants explicitly
- Keep unsafe blocks as small as possible
- Prefer safe abstractions over repeated unsafe code

### Concurrency

- Use `spin::Mutex` for synchronization in no_std
- Use `Arc<T>` for shared ownership across threads
- Implement `Send` and `Sync` manually when required with `unsafe impl`
- Use `Once<T>` for one-time initialization

### Testing

- **Kernel tests**: Use `#[test_case]` attribute (no_std compatible)
- **User library tests**: Standard `#[test]` attribute
- **Integration tests**: Place in `tests/` directory
- All tests must pass before committing
- Write tests for new features and bug fixes

Example (kernel):
```rust
#[test_case]
fn test_alloc_free_desc() {
    let queue_size = 1;
    let mut virtqueue = VirtQueue::new(queue_size);
    virtqueue.init();
    let desc_idx = virtqueue.alloc_desc().unwrap();
    assert_eq!(desc_idx, 0);
    virtqueue.free_desc(desc_idx);
    assert_eq!(virtqueue.free_descriptors.len(), 1);
}
```

### Constants and Configuration

- Use `const` for compile-time constants
- Use `static` with proper interior mutability (e.g., `Mutex`)
- Feature flags in `[features]` section of Cargo.toml
- Architecture-specific code in `arch/` module

### Feature Flags

The kernel uses feature flags to enable optional functionality:

- `hypervisor`: Enable built-in hypervisor (SHV) support for RISC-V H-extension and AArch64 virtualization
- `user-fpu`: Enable FPU support for user tasks
- `user-vector`: Enable vector extension support for user tasks
- `profiler`: Enable profiling support (opt-in, pulls in `lazy_static`)
- `network`: Enable network subsystem
- `limine`: Enable Limine boot protocol support

The kernel enables `network`, `user-fpu`, `user-vector`, `hypervisor`, and `limine` by default. `profiler` is opt-in.

Additional features in user libraries:
- `alloc_debug` (in `user/lib/std`): Enable allocator debug logging (enabled by `user/bin` via dependency features)

### Memory Management

- Kernel uses custom allocator (slab_allocator_rs)
- User space uses talc allocator
- Use `alloc::boxed::Box` for heap allocation
- Use `alloc::vec::Vec` for dynamic arrays
- Prefer stack allocation when possible

### Driver Development

- Implement relevant traits from `device/` module
- Register with `DeviceManager`
- Use initcall system for driver initialization
- Follow virtio spec for virtio devices

### ABI Implementation

- Each ABI in `abi/` module
- Implement full syscall interface
- Use shared kernel objects (VFS, memory, devices)
- Document ABI differences and compatibility

### Development Environment

- Use Docker container `scarlet-dev` for consistency
- Build: `docker build -t scarlet-dev .`
- Run: `docker run -it --rm -v $(pwd):/workspaces/Scarlet scarlet-dev`
- All commands should be run via `cargo make`

### Before Committing

1. Run tests: `cargo make test-riscv64` (and aarch64)
2. Run clippy: `cargo make clippy-riscv64`
3. Check format: `cargo make fmt-check`
4. Build targets: `cargo make build-riscv64` and `cargo make build-aarch64`
5. Write/update documentation
6. Commit frequently with descriptive messages

### Architecture-Specific Code Separation

- **No `#[cfg(target_arch)]` in common code**: Architecture-specific logic must live in `kernel/src/arch/{riscv64,aarch64}/`. The common `kernel/src/` must not contain `#[cfg(target_arch = "...")]` blocks.
- **Expose arch-specific functions via `crate::arch::*`**: Each arch module (`riscv64`, `aarch64`) exports a unified public API. Common code calls `crate::arch::function_name()` without conditional compilation.
- **Both arches must implement the same public API**: If `riscv64` exports `fn start_secondary_cpus()`, `aarch64` must export the same function signature (even if it's a no-op).

### Key Principles

- **Safety First**: Leverage Rust's safety guarantees
- **No Std Compatibility**: Kernel and std lib must work without std
- **Multi-Arch**: Code should work on both RISC-V and AArch64
- **Documentation**: Public APIs must be documented
- **Testing**: Test critical paths and new features
- **CI/CD**: All checks must pass in GitHub Actions
