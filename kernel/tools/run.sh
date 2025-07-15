#!/bin/bash

#!/bin/bash

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
            # This should be the kernel binary path
            KERNEL_PATH="$1"
            shift
            ;;
    esac
done

if [ "$DEBUG_MODE" = "true" ]; then
    echo "Starting qemu in debug mode with gdb server..."
    DEBUG_FLAGS="-gdb tcp::12345 -S"
else
    echo "Starting qemu..."
    DEBUG_FLAGS=""
fi

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
    -device virtio-gpu-device,bus=virtio-mmio-bus.1 \
    -device virtio-net-device,bus=virtio-mmio-bus.2 \
    -net user \
    -vnc :0 \
    $DEBUG_FLAGS \
    -initrd "$INITRAMFS_PATH" \
    -kernel "$KERNEL_PATH" | tee "$TEMP_OUTPUT"

# Capture QEMU exit code
QEMU_EXIT_CODE=$?

# In debug mode, don't check for test patterns since we're debugging
if [ "$DEBUG_MODE" = "true" ]; then
    echo "Debug session ended"
    rm -f "$TEMP_OUTPUT"
    exit 0
fi

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
