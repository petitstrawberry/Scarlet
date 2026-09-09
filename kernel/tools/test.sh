#!/bin/bash

# Shared RISC-V kernel test runner. All boot artifacts stay in kernel/.test/.
DEBUG_MODE=false
ARCH=riscv64
KERNEL_BINARY=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --debug)
            DEBUG_MODE=true
            shift
            ;;
        --arch)
            if [[ $# -lt 2 ]]; then
                echo "Error: --arch requires riscv32 or riscv64" >&2
                exit 1
            fi
            ARCH="$2"
            shift 2
            ;;
        *)
            if [[ -n "$KERNEL_BINARY" ]]; then
                echo "Error: unexpected argument: $1" >&2
                exit 1
            fi
            KERNEL_BINARY="$1"
            shift
            ;;
    esac
done
case "$ARCH" in
    riscv32|riscv64) ;;
    *) echo "Error: unsupported test architecture: $ARCH" >&2; exit 1 ;;
esac
if [[ ! -f "$KERNEL_BINARY" ]]; then
    echo "Error: kernel test binary not found: $KERNEL_BINARY" >&2
    exit 1
fi
KERNEL_BINARY="$(cd "$(dirname "$KERNEL_BINARY")" && pwd)/$(basename "$KERNEL_BINARY")"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KERNEL_DIR="$(dirname "$SCRIPT_DIR")"
PROJECT_ROOT="$(dirname "$KERNEL_DIR")"
TEST_DIR="$KERNEL_DIR/.test"
mkdir -p "$TEST_DIR" || exit 1

echo "Test runner starting ($ARCH)..."
echo "Generating fresh FAT32 test image..."
"$KERNEL_DIR/tools/create-fat32-image.sh" || exit 1
echo "Generating fresh ext2 test image..."
"$KERNEL_DIR/tools/create-ext2-image.sh" || exit 1

# Shared disks are regenerated above; RISC-V and AArch64 test runs are serial.
LINK_PATH="$(dirname "$KERNEL_BINARY")/../test-kernel"
ln -sf "$KERNEL_BINARY" "$LINK_PATH" || exit 1
INITRAMFS="$TEST_DIR/initramfs-test.cpio"
echo "test" > "$TEST_DIR/test-marker"
(cd "$TEST_DIR" && echo "test-marker" | cpio -o -H newc > "$INITRAMFS" 2>/dev/null) || exit 1

BOOT_ARGS=()
if [[ "$ARCH" == riscv32 ]]; then
    BOOT_ARGS=(
        -bios "${SCARLET_QEMU_BIOS_RV32:-default}"
        -cpu "${SCARLET_QEMU_CPU_RV32:-rv32}"
        -m 512M
        -kernel "$KERNEL_BINARY"
        -initrd "$INITRAMFS"
    )
else
    BOOT_IMAGE="$TEST_DIR/limine-riscv64-boot.img"
    EFI_CODE="${SCARLET_EFI_CODE_RV64:-/usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd}"
    EFI_VARS="$TEST_DIR/RISCV_VIRT_VARS.fd"
    LIMINE_CACHE="$TEST_DIR/.cache"

    LIMINE_PLUGIN="$PROJECT_ROOT/cargo-scarlet-plugin-limine/target/release/cargo-scarlet-plugin-limine"
    if [[ ! -f "$LIMINE_PLUGIN" ]]; then
        LIMINE_PLUGIN="$PROJECT_ROOT/cargo-scarlet-plugin-limine/target/debug/cargo-scarlet-plugin-limine"
    fi
    if [[ ! -f "$LIMINE_PLUGIN" ]]; then
        LIMINE_PLUGIN="$PROJECT_ROOT/target/release/cargo-scarlet-plugin-limine"
    fi
    if [[ ! -f "$LIMINE_PLUGIN" ]]; then
        LIMINE_PLUGIN="$PROJECT_ROOT/target/debug/cargo-scarlet-plugin-limine"
    fi
    if [[ ! -f "$LIMINE_PLUGIN" ]]; then
        LIMINE_PLUGIN="$(command -v cargo-scarlet-plugin-limine || true)"
    fi
    if [[ ! -f "$LIMINE_PLUGIN" ]]; then
        echo "Error: cargo-scarlet-plugin-limine not found; use the Nix development shell" >&2
        exit 1
    fi
    "$LIMINE_PLUGIN" \
        --arch riscv64 \
        --kernel "$KERNEL_BINARY" \
        --initramfs "$INITRAMFS" \
        --output "$BOOT_IMAGE" \
        --cache-dir "$LIMINE_CACHE" || exit 1

    if [[ ! -f "$EFI_CODE" ]]; then
        echo "Error: RISC-V EFI firmware not found at $EFI_CODE" >&2
        exit 1
    fi
    if [[ ! -f "$EFI_VARS" ]]; then
        cp "${SCARLET_EFI_VARS_RV64:-/usr/share/qemu-efi-riscv64/RISCV_VIRT_VARS.fd}" "$EFI_VARS" || exit 1
    fi
    chmod u+w "$EFI_VARS" || exit 1
    BOOT_ARGS=(
        -bios default
        -m 4G
        -drive "if=pflash,format=raw,unit=0,file=$EFI_CODE,readonly=on"
        -drive "if=pflash,format=raw,unit=1,file=$EFI_VARS"
        -drive "id=boot,file=$BOOT_IMAGE,format=raw,if=none"
        -device virtio-blk-pci,drive=boot,bus=pcie.0
    )
fi

QEMU_ARGS=(
    -machine virt,acpi=off
    "${BOOT_ARGS[@]}"
    -smp 1
    -nographic
    -serial mon:stdio
    --no-reboot
    -global virtio-mmio.force-legacy=false
    -drive "id=x0,file=$KERNEL_DIR/fat32-test.img,format=raw,if=none"
    -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0
    -drive "id=x1,file=$KERNEL_DIR/ext2-test.img,format=raw,if=none"
    -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.5
    -display "${SCARLET_QEMU_DISPLAY:-vnc=:0}"
    -device virtio-gpu-device,bus=virtio-mmio-bus.1
    -netdev user,id=net0
    -netdev hubport,id=net1,hubid=0
    -netdev hubport,id=net2,hubid=0
    -device virtio-net-device,netdev=net0,mac=52:54:00:12:34:56,bus=virtio-mmio-bus.2
    -device virtio-net-device,netdev=net1,mac=52:54:00:12:34:57,bus=virtio-mmio-bus.3
    -device virtio-net-device,netdev=net2,mac=52:54:00:12:34:58,bus=virtio-mmio-bus.4
    -netdev user,id=pci-net0
    -device virtio-net-pci,netdev=pci-net0,mac=52:54:00:AB:CD:EF,bus=pcie.0
    -device qemu-xhci,id=xhci,bus=pcie.0
    -device usb-kbd,bus=xhci.0
    -device usb-mouse,bus=xhci.0
)
if [[ "${SCARLET_QEMU_DISABLE_VIRTIO_INPUT:-0}" == 1 || "${SCARLET_QEMU_DISABLE_VIRTIO_INPUT:-}" == true ]]; then
    echo "QEMU input configuration: USB HID only (virtio input disabled)"
else
    QEMU_ARGS+=(
        -device virtio-keyboard-device,bus=virtio-mmio-bus.6
        -device virtio-mouse-device,bus=virtio-mmio-bus.7
    )
fi
if [[ "$DEBUG_MODE" == true ]]; then
    echo "Connect with: gdb $KERNEL_BINARY -ex 'target remote :12345'"
    QEMU_ARGS+=(-gdb tcp::12345 -S)
fi

QEMU_DEBUG_FLAGS="${SCARLET_QEMU_DEBUG_FLAGS:-}"
if [[ -z "$QEMU_DEBUG_FLAGS" && ( "${SCARLET_QEMU_GUEST_ERRORS:-0}" == 1 || "${SCARLET_QEMU_GUEST_ERRORS:-}" == true ) ]]; then
    QEMU_DEBUG_FLAGS=guest_errors
fi
if [[ -n "$QEMU_DEBUG_FLAGS" ]]; then
    if [[ "$QEMU_DEBUG_FLAGS" == guest_errors ]]; then
        QEMU_DEBUG_LOG="${SCARLET_QEMU_GUEST_ERRORS_LOG:-$PROJECT_ROOT/qemu-guest-errors-$ARCH.log}"
    else
        QEMU_DEBUG_LOG="${SCARLET_QEMU_DEBUG_LOG:-$PROJECT_ROOT/qemu-debug-$ARCH.log}"
    fi
    QEMU_ARGS+=(-d "$QEMU_DEBUG_FLAGS" -D "$QEMU_DEBUG_LOG")
fi

TEMP_OUTPUT=$(mktemp) || exit 1
trap 'rm -f "$TEMP_OUTPUT"' EXIT
"qemu-system-$ARCH" "${QEMU_ARGS[@]}" | tee "$TEMP_OUTPUT"
QEMU_EXIT_CODE=${PIPESTATUS[0]}
if [[ "$QEMU_EXIT_CODE" -ne 0 ]]; then
    echo "QEMU failed with exit code: $QEMU_EXIT_CODE"
    exit 1
elif grep -q '\[Test Runner\] Test failed' "$TEMP_OUTPUT"; then
    echo "Test failure detected in output"
    exit 1
elif grep -q '\[Test Runner\] All .* tests passed' "$TEMP_OUTPUT"; then
    echo "All tests passed"
    exit 0
elif grep -q 'running 0 tests' "$TEMP_OUTPUT"; then
    echo "No tests were run"
    exit 0
else
    echo "Could not determine test result, QEMU exit code: $QEMU_EXIT_CODE"
    exit 1
fi
