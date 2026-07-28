#!/bin/bash

set -o pipefail

# Check for debug mode environment variable or command line argument
DEBUG_MODE=${SCARLET_DEBUG_MODE:-false}
CHECK_TEST_OUTPUT=${SCARLET_QEMU_CHECK_TEST_OUTPUT:-0}
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
PROJECT_DIR="$(cd "$SCRIPT_DIR" && cd .. && pwd)"
BOOT_IMAGE="$PROJECT_DIR/.scarlet/images/limine-aarch64-full.img"
ROOTFS_IMAGE="$PROJECT_DIR/.scarlet/images/rootfs-aarch64-full.ext2"

QEMU_DEBUG_ARGS=""
QEMU_ACCEL="${SCARLET_QEMU_ACCEL:-tcg}"
QEMU_SMP="${SCARLET_QEMU_SMP:-1}"
QEMU_MEMORY="${SCARLET_QEMU_MEMORY:-8G}"
QEMU_MACHINE="${SCARLET_QEMU_MACHINE_AARCH64:-virt,gic-version=3,acpi=off}"
if [ -n "${SCARLET_QEMU_CPU_AARCH64:-}" ]; then
    QEMU_CPU="$SCARLET_QEMU_CPU_AARCH64"
elif [ "$QEMU_ACCEL" = "hvf" ]; then
    QEMU_CPU="host"
else
    QEMU_CPU="max"
fi
QEMU_DISPLAY="${SCARLET_QEMU_DISPLAY:-vnc=:0}"
QEMU_GPU="${SCARLET_QEMU_GPU:-virtio-gpu-pci}"
QEMU_NET="${SCARLET_QEMU_NET:-1}"
QEMU_INPUT="${SCARLET_QEMU_INPUT:-1}"
if [ -n "${SCARLET_QEMU_ROOTFS_TRANSPORT:-}" ]; then
    QEMU_ROOTFS_TRANSPORT="$SCARLET_QEMU_ROOTFS_TRANSPORT"
elif grep -Eq 'cmdline = ".*root=/dev/usbblk0' "$PROJECT_DIR/scarlet.toml"; then
    QEMU_ROOTFS_TRANSPORT="usb"
else
    QEMU_ROOTFS_TRANSPORT="none"
fi
case "$QEMU_ROOTFS_TRANSPORT" in
    usb|virtio|none)
        ;;
    *)
        echo "Error: unsupported SCARLET_QEMU_ROOTFS_TRANSPORT=$QEMU_ROOTFS_TRANSPORT (expected usb, virtio, or none)"
        exit 1
        ;;
esac

if [ -n "${SCARLET_QEMU_USB_STORAGE:-}" ]; then
    QEMU_USB_STORAGE="$SCARLET_QEMU_USB_STORAGE"
elif [ "$QEMU_ROOTFS_TRANSPORT" = "usb" ]; then
    QEMU_USB_STORAGE=1
else
    QEMU_USB_STORAGE=0
fi

if [ -n "${SCARLET_QEMU_USB_STORAGE_IMAGE:-}" ]; then
    QEMU_USB_STORAGE_IMAGE="$SCARLET_QEMU_USB_STORAGE_IMAGE"
elif [ "$QEMU_ROOTFS_TRANSPORT" = "usb" ]; then
    QEMU_USB_STORAGE_IMAGE="$ROOTFS_IMAGE"
else
    QEMU_USB_STORAGE_IMAGE="$PROJECT_DIR/.scarlet/images/qemu-usb-storage.img"
fi
QEMU_USB_STORAGE_SIZE="${SCARLET_QEMU_USB_STORAGE_SIZE:-64M}"

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
    echo "Error: boot disk image not found at $BOOT_IMAGE"
    exit 1
fi

if [ "$QEMU_ROOTFS_TRANSPORT" != "none" ] && [ ! -f "$ROOTFS_IMAGE" ]; then
    echo "Error: rootfs image not found at $ROOTFS_IMAGE"
    exit 1
fi

TEMP_OUTPUT=""
if [ "$CHECK_TEST_OUTPUT" = "1" ] || [ "$CHECK_TEST_OUTPUT" = "true" ]; then
    TEMP_OUTPUT=$(mktemp)
fi
EFI_VARS_RUNTIME_CLEANUP=""
VHOST_USER_VIDEO_PID=""
VHOST_USER_VIDEO_SOCKET_CLEANUP=""

cleanup() {
    if [ -n "$VHOST_USER_VIDEO_PID" ]; then
        kill "$VHOST_USER_VIDEO_PID" 2>/dev/null || true
        wait "$VHOST_USER_VIDEO_PID" 2>/dev/null || true
    fi
    if [ -n "$VHOST_USER_VIDEO_SOCKET_CLEANUP" ]; then
        rm -f "$VHOST_USER_VIDEO_SOCKET_CLEANUP"
    fi
    if [ -n "$EFI_VARS_RUNTIME_CLEANUP" ]; then
        rm -f "$EFI_VARS_RUNTIME_CLEANUP"
    fi
}

trap cleanup EXIT

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
EFI_VARS_PERSISTENT="${SCARLET_EFI_VARS_RUNTIME_ARM64:-$PROJECT_DIR/.scarlet/AAVMF_VARS.fd}"

if [ -z "$EFI_CODE" ]; then
    echo "Error: AArch64 EFI firmware code image not found."
    exit 1
fi

if [ -z "$EFI_VARS_TEMPLATE" ] && [ -z "${SCARLET_EFI_VARS_RUNTIME_ARM64:-}" ]; then
    echo "Error: AArch64 EFI vars template not found."
    exit 1
fi

echo "Using AArch64 EFI code: $EFI_CODE"
if [ -n "$EFI_VARS_TEMPLATE" ]; then
    echo "Using AArch64 EFI vars template: $EFI_VARS_TEMPLATE"
fi

if [ -n "${SCARLET_EFI_VARS_RUNTIME_ARM64:-}" ]; then
    EFI_VARS_RUNTIME="$SCARLET_EFI_VARS_RUNTIME_ARM64"
elif [ "${SCARLET_EFI_VARS_PERSIST:-0}" = "1" ] || [ "${SCARLET_EFI_VARS_PERSIST:-}" = "true" ]; then
    EFI_VARS_RUNTIME="$EFI_VARS_PERSISTENT"
    if [ ! -f "$EFI_VARS_RUNTIME" ]; then
        cp "$EFI_VARS_TEMPLATE" "$EFI_VARS_RUNTIME"
    fi
else
    EFI_VARS_RUNTIME="$(mktemp "$PROJECT_DIR/.scarlet/AAVMF_VARS.run.XXXXXX.fd")"
    cp "$EFI_VARS_TEMPLATE" "$EFI_VARS_RUNTIME"
    EFI_VARS_RUNTIME_CLEANUP="$EFI_VARS_RUNTIME"
fi

ensure_efi_vars_writable "$EFI_VARS_RUNTIME"


QEMU_DISPLAY_ARGS=(-serial mon:stdio -display "$QEMU_DISPLAY")

QEMU_GPU_ARGS=()
if [ "$QEMU_GPU" != "none" ]; then
    QEMU_GPU_ARGS=(-device "$QEMU_GPU")
fi

QEMU_NET_ARGS=()
if [ "$QEMU_NET" = "1" ] || [ "$QEMU_NET" = "true" ]; then
    QEMU_NETDEV="user,id=net0"
    QEMU_HOSTFWD="${SCARLET_QEMU_HOSTFWD:-tcp::8080-:8080,udp::8080-:8080,udp::1234-:1234}"
    if [ -n "$QEMU_HOSTFWD" ]; then
        IFS=',' read -ra QEMU_HOSTFWD_RULES <<< "$QEMU_HOSTFWD"
        for QEMU_HOSTFWD_RULE in "${QEMU_HOSTFWD_RULES[@]}"; do
            if [ -n "$QEMU_HOSTFWD_RULE" ]; then
                QEMU_NETDEV="${QEMU_NETDEV},hostfwd=${QEMU_HOSTFWD_RULE}"
            fi
        done
    fi
    QEMU_NET_ARGS=(-netdev "$QEMU_NETDEV" -device virtio-net-pci,netdev=net0,bus=pcie.0)
fi

QEMU_INPUT_ARGS=()
if [ "$QEMU_INPUT" = "1" ] || [ "$QEMU_INPUT" = "true" ]; then
    QEMU_INPUT_ARGS=(-device virtio-keyboard-device,bus=virtio-mmio-bus.4 -device virtio-mouse-device,bus=virtio-mmio-bus.5)
fi

QEMU_MEMORY_ARGS=(-m "$QEMU_MEMORY")
QEMU_AUDIO="${SCARLET_QEMU_AUDIO:-0}"
QEMU_AUDIO_DRIVER="${SCARLET_QEMU_AUDIO_DRIVER:-coreaudio}"
QEMU_AUDIO_ARGS=()
if [ "$QEMU_AUDIO" = "1" ] || [ "$QEMU_AUDIO" = "true" ]; then
    QEMU_AUDIO_ARGS=(-audiodev "$QEMU_AUDIO_DRIVER,id=audio0" -device virtio-sound-pci,audiodev=audio0,bus=pcie.0)
fi

QEMU_ROOTFS_ARGS=()
case "$QEMU_ROOTFS_TRANSPORT" in
    virtio)
        QEMU_ROOTFS_ARGS=(
            -drive id=rootfs,file="$ROOTFS_IMAGE",format=raw,if=none
            -device virtio-blk-device,drive=rootfs,bus=virtio-mmio-bus.1
        )
        ;;
    usb|none)
        ;;
esac

ensure_qemu_usb_storage_image() {
    local image="$1"
    local size="$2"

    if [ -f "$image" ]; then
        return 0
    fi

    mkdir -p "$(dirname "$image")"
    echo "Creating QEMU USB storage image: $image ($size)"
    if ! truncate -s "$size" "$image"; then
        echo "Error: failed to create QEMU USB storage image: $image"
        exit 1
    fi
}

QEMU_USB_STORAGE_ARGS=()
if [ "$QEMU_USB_STORAGE" = "1" ] || [ "$QEMU_USB_STORAGE" = "true" ]; then
    if ! qemu-system-aarch64 -device help 2>/dev/null | grep -q 'qemu-xhci'; then
        echo "Error: qemu-system-aarch64 does not provide qemu-xhci"
        exit 1
    fi
    ensure_qemu_usb_storage_image "$QEMU_USB_STORAGE_IMAGE" "$QEMU_USB_STORAGE_SIZE"
    echo "Enabling QEMU USB storage: $QEMU_USB_STORAGE_IMAGE"
    QEMU_USB_STORAGE_ARGS=(
        -device qemu-xhci,id=xhci,bus=pcie.0
        -drive id=usb_storage0,file="$QEMU_USB_STORAGE_IMAGE",format=raw,if=none
        -device usb-storage,bus=xhci.0,drive=usb_storage0,removable=on
    )
fi

QEMU_VHOST_USER_VIDEO_ARGS=()
if [ "${SCARLET_VHOST_USER_VIDEO:-0}" = "1" ] || [ "${SCARLET_VHOST_USER_VIDEO:-}" = "true" ]; then
    VHOST_USER_VIDEO_SOCKET="${SCARLET_VHOST_USER_VIDEO_SOCKET:-/private/tmp/scarlet-video.sock}"
    VHOST_USER_VIDEO_DAEMON="${SCARLET_VHOST_USER_VIDEO_DAEMON:-$PROJECT_ROOT/dist/host/vhost_video_videotoolbox}"
    VHOST_USER_VIDEO_LOG="${SCARLET_VHOST_USER_VIDEO_LOG:-/private/tmp/scarlet-video.log}"
    VHOST_USER_VIDEO_ID="${SCARLET_VHOST_USER_VIDEO_ID:-31}"
    VHOST_USER_VIDEO_QUEUES="${SCARLET_VHOST_USER_VIDEO_QUEUES:-2}"
    VHOST_USER_VIDEO_QUEUE_SIZE="${SCARLET_VHOST_USER_VIDEO_QUEUE_SIZE:-256}"
    VHOST_USER_VIDEO_CONFIG_SIZE="${SCARLET_VHOST_USER_VIDEO_CONFIG_SIZE:-64}"

    echo "Enabling vhost-user video PCI device: socket=$VHOST_USER_VIDEO_SOCKET virtio-id=$VHOST_USER_VIDEO_ID"
    QEMU_MEMORY_ARGS=(
        -m "$QEMU_MEMORY"
        -object memory-backend-shm,id=scarlet-mem,size="$QEMU_MEMORY",share=on
        -numa node,memdev=scarlet-mem
    )
    QEMU_VHOST_USER_VIDEO_ARGS=(
        -chardev socket,id=vuvid,path="$VHOST_USER_VIDEO_SOCKET"
        -device vhost-user-test-device-pci,bus=pcie.0,chardev=vuvid,virtio-id="$VHOST_USER_VIDEO_ID",num_vqs="$VHOST_USER_VIDEO_QUEUES",vq_size="$VHOST_USER_VIDEO_QUEUE_SIZE",config_size="$VHOST_USER_VIDEO_CONFIG_SIZE"
    )

    if [ "$(uname -s)" = "Darwin" ]; then
        if [ ! -x "$VHOST_USER_VIDEO_DAEMON" ]; then
            echo "Error: vhost-user video daemon not found or not executable: $VHOST_USER_VIDEO_DAEMON"
            echo "       Run cargo make build-vhost-video-videotoolbox or set SCARLET_VHOST_USER_VIDEO_DAEMON."
            exit 1
        fi
        rm -f "$VHOST_USER_VIDEO_SOCKET"
        VHOST_USER_VIDEO_SOCKET_CLEANUP="$VHOST_USER_VIDEO_SOCKET"
        echo "Starting vhost-user video daemon: $VHOST_USER_VIDEO_DAEMON --socket $VHOST_USER_VIDEO_SOCKET"
        if [ "$VHOST_USER_VIDEO_LOG" = "stderr" ]; then
            "$VHOST_USER_VIDEO_DAEMON" --socket "$VHOST_USER_VIDEO_SOCKET" &
        else
            mkdir -p "$(dirname "$VHOST_USER_VIDEO_LOG")"
            echo "vhost-user video daemon log: $VHOST_USER_VIDEO_LOG"
            "$VHOST_USER_VIDEO_DAEMON" --socket "$VHOST_USER_VIDEO_SOCKET" >"$VHOST_USER_VIDEO_LOG" 2>&1 &
        fi
        VHOST_USER_VIDEO_PID=$!
        for _ in {1..100}; do
            if [ -S "$VHOST_USER_VIDEO_SOCKET" ]; then
                break
            fi
            if ! kill -0 "$VHOST_USER_VIDEO_PID" 2>/dev/null; then
                echo "Error: vhost-user video daemon exited before creating $VHOST_USER_VIDEO_SOCKET"
                exit 1
            fi
            sleep 0.05
        done
        if [ ! -S "$VHOST_USER_VIDEO_SOCKET" ]; then
            echo "Error: timed out waiting for vhost-user video daemon socket: $VHOST_USER_VIDEO_SOCKET"
            exit 1
        fi
    fi
fi

QEMU_CMD=(qemu-system-aarch64
    -machine "$QEMU_MACHINE" \
    -cpu "$QEMU_CPU" \
    -accel "$QEMU_ACCEL" \
    "${QEMU_MEMORY_ARGS[@]}" \
    -smp "$QEMU_SMP" \
    "${QEMU_DISPLAY_ARGS[@]}" \
    --no-reboot \
    -drive if=pflash,format=raw,unit=0,file="$EFI_CODE",readonly=on \
    -drive if=pflash,format=raw,unit=1,file="$EFI_VARS_RUNTIME" \
    -global virtio-mmio.force-legacy=false \
    -drive id=boot,file="$BOOT_IMAGE",format=raw,if=none \
    -device virtio-blk-pci,drive=boot,bus=pcie.0 \
    "${QEMU_ROOTFS_ARGS[@]}" \
    "${QEMU_GPU_ARGS[@]}" \
    "${QEMU_NET_ARGS[@]}" \
    "${QEMU_INPUT_ARGS[@]}" \
    "${QEMU_AUDIO_ARGS[@]}" \
    "${QEMU_USB_STORAGE_ARGS[@]}" \
    "${QEMU_VHOST_USER_VIDEO_ARGS[@]}" \
    -device virtio-rng-device,bus=virtio-mmio-bus.6 \
    $QEMU_DEBUG_ARGS \
    $DEBUG_FLAGS)

if [ -n "$TEMP_OUTPUT" ]; then
    "${QEMU_CMD[@]}" | tee "$TEMP_OUTPUT"

    # Capture pipeline exit codes (qemu is element 0, tee is element 1)
    QEMU_EXIT_CODE=${PIPESTATUS[0]}
    TEE_EXIT_CODE=${PIPESTATUS[1]}
else
    "${QEMU_CMD[@]}"
    QEMU_EXIT_CODE=$?
    exit $QEMU_EXIT_CODE
fi


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
