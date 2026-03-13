#!/bin/bash

set -o pipefail

# Check for debug mode environment variable or command line argument
DEBUG_MODE=${SCARLET_DEBUG_MODE:-false}
KERNEL_PATH=""
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KERNEL_DIR="$(dirname "$SCRIPT_DIR")"

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
    if [ "$DEBUG_MODE" = "true" ]; then
        KERNEL_PATH="$KERNEL_DIR/target/aarch64-unknown-none-elf/debug/scarlet"
    else
        # For release mode or default run
        if [ -f "$KERNEL_DIR/target/aarch64-unknown-none-elf/release/scarlet" ]; then
            KERNEL_PATH="$KERNEL_DIR/target/aarch64-unknown-none-elf/release/scarlet"
        else
            KERNEL_PATH="$KERNEL_DIR/target/aarch64-unknown-none-elf/debug/scarlet"
        fi
    fi
fi

if [ "${KERNEL_PATH#/}" = "$KERNEL_PATH" ]; then
    case "$KERNEL_PATH" in
        target/*)
            KERNEL_PATH="$KERNEL_DIR/$KERNEL_PATH"
            ;;
        *)
            ;;
    esac
fi

QEMU_MACHINE="${SCARLET_QEMU_MACHINE:-virt}"
QEMU_MACHINE_ARGS=()
QEMU_PLATFORM_ARGS=()
QEMU_STORAGE_ARGS=()
QEMU_DISPLAY_ARGS=()
QEMU_INPUT_ARGS=()
QEMU_NETWORK_ARGS=()
QEMU_DEBUG_ARGS=()
DEBUG_FLAGS=()
BOOT_IMAGE_SIZE_MB=""
EFI_VARS_RUNTIME=""
QEMU_MEMORY="8G"

if [ "$DEBUG_MODE" = "true" ]; then
    echo "Starting qemu-system-aarch64 in debug mode with gdb server..."
    DEBUG_FLAGS=(-gdb tcp::12345 -S)
else
    echo "Starting qemu-system-aarch64..."
fi

PROJECT_ROOT="$(cd "$SCRIPT_DIR" && cd .. && cd .. && cd .. && pwd)"
BOOT_IMAGE="$PROJECT_ROOT/mkfs/dist/limine-aarch64-boot.img"
ROOTFS_IMAGE="$PROJECT_ROOT/mkfs/dist/rootfs.img"

if [ ! -f "$KERNEL_PATH" ]; then
    echo "Error: kernel binary not found at $KERNEL_PATH"
    exit 1
fi

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
        QEMU_DEBUG_LOG="${SCARLET_QEMU_GUEST_ERRORS_LOG:-$PROJECT_ROOT/qemu-guest-errors-aarch64.log}"
    else
        QEMU_DEBUG_LOG="${SCARLET_QEMU_DEBUG_LOG:-$PROJECT_ROOT/qemu-debug-aarch64.log}"
    fi
    echo "QEMU debug logging enabled (-d $QEMU_DEBUG_FLAGS): $QEMU_DEBUG_LOG"
    QEMU_DEBUG_ARGS=(-d "$QEMU_DEBUG_FLAGS" -D "$QEMU_DEBUG_LOG")
fi

if [ ! -f "$BOOT_IMAGE" ]; then
    echo "Error: Limine boot image not found at $BOOT_IMAGE"
    exit 1
fi

if [ ! -f "$ROOTFS_IMAGE" ]; then
    echo "Error: rootfs image not found at $ROOTFS_IMAGE"
    exit 1
fi

TEMP_OUTPUT=$(mktemp)

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
EFI_VARS_PERSISTENT="$PROJECT_ROOT/mkfs/dist/AAVMF_VARS.fd"

if [ -z "$EFI_CODE" ]; then
    echo "Error: AArch64 EFI firmware code image not found."
    exit 1
fi

case "$QEMU_MACHINE" in
    virt)
        if [ -z "$EFI_VARS_TEMPLATE" ]; then
            echo "Error: AArch64 EFI vars template not found."
            exit 1
        fi

        if [ "${SCARLET_EFI_VARS_PERSIST:-0}" = "1" ] || [ "${SCARLET_EFI_VARS_PERSIST:-}" = "true" ]; then
            EFI_VARS_RUNTIME="$EFI_VARS_PERSISTENT"
            if [ ! -f "$EFI_VARS_RUNTIME" ]; then
                cp "$EFI_VARS_TEMPLATE" "$EFI_VARS_RUNTIME"
            fi
        else
            EFI_VARS_RUNTIME="$(mktemp "$PROJECT_ROOT/mkfs/dist/AAVMF_VARS.run.XXXXXX.fd")"
            cp "$EFI_VARS_TEMPLATE" "$EFI_VARS_RUNTIME"
            trap 'rm -f "$EFI_VARS_RUNTIME"' EXIT
        fi

        QEMU_MACHINE_ARGS=(-machine "virt,gic-version=3,acpi=off" -cpu cortex-a57)
        QEMU_PLATFORM_ARGS=(
            -drive if=pflash,format=raw,unit=0,file="$EFI_CODE",readonly=on
            -drive if=pflash,format=raw,unit=1,file="$EFI_VARS_RUNTIME"
            -global virtio-mmio.force-legacy=false
        )
        QEMU_STORAGE_ARGS=(
            -drive id=boot,file="$BOOT_IMAGE",format=raw,if=none
            -device virtio-blk-device,drive=boot,bus=virtio-mmio-bus.0
            -drive id=rootfs,file="$ROOTFS_IMAGE",format=raw,if=none
            -device virtio-blk-device,drive=rootfs,bus=virtio-mmio-bus.1
        )
        QEMU_DISPLAY_ARGS=(-display vnc=:0 -device virtio-gpu-device,bus=virtio-mmio-bus.2)
        QEMU_NETWORK_ARGS=(-netdev user,id=net0 -device virtio-net-device,netdev=net0,bus=virtio-mmio-bus.3)
        QEMU_INPUT_ARGS=(
            -device virtio-keyboard-device,bus=virtio-mmio-bus.4
            -device virtio-mouse-device,bus=virtio-mmio-bus.5
            -device virtio-rng-device,bus=virtio-mmio-bus.6
        )
        ;;
    raspi4b)
        BOOT_IMAGE_SIZE_MB="${SCARLET_QEMU_RASPI4B_BOOT_IMAGE_SIZE_MB:-256}"
        # QEMU's raspi4b machine rejects the 8G virt default and expects a 2 GiB board profile.
        QEMU_MEMORY="2G"
        QEMU_MACHINE_ARGS=(-machine raspi4b -cpu cortex-a72)
        QEMU_PLATFORM_ARGS=(-bios "$EFI_CODE")
        # raspi4b does not expose the virt machine's virtio-mmio network devices.
        QEMU_NETWORK_ARGS=()
        QEMU_STORAGE_ARGS=(
            -drive file="$BOOT_IMAGE",format=raw,if=sd
            -drive id=rootfs,file="$ROOTFS_IMAGE",format=raw,if=none
            -device usb-storage,drive=rootfs
        )
        QEMU_DISPLAY_ARGS=(-display vnc=:0)
        QEMU_INPUT_ARGS=(-device usb-kbd -device usb-mouse)
        ;;
    *)
        echo "Error: unsupported SCARLET_QEMU_MACHINE '$QEMU_MACHINE' (expected: virt or raspi4b)"
        exit 1
        ;;
esac

echo "Using QEMU machine profile: $QEMU_MACHINE"
echo "Rebuilding Limine AArch64 boot image from $KERNEL_PATH"
if [ -n "$BOOT_IMAGE_SIZE_MB" ]; then
    if ! BOOT_IMAGE_SIZE_MB="$BOOT_IMAGE_SIZE_MB" KERNEL_ELF="$KERNEL_PATH" sh "$PROJECT_ROOT/mkfs/make_limine_aarch64_image.sh"; then
        echo "Error: failed to rebuild Limine AArch64 boot image"
        exit 1
    fi
else
    if ! KERNEL_ELF="$KERNEL_PATH" sh "$PROJECT_ROOT/mkfs/make_limine_aarch64_image.sh"; then
        echo "Error: failed to rebuild Limine AArch64 boot image"
        exit 1
    fi
fi

qemu-system-aarch64 \
    "${QEMU_MACHINE_ARGS[@]}" \
    -m "$QEMU_MEMORY" \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    "${QEMU_PLATFORM_ARGS[@]}" \
    "${QEMU_STORAGE_ARGS[@]}" \
    "${QEMU_DISPLAY_ARGS[@]}" \
    "${QEMU_NETWORK_ARGS[@]}" \
    "${QEMU_INPUT_ARGS[@]}" \
    "${QEMU_DEBUG_ARGS[@]}" \
    "${DEBUG_FLAGS[@]}" | tee "$TEMP_OUTPUT"

# Capture pipeline exit codes (qemu is element 0, tee is element 1)
QEMU_EXIT_CODE=${PIPESTATUS[0]}
TEE_EXIT_CODE=${PIPESTATUS[1]}

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
    echo "Could not determine test result, QEMU exit code: $QEMU_EXIT_CODE (tee: $TEE_EXIT_CODE)"
    rm -f "$TEMP_OUTPUT"
    exit $QEMU_EXIT_CODE
fi
