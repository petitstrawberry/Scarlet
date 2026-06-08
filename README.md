# Scarlet

<div align="center">
  
**A kernel in Rust designed to provide a universal, multi-ABI container runtime.**

[![Version](https://img.shields.io/badge/version-0.16.0-blue.svg)](https://github.com/petitstrawberry/Scarlet)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![RISC-V](https://img.shields.io/badge/arch-RISC--V%2064-green)](https://riscv.org/)
[![AArch64](https://img.shields.io/badge/arch-AArch64-orange)](https://www.arm.com/)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/petitstrawberry/Scarlet)

</div>

## Overview

Scarlet is an operating system kernel written in Rust that implements native ABI support for executing binaries across different operating systems and architectures. The kernel provides a universal container runtime environment with strong isolation capabilities, comprehensive filesystem support, dynamic linking, and modern graphics capabilities.

## Quick Start

### Try Scarlet Now

```bash
# Get started with Nix (recommended)
nix develop
cargo make run-riscv64

# Or let direnv enter the same Nix shell automatically
direnv allow
cargo make run-riscv64

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

### Run Linux Userspace Demo (Partial Linux ABI)

See [Linux ABI Demo instructions](docs/abi/linux/demo.md) for detailed instructions on building and running the Linux userspace demo.

```bash
# Quick summary (inside nix develop or the scarlet-dev container):
# For RISC-V (default)
bash tools/linux/build_buildroot.sh
bash tools/linux/build_user_programs.sh
bash tools/linux/deploy_rootfs.sh
cargo make run-riscv64

# (Optional) Build KVM guest image for nested virtualization
bash tools/linux/build_guest_image.sh
bash tools/linux/deploy_rootfs.sh
cargo make run-riscv64
```

These commands rebuild the Buildroot-based Linux rootfs (providing standard utilities via BusyBox) and optional demo binaries, showcasing the initial Linux ABI support alongside Scarlet and xv6. The optional guest image enables running a nested Linux guest via kvmtool.

### Cross-ABI Execution Showcase

Scarlet allows binaries from different operating systems to coexist and communicate via standard Unix pipes. This is not virtualization—it is a unified kernel handling multiple ABIs natively.

```bash
# ✅ Working Now: xv6 shell executing a Scarlet native binary
# The output from 'hello' (Scarlet ABI) is piped to 'cat' (xv6 ABI)
(xv6)$ /scarlet/system/scarlet/bin/hello | cat
Hello, world!
PID  = 10
PPID = 9

# 🚧 In Progress: Linux ABI Integration
# We are expanding this capability to include Linux binaries (via BusyBox):
(scarlet)$ scarlet_cat /etc/passwd | /scarlet/system/linux-riscv64/bin/busybox grep "root" | xv6_wc -l
```

This interoperability is possible because all ABIs share the same underlying kernel objects (VFS, pipes, task structures). The goal is a seamless environment where you can use the best tool for the job, regardless of which OS it was originally written for.

> **Current Status**: 
> - ✅ **Scarlet Native ABI**: Fully implemented with interactive shell
> - 🧪 **xv6 RISC-V 64-bit ABI**: Working with Cross-ABI execution capabilities!
> - 🧩 **Linux RISC-V 64-bit ABI (partial)**: Buildroot-based userland demo available; syscall coverage expanding
> - ✅ **Cross-ABI Pipes**: Already functional between xv6 and Scarlet environments
> - 🧪 **Built-in Hypervisor (SHV)**: RISC-V H-extension (experimental, many features missing); AArch64 not implemented - [Status](docs/hypervisor/status.md)


## Key Features

- **Multi-ABI Support**: Transparent execution of binaries from different operating systems
- **Built-in Hypervisor (SHV)**: Type-2 virtual machine monitor with RISC-V H-extension and AArch64 virtualization support - [Design](docs/hypervisor/type2-design.md)
- **Runtime Delegation**: Execute binaries via userland runtimes (Wasm, emulators, etc.) - [Details](docs/runtime-delegation.md)
- **Service Management**: Stem daemon (stemd) provides systemd-like service management with dependency resolution - [Details](docs/stemd.md)
- **Container Runtime**: Complete filesystem isolation with namespace support
- **Dynamic Linking**: Native dynamic linker support for shared libraries and position-independent executables
- **Advanced VFS**: Modern virtual filesystem with ext2, FAT32, overlay, bind mount, and device file support
- **Graphics Support**: Framebuffer device support with graphics hardware abstraction
- **Windowing / UI (in progress)**: SWS protocol + client libraries - [Protocol](docs/sws_ipc_protocol.md), [sws-client](docs/sws_client.md), [scarlet-ui](docs/scarletui/design.md)
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
- **Linux RISC-V 64-bit (partial)**: 🧩 Early userland demo via Buildroot rootfs; syscall surface expanding toward full POSIX support

This architecture enables true containerization where applications from different operating systems can coexist and communicate without modification.

### ABI Implementation Details

#### xv6 RISC-V 64-bit (Experimental)

The xv6 ABI implementation is currently available as an experimental feature:

- **Testing Ready**: Core functionality is stable and ready for testing
- **Binary Compatibility**: Included xv6 binaries (`cat`, `grep`, `wc`, `sh`, etc.) work correctly
- **Cross-ABI Communication**: Pipes and IPC work seamlessly with other ABI implementations
- **Production Note**: While functional, this is an experimental implementation subject to changes

#### Linux ABI (Partial)

The Linux ABI implementation is currently in active development:

- **Userspace Support**: Runs simple static binaries and Buildroot/BusyBox environments.
- **Syscall Coverage**: Basic file I/O, process management, and memory operations are implemented.
- **Limitations**: Many advanced syscalls (networking, complex signals) are stubbed or missing. See [`docs/abi/linux/status.md`](docs/abi/linux/status.md) for the compatibility matrix.

## Architecture Support

Scarlet supports multiple CPU architectures with a unified codebase:

- **RISC-V 64-bit** - Primary development platform, fully supported
- **AArch64 (ARM 64-bit)** - In development, basic support available

Scarlet is migrating its boot flow to **Limine**, with `riscv64` now serving as the primary higher-half boot path. See [Limine Boot Path](docs/limine-boot.md).

The kernel includes hardware abstraction layers for interrupt handling, memory management, graphics/framebuffer support, and device drivers that work across both architectures.

### Building for Different Architectures

```bash
# RISC-V (default)
cargo make build-riscv64
cargo make run-riscv64

# AArch64
cargo make build-aarch64
cargo make run-aarch64
```

See [Multi-Architecture Support documentation](docs/multi-architecture.md) for detailed information on cross-architecture development.

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

### Docker Environment

The Docker image is generated by Nix from the same package set used by
`nix develop`. It exists for non-Nix users and CI image publication; the primary
development environment is still the Nix shell. The `scarlet-dev-image` package
is built on Linux Nix systems.

```bash
# Build and load the Nix-generated development image
nix build .#scarlet-dev-image
docker load < result

# Run commands in the development image
docker run -it --rm -v $(pwd):/workspaces/Scarlet scarlet-dev:latest
docker run --rm -v $(pwd):/workspaces/Scarlet scarlet-dev:latest cargo make build-riscv64

# Common commands:
cargo make run-riscv64                        # Build (release) and run (RISC-V)
cargo make test-riscv64               # Run tests (RISC-V)
cargo make debug-riscv64              # Debug with GDB
```

### Nix Development Environment

Use the Nix shell directly when you want to run QEMU on the host and use host
virtualization support such as KVM or Apple Hypervisor Framework.

```bash
# Enter the development shell
nix develop

# Common commands:
cargo make run-riscv64                # Build (release) and run (RISC-V)
cargo make test-riscv64               # Run tests (RISC-V)
cargo make run-aarch64                # Build (release) and run (AArch64)
```

With `direnv` and `nix-direnv`, entering the checkout should normally only
require:

```bash
direnv allow
```

The flake declares the Scarlet Rust Cachix binary cache in `nixConfig`. Reading
from that cache does not require a Cachix auth token, but multi-user Nix only
honors flake-provided substituters when the current Unix user is trusted by the
Nix daemon. On personal development machines, add your user to
`trusted-users`, for example:

```conf
trusted-users = root your-user-name
```

Alternatively, register the Scarlet cache directly in the daemon configuration:

```conf
extra-substituters = https://scarlet-rust-toolchain.cachix.org
extra-trusted-public-keys = scarlet-rust-toolchain.cachix.org-1:p+coBExi0nNTIvWF/oM9H9/1/GhwFtqGZ2Vs+4pYl6o=
```

If Nix prints `ignoring untrusted substituter`, it will ignore the Cachix cache
and may start building the Rust toolchain locally.

The default development shell uses the cached Scarlet Rust toolchain from
`scarlet-rust-nix`. The Nix closure is intended to be served from Cachix in CI;
configure the `CACHIX_CACHE_NAME` and `CACHIX_PUBLIC_KEY` repository variables
and `CACHIX_AUTH_TOKEN` repository secret to pull and push it. AArch64 uses
QEMU's pinned pc-bios EDK2 image as the macOS HVF + GICv3 baseline.

The active compiler should include Scarlet targets:

```bash
rustc --print target-list | grep scarlet
# aarch64-unknown-scarlet
# riscv64gc-unknown-scarlet
```

For local Rust fork iteration, keep the fork outside this repository and switch
only the active Rust path inside the shell:

```bash
# Recommended layout:
# /Users/petitstrawberry/Development/Rust/Scarlet
# /Users/petitstrawberry/Development/Rust/rust-scarlet

cd /Users/petitstrawberry/Development/Rust/rust-scarlet
./x build compiler/rustc library/std --target riscv64gc-unknown-scarlet,aarch64-unknown-scarlet

cd /Users/petitstrawberry/Development/Rust/Scarlet
nix develop
scarlet-rust-use-local ../rust-scarlet
scarlet-rust-show

# Return to the cached toolchain.
scarlet-rust-use-cached
```

This keeps the convenience of the Nix shell while allowing fast local stage1
Rust compiler iteration. The Rust and LLVM forks should not be checked out under
the Scarlet repository.

### Local Development

Manual setup without Nix is not the supported path anymore. It must provide an
equivalent Scarlet Rust toolchain, `cargo-make`, QEMU, cross toolchains,
filesystem image tools, firmware paths, fontconfig, and the other tools listed
in `flake.nix`. Use `nix develop` or the Nix-generated Docker image unless you
are intentionally reproducing that environment by hand.

For the Limine workflows, the development environment needs the appropriate UEFI
firmware. The Nix shell, including the Docker image entrypoint, exposes firmware
paths through `SCARLET_EFI_*` environment variables.

### Build Commands

```bash
# Full build (RISC-V, debug)
cargo make build-riscv64

# Individual components
cargo make build-kernel-debug-riscv64     # Kernel only
cargo make build-userlib-debug-riscv64    # User space library
cargo make build-userbin-debug-riscv64    # User programs
cargo make build-initramfs-debug-riscv64  # Initial RAM filesystem
cargo make build-rootfs-riscv64           # Root filesystem image

# Clean build artifacts
cargo make clean
```

### Testing and Debugging

```bash
# Clean build artifacts
cargo make clean
```

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## Documentation

For more detailed information about the Scarlet kernel, visit our documentation:
- [Scarlet Documentation](https://docs.scarlet.ichigo.dev/kernel)
- [Linux ABI Demo](docs/abi/linux/demo.md)
- [Linux userspace artifacts (Buildroot + optional binaries)](docs/abi/linux/userspace-artifacts.md)
- [Linux rootfs deployment guide](docs/abi/linux/deployment.md)
- [Linux ABI support status and roadmap](docs/abi/linux/status.md)
- [Limine Boot Path](docs/limine-boot.md)

### Generating Documentation

To generate the documentation, run:

```bash
# Generate documentation
cargo make doc-riscv64      # Generate docs for all components (RISC-V)
cargo make doc-kernel      # Generate kernel docs only
cargo make doc-userlib     # Generate user library docs only
```

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
