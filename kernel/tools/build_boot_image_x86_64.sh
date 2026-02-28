#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KERNEL_DIR="$(dirname "$SCRIPT_DIR")"
PROJECT_ROOT="$(cd "$KERNEL_DIR" && cd .. && pwd)"

KERNEL_PATH="${1:-$KERNEL_DIR/target/x86_64-unknown-none-elf/release/kernel}"
INITRAMFS_PATH="$PROJECT_ROOT/mkfs/dist/initramfs-x86_64.cpio"
OUTPUT_IMAGE="$PROJECT_ROOT/mkfs/dist/scarlet-x86_64.img"

if [ ! -f "$KERNEL_PATH" ]; then
    echo "Error: Kernel not found at $KERNEL_PATH"
    exit 1
fi

if [ ! -f "$INITRAMFS_PATH" ]; then
    echo "Error: Initramfs not found at $INITRAMFS_PATH"
    echo "Run: cargo make build-initramfs-x86_64"
    exit 1
fi

echo "Creating bootable disk image..."
echo "  Kernel: $KERNEL_PATH"
echo "  Initramfs: $INITRAMFS_PATH"

rm -f "$OUTPUT_IMAGE"

dd if=/dev/zero of="$OUTPUT_IMAGE" bs=1M count=64 status=none

mkfs.ext4 -F "$OUTPUT_IMAGE" >/dev/null 2>&1

MOUNT_DIR=$(mktemp -d)
cleanup() {
    sudo umount "$MOUNT_DIR" 2>/dev/null || true
    rmdir "$MOUNT_DIR" 2>/dev/null || true
}
trap cleanup EXIT

sudo mount -o loop "$OUTPUT_IMAGE" "$MOUNT_DIR"

sudo mkdir -p "$MOUNT_DIR/boot/grub"
sudo cp "$KERNEL_PATH" "$MOUNT_DIR/boot/kernel"
sudo cp "$INITRAMFS_PATH" "$MOUNT_DIR/boot/initramfs.cpio"
sudo cp "$KERNEL_DIR/boot/grub/grub.cfg" "$MOUNT_DIR/boot/grub/grub.cfg"

sudo grub-install --target=x86_64-efi --efi-directory="$MOUNT_DIR" \
    --boot-directory="$MOUNT_DIR/boot" --removable --no-nvram \
    --no-floppy --modules="part_gpt part_msdos ext2 multiboot2" \
    "$OUTPUT_IMAGE" 2>/dev/null || true

sudo grub-install --target=i386-pc --boot-directory="$MOUNT_DIR/boot" \
    --modules="part_msdos ext2 multiboot2" \
    "$OUTPUT_IMAGE" 2>/dev/null || true

sudo umount "$MOUNT_DIR"
rmdir "$MOUNT_DIR"
trap - EXIT

echo "Created bootable image: $OUTPUT_IMAGE"
echo "Size: $(du -h "$OUTPUT_IMAGE" | cut -f1)"
