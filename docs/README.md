# Scarlet Documentation

This directory contains design notes, subsystem references, and status documents
for the current Scarlet integration tree. The root [README](../README.md)
describes the repository at a high level; this page is the documentation map.

## Start Here

- [Kernel development map](kernel/README.md) - kernel library/BSP boundary,
  source map, boot sequence, memory ownership, modules, and test scope.
- [Userspace development map](userspace/README.md) - normal Rust std targets,
  legacy no_std libraries, SDK tooling, image layers, and service startup.

The entry, build, boot, and memory guides were reconciled with current source on
2026-09-06. Dated experiments and subsystem design notes retain their own scope;
this is not a claim that every historical document or device was revalidated.

## Release Preparation

- [Coordinated 1.0 roadmap](release/1.0-roadmap.md) - Scarlet, SGFX, ScarletUI,
  scarlet-sdk, and Adreno compatibility decisions, dependency migration, CI,
  and release gates.
- [1.0 scope and compatibility policy](release/1.0-scope.md) - Package membership,
  public API versus internal wire contracts, supported configurations, and RC1 gates.
- [1.0 package inventory](release/1.0-packages.md) - First-party crates,
  development fixtures, and vendored code before version alignment.

## Build, Boot, and Project Layout

- [Build system](build-system/README.md) - `cargo-scarlet`, project manifests,
  image layers, and generated artifacts.
- [Distribution model](architecture/distro-model.md) - repository role,
  projects, bundles, layers, and lock files.
- [Limine boot](boot/limine.md) - QEMU UEFI/Limine boot images for RISC-V and
  AArch64.
- [Multi-architecture support](architecture/multi-architecture.md) - RISC-V and
  AArch64 build, test, linker, and target layout.
- [Memory map](architecture/memory-map.md) - higher-half kernel and ioremap
  address layout.
- [Scheduler placement and fairness](architecture/scheduler.md) - capacity-aware
  scheduling policy, hints, fairness, and preemption integration points.
- [Scheduler benchmark scenarios](architecture/scheduler-benchmarks.md) -
  repeatable workloads for homogeneous and heterogeneous scheduler checks.

## ABI and Runtime Compatibility

- [Linux ABI demo](abi/linux/demo.md) - building and running the partial Linux
  userspace demo.
- [Linux ABI status](abi/linux/status.md) - syscall and feature compatibility
  matrix.
- [Linux userspace artifacts](abi/linux/userspace-artifacts.md) - Buildroot and
  optional userspace artifact generation.
- [Linux rootfs deployment](abi/linux/deployment.md) - placing Linux artifacts
  into Scarlet images.
- [Linux thread support](abi/linux/thread-support.md) - clone, futex, and
  per-thread state notes.
- [Runtime delegation](abi/runtime-delegation.md) - delegating unsupported binary
  formats to userland runtimes.

## Kernel and Device Subsystems

- [PCI/PCIe](device/pci.md) - ECAM discovery and PCI device model.
- [Input event devices](device/input-event.md) - Scarlet input event interface.
- [VirtIO input](device/virtio-input.md) and [VirtIO RNG](device/virtio-rng.md)
  - VirtIO device notes.
- [Display presentation](device/display-present.md) - CPU-composited display
  presentation paths.
- [Video decode device](device/video-decode.md) - `/dev/videoN` ABI used by
  `video-player`.
- [USB subsystem](usb/README.md) - xHCI/USB host stack status and milestones.
- [Audio system](audio/design.md) - kernel PCM transport, SAS, and playback
  model.
- [Loadable Scarlet modules](modules/lsm.md) - `.lsm` module loading and
  relocations.

## User Space and Desktop

- [Workspace shell](desktop/workspace-shell.md) - unified laptop/tablet
  workspace model, Home, Overview, gestures, and compositor boundary.
- [SWS IPC protocol](graphics/sws-ipc-protocol.md) - Scarlet Window Server wire
  protocol.
- [sws-client](graphics/sws-client.md) - low-level SWS client library.
- [ScarletUI repository](https://github.com/petitstrawberry/scarlet-ui) - owning
  source, current architecture, application/extension contract, and examples.
  Local [design](graphics/scarletui/design.md) and
  [API](graphics/scarletui/api.md) pages are historical integration snapshots,
  not current trait references.
- [Wayland bridge](graphics/wayland-bridge.md) - Wayland-to-SWS bridge design.
- [stemd](services/stemd.md) - service manager and desktop application registry.
- [Scarlet SKK](services/scarlet-skk.md) and
  [Scarlet Mozc](services/scarlet-mozc.md) - input method service experiments.
- [TTY and PTY](system/tty-pty.md) - terminal and pseudoterminal support.

## Networking, Containers, and Packages

- [Network architecture](network/architecture.md) - network layers, sockets, and
  VirtIO-net.
- [Socket/VFS integration](network/socket-vfs-integration.md) - local socket path
  model.
- [Namespace isolation](container/namespace-isolation.md) and
  [task namespaces](container/task-namespace.md) - container isolation model.
- [SCPM](scpm/README.md), [package format](scpm/package-format.md), and
  [package API](scpm/api.md) - Scarlet package manager notes.

## Hypervisor

- [SHV overview](hypervisor/README.md)
- [Type-2 hypervisor design](hypervisor/type2-design.md)
- [Hypervisor status](hypervisor/status.md)

The documentation tracks implementation direction as well as completed behavior.
When code and docs disagree, treat the code and project manifests as the source
of truth, then update the relevant document in the same change.
