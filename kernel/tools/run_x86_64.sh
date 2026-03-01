#!/bin/bash

set -o pipefail

# Check for debug mode environment variable or command line argument
DEBUG_MODE=${SCARLET_DEBUG_MODE:-false}
KERNEL_PATH=""

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --debug)
            DEBUG_MODE=true
            shift
            ;;
        *)
            KERNEL_PATH="$1"
            shift
            ;;
    esac
done

# If no kernel path provided, try to find the default build
if [ -z "$KERNEL_PATH" ]; then
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    KERNEL_DIR="$(dirname "$SCRIPT_DIR")"

    if [ "$DEBUG_MODE" = "true" ]; then
        KERNEL_PATH="$KERNEL_DIR/target/x86_64-unknown-none-elf/debug/kernel"
    else
        if [ -f "$KERNEL_DIR/target/x86_64-unknown-none-elf/release/kernel" ]; then
            KERNEL_PATH="$KERNEL_DIR/target/x86_64-unknown-none-elf/release/kernel"
        else
            KERNEL_PATH="$KERNEL_DIR/target/x86_64-unknown-none-elf/debug/kernel"
        fi
    fi
fi

if [ "$DEBUG_MODE" = "true" ]; then
    echo "Starting qemu-system-x86_64 in debug mode..."
    DEBUG_FLAGS="-gdb tcp::12345 -S"
else
    echo "Starting qemu-system-x86_64..."
    DEBUG_FLAGS=""
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR" && cd .. && cd .. && pwd)"
INITRAMFS_PATH="$PROJECT_ROOT/mkfs/dist/initramfs-x86_64.cpio"
DISK_IMAGE="$PROJECT_ROOT/mkfs/dist/uefi-x86_64/scarlet-x86_64.img"

if [ ! -f "$INITRAMFS_PATH" ]; then
    echo "Error: initramfs not found at $INITRAMFS_PATH"
    echo "Please build it first: cargo make build-initramfs-x86_64"
    exit 1
fi

echo "Building UEFI disk image..."
"$SCRIPT_DIR/build_uefi_image_x86_64.sh" "$KERNEL_PATH" || {
    echo "Error: Failed to build UEFI disk image"
    exit 1
}

OVMF_CODE=""
if [ -f "/usr/share/OVMF/OVMF_CODE_4M.fd" ]; then
    OVMF_CODE="/usr/share/OVMF/OVMF_CODE_4M.fd"
elif [ -f "/usr/share/OVMF/OVMF_CODE.fd" ]; then
    OVMF_CODE="/usr/share/OVMF/OVMF_CODE.fd"
elif [ -f "/opt/share/OVMF/OVMF_CODE_4M.fd" ]; then
    OVMF_CODE="/opt/share/OVMF/OVMF_CODE_4M.fd"
else
    echo "Error: OVMF firmware not found"
    exit 1
fi

TEMP_OUTPUT=$(mktemp)

qemu-system-x86_64 \
    -machine q35 \
    -cpu Haswell \
    -m 512M \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive file="$DISK_IMAGE",format=raw,if=none,id=boot \
    -device virtio-blk-pci,drive=boot \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    $DEBUG_FLAGS | tee "$TEMP_OUTPUT"

QEMU_EXIT_CODE=${PIPESTATUS[0]}
TEE_EXIT_CODE=${PIPESTATUS[1]}

if [ "$DEBUG_MODE" = "true" ]; then
    echo "Debug session ended"
    rm -f "$TEMP_OUTPUT"
    exit 0
fi

if grep -q "\[Test Runner\] Test failed" "$TEMP_OUTPUT"; then
    echo "Test failure detected"
    rm -f "$TEMP_OUTPUT"
    exit 1
elif grep -q "\[Test Runner\] All .* tests passed" "$TEMP_OUTPUT"; then
    echo "All tests passed"
    rm -f "$TEMP_OUTPUT"
    exit 0
else
    echo "QEMU exit code: $QEMU_EXIT_CODE"
    rm -f "$TEMP_OUTPUT"
    exit $QEMU_EXIT_CODE
fi
