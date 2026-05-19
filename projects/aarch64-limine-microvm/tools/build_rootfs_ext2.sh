#!/bin/sh
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

if [ "$#" -lt 5 ] || [ "$#" -gt 6 ]; then
    echo "usage: $0 <rootfs-base> <scarlet-bin-dir> <modules-dir> <stage-dir> <output-image> [project-prebuilt-dir]" >&2
    exit 2
fi

ROOTFS_BASE="$1"
SCARLET_BIN_DIR="$2"
MODULES_DIR="$3"
STAGE_DIR="$4"
OUTPUT_IMAGE="$5"
PROJECT_PREBUILT_DIR="${6:-}"

BLOCK_SIZE="${SCARLET_EXT2_BLOCK_SIZE:-4096}"
LABEL="${SCARLET_EXT2_LABEL:-SCARLET_ROOT}"
EXTRA_KB="${SCARLET_EXT2_EXTRA_KB:-65536}"
FIRECRACKER_VERSION="${SCARLET_FIRECRACKER_VERSION:-1.15.1}"
FIRECRACKER_ARCHIVE="firecracker-v${FIRECRACKER_VERSION}-aarch64.tgz"
FIRECRACKER_URL="${SCARLET_FIRECRACKER_URL:-https://github.com/firecracker-microvm/firecracker/releases/download/v${FIRECRACKER_VERSION}/${FIRECRACKER_ARCHIVE}}"
FIRECRACKER_CACHE_DIR="${SCARLET_FIRECRACKER_CACHE_DIR:-$ROOTFS_BASE/../cache/firecracker}"

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

if [ -n "$PROJECT_PREBUILT_DIR" ] && [ -d "$PROJECT_PREBUILT_DIR" ]; then
    cp -a "$PROJECT_PREBUILT_DIR"/. "$STAGE_DIR"/
fi

ensure_firecracker() {
    target="$STAGE_DIR/system/linux-aarch64/usr/bin/firecracker"
    if [ -x "$target" ]; then
        return 0
    fi

    if [ -n "$PROJECT_PREBUILT_DIR" ]; then
        echo "microvm rootfs is missing Firecracker in project prebuilt artifacts" >&2
        echo "Run: $SCRIPT_DIR/prepare_prebuilt.sh" >&2
        exit 1
    fi

    if ! command -v curl >/dev/null 2>&1; then
        echo "microvm rootfs is missing Firecracker and curl is not available to download it" >&2
        exit 1
    fi

    if ! command -v tar >/dev/null 2>&1; then
        echo "microvm rootfs is missing Firecracker and tar is not available to extract it" >&2
        exit 1
    fi

    mkdir -p "$FIRECRACKER_CACHE_DIR"
    archive="$FIRECRACKER_CACHE_DIR/$FIRECRACKER_ARCHIVE"
    if [ ! -f "$archive" ]; then
        echo "Downloading Firecracker v$FIRECRACKER_VERSION for aarch64..."
        curl -fL "$FIRECRACKER_URL" -o "$archive"
    fi

    tmp_dir="$FIRECRACKER_CACHE_DIR/extract-v$FIRECRACKER_VERSION"
    rm -rf "$tmp_dir"
    mkdir -p "$tmp_dir"
    tar -xzf "$archive" -C "$tmp_dir"

    binary="$(find "$tmp_dir" -type f -name "firecracker-v${FIRECRACKER_VERSION}-aarch64" | head -n 1)"
    if [ -z "$binary" ]; then
        echo "Firecracker binary not found in $archive" >&2
        exit 1
    fi

    mkdir -p "$(dirname "$target")"
    cp "$binary" "$target"
    chmod +x "$target"
}

write_firecracker_config() {
    config_dir="$STAGE_DIR/system/linux-aarch64/etc/firecracker"
    mkdir -p "$config_dir"
    cat > "$config_dir/scarlet-microvm-aarch64.json" <<'EOF'
{
  "boot-source": {
    "kernel_image_path": "/usr/bin/guest-Image",
    "initrd_path": "/usr/bin/guest-initramfs.cpio.gz",
    "boot_args": "console=ttyS0 reboot=k panic=1 pci=off rdinit=/sbin/init"
  },
  "machine-config": {
    "vcpu_count": 1,
    "mem_size_mib": 512
  },
  "drives": []
}
EOF
}

ensure_firecracker
write_firecracker_config

for required in \
    "$STAGE_DIR/system/linux-aarch64/usr/bin/firecracker" \
    "$STAGE_DIR/system/linux-aarch64/etc/firecracker/scarlet-microvm-aarch64.json" \
    "$STAGE_DIR/system/linux-aarch64/usr/bin/guest-Image" \
    "$STAGE_DIR/system/linux-aarch64/usr/bin/guest-initramfs.cpio.gz"
do
    if [ ! -f "$required" ]; then
        echo "microvm rootfs is missing required artifact: ${required#$STAGE_DIR/}" >&2
        if [ -n "$PROJECT_PREBUILT_DIR" ]; then
            echo "Run: $SCRIPT_DIR/prepare_prebuilt.sh" >&2
        else
            echo "Build and deploy AArch64 Linux artifacts before building the microvm image." >&2
        fi
        exit 1
    fi
done

mkdir -p "$(dirname "$OUTPUT_IMAGE")"

SOURCE_KB="$(du -sk "$STAGE_DIR" | cut -f1)"
SIZE_KB="$((SOURCE_KB + SOURCE_KB / 3 + EXTRA_KB))"
ALIGN_KB=16384
SIZE_KB="$((((SIZE_KB + ALIGN_KB - 1) / ALIGN_KB) * ALIGN_KB))"

echo "Creating ext2 rootfs: $OUTPUT_IMAGE (${SIZE_KB}KB, source=${SOURCE_KB}KB)"
rm -f "$OUTPUT_IMAGE"
truncate -s "${SIZE_KB}K" "$OUTPUT_IMAGE"
mke2fs -q -F -t ext2 -b "$BLOCK_SIZE" -i 2048 -m 1 -L "$LABEL" -E no_copy_xattrs -d "$STAGE_DIR" "$OUTPUT_IMAGE"
echo "ext2 rootfs created: $OUTPUT_IMAGE"
