#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_DIR="$(cd "$PROJECT_DIR/../.." && pwd)"
PROFILE=debug
if [[ "${SCARLET_RELEASE:-0}" == 1 ]]; then
    PROFILE=release
fi
KERNEL="$PROJECT_DIR/bsp/target/riscv32ima-unknown-none-elf/$PROFILE/scarlet"
if [[ $# -gt 0 && "$1" != -* ]]; then
    KERNEL=$1
    shift
fi
DEBUG_ARGS=()
if [[ "${1:-}" == --debug ]]; then
    DEBUG_ARGS=(-S -gdb "tcp::${SCARLET_GDB_PORT:-12345}")
    shift
fi
INITRAMFS="$PROJECT_DIR/.scarlet/images/initramfs-riscv32-sbi-desktop.cpio"
ROOTFS="$PROJECT_DIR/.scarlet/images/rootfs-riscv32-sbi-desktop.ext2"
for artifact in "$KERNEL" "$INITRAMFS" "$ROOTFS"; do
    if [[ ! -f "$artifact" ]]; then
        echo "Missing $artifact; build this project's images first" >&2
        exit 1
    fi
done

QEMU="${SCARLET_QEMU_BINARY:-qemu-system-riscv32}"
source "$REPO_DIR/tools/qemu-display.sh"
DISPLAY_BACKEND="$(scarlet_qemu_display "$QEMU")"
GPU="$(scarlet_qemu_gpu "$DISPLAY_BACKEND")"
# SBI supplies no PCI BAR assignments. Use the corresponding MMIO transports,
# which also avoid reserving a 256 MiB PCI ECAM window in the Sv32 address space.
case "$GPU" in
    virtio-gpu-pci) GPU=virtio-gpu-device ;;
    virtio-gpu-gl-pci) GPU=virtio-gpu-gl-device ;;
esac
GPU_ARGS=()
if [[ "$GPU" != none ]]; then
    GPU_ARGS=(-device "$GPU,bus=virtio-mmio-bus.2")
fi
NET_ARGS=(-nic none)
if [[ "${SCARLET_QEMU_NET:-1}" == 1 ]]; then
    NETDEV=user,id=net0
    IFS=, read -ra FORWARDS <<< "${SCARLET_QEMU_HOSTFWD-tcp::8080-:8080,udp::8080-:8080,udp::1234-:1234}"
    for forward in "${FORWARDS[@]}"; do
        [[ -z "$forward" ]] || NETDEV+=",hostfwd=$forward"
    done
    NET_ARGS=(-netdev "$NETDEV" -device virtio-net-device,netdev=net0,bus=virtio-mmio-bus.1)
fi
INPUT_ARGS=()
if [[ "${SCARLET_QEMU_INPUT:-1}" == 1 ]]; then
    INPUT_ARGS=(-device virtio-keyboard-device,bus=virtio-mmio-bus.3
                -device virtio-mouse-device,bus=virtio-mmio-bus.4)
fi
AUDIO_ARGS=()
if [[ "${SCARLET_QEMU_AUDIO:-0}" == 1 ]]; then
    AUDIO_DRIVER="${SCARLET_QEMU_AUDIO_DRIVER:-coreaudio}"
    AUDIO_ARGS=(-audiodev "$AUDIO_DRIVER,id=audio0"
                -device virtio-sound-device,audiodev=audio0,bus=virtio-mmio-bus.6)
fi
CONTROL_ARGS=()
if [[ -n "${SCARLET_QEMU_QMP:-}" ]]; then
    CONTROL_ARGS+=(-qmp "unix:$SCARLET_QEMU_QMP,server=on,wait=off")
fi
if [[ "${SCARLET_QEMU_SNAPSHOT:-0}" == 1 ]]; then
    CONTROL_ARGS+=(-snapshot)
fi

exec "$QEMU" -machine virt,acpi=off -accel tcg \
    -cpu "${SCARLET_QEMU_CPU_RV32:-rv32}" \
    -smp "${SCARLET_QEMU_SMP:-4}" -m "${SCARLET_QEMU_MEMORY:-768M}" \
    -bios "${SCARLET_QEMU_BIOS_RV32:-default}" \
    -kernel "$KERNEL" -initrd "$INITRAMFS" \
    -append "${SCARLET_CMDLINE:-console=ttyS0 root=/dev/vblk0 rootfstype=ext2}" \
    -serial "${SCARLET_QEMU_SERIAL:-mon:stdio}" -display "$DISPLAY_BACKEND" \
    -global virtio-mmio.force-legacy=false -no-reboot \
    -drive "id=rootfs,file=$ROOTFS,format=raw,if=none" \
    -device virtio-blk-device,drive=rootfs,bus=virtio-mmio-bus.0 \
    -device virtio-rng-device,bus=virtio-mmio-bus.5 \
    "${GPU_ARGS[@]}" "${NET_ARGS[@]}" "${INPUT_ARGS[@]}" "${AUDIO_ARGS[@]}" \
    "${CONTROL_ARGS[@]}" "${DEBUG_ARGS[@]}" "$@"
