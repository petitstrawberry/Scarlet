#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE=debug
if [[ "${SCARLET_RELEASE:-0}" == 1 ]]; then
    PROFILE=release
fi

KERNEL="$PROJECT_DIR/bsp/target/riscv32ima-unknown-none-elf/$PROFILE/scarlet"
INITRAMFS="$PROJECT_DIR/.scarlet/images/initramfs-riscv32-console.cpio"
for artifact in "$KERNEL" "$INITRAMFS"; do
    if [[ ! -f "$artifact" ]]; then
        echo "Missing artifact: $artifact (run cargo scarlet image for this project)" >&2
        exit 1
    fi
done

args=(
    -machine "${SCARLET_QEMU_MACHINE_RV32:-virt,acpi=off}"
    -cpu "${SCARLET_QEMU_CPU_RV32:-rv32}"
    -m "${SCARLET_QEMU_MEMORY:-512M}"
    -smp "${SCARLET_QEMU_SMP:-4}"
    -display none
    -serial "${SCARLET_QEMU_SERIAL:-mon:stdio}"
    -no-reboot
    -kernel "$KERNEL"
    -initrd "$INITRAMFS"
    -append "${SCARLET_QEMU_CMDLINE:-console=ttyS0 init.exec=/bin/login}"
)

# QEMU supplies its default OpenSBI. A matching IMA-only firmware and CPU can
# also be selected explicitly without changing the project or image recipes.
if [[ -n "${SCARLET_QEMU_BIOS_RV32:-}" ]]; then
    args+=(-bios "$SCARLET_QEMU_BIOS_RV32")
fi
if [[ "${SCARLET_DEBUG_MODE:-false}" == true ]]; then
    args+=(-gdb "${SCARLET_QEMU_GDB:-tcp::12345}" -S)
fi

exec qemu-system-riscv32 "${args[@]}" "$@"
