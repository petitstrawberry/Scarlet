# Limine Boot

Scarlet uses the [Limine](https://limine-bootloader.org/) boot protocol via UEFI on both supported architectures. The boot flow is unified: `cargo-scarlet` builds a FAT image containing the Limine UEFI loader, the kernel ELF, and an initramfs, then launches it under QEMU.

## Supported Architectures

| Architecture | Project | Boot image |
|---|---|---|
| RISC-V 64-bit | `projects/riscv64-limine-full` | `limine-riscv64-full.img` |
| RISC-V 64-bit (desktop) | `projects/riscv64-limine-desktop` | `limine-riscv64-desktop.img` |
| AArch64 | `projects/aarch64-limine-full` | `limine-aarch64-full.img` |
| AArch64 (desktop) | `projects/aarch64-limine-desktop` | `limine-aarch64-desktop.img` |
| AArch64 (microvm) | `projects/aarch64-limine-microvm` | `limine-aarch64-microvm.img` |

All projects use `format = "limine-uefi"` in their `scarlet.toml`.

## Boot Protocol

Limine negotiates a higher-half boot with the kernel via the Limine boot protocol. On boot:

1. UEFI firmware loads the Limine UEFI bootloader from the FAT image.
2. Limine reads `limine.conf`, loads the kernel ELF, and applies relocations for higher-half placement.
3. Limine passes a `BootInfo` structure containing memory map, framebuffer info, HHDM base address, and boot modules (initramfs).
4. The kernel entry point runs in the higher-half with full HHDM-backed physical memory access.

On RISC-V, a Device Tree Blob (DTB) is also passed and used to create the `BootInfo`. On AArch64, Limine provides the same information via its protocol.

## Build and Run

```bash
# RISC-V
cargo make run-riscv64

# AArch64
cargo make run-aarch64

# AArch64 microvm
cargo make run-aarch64-microvm
```

These commands build the kernel, generate the initramfs and rootfs images, assemble the FAT boot image, and launch QEMU — all in one step.

For debug builds:

```bash
cargo make run-debug-riscv64
cargo make run-debug-aarch64
```

For GDB debugging:

```bash
cargo make debug-riscv64
```

## Image Layout

The FAT boot image contains:

| Path | Description |
|---|---|
| `EFI/BOOT/BOOTRISCV64.EFI` | Limine UEFI loader (RISC-V) |
| `EFI/BOOT/BOOTAA64.EFI` | Limine UEFI loader (AArch64) |
| `EFI/BOOT/limine.conf` | Limine configuration |
| `boot/kernel` | Higher-half-linked kernel ELF |
| `boot/initramfs` | Initramfs CPIO archive |

The kernel command line is configured in `scarlet.toml` under `[images.boot].cmdline` (default: `console=ttyS0 root=/dev/vblk1`).

## UEFI Firmware

The Nix development shell provides the required UEFI firmware for QEMU via `SCARLET_EFI_*` environment variables. The runner script (`tools/run.sh`) uses these to point QEMU at the correct firmware files.

For non-Nix setups, ensure the appropriate OVMF/EDK2 firmware is available for your target architecture.
