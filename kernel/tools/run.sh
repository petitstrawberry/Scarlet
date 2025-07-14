#!/bin/bash

echo Starting qemu...

# Find the project root by looking for Makefile.toml
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR" && cd .. && cd .. && pwd)"
INITRAMFS_PATH="$PROJECT_ROOT/mkfs/dist/initramfs.cpio"

# Create temporary file for capturing output
TEMP_OUTPUT=$(mktemp)

# Run QEMU and capture output
qemu-system-riscv64 \
    -machine virt \
    -bios default \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    -global virtio-mmio.force-legacy=false \
    -drive id=x0,file=test.txt,format=raw,if=none \
    -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
    -initrd "$INITRAMFS_PATH" \
    -kernel $1 | tee "$TEMP_OUTPUT"

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
else
    echo "Could not determine test result, QEMU exit code: $QEMU_EXIT_CODE"
    rm -f "$TEMP_OUTPUT"
    exit $QEMU_EXIT_CODE
fi
