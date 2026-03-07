#!/bin/bash

set -o pipefail

DEBUG_MODE=${SCARLET_DEBUG_MODE:-false}

while [[ $# -gt 0 ]]; do
    case $1 in
        --debug)
            DEBUG_MODE=true
            shift
            ;;
        *)
            shift
            ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR" && cd .. && cd .. && pwd)"
BOOT_IMAGE="$PROJECT_ROOT/mkfs/dist/limine-aarch64-boot.img"
ROOTFS_IMAGE="$PROJECT_ROOT/mkfs/dist/rootfs.img"

find_efi_firmware() {
    local candidate
    for candidate in \
        /usr/share/qemu-efi-aarch64/QEMU_EFI.fd \
        /usr/share/AAVMF/AAVMF_CODE.fd \
        /usr/share/AAVMF/AAVMF32_CODE.fd \
        /usr/share/edk2/aarch64/QEMU_EFI.fd; do
        if [ -f "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    return 1
}

if [ ! -f "$BOOT_IMAGE" ]; then
    echo "Error: Limine boot image not found at $BOOT_IMAGE"
    echo "Please build it first with: cargo make build-limine-aarch64"
    exit 1
fi

if [ ! -f "$ROOTFS_IMAGE" ]; then
    echo "Error: rootfs image not found at $ROOTFS_IMAGE"
    echo "Please build it first with: cargo make build-rootfs-aarch64"
    exit 1
fi

EFI_FIRMWARE="$(find_efi_firmware || true)"
if [ -z "$EFI_FIRMWARE" ]; then
    echo "Error: AArch64 EFI firmware not found."
    echo "Install qemu-efi-aarch64 or use the scarlet-dev container image after rebuilding it."
    exit 1
fi

if [ "$DEBUG_MODE" = "true" ]; then
    DEBUG_FLAGS="-gdb tcp::12345 -S"
else
    DEBUG_FLAGS=""
fi

TEMP_OUTPUT=$(mktemp)

qemu-system-aarch64 \
    -machine virt,gic-version=3 \
    -cpu cortex-a57 \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    -bios "$EFI_FIRMWARE" \
    -global virtio-mmio.force-legacy=false \
    -drive id=boot,file="$BOOT_IMAGE",format=raw,if=none \
    -device virtio-blk-device,drive=boot,bus=virtio-mmio-bus.0 \
    -drive id=rootfs,file="$ROOTFS_IMAGE",format=raw,if=none \
    -device virtio-blk-device,drive=rootfs,bus=virtio-mmio-bus.1 \
    -display vnc=:0 \
    -device virtio-gpu-device,bus=virtio-mmio-bus.2 \
    -netdev user,id=net0 \
    -device virtio-net-device,netdev=net0,bus=virtio-mmio-bus.3 \
    -device virtio-keyboard-device,bus=virtio-mmio-bus.4 \
    -device virtio-mouse-device,bus=virtio-mmio-bus.5 \
    -device virtio-rng-device,bus=virtio-mmio-bus.6 \
    $DEBUG_FLAGS | tee "$TEMP_OUTPUT"

QEMU_EXIT_CODE=${PIPESTATUS[0]}

if [ "$DEBUG_MODE" = "true" ]; then
    echo "Debug session ended"
    rm -f "$TEMP_OUTPUT"
    exit 0
fi

rm -f "$TEMP_OUTPUT"
exit "$QEMU_EXIT_CODE"
