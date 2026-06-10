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
PROJECT_DIR="$(cd "$SCRIPT_DIR" && cd .. && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR" && cd .. && cd .. && cd .. && pwd)"
BOOT_IMAGE="$PROJECT_DIR/.scarlet/images/limine-riscv64-full.img"
ROOTFS_IMAGE="$PROJECT_DIR/.scarlet/images/rootfs-riscv64-full.ext2"
EFI_CODE="${SCARLET_EFI_CODE_RV64:-/usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd}"
EFI_VARS_TEMPLATE="${SCARLET_EFI_VARS_RV64:-/usr/share/qemu-efi-riscv64/RISCV_VIRT_VARS.fd}"
EFI_VARS_PERSISTENT="$PROJECT_DIR/.scarlet/RISCV_VIRT_VARS.fd"

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

if [ ! -f "$BOOT_IMAGE" ]; then
    echo "Error: Boot image not found at $BOOT_IMAGE"
    exit 1
fi

if [ ! -f "$ROOTFS_IMAGE" ]; then
    echo "Error: Rootfs image not found at $ROOTFS_IMAGE"
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
    EFI_VARS_RUNTIME="$(mktemp "$PROJECT_DIR/.scarlet/RISCV_VIRT_VARS.run.XXXXXX.fd")"
    cp "$EFI_VARS_TEMPLATE" "$EFI_VARS_RUNTIME"
    EFI_VARS_RUNTIME_CLEANUP="$EFI_VARS_RUNTIME"
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
    virtio-gpu-pci)
        QEMU_GPU_ARGS=(-device virtio-gpu-pci,bus=pcie.0,xres="$QEMU_VIRTIO_GPU_XRES",yres="$QEMU_VIRTIO_GPU_YRES")
        ;;
    virgl|virtio-gpu-gl)
        require_virtio_gpu_gl_device qemu-system-riscv64
        QEMU_GPU_ARGS=(-device virtio-gpu-gl-device,bus=virtio-mmio-bus.1,xres="$QEMU_VIRTIO_GPU_XRES",yres="$QEMU_VIRTIO_GPU_YRES")
        ;;
    none)
        ;;
    *)
        echo "Error: unsupported SCARLET_QEMU_GPU=$QEMU_GPU (expected virtio-gpu, virtio-gpu-pci, virgl, virtio-gpu-gl, or none)"
        exit 1
        ;;
esac

QEMU_AUDIO="${SCARLET_QEMU_AUDIO:-0}"
QEMU_AUDIO_DRIVER="${SCARLET_QEMU_AUDIO_DRIVER:-coreaudio}"
QEMU_AUDIO_ARGS=()
if [ "$QEMU_AUDIO" = "1" ] || [ "$QEMU_AUDIO" = "true" ]; then
    QEMU_AUDIO_ARGS=(-audiodev "$QEMU_AUDIO_DRIVER,id=audio0" -device virtio-sound-pci,audiodev=audio0,bus=pcie.0)
fi

QEMU_MEMORY_ARGS=(-m "$QEMU_MEMORY")
QEMU_VHOST_USER_VIDEO_ARGS=()
if [ "${SCARLET_VHOST_USER_VIDEO:-0}" = "1" ] || [ "${SCARLET_VHOST_USER_VIDEO:-}" = "true" ]; then
    VHOST_USER_VIDEO_SOCKET="${SCARLET_VHOST_USER_VIDEO_SOCKET:-/private/tmp/scarlet-video.sock}"
    VHOST_USER_VIDEO_DAEMON="${SCARLET_VHOST_USER_VIDEO_DAEMON:-$PROJECT_ROOT/dist/host/vhost_video_videotoolbox}"
    VHOST_USER_VIDEO_LOG="${SCARLET_VHOST_USER_VIDEO_LOG:-/private/tmp/scarlet-video.log}"
    VHOST_USER_VIDEO_SOCKET_WAIT_ATTEMPTS="${SCARLET_VHOST_USER_VIDEO_SOCKET_WAIT_ATTEMPTS:-100}"
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
        rm -f "$VHOST_USER_VIDEO_SOCKET"
        VHOST_USER_VIDEO_SOCKET_CLEANUP="$VHOST_USER_VIDEO_SOCKET"
        if [ "$VHOST_USER_VIDEO_LOG" != "stderr" ]; then
            mkdir -p "$(dirname "$VHOST_USER_VIDEO_LOG")"
            echo "vhost-user video daemon log: $VHOST_USER_VIDEO_LOG"
        fi

        if [ -x "$VHOST_USER_VIDEO_DAEMON" ]; then
            echo "Starting vhost-user video daemon: $VHOST_USER_VIDEO_DAEMON --socket $VHOST_USER_VIDEO_SOCKET"
            if [ "$VHOST_USER_VIDEO_LOG" = "stderr" ]; then
                "$VHOST_USER_VIDEO_DAEMON" --socket "$VHOST_USER_VIDEO_SOCKET" &
            else
                "$VHOST_USER_VIDEO_DAEMON" --socket "$VHOST_USER_VIDEO_SOCKET" >"$VHOST_USER_VIDEO_LOG" 2>&1 &
            fi
        elif [ -f "$PROJECT_ROOT/tools/vhost-video-videotoolbox/Package.swift" ] && [ -x /usr/bin/xcrun ]; then
            echo "Starting vhost-user video daemon with swift run: $VHOST_USER_VIDEO_SOCKET"
            VHOST_USER_VIDEO_SOCKET_WAIT_ATTEMPTS="${SCARLET_VHOST_USER_VIDEO_SOCKET_WAIT_ATTEMPTS:-1200}"
            if [ "$VHOST_USER_VIDEO_LOG" = "stderr" ]; then
                (
                    cd "$PROJECT_ROOT/tools/vhost-video-videotoolbox" &&
                    mkdir -p .build/clang-module-cache &&
                    env -u DEVELOPER_DIR -u SDKROOT CLANG_MODULE_CACHE_PATH=.build/clang-module-cache /usr/bin/xcrun swift run --disable-sandbox vhost-video-videotoolbox --socket "$VHOST_USER_VIDEO_SOCKET"
                ) &
            else
                (
                    cd "$PROJECT_ROOT/tools/vhost-video-videotoolbox" &&
                    mkdir -p .build/clang-module-cache &&
                    env -u DEVELOPER_DIR -u SDKROOT CLANG_MODULE_CACHE_PATH=.build/clang-module-cache /usr/bin/xcrun swift run --disable-sandbox vhost-video-videotoolbox --socket "$VHOST_USER_VIDEO_SOCKET"
                ) >"$VHOST_USER_VIDEO_LOG" 2>&1 &
            fi
        else
            echo "Error: vhost-user video daemon not found or not executable: $VHOST_USER_VIDEO_DAEMON"
            echo "       Run cargo make build-vhost-video-videotoolbox or set SCARLET_VHOST_USER_VIDEO_DAEMON."
            exit 1
        fi

        VHOST_USER_VIDEO_PID=$!
        for ((i = 0; i < VHOST_USER_VIDEO_SOCKET_WAIT_ATTEMPTS; i++)); do
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

qemu-system-riscv64 \
    -machine "$QEMU_MACHINE" \
    -accel "$QEMU_ACCEL" \
    "${QEMU_MEMORY_ARGS[@]}" \
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
    -device virtio-net-pci,netdev=net0,bus=pcie.0 \
    "${QEMU_AUDIO_ARGS[@]}" \
    "${QEMU_VHOST_USER_VIDEO_ARGS[@]}" \
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
