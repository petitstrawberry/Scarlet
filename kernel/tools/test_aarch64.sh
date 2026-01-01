#!/bin/bash

set -o pipefail

# Test runner for Scarlet kernel (aarch64)
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
INITRAMFS_PATH="$PROJECT_ROOT/mkfs/dist/initramfs-aarch64.cpio"

echo "Test runner starting (aarch64)..."

# Ensure initramfs exists
if [ ! -f "$INITRAMFS_PATH" ]; then
    echo "Error: initramfs not found at $INITRAMFS_PATH"
    echo "Please build it explicitly before running tests. Example:"
    echo "  cargo make build-initramfs-aarch64"
    exit 1
fi

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
    echo "DEBUG MODE: Starting qemu-system-aarch64 with gdb server..."
    echo "Connect with: gdb $KERNEL_BINARY -ex 'target remote :12345'"
fi

# Create temporary file for capturing output
TEMP_OUTPUT=$(mktemp)

# U-Boot binary path (built from source, see Dockerfile)
UBOOT_BIN="/opt/u-boot-aarch64.bin"

if [ ! -f "$UBOOT_BIN" ]; then
    echo "Error: U-Boot binary not found at $UBOOT_BIN"
    echo "Please build U-Boot first (see Dockerfile for instructions)"
    exit 1
fi

# Convert ELF kernel to raw binary for U-Boot booti
# The kernel already includes Linux arm64 Image header in .head.text section
KERNEL_BIN="${KERNEL_BINARY}.bin"
echo "Converting kernel ELF to raw binary..."
aarch64-linux-gnu-objcopy -O binary "$KERNEL_BINARY" "$KERNEL_BIN"

if [ "$DEBUG_MODE" = true ]; then
    DEBUG_FLAGS="-gdb tcp::12345 -S"
else
    DEBUG_FLAGS=""
fi

# Run QEMU with U-Boot as BIOS
# Use gic-version=3 to match run_aarch64.sh configuration
# Add Virtio devices for comprehensive testing
qemu-system-aarch64 \
    -machine virt,gic-version=3 \
    -cpu cortex-a57 \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    -bios "$UBOOT_BIN" \
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
    -kernel "$KERNEL_BIN" \
    $DEBUG_FLAGS | tee "$TEMP_OUTPUT"

# Capture pipeline exit codes (qemu is element 0, tee is element 1)
QEMU_EXIT_CODE=${PIPESTATUS[0]}
TEE_EXIT_CODE=${PIPESTATUS[1]}

# In debug mode, don't check for test patterns since we're debugging
if [ "$DEBUG_MODE" = true ]; then
    echo "Debug session ended"
    rm -f "$TEMP_OUTPUT" "$KERNEL_BIN"
    exit 0
fi

# Check for test failure patterns in output
if grep -q "\[Test Runner\] Test failed" "$TEMP_OUTPUT"; then
    echo "Test failure detected in output"
    rm -f "$TEMP_OUTPUT" "$KERNEL_BIN"
    exit 1
elif grep -q "\[Test Runner\] All .* tests passed" "$TEMP_OUTPUT"; then
    echo "All tests passed"
    rm -f "$TEMP_OUTPUT" "$KERNEL_BIN"
    exit 0
elif grep -q "running 0 tests" "$TEMP_OUTPUT"; then
    echo "No tests were run"
    rm -f "$TEMP_OUTPUT" "$KERNEL_BIN"
    exit 0
else
    echo "Could not determine test result, QEMU exit code: $QEMU_EXIT_CODE (tee: $TEE_EXIT_CODE)"
    rm -f "$TEMP_OUTPUT" "$KERNEL_BIN"
    exit $QEMU_EXIT_CODE
fi