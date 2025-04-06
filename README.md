# Scarlet

<div align="center">
  
**A minimal operating system kernel written in Rust**

[![Version](https://img.shields.io/badge/version-0.8.1-blue.svg)](https://github.com/yourusername/Scarlet)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![RISC-V](https://img.shields.io/badge/arch-RISC--V%2064-green)](https://riscv.org/)

</div>

## Overview

Scarlet is a bare metal, minimalist operating system kernel written in Rust. The project aims to provide a clean design with strong safety guarantees through Rust's ownership model. While the current implementation focuses on essential kernel functionality, our long-term vision is to develop a fully modular operating system with dynamic component loading and unloading capabilities.

### Core Features

- **No Standard Library**: Built using `#![no_std]` for bare metal environments
- **Architecture Support**: Currently implemented for RISC-V 64-bit with plans for additional architectures
- **Memory Management**: Custom heap allocator with virtual memory support
- **Task Scheduling**: Simple but effective task scheduler
- **Driver Framework**: Organized device driver architecture with device discovery
- **Filesystem Support**: Basic filesystem abstractions
- **Hardware Abstraction**: Clean architecture-specific abstractions for multi-architecture support
- **Modularity Vision**: Working toward a fully modular OS design where components can be dynamically loaded and unloaded




## Setting Up the Environment

To build and run Scarlet, you need to have the following prerequisites installed:
- Rust (nightly version)
- `cargo-make` for build automation
- `qemu` for emulation
- Architecture-specific toolchain (currently RISC-V)

Also, you can use docker to set up a development environment.

#### Using Docker (Recommended)

You can set up a development environment using Docker. This is the recommended way to build and run Scarlet.

```bash
# Build the Docker image
docker build -t scarlet-build .

# Run the container
docker run -it --rm -v $(pwd):/workspaces/Scarlet scarlet-build

# Inside the container, you can run the following commands:
# Build the kernel
cargo make build
```

## Building and Running

To build and run Scarlet, you can use the following commands:

```bash
# Build all components (kernel, userlib, and user programs)
cargo make build

# Build only specific components:
cargo make build-kernel    # Build only the kernel
cargo make build-userlib   # Build only the user library
cargo make build-userbin   # Build only the user programs

# Clean all build artifacts
cargo make clean

# Run the kernel
cargo make run
```

### Debugging

To debug the kernel, you can use following command:

```bash
cargo make debug
```
This will start the kernel in QEMU with GDB support. You can then attach a GDB session to the running kernel.

### Testing

```bash
cargo make test
```

## Project Structure

```
kernel/src/           - Kernel source code
├── arch/             - Architecture specific code
│   └── riscv64/      - RISC-V 64-bit implementation
├── device/           - Device abstractions and management
├── drivers/          - Hardware drivers
│   ├── block/        - Block device drivers
│   ├── uart/         - UART drivers
│   └── virtio/       - VirtIO device drivers
├── fs/               - Filesystem implementations
├── initcall/         - Initialization sequence management
├── library/          - Internal library code (std replacement)
├── mem/              - Memory management
├── sched/            - Scheduler implementation
├── syscall/          - System call interface
├── task/             - Task and process management
├── traits/           - Shared interfaces
└── vm/               - Virtual memory management
user
├── bin/              - User programs
└── lib/              - User library code
```

## Architecture Support

Currently, Scarlet supports the RISC-V 64-bit architecture, with plans to expand to additional architectures in the future. The clean abstraction layer is designed to facilitate porting to other architectures.

### Current Implementation

The RISC-V implementation includes:
- Boot sequence for both bootstrap processors and application processors
- Interrupt handling through trap frames
- Memory management with virtual memory support
- Architecture-specific timer implementation
- Instruction abstractions and SBI interface

## Boot Process

Scarlet's boot process follows this sequence:
1. Architecture initialization (`init_arch`)
2. FDT (Flattened Device Tree) parsing
3. Heap initialization  
4. Early driver initialization via initcalls
5. Virtual memory setup
6. Device discovery and initialization
7. Timer initialization
8. Scheduler initialization and task creation
9. Task scheduling and kernel main loop

## Resource Management with Rust's Ownership Model

Scarlet leverages Rust's ownership and borrowing system to provide memory safety without garbage collection:

- **Zero-Cost Abstractions**: Using Rust's type system for resource management without runtime overhead
- **RAII Resource Management**: Kernel resources are automatically cleaned up when they go out of scope
- **Mutex and RwLock**: Thread-safe concurrent access to shared resources using the `spin` crate
- **Arc** (Atomic Reference Counting): Safe sharing of resources between kernel components
- **Memory Safety**: Prevention of use-after-free, double-free, and data races at compile time
- **Trait-based Abstractions**: Common interfaces for device drivers and subsystems enabling modularity

Examples of this can be seen in device management, filesystem access, and task scheduling, where resources are borrowed rather than copied, and ownership is clearly defined.

## Virtual File System

Scarlet implements a flexible Virtual File System (VFS) layer that provides:

- **Filesystem Abstraction**: Common interface for multiple filesystem implementations
- **Mount Point Management**: Support for mounting filesystems at different locations in a unified hierarchy
- **Path Resolution**: Normalization and resolution of file paths across different mounted filesystems
- **File Operations**: Standard operations (open, read, write, seek, close) with resource safety
- **Block Device Interface**: Abstraction layer for interacting with storage devices
- **Driver Framework**: Extensible system for adding new filesystem implementations

The VFS implementation uses Rust's trait system to define interfaces that different filesystems must implement, allowing for strong typing while maintaining flexibility.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## Documentation

For more detailed information about the Scarlet kernel, visit our documentation:
[Scarlet Documentation](https://docs.scarlet.ichigo.dev/kernel)

### Generating Documentation

To generate the documentation, run:

```bash
# Generate documentation
cargo make doc             # Generate docs for all components
cargo make doc-kernel      # Generate kernel docs only
cargo make doc-userlib     # Generate user library docs only
```

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
