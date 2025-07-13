# Scarlet

<div align="center">
  
**A kernel in Rust designed to provide a universal, multi-ABI container runtime.**

[![Version](https://img.shields.io/badge/version-0.12.0-blue.svg)](https://github.com/petitstrawberry/Scarlet)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![RISC-V](https://img.shields.io/badge/arch-RISC--V%2064-green)](https://riscv.org/)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/petitstrawberry/Scarlet)

</div>

## Overview

Scarlet is an operating system kernel written in Rust that implements a transparent ABI conversion layer for executing binaries across different operating systems and architectures. The kernel provides a universal container runtime environment with strong isolation capabilities and comprehensive filesystem support.

## Quick Start

Want to see Multi-ABI execution in action? Try Scarlet using Docker:

```bash
# Get started with Docker (recommended)
docker build -t scarlet-build .
docker run -it --rm scarlet-build bash -c "cargo make build && cargo make run"

# Once Scarlet is running, you can try Multi-ABI pipeline communication:
(scarlet)$ scarlet_cat /etc/passwd | linux_grep "root" | xv6_wc -l
1

# Different binaries, same environment:
# - scarlet_cat: Scarlet Native binary using Scarlet syscalls
# - linux_grep: Linux binary with transparent ABI conversion  
# - xv6_wc: xv6 binary running through compatibility layer
# All communicating seamlessly via pipes!
```

This demonstrates true ABI transparency - binaries from different operating systems working together as if they were native.

> **Note**: Currently, Scarlet Native ABI is implemented. Linux and xv6 ABI support are under development and will be available in future releases.

## Key Features

- **Multi-ABI Support**: Transparent execution of binaries from different operating systems
- **Container Runtime**: Complete filesystem isolation with namespace support
- **Advanced VFS**: Modern virtual filesystem with overlay, bind mount, and device file support
- **System Integration**: TTY devices, interrupt handling, and comprehensive device management
- **Task Management**: Full task lifecycle with environment variables and IPC pipes
- **Memory Safety**: Built with Rust's safety guarantees for reliable system operation
- **RISC-V Ready**: Native support for RISC-V 64-bit architecture

## ABI Module System

Scarlet's Multi-ABI support is built around a modular ABI implementation system:

### How It Works

- **Binary Detection**: Automatic identification of binary format and target ABI
- **Native Implementation**: Each ABI module implements its own syscall interface using shared kernel APIs
- **Shared Kernel Resources**: All ABIs operate on common kernel objects (VFS, memory, devices, etc.)

### ABI Modules

- **Scarlet Native**: Direct kernel interface with optimal performance
- **Linux Compatibility** *(in development)*: Full POSIX syscall implementation
- **xv6 Compatibility** *(in development)*: Educational OS syscall implementation

This architecture enables true containerization where applications from different operating systems can coexist and communicate without modification.

## Architecture Support

Currently supports RISC-V 64-bit architecture with plans for additional architectures. The kernel includes hardware abstraction layers for interrupt handling, memory management, and device drivers.

## Filesystem Support

Scarlet implements a modern Virtual File System (VFS v2) with support for multiple filesystem types and container isolation:

### Supported Filesystems

- **TmpFS**: Memory-based temporary filesystem
- **CpioFS**: Read-only CPIO archive filesystem for initramfs
- **OverlayFS**: Union filesystem combining multiple layers
- **DevFS**: Device file system for hardware access

### Container Features

- **Mount Namespace Isolation**: Per-task filesystem namespaces
- **Bind Mount Operations**: Directory mounting across namespaces
- **Overlay Support**: Layered filesystems with copy-on-write semantics

## Development

## Development

### Docker Environment (Recommended)

```bash
# Build and run development container
docker build -t scarlet-dev .
docker run -it --rm -v $(pwd):/workspaces/Scarlet scarlet-dev

# Common commands:
cargo make build && cargo make run    # Build and run
cargo make test                       # Run tests  
cargo make debug                      # Debug with GDB
```

### Local Development

Requirements: Rust nightly, `cargo-make`, `qemu`, RISC-V toolchain

### Build Commands

```bash
# Full build (recommended for first time)
cargo make build

# Individual components
cargo make build-kernel    # Kernel only
cargo make build-userlib   # User space library
cargo make build-userbin   # User programs
cargo make build-initramfs # Initial RAM filesystem

# Clean build artifacts
cargo make clean
```

### Testing and Debugging

```bash
# Run all tests
cargo make test

# Debug kernel with GDB
cargo make debug
# Then in another terminal: gdb and connect to :1234
```

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
