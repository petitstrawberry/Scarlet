# Scarlet

<div align="center">

**A Rust operating system kernel and reference distribution for multi-ABI systems.**

[![Version](https://img.shields.io/badge/version-0.16.0-blue.svg)](https://github.com/petitstrawberry/Scarlet)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![RISC-V](https://img.shields.io/badge/arch-RISC--V%2064-green)](https://riscv.org/)
[![AArch64](https://img.shields.io/badge/arch-AArch64-orange)](https://www.arm.com/)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/petitstrawberry/Scarlet)

</div>

## Overview

Scarlet is an operating system project written primarily in Rust. The kernel is
designed around shared kernel objects and ABI modules, so Scarlet-native, xv6,
and Linux-compatible user programs can eventually coexist in one system without
putting each ABI in a separate virtual machine.

This repository is the current reference distribution and integration tree. It
contains the kernel, in-tree user libraries and programs, loadable modules,
drivers, filesystem bundles, bootable project manifests, and documentation. The
project/image tooling is provided by `cargo-scarlet` from `scarlet-sdk` and is
consumed here through `scarlet.toml` project manifests.

## Current Status

| Area | Status |
| --- | --- |
| RISC-V 64 | Primary QEMU development target. Limine boot, kernel tests, userland, VirtIO devices, networking, and desktop-oriented images are maintained here first. |
| AArch64 QEMU | Active target. Limine/UEFI boot, kernel tests, VirtIO devices, and desktop/full projects are supported. |
| Apple Silicon | Experimental bring-up under `projects/aarch64-apple-limine-full`. Current work includes AIC, PMGR, DART, SMC/SPMI, SPI HID, framebuffer/display experiments, MCA/ADMAC audio, codecs, DWC3 USB, and PCIe-related drivers. Expect hardware-specific rough edges. |
| ABI support | Scarlet native ABI works for the in-tree userland. xv6 RISC-V is experimental but usable for basic commands. Linux ABI is partial and focused on selected static and Buildroot/BusyBox-style userlands. |
| Desktop/UI | SWS, `sws-client`, ScarletUI, desktop shell, taskbar, terminal, settings, IME experiments, and a Wayland bridge are in progress. |
| Audio/media | `/dev/audioN`, VirtIO sound, Apple MCA/ADMAC playback, SAS, `sasctl`, `mplayer`, and `video_player` are available for current experiments. Audio design is still evolving around realtime and device-routing constraints. |
| Storage/USB | VFS supports tmpfs, cpiofs, ext2, FAT32, overlay, bind mounts, and devfs. VirtIO block is the primary block path. xHCI/DWC3 USB is early; USB storage is not a supported path yet. |
| Hypervisor | SHV exists as an experimental Type-2 hypervisor direction. RISC-V H-extension support is the active path; AArch64 virtualization is not complete. |

## Quick Start

Use the Nix development shell unless you are intentionally reproducing the tool
environment by hand.

```bash
nix develop

# Build and run the default RISC-V full image.
cargo make run-riscv64

# Build and run the AArch64 QEMU full image.
cargo make run-aarch64

# Run kernel tests for both maintained architectures.
cargo make test-riscv64
cargo make test-aarch64

# Check formatting before committing.
cargo make fmt-check
```

With `direnv` and `nix-direnv`, entering the checkout can be reduced to:

```bash
direnv allow
```

Manual setup is not the supported path. It must provide an equivalent Scarlet
Rust toolchain, `cargo-make`, `cargo-scarlet`, QEMU, cross tools, filesystem
image tools, firmware paths, fontconfig, and the other tools described in
`flake.nix`.

## Project Model

Scarlet is built from project manifests rather than from a single root Cargo
workspace.

```text
Scarlet/
  kernel/                         # no_std kernel
  drivers/                        # loadable and in-tree driver crates
  modules/                        # loadable Scarlet modules
  user/
    lib/                          # Scarlet user libraries
    bin/                          # core Scarlet user programs
    std-bin/                      # Rust std-based Scarlet programs
  bundles/                        # reusable filesystem/image layer bundles
  projects/                       # bootable target manifests
  docs/                           # design notes and subsystem documentation
```

Important project variants:

| Project | Purpose |
| --- | --- |
| `projects/riscv64-limine-full` | Default RISC-V QEMU full system. |
| `projects/riscv64-limine-desktop` | RISC-V desktop-focused image. |
| `projects/aarch64-limine-full` | Default AArch64 QEMU full system. |
| `projects/aarch64-limine-desktop` | AArch64 desktop-focused image. |
| `projects/aarch64-limine-microvm` | AArch64 microvm-oriented project. |
| `projects/aarch64-apple-limine-full` | Experimental Apple Silicon deployment target. |

`scarlet.toml` is the source of truth for each project. It selects the BSP,
kernel features, module set, ordered image layers, boot image format, and runner.
`scarlet.lock` pins resolved layer inputs for reproducible image composition.
See [Scarlet Distribution Model](docs/architecture/distro-model.md) and
[Scarlet Build System](docs/build-system/README.md).

## Build and Run

The `cargo make` tasks are convenience wrappers around `cargo scarlet`.

```bash
# Build kernel, user libraries, user programs, and images.
cargo make build-riscv64
cargo make build-aarch64

# Run through the project runner.
cargo make run-riscv64
cargo make run-aarch64
cargo make run-aarch64-microvm

# Call cargo-scarlet directly when working on a specific project.
cargo scarlet image --project projects/riscv64-limine-full
cargo scarlet run --project projects/riscv64-limine-full --release
cargo scarlet image --project projects/aarch64-apple-limine-full
```

Apple Silicon deployment is intentionally separate from normal QEMU runs. See
[`projects/aarch64-apple-limine-full/DEPLOY.md`](projects/aarch64-apple-limine-full/DEPLOY.md)
for the m1n1/U-Boot/Limine flow.

## ABI Model

Scarlet ABI support is implemented as kernel ABI modules over shared kernel
objects:

- Binary format detection selects the ABI implementation.
- Each ABI translates its syscall surface into Scarlet kernel primitives.
- ABIs share VFS nodes, pipes, task objects, sockets, and devices rather than
  communicating through a VM boundary.

Implemented and active ABI work:

| ABI | State |
| --- | --- |
| Scarlet native | Main in-tree userland and services. |
| xv6 RISC-V 64 | Experimental, enough for shell and common xv6 commands. |
| Linux RISC-V/AArch64 | Partial syscall layer for selected static and Buildroot/BusyBox userlands. |

See [Linux ABI status](docs/abi/linux/status.md),
[Linux userspace artifacts](docs/abi/linux/userspace-artifacts.md), and
[runtime delegation](docs/abi/runtime-delegation.md).

## Major Subsystems

- **Boot and images**: Limine UEFI boot images generated by `cargo-scarlet`.
  See [Limine Boot](docs/boot/limine.md).
- **VFS and filesystems**: tmpfs, cpiofs, ext2, FAT32, overlay, bind mounts, and
  devfs with task namespace support.
- **Device model**: PCI/PCIe, VirtIO, framebuffer/display presentation,
  input-event devices, audio devices, video decode devices, and loadable driver
  modules.
- **Networking**: in-kernel network layers with VirtIO-net as the main backend.
  See [Network Architecture](docs/network/architecture.md).
- **Windowing and UI**: SWS protocol, `sws-client`, ScarletUI, desktop shell,
  IME services, and Wayland bridge experiments.
- **Audio**: kernel PCM transport, SAS userspace server, audio routing/control,
  VirtIO sound, and Apple MCA/ADMAC experiments. See
  [Scarlet Audio System Design](docs/audio/design.md).
- **Modules**: loadable Scarlet modules (`.lsm`) with per-architecture
  relocation support. See [Loadable Scarlet Module](docs/modules/lsm.md).

## Development Commands

```bash
# Formatting
cargo make fmt
cargo make fmt-check

# Clippy
cargo make clippy-riscv64
cargo make clippy-aarch64

# Tests
cargo make test-riscv64
cargo make test-aarch64

# Individual kernel test command form
cd kernel
cargo test --target targets/riscv64gc-unknown-none-elf.json test_name
```

The root test tasks run the kernel tests under QEMU and require the same Nix
shell environment as normal runs. CI currently expects both `test-riscv64` and
`test-aarch64` to pass.

## Documentation

Start with the [documentation index](docs/README.md). Useful entry points:

- [Build system](docs/build-system/README.md)
- [Distribution model](docs/architecture/distro-model.md)
- [Multi-architecture support](docs/architecture/multi-architecture.md)
- [Limine boot](docs/boot/limine.md)
- [Linux ABI demo](docs/abi/linux/demo.md)
- [Audio design](docs/audio/design.md)
- [USB subsystem](docs/usb/README.md)
- [SWS protocol](docs/graphics/sws-ipc-protocol.md)
- [ScarletUI design](docs/graphics/scarletui/design.md)
- [Hypervisor status](docs/hypervisor/status.md)

To generate Rust documentation:

```bash
cargo make doc-riscv64
cargo make doc-kernel
cargo make doc-userlib
```

## Contributing

Contributions are welcome. Please keep changes scoped, run the relevant
`cargo make` tasks before sending a PR, and update docs when changing public
interfaces, project manifests, or user-visible behavior.

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE).
