#!/bin/bash

set -o pipefail

DEBUG_MODE=false
KERNEL_BINARY=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --debug)
            DEBUG_MODE=true
            shift
            ;;
        *)
            KERNEL_BINARY="$1"
            shift
            ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR" && cd .. && cd .. && pwd)"
BOOT_IMAGE="$PROJECT_ROOT/mkfs/dist/limine-aarch64-boot.img"

echo "Test runner starting (aarch64)..."

echo "Generating fresh FAT32 test image..."
KERNEL_DIR="$(dirname "$SCRIPT_DIR")"
"$KERNEL_DIR/tools/create-fat32-image.sh"
if [ $? -ne 0 ]; then
    echo "Error: Failed to create FAT32 test image"
    exit 1
fi

echo "Generating fresh ext2 test image..."
"$KERNEL_DIR/tools/create-ext2-image.sh"
if [ $? -ne 0 ]; then
    echo "Error: Failed to create ext2 test image"
    exit 1
fi

if [ -n "$KERNEL_BINARY" ]; then
    LINK_PATH="$(dirname "$KERNEL_BINARY")/../test-kernel"
    ln -sf "$KERNEL_BINARY" "$LINK_PATH"
    echo "Created symbolic link: $LINK_PATH -> $KERNEL_BINARY"
    KERNEL_ELF="$KERNEL_BINARY" sh "$PROJECT_ROOT/mkfs/make_limine_aarch64_image.sh"
fi

if [ "$DEBUG_MODE" = true ]; then
    echo "DEBUG MODE: Starting qemu-system-aarch64 with gdb server..."
    echo "Connect with: gdb $KERNEL_BINARY -ex 'target remote :12345'"
fi

TEMP_OUTPUT=$(mktemp)

if [ "$DEBUG_MODE" = true ]; then
    DEBUG_FLAGS="-gdb tcp::12345 -S"
else
    DEBUG_FLAGS=""
fi

QEMU_DEBUG_ARGS=""
QEMU_DEBUG_FLAGS=""
if [ -n "${SCARLET_QEMU_DEBUG_FLAGS:-}" ]; then
    QEMU_DEBUG_FLAGS="${SCARLET_QEMU_DEBUG_FLAGS}"
elif [ "${SCARLET_QEMU_GUEST_ERRORS:-0}" = "1" ] || [ "${SCARLET_QEMU_GUEST_ERRORS:-}" = "true" ]; then
    QEMU_DEBUG_FLAGS="guest_errors"
fi

if [ -n "$QEMU_DEBUG_FLAGS" ]; then
    if [ "$QEMU_DEBUG_FLAGS" = "guest_errors" ]; then
        QEMU_DEBUG_LOG="${SCARLET_QEMU_GUEST_ERRORS_LOG:-$PROJECT_ROOT/qemu-guest-errors-aarch64.log}"
    else
        QEMU_DEBUG_LOG="${SCARLET_QEMU_DEBUG_LOG:-$PROJECT_ROOT/qemu-debug-aarch64.log}"
    fi
    echo "QEMU debug logging enabled (-d $QEMU_DEBUG_FLAGS): $QEMU_DEBUG_LOG"
    QEMU_DEBUG_ARGS="-d $QEMU_DEBUG_FLAGS -D $QEMU_DEBUG_LOG"
fi

if [ ! -f "$BOOT_IMAGE" ]; then
    echo "Error: Limine boot image not found at $BOOT_IMAGE"
    exit 1
fi

find_efi_code() {
    local candidate
    for candidate in \
        /usr/share/AAVMF/AAVMF_CODE.fd \
        /usr/share/AAVMF/AAVMF_CODE.no-secboot.fd \
        /usr/share/qemu-efi-aarch64/QEMU_EFI.fd; do
        if [ -f "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    return 1
}

find_efi_vars_template() {
    local candidate
    for candidate in \
        /usr/share/AAVMF/AAVMF_VARS.fd \
        /usr/share/AAVMF/AAVMF_VARS.ms.fd \
        /usr/share/AAVMF/AAVMF_VARS.snakeoil.fd; do
        if [ -f "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    return 1
}

EFI_CODE="$(find_efi_code || true)"
EFI_VARS_TEMPLATE="$(find_efi_vars_template || true)"
EFI_VARS_RUNTIME="$(mktemp "$PROJECT_ROOT/mkfs/dist/AAVMF_VARS.run.XXXXXX.fd")"

if [ -z "$EFI_CODE" ]; then
    echo "Error: AArch64 EFI firmware code image not found."
    exit 1
fi

if [ -z "$EFI_VARS_TEMPLATE" ]; then
    echo "Error: AArch64 EFI vars template not found."
    exit 1
fi

cp "$EFI_VARS_TEMPLATE" "$EFI_VARS_RUNTIME"
trap 'rm -f "$EFI_VARS_RUNTIME" "$TEMP_OUTPUT"' EXIT

qemu-system-aarch64 \
    -machine virt,gic-version=3,acpi=off \
    -cpu max \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    -drive if=pflash,format=raw,unit=0,file="$EFI_CODE",readonly=on \
    -drive if=pflash,format=raw,unit=1,file="$EFI_VARS_RUNTIME" \
    -global virtio-mmio.force-legacy=false \
    -drive id=boot,file="$BOOT_IMAGE",format=raw,if=none \
    -device virtio-blk-device,drive=boot,bus=virtio-mmio-bus.0 \
    -drive id=x0,file="$KERNEL_DIR/fat32-test.img",format=raw,if=none \
    -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.1 \
    -drive id=x1,file="$KERNEL_DIR/ext2-test.img",format=raw,if=none \
    -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.2 \
    -display none \
    -device virtio-gpu-device,bus=virtio-mmio-bus.3 \
    -netdev user,id=net0 \
    -netdev hubport,id=net1,hubid=0 \
    -netdev hubport,id=net2,hubid=0 \
    -device virtio-net-device,netdev=net0,mac=52:54:00:12:34:56,bus=virtio-mmio-bus.4 \
    -device virtio-net-device,netdev=net1,mac=52:54:00:12:34:57,bus=virtio-mmio-bus.5 \
    -device virtio-net-device,netdev=net2,mac=52:54:00:12:34:58,bus=virtio-mmio-bus.6 \
    -device virtio-keyboard-device,bus=virtio-mmio-bus.7 \
    -device virtio-mouse-device,bus=virtio-mmio-bus.8 \
    $QEMU_DEBUG_ARGS \
    $DEBUG_FLAGS | tee "$TEMP_OUTPUT"

QEMU_EXIT_CODE=${PIPESTATUS[0]}
TEE_EXIT_CODE=${PIPESTATUS[1]}

if [ "$DEBUG_MODE" = true ]; then
    echo "Debug session ended"
    exit 0
fi

if grep -q "\[Test Runner\] Test failed" "$TEMP_OUTPUT"; then
    echo "Test failure detected in output"
    exit 1
elif grep -q "\[Test Runner\] All .* tests passed" "$TEMP_OUTPUT"; then
    echo "All tests passed"
    exit 0
elif grep -q "running 0 tests" "$TEMP_OUTPUT"; then
    echo "No tests were run"
    exit 0
else
    echo "Could not determine test result, QEMU exit code: $QEMU_EXIT_CODE (tee: $TEE_EXIT_CODE)"
    exit $QEMU_EXIT_CODE
fi
