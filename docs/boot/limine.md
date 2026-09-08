# Limine boot

The reference projects boot through UEFI and Limine on AArch64 and RISC-V 64.
`cargo-scarlet` composes the declared images; the matching
`cargo-scarlet-plugin-limine` packages the UEFI loader, kernel, configuration,
and boot inputs. The project runner owns QEMU and firmware configuration.

This describes the checked-in reference manifests, not every external BSP.
See the [build-system guide](../build-system/README.md) for composition rules.

## Reference image layouts

All paths below are relative to the selected project's `.scarlet/images/`.

| Project | Boot payload | Root filesystem and final disk |
| --- | --- | --- |
| [AArch64 full](../../projects/aarch64-limine-full/scarlet.toml) | `esp-aarch64-full.img`, a `limine-uefi` FAT image | `gpt` disk `limine-aarch64-full.img` combines the boot payload and `rootfs-aarch64-full.ext2` |
| [RISC-V full](../../projects/riscv64-limine-full/scarlet.toml) | `esp-riscv64-full.img`, a `limine-uefi` FAT image | `gpt` disk `limine-riscv64-full.img` combines the boot payload and `rootfs-riscv64-full.ext2` |
| [AArch64 microvm](../../projects/aarch64-limine-microvm/scarlet.toml) | `limine-aarch64-microvm.img`, a `limine-uefi` FAT image | Separate `rootfs-aarch64-microvm.ext2` |

There are no separate tracked `*-limine-desktop` projects. Full images select
their desktop content through bundles. A FAT boot payload and a final GPT
disk are different artifacts; do not substitute one for the other in a runner.

Both full projects explicitly declare this dependency order:

```text
initramfs (newc) ──> boot (Limine FAT / ESP) ──┐
                                             ├──> disk (GPT)
rootfs (ext2) ────────────────────────────────┘
```

The microvm project replaces `/init` with
`microvm-init`. It does not use the normal full desktop startup sequence.

## Protocol handoff

1. UEFI firmware loads the architecture's Limine EFI executable.
2. Limine reads its configuration and loads the kernel ELF and boot modules.
3. The BSP enters Scarlet's architecture-specific adapter:
   [AArch64](../../kernel/src/arch/aarch64/boot/limine.rs) or
   [RISC-V](../../kernel/src/arch/riscv64/boot/limine.rs).
4. That adapter reads Limine responses and the device tree, then constructs
   Scarlet's [BootInfo](../../kernel/src/lib.rs). Limine does **not** pass
   this Rust structure directly. The handoff includes usable memory regions,
   kernel and initramfs locations, CPU count, and the secondary-CPU release hook.
5. `start_kernel` establishes Scarlet-owned page tables and changes from
   bootloader addressing to the kernel's sparse HHDM and heap layout before
   completing device and task initialization.

Both current adapters require the device-tree handoff. A Limine response or
an HHDM offset is not permission to access every physical address. The
[memory map](../architecture/memory-map.md) describes mapped-region membership
and boot-time versus runtime address helpers.

## Build and run

From the repository root in the configured Nix shell:

```sh
# Compose images without starting QEMU.
cargo scarlet image --project projects/aarch64-limine-full --release
cargo scarlet image --project projects/riscv64-limine-full --release

# Compose and run the selected project.
cargo make run-aarch64
cargo make run-riscv64
cargo make run-aarch64-microvm
```

Use `cargo make run-debug-aarch64` or `run-debug-riscv64` for debug-profile
images. `cargo make debug-aarch64` / `debug-riscv64` build debug images and
invoke the runner's GDB mode. These commands start an emulator; image
composition alone does not.

## Boot payload contents and command line

| FAT path | Input |
| --- | --- |
| `EFI/BOOT/BOOTAA64.EFI` or `EFI/BOOT/BOOTRISCV64.EFI` | Architecture-specific Limine loader |
| `EFI/BOOT/limine.conf` | Generated Limine configuration |
| `boot/kernel` | BSP kernel ELF |
| `boot/initramfs-aarch64.cpio` or `boot/initramfs-riscv64.cpio` | Architecture-named copy of the declared initramfs input |

The manifest's image layer uses `to = "/boot/initramfs"` to select the dedicated
initramfs input. The plugin packages it under the architecture-specific name
above and points Limine's `module_path` there; the manifest destination is not
the literal final FAT filename.

The manifest's `[images.boot].cmdline` supplies the kernel command line.
It is project policy, not a universal SDK default:

- AArch64 full and RISC-V full: `console=ttyS0 root=/dev/vblk0p2 rootfstype=ext2`.
- AArch64 microvm: `console=ttyAMA0`.

The normal [init](../../user/bin/src/init.rs) interprets the root-device
settings and mounts the persistent root before handing off to `stemd`.
See [userspace startup](../userspace/README.md#startup-and-services).

## Firmware and runner ownership

The Nix shell supplies firmware paths through `SCARLET_EFI_*` environment
variables. The project runners
[AArch64 full](../../projects/aarch64-limine-full/tools/run_aarch64.sh),
[RISC-V](../../projects/riscv64-limine-full/tools/run.sh), and
[AArch64 microvm](../../projects/aarch64-limine-microvm/tools/run_aarch64.sh)
select firmware, drives, device-tree preparation, and emulator options.

Use the selected runner and manifest together. External boards may need a
different firmware/boot path; the QEMU recipe is not a hardware-support claim.
