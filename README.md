# Scarlet

<div align="center">

**A Rust operating system kernel and reference distribution for multi-ABI systems.**

[![Version](https://img.shields.io/badge/version-0.16.0-blue.svg)](https://github.com/petitstrawberry/Scarlet)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![RISC-V](https://img.shields.io/badge/arch-RISC--V%2064-green)](https://riscv.org/)
[![AArch64](https://img.shields.io/badge/arch-AArch64-orange)](https://www.arm.com/)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/petitstrawberry/Scarlet)

<img src="docs/assets/screenshots/scarlet-desktop.png" alt="Scarlet desktop running Myrica, Files, Boxcraft, a terminal, and a video player" width="900">

</div>

## Overview

Scarlet is an operating system project written primarily in Rust. It combines a
kernel, Scarlet-native userland, ABI compatibility layers, desktop services,
device drivers, and image tooling in one integration tree.

The kernel is built around shared kernel objects and ABI modules. Scarlet-native,
xv6, and Linux-compatible programs are intended to coexist over the same VFS,
socket, task, device, and event primitives instead of being isolated behind
separate virtual machines.

This repository is the current reference distribution. It contains the kernel,
in-tree user libraries and programs, loadable modules, drivers, filesystem
bundles, bootable project manifests, and documentation. Project and image
composition is driven by `cargo-scarlet` from `scarlet-sdk` and the
`scarlet.toml` manifests under `projects/`.

## Highlights

- RISC-V 64 and AArch64 kernel support, with QEMU images and experimental
  [real-hardware projects](#hardware-support).
- Scarlet-native userland, xv6 support, and partial Linux ABI support over the
  same kernel objects.
- Scarlet SDK and a Scarlet Rust toolchain for building Rust `std`
  applications targeting Scarlet.
- Bootable distribution projects composed from reusable filesystem bundles.
- In-tree desktop services and applications with SWS, terminal, taskbar,
  settings, and IME experiments, using the separate ScarletUI/SGFX repositories.
- Wayland bridge support for running selected Linux GUI applications on the
  Scarlet desktop.
- SHV Type-2 hypervisor support with Linux `/dev/kvm` compatibility, including
  Firecracker-class AArch64 microVM workloads.
- Device work covering VirtIO, networking, display presentation, audio, video
  decode.

## Desktop

| Virtual desktops | Application launcher |
| :---: | :---: |
| [<img src="docs/assets/screenshots/scarlet-virtual-desktops.png" alt="Scarlet workspace overview with virtual desktop thumbnails and open application windows" width="440">](docs/assets/screenshots/scarlet-virtual-desktops.png) | [<img src="docs/assets/screenshots/scarlet-launcher.png" alt="Scarlet application launcher with a search field and application grid" width="440">](docs/assets/screenshots/scarlet-launcher.png) |
| The workspace overview shows virtual desktops and their open windows. | Open installed desktop applications from the launcher. |

## Quick Start

Use the Nix development shell unless you are intentionally reproducing the tool
environment by hand.

```bash
nix develop

# Build and run the default RISC-V full image.
cargo make run-riscv64

# Build and run the AArch64 QEMU full image.
cargo make run-aarch64

# Pass QEMU display and GPU arguments through explicitly.
SCARLET_QEMU_DISPLAY='cocoa,gl=on,retina=on' \
SCARLET_QEMU_GPU=virtio-gpu-gl-pci \
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

## Application Development

Scarlet applications can be built with the Scarlet Rust toolchain against
Scarlet `std` targets:

```bash
cd user/std-bin
cargo build --target riscv64gc-unknown-scarlet
cargo build --target aarch64-unknown-scarlet
```

The in-tree `user/std-bin` programs use normal Rust dependencies together with
Scarlet-specific crates such as `scarlet-os`, `scarlet-ui`, `sas-client`, and
`sas-protocol`. The same source can also use host-side UI backends where the
crate supports them.

Scarlet SDK provides `cargo-scarlet` and image plugins used to build kernels,
compose filesystem bundles, generate boot images, and run project manifests.
It is distinct from `scarlet-os` and the legacy `scarlet-std` user libraries.
See the [userspace development map](docs/userspace/README.md) for std/no_std
build paths, native API boundaries, image installation, and service startup.

## Project Model

Scarlet is built from project manifests rather than from a single root Cargo
workspace.

```text
Scarlet/
  kernel/                         # kernel
  drivers/                        # loadable and in-tree driver crates
  modules/                        # static and loadable Scarlet module crates
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
| `projects/aarch64-limine-full` | Default AArch64 QEMU full system. |
| `projects/aarch64-limine-microvm` | AArch64 microvm-oriented project. |

`scarlet.toml` is the source of truth for each project. It selects the BSP,
kernel features, module set, ordered image layers, boot image format, and runner.
The BSP executable lives in `projects/<project>/bsp/`; `kernel/` is a library.
`scarlet.lock` records resolved image-layer inputs, separately from the BSP and
userspace Cargo locks. Full images include desktop content through bundles;
there are no separate tracked desktop project directories.
See [Scarlet Distribution Model](docs/architecture/distro-model.md) and
[Scarlet Build System](docs/build-system/README.md).

## Hardware Support

Alongside the in-tree QEMU images, Scarlet has experimental support for real
hardware through standalone projects:

- [Apple Silicon](https://github.com/petitstrawberry/scarlet-project-applesilicon):
  Apple-specific BSP, drivers, and deployment tooling.
- [Qualcomm SC7180 Chromebook](https://github.com/petitstrawberry/scarlet-project-chromebook):
  Google CoachZ rev3 (Trogdor) board integration.

See each project for supported models, device coverage, and build/deployment
instructions.

## Media Codecs

The in-tree `video-player` binary is built as its own user program crate under
`user/video_player`. The default build enables the stateful H.264 hardware path
for H.264 decode and stateful AV1 hardware decode. Other codec paths are
selected explicitly by crate or bundle features. See
[`user/video_player`](user/video_player/README.md) for the codec feature policy.

## Build and Run

The `cargo make` tasks are convenience wrappers around `cargo scarlet`.

```bash
# Build kernel and core user components (image composition is separate).
cargo make build-riscv64
cargo make build-aarch64

# Run through the project runner.
cargo make run-riscv64
cargo make run-aarch64
cargo make run-aarch64-microvm

# Call cargo-scarlet directly when working on a specific project.
cargo scarlet image --project projects/riscv64-limine-full
cargo scarlet run --project projects/riscv64-limine-full --release
```

## ABI Model

Scarlet ABI support is implemented as kernel ABI modules over shared kernel
objects:

- Binary format detection selects the ABI implementation.
- Each ABI translates its syscall surface into Scarlet kernel primitives.
- ABIs share VFS nodes, sockets, task objects, devices, and events rather than
  communicating through a VM boundary.

Implemented and active ABI work:

| ABI | State |
| --- | --- |
| Scarlet native | Main in-tree userland and services. |
| xv6 RISC-V 64 | Supported for shell and common xv6 commands. |
| Linux RISC-V/AArch64 | Partial but actively used syscall layer for selected Buildroot/BusyBox, GUI, and service workloads. |

See [Linux ABI status](docs/abi/linux/status.md),
[Linux userspace artifacts](docs/abi/linux/userspace-artifacts.md), and
[runtime delegation](docs/abi/runtime-delegation.md).

The Linux ABI also exposes a `/dev/kvm` compatibility layer backed by SHV, so
KVM-oriented VMMs can target Scarlet's hypervisor path instead of a separate
kernel API.

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
  IME services, and Wayland bridge support for selected Linux GUI applications.
- **Audio**: kernel PCM transport, SAS userspace server, audio routing/control,
  VirtIO sound, and Apple MCA/ADMAC experiments. See
  [Scarlet Audio System Design](docs/audio/design.md).
- **Hypervisor**: SHV Type-2 virtualization with Scarlet-native APIs and Linux
  `/dev/kvm` compatibility for RISC-V and AArch64 guests.
- **Modules**: static crates selected by `[modules]` and loadable Scarlet
  modules (`.lsm`) with per-architecture relocation support. See
  [kernel development](docs/kernel/README.md) and
  [Loadable Scarlet Module](docs/modules/lsm.md).

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

- [Kernel development map](docs/kernel/README.md)
- [Userspace development map](docs/userspace/README.md)
- [Build system](docs/build-system/README.md)
- [Distribution model](docs/architecture/distro-model.md)
- [Multi-architecture support](docs/architecture/multi-architecture.md)
- [Limine boot](docs/boot/limine.md)
- [Linux ABI demo](docs/abi/linux/demo.md)
- [Audio design](docs/audio/design.md)
- [USB subsystem](docs/usb/README.md)
- [SWS protocol](docs/graphics/sws-ipc-protocol.md)
- [ScarletUI repository](https://github.com/petitstrawberry/scarlet-ui)
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
