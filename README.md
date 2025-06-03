# Scarlet

<div align="center">
  
**A minimal operating system kernel written in Rust**

[![Version](https://img.shields.io/badge/version-0.11.1-blue.svg)](https://github.com/yourusername/Scarlet)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![RISC-V](https://img.shields.io/badge/arch-RISC--V%2064-green)](https://riscv.org/)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/petitstrawberry/Scarlet)

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
# Build all components (kernel, userlib, user programs, initramfs)
cargo make build

# Build only specific components:
cargo make build-kernel    # Build only the kernel
cargo make build-userlib   # Build only the user library
cargo make build-userbin   # Build only the user programs
cargo make build-initramfs # Build only the initial ramfs (copies user programs to initramfs)

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
│   └── drivers/      - Filesystem drivers
│       └── cpio/     - CPIO archive filesystem support
├── initcall/         - Initialization sequence management
├── library/          - Internal library code (std replacement)
├── mem/              - Memory management
├── sched/            - Scheduler implementation
├── syscall/          - System call interface
├── task/             - Task and process management
│   └── elf_loader/   - ELF executable loader
├── traits/           - Shared interfaces
└── vm/               - Virtual memory management
user/                 - User space code
├── bin/              - User programs
└── lib/              - User library code
mkfs/                 - Filesystem build tools
├── initramfs/        - Initial RAM filesystem contents
└── make_initramfs.sh - Script to build the initial RAM filesystem
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

Scarlet implements a highly flexible Virtual File System (VFS) layer designed for containerization, process isolation, and advanced bind mount capabilities:

### Core Architecture

- **Per-Task VFS Management**: Each task can have its own isolated `VfsManager` instance for containerization and namespace isolation, supporting both complete filesystem isolation and selective resource sharing through Arc-based filesystem object sharing
- **Filesystem Driver Framework**: Modular driver system with global `FileSystemDriverManager` singleton, supporting block device, memory-based, and virtual filesystem creation with type-safe structured parameter handling
- **Enhanced Mount Tree**: Hierarchical mount point management with O(log k) path resolution performance, independent mount point namespaces per VfsManager, and security-enhanced path normalization preventing directory traversal attacks

### Advanced Bind Mount System

- **Basic Bind Mounts**: Mount directories from one location to another within the same VfsManager for flexible filesystem layout management
- **Cross-VFS Bind Mounts**: Share directories between isolated VfsManager instances, enabling controlled resource sharing between containers while maintaining namespace isolation
- **Security-Enhanced Mounting**: Read-only bind mount support with write protection and proper permission inheritance
- **Shared Bind Mounts**: Mount propagation sharing for complex namespace scenarios and container orchestration
- **Thread-Safe Operations**: All bind mount operations are callable from system call context with proper locking

### Path Resolution & Security

- **Normalized Path Handling**: Automatic resolution of relative paths (`.` and `..`) with security validation
- **Directory Traversal Protection**: Comprehensive path validation preventing escape attacks through malicious path components
- **Transparent Resolution**: Seamless handling of bind mounts and nested mount points with proper filesystem delegation
- **Performance Optimization**: Efficient Trie-based mount point storage with O(log k) lookup complexity

### File Operations & Resource Management

- **RAII Resource Safety**: Files automatically close when dropped, filesystem handles are properly released, and memory is freed automatically
- **Thread-Safe File Access**: Concurrent file operations with RwLock protection and proper handle sharing through Arc
- **Standard Operations**: Complete support for open, read, write, seek, close operations with resource safety guarantees
- **Directory Operations**: Full directory manipulation including creation, deletion, listing with metadata support
- **Handle Management**: Arc-based file handle sharing with automatic cleanup and reference counting

### Storage & Device Integration

- **Block Device Interface**: Abstraction layer for interacting with storage devices including disk drives and other block-oriented storage
- **Memory-Based Filesystems**: Support for RAM-based filesystems like tmpfs with configurable size limits and persistence options
- **Device File Support**: Integration with character and block device management for /dev filesystem functionality
- **Hybrid Filesystem Support**: Filesystems that can operate on both block devices and memory regions for maximum flexibility
- **Driver Framework**: Extensible system for adding new filesystem implementations with proper type safety and error handling

The VFS implementation enables flexible deployment scenarios from simple shared filesystems to complete filesystem isolation with selective resource sharing, making it ideal for containerized applications, microkernel architectures, and security-conscious environments.

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
