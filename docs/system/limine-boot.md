# Limine Boot Path

Scarlet now ships a Limine-based boot workflow with **RISC-V as the primary path**.

Issue #345 now targets full boot unification around Limine. The current in-tree implementation moves the kernel toward that by:

- native Limine boot protocol on RISC-V
- DTB/FDT-driven `BootInfo` creation
- initramfs passed as a boot module
- higher-half kernel linking with HHDM-backed physical access

## Why RISC-V first

`riscv64` is Scarlet's primary architecture, and issue #345 explicitly needs the higher-half move bundled with the Limine migration.
That makes RISC-V the correct place to establish the new boot contract first.

## Build the image

```bash
cargo make build-riscv64
```

This produces the boot image and refreshes the matching initramfs/rootfs artifacts.

## Run it

```bash
cargo make run-riscv64
```

For debugging:

```bash
cargo make debug-riscv64
```

## Image layout

The FAT boot image contains:

- `EFI/BOOT/BOOTRISCV64.EFI` - Limine UEFI loader
- `EFI/BOOT/limine.conf` - Limine configuration
- `boot/kernel` - higher-half-linked RISC-V kernel ELF
- `boot/initramfs-riscv64.cpio` - initramfs module
- optional DTB override if provided

## Current scope

This path establishes Limine-native bring-up and higher-half groundwork on RISC-V.
AArch64 still has transitional pieces in-tree, but the long-term target is one unified Limine-oriented boot model across supported architectures.
