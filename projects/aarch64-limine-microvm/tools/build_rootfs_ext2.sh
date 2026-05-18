#!/bin/sh
set -eu

if [ "$#" -ne 5 ]; then
    echo "usage: $0 <rootfs-base> <scarlet-bin-dir> <modules-dir> <stage-dir> <output-image>" >&2
    exit 2
fi

ROOTFS_BASE="$1"
SCARLET_BIN_DIR="$2"
MODULES_DIR="$3"
STAGE_DIR="$4"
OUTPUT_IMAGE="$5"

BLOCK_SIZE="${SCARLET_EXT2_BLOCK_SIZE:-4096}"
LABEL="${SCARLET_EXT2_LABEL:-SCARLET_ROOT}"
EXTRA_KB="${SCARLET_EXT2_EXTRA_KB:-65536}"

if [ ! -d "$ROOTFS_BASE" ]; then
    echo "rootfs base not found: $ROOTFS_BASE" >&2
    exit 1
fi

if [ ! -d "$SCARLET_BIN_DIR" ]; then
    echo "Scarlet binary directory not found: $SCARLET_BIN_DIR" >&2
    exit 1
fi

rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR"
cp -a "$ROOTFS_BASE"/. "$STAGE_DIR"/

mkdir -p "$STAGE_DIR/system/scarlet/bin"
find "$SCARLET_BIN_DIR" -maxdepth 1 -type f ! -name '*.debug' -exec cp -a {} "$STAGE_DIR/system/scarlet/bin/" \;

if [ -d "$MODULES_DIR" ]; then
    mkdir -p "$STAGE_DIR/system/scarlet/modules"
    cp -a "$MODULES_DIR"/. "$STAGE_DIR/system/scarlet/modules"/
fi

mkdir -p "$(dirname "$OUTPUT_IMAGE")"

SOURCE_KB="$(du -sk "$STAGE_DIR" | cut -f1)"
SIZE_KB="$((SOURCE_KB + SOURCE_KB / 3 + EXTRA_KB))"
ALIGN_KB=16384
SIZE_KB="$((((SIZE_KB + ALIGN_KB - 1) / ALIGN_KB) * ALIGN_KB))"

echo "Creating ext2 rootfs: $OUTPUT_IMAGE (${SIZE_KB}KB, source=${SOURCE_KB}KB)"
rm -f "$OUTPUT_IMAGE"
truncate -s "${SIZE_KB}K" "$OUTPUT_IMAGE"
mke2fs -q -F -t ext2 -b "$BLOCK_SIZE" -i 2048 -m 1 -L "$LABEL" -d "$STAGE_DIR" "$OUTPUT_IMAGE"
echo "ext2 rootfs created: $OUTPUT_IMAGE"
