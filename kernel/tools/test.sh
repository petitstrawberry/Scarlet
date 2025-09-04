#!/bin/bash

# Test runner for Scarlet kernel
# This script is called by cargo test and can also be used for debugging tests

# Default values
DEBUG_MODE=false
KERNEL_BINARY=""

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --debug)
            DEBUG_MODE=true
            shift
            ;;
        *)
            # This should be the kernel binary path from cargo test
            KERNEL_BINARY="$1"
            shift
            ;;
    esac
done

# Find the project root by looking for Makefile.toml
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR" && cd .. && cd .. && pwd)"
INITRAMFS_PATH="$PROJECT_ROOT/mkfs/dist/initramfs.cpio"

echo "Test runner starting..."

# Generate fresh FAT32 test image before each test run
echo "Generating fresh FAT32 test image..."
KERNEL_DIR="$(dirname "$SCRIPT_DIR")"
"$KERNEL_DIR/tools/create-fat32-image.sh"
if [ $? -ne 0 ]; then
    echo "Error: Failed to create FAT32 test image"
    exit 1
fi

# Generate fresh ext2 test image before each test run
echo "Generating fresh ext2 test image..."
"$KERNEL_DIR/tools/create-ext2-image.sh"
if [ $? -ne 0 ]; then
    echo "Error: Failed to create ext2 test image"
    exit 1
fi

# Create symbolic link for VSCode debugging
if [ -n "$KERNEL_BINARY" ]; then
    LINK_PATH="$(dirname "$KERNEL_BINARY")/../test-kernel"
    ln -sf "$KERNEL_BINARY" "$LINK_PATH"
    echo "Created symbolic link: $LINK_PATH -> $KERNEL_BINARY"
fi

if [ "$DEBUG_MODE" = true ]; then
    echo "DEBUG MODE: Starting qemu with gdb server..."
    echo "Connect with: gdb $KERNEL_BINARY -ex 'target remote :12345'"
fi

# Create temporary file for capturing output
TEMP_OUTPUT=$(mktemp)

if [ "$DEBUG_MODE" = true ]; then
    # Debug mode: start with gdb server
    qemu-system-riscv64 \
        -machine virt \
        -bios default \
        -m 2G \
        -nographic \
        -serial mon:stdio \
        --no-reboot \
        -global virtio-mmio.force-legacy=false \
        -drive id=x0,file="$KERNEL_DIR/fat32-test.img",format=raw,if=none \
        -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
        -drive id=x1,file="$KERNEL_DIR/ext2-test.img",format=raw,if=none \
        -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.5 \
        -display vnc=:0 \
        -device virtio-gpu-device,bus=virtio-mmio-bus.1 \
        -netdev user,id=net0 \
        -netdev hubport,id=net1,hubid=0 \
        -netdev hubport,id=net2,hubid=0 \
        -device virtio-net-device,netdev=net0,mac=52:54:00:12:34:56,bus=virtio-mmio-bus.2 \
        -device virtio-net-device,netdev=net1,mac=52:54:00:12:34:57,bus=virtio-mmio-bus.3 \
        -device virtio-net-device,netdev=net2,mac=52:54:00:12:34:58,bus=virtio-mmio-bus.4 \
        -initrd "$INITRAMFS_PATH" \
        -gdb tcp::12345 -S \
        -kernel "$KERNEL_BINARY" | tee "$TEMP_OUTPUT"
else
    # Normal test mode
    qemu-system-riscv64 \
        -machine virt \
        -bios default \
        -m 2G \
        -nographic \
        -serial mon:stdio \
        --no-reboot \
        -global virtio-mmio.force-legacy=false \
        -drive id=x0,file="$KERNEL_DIR/fat32-test.img",format=raw,if=none \
        -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
        -drive id=x1,file="$KERNEL_DIR/ext2-test.img",format=raw,if=none \
        -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.5 \
        -display vnc=:0 \
        -device virtio-gpu-device,bus=virtio-mmio-bus.1 \
        -netdev user,id=net0 \
        -netdev hubport,id=net1,hubid=0 \
        -netdev hubport,id=net2,hubid=0 \
        -device virtio-net-device,netdev=net0,mac=52:54:00:12:34:56,bus=virtio-mmio-bus.2 \
        -device virtio-net-device,netdev=net1,mac=52:54:00:12:34:57,bus=virtio-mmio-bus.3 \
        -device virtio-net-device,netdev=net2,mac=52:54:00:12:34:58,bus=virtio-mmio-bus.4 \
        -initrd "$INITRAMFS_PATH" \
        -kernel "$KERNEL_BINARY" | tee "$TEMP_OUTPUT"
fi

# Capture QEMU exit code
QEMU_EXIT_CODE=$?

# Check for test failure patterns in output
if grep -q "\[Test Runner\] Test failed" "$TEMP_OUTPUT"; then
    echo "Test failure detected in output"
    rm -f "$TEMP_OUTPUT"
    exit 1
elif grep -q "\[Test Runner\] All .* tests passed" "$TEMP_OUTPUT"; then
    echo "All tests passed"
    rm -f "$TEMP_OUTPUT"
    exit 0
elif grep -q "running 0 tests" "$TEMP_OUTPUT"; then
    echo "No tests were run"
    rm -f "$TEMP_OUTPUT"
    exit 0
else
    echo "Could not determine test result, QEMU exit code: $QEMU_EXIT_CODE"
    rm -f "$TEMP_OUTPUT"
    exit $QEMU_EXIT_CODE
fi
