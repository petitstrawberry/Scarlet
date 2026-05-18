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
        KERNEL_PATH="$KERNEL_DIR/target/riscv64gc-unknown-none-elf/debug/scarlet"
    else
        # For release mode or default run
        if [ -f "$KERNEL_DIR/target/riscv64gc-unknown-none-elf/release/scarlet" ]; then
            KERNEL_PATH="$KERNEL_DIR/target/riscv64gc-unknown-none-elf/release/scarlet"
        else
            KERNEL_PATH="$KERNEL_DIR/target/riscv64gc-unknown-none-elf/debug/scarlet"
        fi
    fi
fi

if [ -n "$KERNEL_PATH" ] && [ -f "$KERNEL_PATH" ]; then
    KERNEL_PATH="$(realpath "$KERNEL_PATH")"
fi

if [ "$DEBUG_MODE" = "true" ]; then
    echo "Starting qemu in debug mode with gdb server..."
    DEBUG_FLAGS="-gdb tcp::12345 -S"
else
    echo "Starting qemu..."
    DEBUG_FLAGS=""
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR" && cd .. && cd .. && cd .. && pwd)"
BOOT_IMAGE="$PROJECT_ROOT/mkfs/dist/limine-riscv64-boot.img"
ROOTFS_IMAGE="$PROJECT_ROOT/mkfs/dist/rootfs.img"
EFI_CODE="/usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd"
EFI_VARS_TEMPLATE="/usr/share/qemu-efi-riscv64/RISCV_VIRT_VARS.fd"
EFI_VARS_PERSISTENT="$PROJECT_ROOT/mkfs/dist/RISCV_VIRT_VARS.fd"

QEMU_DEBUG_ARGS=""
QEMU_SMP="${SCARLET_QEMU_SMP:-1}"

# Optional QEMU debug logging
# - Enable guest errors: SCARLET_QEMU_GUEST_ERRORS=1
# - Or pass explicit QEMU -d flags: SCARLET_QEMU_DEBUG_FLAGS=virtio (comma-separated)
QEMU_DEBUG_FLAGS=""
if [ -n "${SCARLET_QEMU_DEBUG_FLAGS:-}" ]; then
    QEMU_DEBUG_FLAGS="${SCARLET_QEMU_DEBUG_FLAGS}"
elif [ "${SCARLET_QEMU_GUEST_ERRORS:-0}" = "1" ] || [ "${SCARLET_QEMU_GUEST_ERRORS:-}" = "true" ]; then
    QEMU_DEBUG_FLAGS="guest_errors"
fi

if [ -n "$QEMU_DEBUG_FLAGS" ]; then
    if [ "$QEMU_DEBUG_FLAGS" = "guest_errors" ]; then
        QEMU_DEBUG_LOG="${SCARLET_QEMU_GUEST_ERRORS_LOG:-$PROJECT_ROOT/qemu-guest-errors-riscv64.log}"
    else
        QEMU_DEBUG_LOG="${SCARLET_QEMU_DEBUG_LOG:-$PROJECT_ROOT/qemu-debug-riscv64.log}"
    fi
    echo "QEMU debug logging enabled (-d $QEMU_DEBUG_FLAGS): $QEMU_DEBUG_LOG"
    QEMU_DEBUG_ARGS="-d $QEMU_DEBUG_FLAGS -D $QEMU_DEBUG_LOG"
fi

TEMP_OUTPUT=$(mktemp)

if [ ! -f "$BOOT_IMAGE" ]; then
    echo "Error: Limine boot image not found at $BOOT_IMAGE"
    exit 1
fi

if [ ! -f "$ROOTFS_IMAGE" ]; then
    echo "Error: rootfs image not found at $ROOTFS_IMAGE"
    exit 1
fi

if [ ! -f "$KERNEL_PATH" ]; then
    echo "Error: kernel binary not found at $KERNEL_PATH"
    exit 1
fi

echo "Rebuilding Limine RISC-V boot image from $KERNEL_PATH"
if ! KERNEL_ELF="$KERNEL_PATH" sh "$PROJECT_ROOT/mkfs/make_limine_riscv64_image.sh"; then
    echo "Error: failed to rebuild Limine RISC-V boot image"
    exit 1
fi

if [ ! -f "$EFI_CODE" ]; then
    echo "Error: RISC-V EFI firmware not found at $EFI_CODE"
    exit 1
fi

if [ ! -f "$EFI_VARS_TEMPLATE" ]; then
    echo "Error: RISC-V EFI VARS template not found at $EFI_VARS_TEMPLATE"
    exit 1
fi

if [ "${SCARLET_EFI_VARS_PERSIST:-0}" = "1" ] || [ "${SCARLET_EFI_VARS_PERSIST:-}" = "true" ]; then
    EFI_VARS_RUNTIME="$EFI_VARS_PERSISTENT"
    if [ ! -f "$EFI_VARS_RUNTIME" ]; then
        cp "$EFI_VARS_TEMPLATE" "$EFI_VARS_RUNTIME"
    fi
else
    EFI_VARS_RUNTIME="$(mktemp "$PROJECT_ROOT/mkfs/dist/RISCV_VIRT_VARS.run.XXXXXX.fd")"
    cp "$EFI_VARS_TEMPLATE" "$EFI_VARS_RUNTIME"
    trap 'rm -f "$EFI_VARS_RUNTIME"' EXIT
fi

qemu-system-riscv64 \
    -machine virt,acpi=off \
    -m 8G \
    -smp "$QEMU_SMP" \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    -bios default \
    -drive if=pflash,format=raw,unit=0,file="$EFI_CODE",readonly=on \
    -drive if=pflash,format=raw,unit=1,file="$EFI_VARS_RUNTIME" \
    -global virtio-mmio.force-legacy=false \
    -drive id=boot,file="$BOOT_IMAGE",format=raw,if=none \
    -device virtio-blk-pci,drive=boot,bus=pcie.0 \
    -drive id=rootfs,file="$ROOTFS_IMAGE",format=raw,if=none \
    -device virtio-blk-device,drive=rootfs,bus=virtio-mmio-bus.0 \
    -display vnc=:0 \
    -device virtio-gpu-device,bus=virtio-mmio-bus.1 \
    -netdev user,id=net0,hostfwd=tcp::8080-:8080,hostfwd=udp::8080-:8080,hostfwd=udp::1234-:1234 \
    -device virtio-net-device,netdev=net0,bus=virtio-mmio-bus.2 \
    -device virtio-keyboard-device,bus=virtio-mmio-bus.3 \
    -device virtio-mouse-device,bus=virtio-mmio-bus.4 \
    -device virtio-rng-device,bus=virtio-mmio-bus.5 \
    $QEMU_DEBUG_ARGS \
    $DEBUG_FLAGS | tee "$TEMP_OUTPUT"

QEMU_EXIT_CODE=${PIPESTATUS[0]}

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
