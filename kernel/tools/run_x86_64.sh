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
            # This should be the kernel binary path
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
        # For release mode or default run
        if [ -f "$KERNEL_DIR/target/x86_64-unknown-none-elf/release/kernel" ]; then
            KERNEL_PATH="$KERNEL_DIR/target/x86_64-unknown-none-elf/release/kernel"
        else
            KERNEL_PATH="$KERNEL_DIR/target/x86_64-unknown-none-elf/debug/kernel"
        fi
    fi
fi

if [ "$DEBUG_MODE" = "true" ]; then
    echo "Starting qemu-system-x86_64 in debug mode with gdb server..."
    DEBUG_FLAGS="-gdb tcp::12345 -S"
else
    echo "Starting qemu-system-x86_64..."
    DEBUG_FLAGS=""
fi

# Find the project root by looking for Makefile.toml
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR" && cd .. && cd .. && pwd)"
INITRAMFS_PATH="$PROJECT_ROOT/mkfs/dist/initramfs-x86_64.cpio"

QEMU_DEBUG_ARGS=""

# Optional QEMU debug logging
QEMU_DEBUG_FLAGS=""
if [ -n "${SCARLET_QEMU_DEBUG_FLAGS:-}" ]; then
    QEMU_DEBUG_FLAGS="${SCARLET_QEMU_DEBUG_FLAGS}"
elif [ "${SCARLET_QEMU_GUEST_ERRORS:-0}" = "1" ] || [ "${SCARLET_QEMU_GUEST_ERRORS:-}" = "true" ]; then
    QEMU_DEBUG_FLAGS="guest_errors"
fi

if [ -n "$QEMU_DEBUG_FLAGS" ]; then
    if [ "$QEMU_DEBUG_FLAGS" = "guest_errors" ]; then
        QEMU_DEBUG_LOG="${SCARLET_QEMU_GUEST_ERRORS_LOG:-$PROJECT_ROOT/qemu-guest-errors-x86_64.log}"
    else
        QEMU_DEBUG_LOG="${SCARLET_QEMU_DEBUG_LOG:-$PROJECT_ROOT/qemu-debug-x86_64.log}"
    fi
    echo "QEMU debug logging enabled (-d $QEMU_DEBUG_FLAGS): $QEMU_DEBUG_LOG"
    QEMU_DEBUG_ARGS="-d $QEMU_DEBUG_FLAGS -D $QEMU_DEBUG_LOG"
fi

CMDLINE_ARGS=()
if [ -n "${SCARLET_CMDLINE:-}" ]; then
    CMDLINE_ARGS=(-append "${SCARLET_CMDLINE}")
fi

# Ensure initramfs exists
if [ ! -f "$INITRAMFS_PATH" ]; then
    echo "Error: initramfs not found at $INITRAMFS_PATH"
    echo "Please build it explicitly before running QEMU. Example:"
    echo "  cargo make build-initramfs-x86_64"
    echo "  cargo make build-initramfs-release-x86_64"
    exit 1
fi

# Create temporary file for capturing output
TEMP_OUTPUT=$(mktemp)

# Convert ELF kernel to raw binary if needed (multiboot header support)
KERNEL_BIN="${KERNEL_PATH}"

# Run QEMU for x86_64
# Using PC machine type with standard x86_64 configuration
qemu-system-x86_64 \
    -machine pc,q35=true \
    -cpu qemu64 \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    -kernel "$KERNEL_BIN" \
    -initrd "$INITRAMFS_PATH" \
    -drive id=x0,file="$PROJECT_ROOT/mkfs/dist/rootfs.img",format=raw,if=none \
    -device ide-hd,drive=x0 \
    -device virtio-gpu-pci \
    -netdev user,id=net0 \
    -device virtio-net-pci,netdev=net0 \
    -device virtio-keyboard-pci \
    -device virtio-mouse-pci \
    -device virtio-rng-pci \
    "${CMDLINE_ARGS[@]}" \
    $QEMU_DEBUG_ARGS \
    $DEBUG_FLAGS | tee "$TEMP_OUTPUT"

# Capture pipeline exit codes
QEMU_EXIT_CODE=${PIPESTATUS[0]}
TEE_EXIT_CODE=${PIPESTATUS[1]}

# In debug mode, don't check for test patterns
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
    echo "Could not determine test result, QEMU exit code: $QEMU_EXIT_CODE (tee: $TEE_EXIT_CODE)"
    rm -f "$TEMP_OUTPUT"
    exit $QEMU_EXIT_CODE
fi
