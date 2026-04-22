#!/bin/sh

set -eu

# cd to the script directory
cd "$(dirname "$0")" || exit 1

ARCH=${1:-riscv64}
USER_DIST_DIR="../user/bin/dist/${ARCH}"

if [ ! -d "$USER_DIST_DIR" ]; then
	echo "Error: user dist directory not found: $USER_DIST_DIR" >&2
	echo "Hint: build user programs first (e.g. cargo make build-userbin-${ARCH})" >&2
	exit 1
fi

mkdir -p initramfs/system/scarlet/bin
rm -f initramfs/system/scarlet/bin/*

# Fail fast if there are no artifacts
if ! ls "$USER_DIST_DIR"/* >/dev/null 2>&1; then
	echo "Error: no user program artifacts in: $USER_DIST_DIR" >&2
	exit 1
fi

for binary_path in "$USER_DIST_DIR"/*; do
    if [ -f "$binary_path" ]; then
        case "$binary_path" in
            *.debug) continue ;;
        esac
        cp "$binary_path" initramfs/system/scarlet/bin/
    fi
done

# --- Kernel modules (.lsm files) ---
mkdir -p initramfs/system/scarlet/modules
rm -f initramfs/system/scarlet/modules/*.lsm

case "$ARCH" in
    riscv64) MODULE_TARGET="riscv64gc-unknown-none-elf" ;;
    aarch64) MODULE_TARGET="aarch64-unknown-none-elf" ;;
    *)       MODULE_TARGET="" ;;
esac
MODULES_DIST_DIR="dist/modules/${MODULE_TARGET}"
if [ -n "$MODULE_TARGET" ] && [ -d "$MODULES_DIST_DIR" ]; then
    for lsm_path in "$MODULES_DIST_DIR"/*.lsm; do
        if [ -f "$lsm_path" ]; then
            cp "$lsm_path" "initramfs/system/scarlet/modules/"
        fi
    done
fi

mkdir -p dist
cd initramfs || exit 1

OUT="../dist/initramfs-${ARCH}.cpio"
find . | cpio -o -H newc > "$OUT"
