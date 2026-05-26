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

if [ "$DEBUG_MODE" = "true" ]; then
    echo "Starting qemu-system-aarch64 in debug mode with gdb server..."
    DEBUG_FLAGS="-gdb tcp::12345 -S"
else
    echo "Starting qemu-system-aarch64..."
    DEBUG_FLAGS=""
fi

PROJECT_ROOT="$(cd "$SCRIPT_DIR" && cd .. && cd .. && cd .. && pwd)"
PROJECT_DIR="$KERNEL_DIR"
BOOT_IMAGE="$PROJECT_DIR/.scarlet/images/limine-aarch64-microvm.img"
ROOTFS_IMAGE="$PROJECT_DIR/.scarlet/images/rootfs-aarch64-microvm.ext2"

if [ ! -f "$KERNEL_PATH" ]; then
    echo "Error: kernel binary not found at $KERNEL_PATH"
    exit 1
fi

echo "Rebuilding microvm images from $KERNEL_PATH"
if ! (
    cd "$PROJECT_ROOT" &&
    cargo run --manifest-path "$PROJECT_ROOT/cargo-scarlet/Cargo.toml" -- image \
        --project "$PROJECT_DIR" \
        --release \
        --no-build \
        --kernel-elf "$KERNEL_PATH"
); then
    echo "Error: failed to rebuild microvm images"
    exit 1
fi

QEMU_DEBUG_ARGS=""
QEMU_ACCEL="${SCARLET_QEMU_ACCEL:-tcg}"
QEMU_SMP="${SCARLET_QEMU_SMP:-1}"
QEMU_MEMORY="${SCARLET_QEMU_MEMORY:-8G}"
QEMU_MACHINE="${SCARLET_QEMU_MACHINE_AARCH64:-virt,gic-version=3,acpi=off}"
QEMU_CPU="${SCARLET_QEMU_CPU_AARCH64:-max}"
QEMU_DISPLAY="${SCARLET_QEMU_DISPLAY:-none}"
QEMU_GPU="${SCARLET_QEMU_GPU:-none}"
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
        if { [ "${SCARLET_QEMU_VIRTUALIZATION:-1}" = "1" ] || [ "${SCARLET_QEMU_VIRTUALIZATION:-}" = "true" ]; } && [ "$QEMU_ACCEL" != "hvf" ]; then
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

if [ ! -f "$ROOTFS_IMAGE" ]; then
    echo "Error: rootfs image not found at $ROOTFS_IMAGE"
    exit 1
fi

TEMP_OUTPUT=$(mktemp)


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
        QEMU_GPU_ARGS=(-device virtio-gpu-device,bus=virtio-mmio-bus.2,xres="$QEMU_VIRTIO_GPU_XRES",yres="$QEMU_VIRTIO_GPU_YRES")
        ;;
    virtio-gpu-pci)
        QEMU_GPU_ARGS=(-device virtio-gpu-pci,bus=pcie.0,xres="$QEMU_VIRTIO_GPU_XRES",yres="$QEMU_VIRTIO_GPU_YRES")
        ;;
    virgl|virtio-gpu-gl)
        require_virtio_gpu_gl_device qemu-system-aarch64
        QEMU_GPU_ARGS=(-device virtio-gpu-gl-device,bus=virtio-mmio-bus.2,xres="$QEMU_VIRTIO_GPU_XRES",yres="$QEMU_VIRTIO_GPU_YRES")
        ;;
    ramfb)
        QEMU_GPU_ARGS=(-device ramfb)
        ;;
    none)
        ;;
    *)
        echo "Error: unsupported SCARLET_QEMU_GPU=$QEMU_GPU (expected virtio-gpu, virtio-gpu-pci, virgl, virtio-gpu-gl, ramfb, or none)"
        exit 1
        ;;
esac

QEMU_AUDIO="${SCARLET_QEMU_AUDIO:-0}"
QEMU_AUDIO_DRIVER="${SCARLET_QEMU_AUDIO_DRIVER:-coreaudio}"
QEMU_AUDIO_ARGS=()
if [ "$QEMU_AUDIO" = "1" ] || [ "$QEMU_AUDIO" = "true" ]; then
    QEMU_AUDIO_ARGS=(-audiodev "$QEMU_AUDIO_DRIVER,id=audio0" -device virtio-sound-pci,audiodev=audio0,bus=pcie.0)
fi

find_efi_code() {
    if [ "$QEMU_ACCEL" = "hvf" ] && [ -n "${SCARLET_EFI_CODE_ARM64_HVF:-}" ] && [ -f "${SCARLET_EFI_CODE_ARM64_HVF}" ]; then
        printf '%s\n' "${SCARLET_EFI_CODE_ARM64_HVF}"
        return 0
    fi
    if [ -n "${SCARLET_EFI_CODE_ARM64_EL2:-}" ] && [ -f "${SCARLET_EFI_CODE_ARM64_EL2}" ]; then
        printf '%s\n' "${SCARLET_EFI_CODE_ARM64_EL2}"
        return 0
    fi
    if [ -n "${SCARLET_EFI_CODE_ARM64:-}" ] && [ -f "${SCARLET_EFI_CODE_ARM64}" ]; then
        printf '%s\n' "${SCARLET_EFI_CODE_ARM64}"
        return 0
    fi
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
    if [ "$QEMU_ACCEL" = "hvf" ] && [ -n "${SCARLET_EFI_VARS_ARM64_HVF:-}" ] && [ -f "${SCARLET_EFI_VARS_ARM64_HVF}" ]; then
        printf '%s\n' "${SCARLET_EFI_VARS_ARM64_HVF}"
        return 0
    fi
    if [ -n "${SCARLET_EFI_VARS_ARM64_EL2:-}" ] && [ -f "${SCARLET_EFI_VARS_ARM64_EL2}" ]; then
        printf '%s\n' "${SCARLET_EFI_VARS_ARM64_EL2}"
        return 0
    fi
    if [ -n "${SCARLET_EFI_VARS_ARM64:-}" ] && [ -f "${SCARLET_EFI_VARS_ARM64}" ]; then
        printf '%s\n' "${SCARLET_EFI_VARS_ARM64}"
        return 0
    fi
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

ensure_efi_vars_writable() {
    local vars_file="$1"

    if ! chmod u+w "$vars_file" 2>/dev/null || [ ! -w "$vars_file" ]; then
        echo "Error: AArch64 EFI vars runtime file is not writable: $vars_file"
        exit 1
    fi
}

EFI_CODE="$(find_efi_code || true)"
EFI_VARS_TEMPLATE="$(find_efi_vars_template || true)"
EFI_VARS_PERSISTENT="$PROJECT_DIR/.scarlet/images/AAVMF_VARS.fd"

set_efi_timeout_zero() {
    local vars_file="$1"
    local timeout_json

    if ! command -v virt-fw-vars >/dev/null 2>&1; then
        echo "Warning: virt-fw-vars not found; UEFI boot timeout may remain enabled." >&2
        return 0
    fi

    timeout_json="$(mktemp "$PROJECT_DIR/.scarlet/images/uefi-timeout.XXXXXX.json")"
    cat >"$timeout_json" <<'JSON'
{
  "version": 2,
  "variables": [
    {
      "name": "Timeout",
      "guid": "8be4df61-93ca-11d2-aa0d-00e098032b8c",
      "attr": 7,
      "data": "0000"
    }
  ]
}
JSON
    virt-fw-vars --inplace "$vars_file" --set-json "$timeout_json" >/dev/null
    rm -f "$timeout_json"
}

if [ -z "$EFI_CODE" ]; then
    echo "Error: AArch64 EFI firmware code image not found."
    exit 1
fi

if [ -z "$EFI_VARS_TEMPLATE" ]; then
    echo "Error: AArch64 EFI vars template not found."
    exit 1
fi

echo "Using AArch64 EFI code: $EFI_CODE"
echo "Using AArch64 EFI vars template: $EFI_VARS_TEMPLATE"

if [ "${SCARLET_EFI_VARS_PERSIST:-0}" = "1" ] || [ "${SCARLET_EFI_VARS_PERSIST:-}" = "true" ]; then
    EFI_VARS_RUNTIME="$EFI_VARS_PERSISTENT"
    if [ ! -f "$EFI_VARS_RUNTIME" ]; then
        cp "$EFI_VARS_TEMPLATE" "$EFI_VARS_RUNTIME"
    fi
    ensure_efi_vars_writable "$EFI_VARS_RUNTIME"
    set_efi_timeout_zero "$EFI_VARS_RUNTIME"
else
    EFI_VARS_RUNTIME="$(mktemp "$PROJECT_DIR/.scarlet/images/AAVMF_VARS.run.XXXXXX.fd")"
    cp "$EFI_VARS_TEMPLATE" "$EFI_VARS_RUNTIME"
    ensure_efi_vars_writable "$EFI_VARS_RUNTIME"
    set_efi_timeout_zero "$EFI_VARS_RUNTIME"
    trap 'rm -f "$EFI_VARS_RUNTIME"' EXIT
fi

qemu-system-aarch64 \
    -machine "$QEMU_MACHINE" \
    -cpu "$QEMU_CPU" \
    -accel "$QEMU_ACCEL" \
    -m "$QEMU_MEMORY" \
    -smp "$QEMU_SMP" \
    "${QEMU_DISPLAY_ARGS[@]}" \
    --no-reboot \
    -boot menu=off,splash-time=0,strict=on \
    -drive if=pflash,format=raw,unit=0,file="$EFI_CODE",readonly=on \
    -drive if=pflash,format=raw,unit=1,file="$EFI_VARS_RUNTIME" \
    -global virtio-mmio.force-legacy=false \
    -drive id=boot,file="$BOOT_IMAGE",format=raw,if=none \
    -device virtio-blk-pci,drive=boot,bus=pcie.0,bootindex=0 \
    -drive id=rootfs,file="$ROOTFS_IMAGE",format=raw,if=none \
    -device virtio-blk-device,drive=rootfs,bus=virtio-mmio-bus.1 \
    "${QEMU_GPU_ARGS[@]}" \
    "${QEMU_AUDIO_ARGS[@]}" \
    -device virtio-rng-device,bus=virtio-mmio-bus.3 \
    $QEMU_DEBUG_ARGS \
    $DEBUG_FLAGS | tee "$TEMP_OUTPUT"

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
