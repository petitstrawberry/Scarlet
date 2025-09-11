# Scarlet

<div align="center">
  
**A kernel in Rust designed to provide a universal, multi-ABI container runtime.**

[![Version](https://img.shields.io/badge/version-0.14.2-blue.svg)](https://github.com/petitstrawberry/Scarlet)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![RISC-V](https://img.shields.io/badge/arch-RISC--V%2064-green)](https://riscv.org/)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/petitstrawberry/Scarlet)

</div>

## Overview

Scarlet is an operating system kernel written in Rust that implements native ABI support for executing binaries across different operating systems and architectures. The kernel provides a universal container runtime environment with strong isolation capabilities, comprehensive filesystem support, dynamic linking, and modern graphics capabilities.

## Quick Start

### Try Scarlet Now

```bash
# Get started with Docker (recommended)
docker build -t scarlet-build .
docker run -it --rm scarlet-build bash -c "cargo make build && cargo make run"

# Once Scarlet boots, you'll see:
Login successful for user: root
Scarlet Shell (Interactive Mode)
# 

# Try Scarlet native binaries:
# hello
Hello, world!
PID  = 5
PPID = 3

# Enter xv6 environment (experimental ABI):
# xv6
xv6 container
Preparing to execute xv6 init...
init: starting sh
$ 

# Try xv6 binaries:
$ echo hello from xv6!
hello from xv6!

# Cross-ABI execution - xv6 calling Scarlet binary with pipe!
$ /scarlet/system/scarlet/bin/hello | cat
Hello, world!
PID  = 10
PPID = 9
```

### Multi-ABI Vision (Extended Goals)

```bash
# Current: Cross-ABI execution already works!
(xv6)$ /scarlet/system/scarlet/bin/hello | cat    # ✅ Working now

# Future goal: Full Linux ABI integration
(scarlet)$ scarlet_cat /etc/passwd | linux_grep "root" | xv6_wc -l
1

# Complete vision: All ABIs in one seamless environment:
# - scarlet_cat: Scarlet Native binary using Scarlet syscalls
# - linux_grep: Linux binary with native Linux ABI implementation
# - xv6_wc: xv6 binary through native xv6 ABI implementation
# All communicating seamlessly via unified pipe system!
```

This demonstrates **real Cross-ABI execution** - the xv6 environment can execute Scarlet native binaries and pipe their output through xv6 utilities! This proves that true multi-ABI functionality is already working.

> **Current Status**: 
> - ✅ **Scarlet Native ABI**: Fully implemented with interactive shell
> - 🧪 **xv6 RISC-V 64-bit ABI**: Working with Cross-ABI execution capabilities!
> - ✅ **Cross-ABI Pipes**: Already functional between xv6 and Scarlet environments
> - 🚧 **Linux ABI**: Under development (planned for future releases)

## Key Features

- **Multi-ABI Support**: Transparent execution of binaries from different operating systems
- **Container Runtime**: Complete filesystem isolation with namespace support
- **Dynamic Linking**: Native dynamic linker support for shared libraries and position-independent executables
- **Advanced VFS**: Modern virtual filesystem with ext2, FAT32, overlay, bind mount, and device file support
- **Graphics Support**: Framebuffer device support with graphics hardware abstraction
- **System Integration**: TTY devices, interrupt handling, and comprehensive device management
- **Task Management**: Full task lifecycle with environment variables and IPC pipes
- **Event System**: Advanced IPC with event-driven communication and synchronization
- **Memory Safety**: Built with Rust's safety guarantees for reliable system operation
- **RISC-V Ready**: Native support for RISC-V 64-bit architecture

## ABI Module System

Scarlet's Multi-ABI support is built around a modular ABI implementation system:

### How It Works

- **Binary Detection**: Automatic identification of binary format and target ABI
- **Native Implementation**: Each ABI module implements its own syscall interface using shared kernel APIs
- **Shared Kernel Resources**: All ABIs operate on common kernel objects (VFS, memory, devices, etc.)

### ABI Modules

- **Scarlet Native**: ✅ Complete - Direct kernel interface with optimal performance
- **xv6 RISC-V 64-bit**: 🧪 Experimental - Largely implemented with core functionality available
  - ✅ File operations (open, close, read, write, etc.)
  - ✅ Process management (fork, exec, wait, exit)
  - ✅ Memory management (sbrk)
  - ✅ Inter-process communication (pipes)
  - ✅ Device operations (mknod, console integration)
- **Linux Compatibility**: 🚧 In Development - Full POSIX syscall implementation planned

This architecture enables true containerization where applications from different operating systems can coexist and communicate without modification.

### Experimental Features

The xv6 RISC-V 64-bit ABI implementation is currently available as an experimental feature:

- **Testing Ready**: Core functionality is stable and ready for testing
- **Binary Compatibility**: Included xv6 binaries (`cat`, `grep`, `wc`, `sh`, etc.) work correctly
- **Cross-ABI Communication**: Pipes and IPC work seamlessly with other ABI implementations
- **Production Note**: While functional, this is an experimental implementation subject to changes

## Architecture Support

Currently supports RISC-V 64-bit architecture with plans for additional architectures. The kernel includes hardware abstraction layers for interrupt handling, memory management, graphics/framebuffer support, and device drivers.

## Filesystem Support

Scarlet implements a modern Virtual File System (VFS v2) with support for multiple filesystem types and container isolation:

### Supported Filesystems

- **TmpFS**: Memory-based temporary filesystem
- **CpioFS**: Read-only CPIO archive filesystem for initramfs
- **ext2**: Full ext2 filesystem implementation for persistent storage
- **FAT32**: Complete FAT32 filesystem support
- **OverlayFS**: Union filesystem combining multiple layers
- **DevFS**: Device file system for hardware access

### Container Features

- **Mount Namespace Isolation**: Per-task filesystem namespaces
- **Bind Mount Operations**: Directory mounting across namespaces
- **Overlay Support**: Layered filesystems with copy-on-write semantics

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
