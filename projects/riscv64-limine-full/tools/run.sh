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
EFI_CODE="${SCARLET_EFI_CODE_RV64:-/usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd}"
EFI_VARS_TEMPLATE="${SCARLET_EFI_VARS_RV64:-/usr/share/qemu-efi-riscv64/RISCV_VIRT_VARS.fd}"
EFI_VARS_PERSISTENT="$PROJECT_ROOT/mkfs/dist/RISCV_VIRT_VARS.fd"

QEMU_DEBUG_ARGS=""
QEMU_ACCEL="${SCARLET_QEMU_ACCEL:-tcg}"
QEMU_SMP="${SCARLET_QEMU_SMP:-1}"
QEMU_MEMORY="${SCARLET_QEMU_MEMORY:-8G}"
QEMU_MACHINE="${SCARLET_QEMU_MACHINE_RV64:-virt,acpi=off}"
QEMU_DISPLAY="${SCARLET_QEMU_DISPLAY:-vnc}"
QEMU_GPU="${SCARLET_QEMU_GPU:-virtio-gpu}"
QEMU_VIRTIO_GPU_XRES="${SCARLET_QEMU_VIRTIO_GPU_XRES:-1280}"
QEMU_VIRTIO_GPU_YRES="${SCARLET_QEMU_VIRTIO_GPU_YRES:-800}"

case "$QEMU_GPU" in
    virgl|virtio-gpu-gl)
        if [ -z "${SCARLET_QEMU_DISPLAY+x}" ]; then
            QEMU_DISPLAY="egl-headless"
        fi
        ;;
esac

case ",$QEMU_MACHINE," in
    *,virtualization=*)
        ;;
    *)
        if { [ "${SCARLET_QEMU_VIRTUALIZATION:-0}" = "1" ] || [ "${SCARLET_QEMU_VIRTUALIZATION:-}" = "true" ]; } && [ "$QEMU_ACCEL" != "hvf" ]; then
            QEMU_MACHINE="${QEMU_MACHINE},virtualization=on"
        fi
        ;;
esac

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


require_virtio_gpu_gl_device() {
    if ! "$1" -device help 2>/dev/null | grep -q 'virtio-gpu-gl-device'; then
        echo "Error: $1 does not provide virtio-gpu-gl-device; install a QEMU build with virgl/GL support or use SCARLET_QEMU_GPU=virtio-gpu"
        exit 1
    fi
}

QEMU_DISPLAY_ARGS=()
case "$QEMU_DISPLAY" in
    vnc)
        QEMU_DISPLAY_ARGS=(-nographic -serial mon:stdio -display vnc=:0)
        ;;
    cocoa)
        QEMU_DISPLAY_ARGS=(-serial mon:stdio -display cocoa)
        ;;
    sdl)
        QEMU_DISPLAY_ARGS=(-serial mon:stdio -display sdl)
        ;;
    gtk-gl)
        QEMU_DISPLAY_ARGS=(-serial mon:stdio -display gtk,gl=on)
        ;;
    sdl-gl)
        QEMU_DISPLAY_ARGS=(-serial mon:stdio -display sdl,gl=on)
        ;;
    egl-headless)
        QEMU_DISPLAY_ARGS=(-nographic -serial mon:stdio -display egl-headless,gl=on)
        ;;
    none)
        QEMU_DISPLAY_ARGS=(-nographic -serial mon:stdio)
        ;;
    *)
        echo "Error: unsupported SCARLET_QEMU_DISPLAY=$QEMU_DISPLAY (expected vnc, cocoa, sdl, gtk-gl, sdl-gl, egl-headless, or none)"
        exit 1
        ;;
esac

QEMU_GPU_ARGS=()
case "$QEMU_GPU" in
    virtio-gpu)
        QEMU_GPU_ARGS=(-device virtio-gpu-device,bus=virtio-mmio-bus.1,xres="$QEMU_VIRTIO_GPU_XRES",yres="$QEMU_VIRTIO_GPU_YRES")
        ;;
    virgl|virtio-gpu-gl)
        require_virtio_gpu_gl_device qemu-system-riscv64
        QEMU_GPU_ARGS=(-device virtio-gpu-gl-device,bus=virtio-mmio-bus.1,xres="$QEMU_VIRTIO_GPU_XRES",yres="$QEMU_VIRTIO_GPU_YRES")
        ;;
    none)
        ;;
    *)
        echo "Error: unsupported SCARLET_QEMU_GPU=$QEMU_GPU (expected virtio-gpu, virgl, virtio-gpu-gl, or none)"
        exit 1
        ;;
esac

qemu-system-riscv64 \
    -machine "$QEMU_MACHINE" \
    -accel "$QEMU_ACCEL" \
    -m "$QEMU_MEMORY" \
    -smp "$QEMU_SMP" \
    "${QEMU_DISPLAY_ARGS[@]}" \
    --no-reboot \
    -bios default \
    -drive if=pflash,format=raw,unit=0,file="$EFI_CODE",readonly=on \
    -drive if=pflash,format=raw,unit=1,file="$EFI_VARS_RUNTIME" \
    -global virtio-mmio.force-legacy=false \
    -drive id=boot,file="$BOOT_IMAGE",format=raw,if=none \
    -device virtio-blk-pci,drive=boot,bus=pcie.0 \
    -drive id=rootfs,file="$ROOTFS_IMAGE",format=raw,if=none \
    -device virtio-blk-device,drive=rootfs,bus=virtio-mmio-bus.0 \
    "${QEMU_GPU_ARGS[@]}" \
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
