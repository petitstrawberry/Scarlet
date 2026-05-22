#!/bin/sh
set -eu

# cd to the script directory
cd "$(dirname "$0")" || exit 1

# Create ext2 image from rootfs directory
ROOTFS_DIR="rootfs"
EXT2_IMAGE="dist/rootfs.img"

# Block size (can be overridden by environment variable)
BLOCK_SIZE=${EXT2_BLOCK_SIZE:-4096}

if [ ! -d "$ROOTFS_DIR" ]; then
    echo "Error: $ROOTFS_DIR directory not found"
    echo "Please create the rootfs directory and populate it with your files"
    exit 1
fi

# Calculate source directory size (for logging/reference)
ROOTFS_SIZE_KB=$(du -sk "$ROOTFS_DIR" | cut -f1)

# Use a fixed 8 GiB ext2 image (can be overridden via EXT2_SIZE_KB_OVERRIDE)
if [ -n "${EXT2_SIZE_KB_OVERRIDE:-}" ]; then
    EXT2_SIZE_KB="$EXT2_SIZE_KB_OVERRIDE"
else
    EXT2_SIZE_KB=$((8 * 1024 * 1024))
fi

echo "Creating ext2 image: $EXT2_IMAGE (${EXT2_SIZE_KB}KB)"
echo "Source rootfs directory size: ${ROOTFS_SIZE_KB}KB"

# Create directory for output if it doesn't exist
mkdir -p "$(dirname "$EXT2_IMAGE")"
ROOTFS_TAR="$(dirname "$EXT2_IMAGE")/rootfs-input.tar"
trap 'rm -f "$ROOTFS_TAR"' EXIT

# Create ext2 filesystem with optimized parameters
echo "Using block size: ${BLOCK_SIZE} bytes"
dd if=/dev/zero of="$EXT2_IMAGE" bs=$BLOCK_SIZE count=$((EXT2_SIZE_KB * 1024 / BLOCK_SIZE))
echo "Packing rootfs without host extended attributes"
tar --no-xattrs --numeric-owner --owner=0 --group=0 -cf "$ROOTFS_TAR" -C "$ROOTFS_DIR" .
# Use smaller inode ratio (2048) to support more files and directories. Feeding
# mke2fs a tarball avoids copying host extended attributes from the source tree
# while still preserving symlinks and file modes.
mke2fs -F -t ext2 -b $BLOCK_SIZE -i 2048 -m 1 -L "SCARLET_ROOT" -d "$ROOTFS_TAR" "$EXT2_IMAGE"

echo "ext2 rootfs created successfully: $EXT2_IMAGE"
