#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KERNEL_DIR="$(dirname "$SCRIPT_DIR")"
PROJECT_ROOT="$(cd "$KERNEL_DIR" && cd .. && pwd)"

KERNEL_PATH="${1:-$KERNEL_DIR/target/x86_64-unknown-none-elf/release/kernel}"
INITRAMFS_PATH="$PROJECT_ROOT/mkfs/dist/initramfs-x86_64.cpio"
OUTPUT_DIR="$PROJECT_ROOT/mkfs/dist/uefi-x86_64"
DISK_IMAGE="$OUTPUT_DIR/scarlet-x86_64.img"

LIMINE_DIR="/opt/limine"

if [ ! -f "$KERNEL_PATH" ]; then
    echo "Error: Kernel not found at $KERNEL_PATH"
    exit 1
fi

if [ ! -f "$INITRAMFS_PATH" ]; then
    echo "Error: Initramfs not found at $INITRAMFS_PATH"
    echo "Run: cargo make build-initramfs-x86_64"
    exit 1
fi

echo "Creating UEFI bootable disk image..."
echo "  Kernel: $KERNEL_PATH"
echo "  Initramfs: $INITRAMFS_PATH"

rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

dd if=/dev/zero of="$DISK_IMAGE" bs=1M count=64 status=none

mkfs.vfat -F 32 "$DISK_IMAGE" >/dev/null 2>&1

MTOOLS_SKIP_CHECK=1 mmd -i "$DISK_IMAGE" ::/EFI
MTOOLS_SKIP_CHECK=1 mmd -i "$DISK_IMAGE" ::/EFI/BOOT
MTOOLS_SKIP_CHECK=1 mmd -i "$DISK_IMAGE" ::/boot

MTOOLS_SKIP_CHECK=1 mcopy -i "$DISK_IMAGE" "$KERNEL_PATH" ::/boot/kernel
MTOOLS_SKIP_CHECK=1 mcopy -i "$DISK_IMAGE" "$INITRAMFS_PATH" ::/boot/initramfs.cpio
MTOOLS_SKIP_CHECK=1 mcopy -i "$DISK_IMAGE" "$LIMINE_DIR/bin/BOOTX64.EFI" ::/EFI/BOOT/BOOTX64.EFI

TMP_CONF=$(mktemp)
cat > "$TMP_CONF" << 'EOF'
timeout: 3

/Scarlet
    protocol: limine
    kernel_path: boot():/boot/kernel
    kernel_cmdline:

    module_path: boot():/boot/initramfs.cpio
    module_cmdline: initramfs
EOF

MTOOLS_SKIP_CHECK=1 mcopy -i "$DISK_IMAGE" "$TMP_CONF" ::/boot/limine.conf
rm -f "$TMP_CONF"

echo ""
echo "Created UEFI disk image: $DISK_IMAGE"
echo "Size: $(du -h "$DISK_IMAGE" | cut -f1)"
echo ""
echo "Run with:"
echo "  qemu-system-x86_64 -machine q35 -cpu qemu64 -m 512M \\"
echo "    -bios /usr/share/OVMF/OVMF_CODE_4M.fd \\"
echo "    -drive file=$DISK_IMAGE,format=raw,if=none,id=boot \\"
echo "    -device virtio-blk-pci,drive=boot \\"
echo "    -nographic -serial mon:stdio"
