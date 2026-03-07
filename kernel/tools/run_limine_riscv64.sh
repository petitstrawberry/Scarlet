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
BOOT_IMAGE="$PROJECT_ROOT/mkfs/dist/limine-riscv64-boot.img"
ROOTFS_IMAGE="$PROJECT_ROOT/mkfs/dist/rootfs.img"
EFI_CODE="/usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd"
EFI_VARS="$PROJECT_ROOT/mkfs/dist/RISCV_VIRT_VARS.fd"

if [ ! -f "$BOOT_IMAGE" ]; then
    echo "Error: Limine boot image not found at $BOOT_IMAGE"
    exit 1
fi

if [ ! -f "$ROOTFS_IMAGE" ]; then
    echo "Error: rootfs image not found at $ROOTFS_IMAGE"
    exit 1
fi

if [ ! -f "$EFI_CODE" ]; then
    echo "Error: RISC-V EFI firmware not found at $EFI_CODE"
    exit 1
fi

if [ ! -f "$EFI_VARS" ]; then
    cp /usr/share/qemu-efi-riscv64/RISCV_VIRT_VARS.fd "$EFI_VARS"
fi

if [ "$DEBUG_MODE" = "true" ]; then
    DEBUG_FLAGS="-gdb tcp::12345 -S"
else
    DEBUG_FLAGS=""
fi

TEMP_OUTPUT=$(mktemp)

qemu-system-riscv64 \
    -machine virt \
    -cpu rv64,v=true,vlen=256 \
    -m 8G \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    -bios default \
    -drive if=pflash,format=raw,unit=0,file="$EFI_CODE",readonly=on \
    -drive if=pflash,format=raw,unit=1,file="$EFI_VARS" \
    -global virtio-mmio.force-legacy=false \
    -drive id=boot,file="$BOOT_IMAGE",format=raw,if=none \
    -device virtio-blk-device,drive=boot,bus=virtio-mmio-bus.0 \
    -drive id=rootfs,file="$ROOTFS_IMAGE",format=raw,if=none \
    -device virtio-blk-device,drive=rootfs,bus=virtio-mmio-bus.1 \
    -display vnc=:0 \
    -device virtio-gpu-device,bus=virtio-mmio-bus.2 \
    -netdev user,id=net0,hostfwd=tcp::8080-:8080,hostfwd=udp::8080-:8080,hostfwd=udp::1234-:1234 \
    -device virtio-net-device,netdev=net0,bus=virtio-mmio-bus.3 \
    -device virtio-keyboard-device,bus=virtio-mmio-bus.4 \
    -device virtio-mouse-device,bus=virtio-mmio-bus.5 \
    -device virtio-rng-device,bus=virtio-mmio-bus.6 \
    $DEBUG_FLAGS | tee "$TEMP_OUTPUT"

QEMU_EXIT_CODE=${PIPESTATUS[0]}

if [ "$DEBUG_MODE" = "true" ]; then
    rm -f "$TEMP_OUTPUT"
    exit 0
fi

if grep -q "\[Test Runner\] Test failed" "$TEMP_OUTPUT"; then
    rm -f "$TEMP_OUTPUT"
    exit 1
elif grep -q "\[Test Runner\] All .* tests passed" "$TEMP_OUTPUT"; then
    rm -f "$TEMP_OUTPUT"
    exit 0
fi

rm -f "$TEMP_OUTPUT"
exit "$QEMU_EXIT_CODE"
