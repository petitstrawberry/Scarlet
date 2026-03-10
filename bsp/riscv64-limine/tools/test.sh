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
PROJECT_ROOT="$(cd "$SCRIPT_DIR" && cd .. & cd .. && cd .. && pwd)"
INITRAMFS_PATH="$PROJECT_ROOT/mkfs/dist/initramfs-riscv64.cpio"
BOOT_IMAGE="$PROJECT_ROOT/mkfs/dist/limine-riscv64-boot.img"
EFI_CODE="/usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd"
EFI_VARS="$PROJECT_ROOT/mkfs/dist/RISCV_VIRT_VARS.fd"

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
    KERNEL_ELF="$KERNEL_BINARY" sh "$PROJECT_ROOT/mkfs/make_limine_riscv64_image.sh"
fi

if [ "$DEBUG_MODE" = true ]; then
    echo "DEBUG MODE: Starting qemu with gdb server..."
    echo "Connect with: gdb $KERNEL_BINARY -ex 'target remote :12345'"
fi

# Create temporary file for capturing output
TEMP_OUTPUT=$(mktemp)

QEMU_DEBUG_ARGS=""

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

if [ ! -f "$BOOT_IMAGE" ]; then
    echo "Error: Limine boot image not found at $BOOT_IMAGE"
    exit 1
fi

if [ ! -f "$EFI_CODE" ]; then
    echo "Error: RISC-V EFI firmware not found at $EFI_CODE"
    exit 1
fi

if [ ! -f "$EFI_VARS" ]; then
    cp /usr/share/qemu-efi-riscv64/RISCV_VIRT_VARS.fd "$EFI_VARS"
fi

if [ "$DEBUG_MODE" = true ]; then
    # Debug mode: start with gdb server
    qemu-system-riscv64 \
        -machine virt,acpi=off \
        -bios default \
        -m 4G \
        -nographic \
        -serial mon:stdio \
        --no-reboot \
        -drive if=pflash,format=raw,unit=0,file="$EFI_CODE",readonly=on \
        -drive if=pflash,format=raw,unit=1,file="$EFI_VARS" \
        -global virtio-mmio.force-legacy=false \
        -drive id=boot,file="$BOOT_IMAGE",format=raw,if=none \
        -device virtio-blk-pci,drive=boot,bus=pcie.0 \
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
        -device virtio-keyboard-device,bus=virtio-mmio-bus.6 \
        -device virtio-mouse-device,bus=virtio-mmio-bus.7 \
        -netdev user,id=pci-net0 \
        -device virtio-net-pci,netdev=pci-net0,mac=52:54:00:AB:CD:EF,bus=pcie.0 \
        -gdb tcp::12345 -S \
        $QEMU_DEBUG_ARGS \
        | tee "$TEMP_OUTPUT"
else
    # Normal test mode
    qemu-system-riscv64 \
        -machine virt,acpi=off \
        -bios default \
        -m 4G \
        -nographic \
        -serial mon:stdio \
        --no-reboot \
        -drive if=pflash,format=raw,unit=0,file="$EFI_CODE",readonly=on \
        -drive if=pflash,format=raw,unit=1,file="$EFI_VARS" \
        -global virtio-mmio.force-legacy=false \
        -drive id=boot,file="$BOOT_IMAGE",format=raw,if=none \
        -device virtio-blk-pci,drive=boot,bus=pcie.0 \
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
        -device virtio-keyboard-device,bus=virtio-mmio-bus.6 \
        -device virtio-mouse-device,bus=virtio-mmio-bus.7 \
        -netdev user,id=pci-net0 \
        -device virtio-net-pci,netdev=pci-net0,mac=52:54:00:AB:CD:EF,bus=pcie.0 \
        $QEMU_DEBUG_ARGS \
        | tee "$TEMP_OUTPUT"
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
