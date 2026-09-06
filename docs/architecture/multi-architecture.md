# Multi-architecture development

Scarlet implements RISC-V 64 and AArch64 kernel ports. Its ABI layer translates
operating-system interfaces over shared kernel objects; it does **not** translate
CPU instructions. A Linux or Scarlet executable must match the machine's
architecture. Linux ABI support is partial; see the
[compatibility status](../abi/linux/status.md).

This page describes the current repository layout, reviewed on 2026-09-06.
Start with the [kernel development map](../kernel/README.md) for common code.

## Project and target selection

The project manifest selects a BSP, whose Cargo configuration selects the kernel
target. The tracked reference projects are:

| Project | Kernel target | Configuration |
| --- | --- | --- |
| [riscv64-limine-full](../../projects/riscv64-limine-full/scarlet.toml) | `riscv64gc-unknown-none-elf.json` | RISC-V QEMU full system |
| [aarch64-limine-full](../../projects/aarch64-limine-full/scarlet.toml) | `aarch64-unknown-none-elf.json` | AArch64 QEMU full system; hypervisor explicitly disabled |
| [aarch64-limine-microvm](../../projects/aarch64-limine-microvm/scarlet.toml) | `aarch64-unknown-none-elf.json` | AArch64 microvm configuration with hypervisor and `microvm-init` |

`ARCH` is used by some helper scripts; setting it is not a substitute for
selecting a project. From the repository root in the Nix development shell:

```sh
# Build the selected kernel/BSP.
cargo scarlet build --project projects/riscv64-limine-full
cargo scarlet build --project projects/aarch64-limine-full

# Compose the selected project's userspace and boot images.
cargo scarlet image --project projects/riscv64-limine-full
cargo scarlet image --project projects/aarch64-limine-full
```

The `cargo make build-riscv64` / `build-aarch64` tasks build kernel and core
user components. Image composition is a separate `image` operation; the
`run` tasks compose images before invoking the project runner.

## Userspace is a separate target family

Normal Rust `std` applications use `riscv64gc-unknown-scarlet` or
`aarch64-unknown-scarlet` from the Scarlet Rust toolchain. For example:

```sh
cd user/std-bin
cargo check -p scarlet-std-bin --target riscv64gc-unknown-scarlet
cargo check -p scarlet-std-bin --target aarch64-unknown-scarlet
```

The JSON files in [user/targets](../../user/targets) ending in
`-unknown-scarlet-elf.json` are retained for the legacy `no_std` userland,
not aliases for the std-capable targets. Kernel JSON targets in
[kernel/targets](../../kernel/targets) are bare-metal targets, not application
targets. See the [userspace development map](../userspace/README.md).

## Architecture boundary

[kernel/src/arch/mod.rs](../../kernel/src/arch/mod.rs) selects and re-exports
the active implementation. Common kernel code calls `crate::arch::*`; new
architecture-specific logic belongs under `arch/riscv64/` or `arch/aarch64/`,
not in scattered `#[cfg(target_arch)]` branches in common subsystems.

Both ports provide corresponding public entry points. The implementation is
organized around `boot/`, `context.rs`, `switch.rs`, `vcpu/`, `trap/`,
`vm/`, `interrupt/`, `timer.rs`, `lsm/`, and optional `hv/`.
There is not one trait whose implementation alone completes a port. Check both
[port](../../kernel/src/arch/riscv64/mod.rs)
[exports](../../kernel/src/arch/aarch64/mod.rs) when changing a common call.

The current RISC-V MMU implementation uses
[Sv48](../../kernel/src/arch/riscv64/vm/mmu/sv48.rs); AArch64 uses
[armv8_4k](../../kernel/src/arch/aarch64/vm/mmu/armv8_4k.rs).
Both use the kernel's 4 KiB page size. CPU architecture capabilities are not
automatically implemented kernel features. In particular, AArch64 hypervisor
operation requires the project's EL2/VHE path; do not infer it from AArch64
userspace support alone.

## Boot entries, linker scripts, and device trees

Each reference project's executable lives in `projects/<project>/bsp/`:

- `src/main.rs` links the generated module aggregation and enters the arch
  boot adapter.
- `.cargo/config.toml` selects a target JSON under `kernel/targets/`.
- `build.rs` and `lds/` own the executable's link configuration.
- `scarlet.toml`, one directory above the BSP, owns images and the runner.

The active full-project linker scripts are
[RISC-V](../../projects/riscv64-limine-full/bsp/lds/riscv64_limine.ld) and
[AArch64](../../projects/aarch64-limine-full/bsp/lds/aarch64_limine.ld).
`kernel/lds/` and `kernel/tools/` also contain kernel test/legacy boot
support; they are not the project image entry points.

Both current Limine adapters consume a device tree while constructing
`BootInfo`. The AArch64 QEMU runner materializes a QEMU `virt` DTB for the
boot image. See [Limine boot](../boot/limine.md) for the actual image layouts
and [memory map](memory-map.md) for address and stack policy.

## Linux artifacts

Buildroot and optional Linux application artifacts are built on a Linux host
for the selected architecture. Their helper scripts under
[bundles/linux/tools](../../bundles/linux/tools) use `ARCH` (`riscv64` or
`aarch64`), unlike project selection above. macOS execution is rejected by
those artifact-building helpers.

The [Linux userspace artifact guide](../abi/linux/userspace-artifacts.md) and
[deployment guide](../abi/linux/deployment.md) describe toolchains and the
`bundles/linux/rootfs/system/linux-<arch>/` destinations. Building a Linux
binary does not establish that Scarlet implements every syscall it needs.

## Running and tests

`cargo make run-riscv64`, `run-aarch64`, and `run-aarch64-microvm` launch
the corresponding project through QEMU. `run-debug-*` selects debug builds;
`debug-riscv64` / `debug-aarch64` enable the runner's GDB mode.

Kernel tests use `cargo make test-riscv64` and `cargo make test-aarch64`.
These invoke [test.sh](../../kernel/tools/test.sh) or
[test_aarch64.sh](../../kernel/tools/test_aarch64.sh) under QEMU; they are not
host unit tests or full desktop validation.

A new port needs arch entry points, target configuration, memory/trap/context
support, a BSP and linker layout, device discovery, runners, and tests.
Linux artifacts or hypervisor support are separate work where required, not
automatic consequences of getting the kernel to boot.
