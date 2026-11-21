#!/bin/sh
set -eu

project=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
profile=debug
if [ "${SCARLET_RELEASE:-0}" = 1 ]; then
    profile=release
fi

# cargo-scarlet runs Cargo with the BSP as its working directory.
target_dir=${CARGO_TARGET_DIR:-target}
case "$target_dir" in
    /*) ;;
    *) target_dir="$project/bsp/$target_dir" ;;
esac

exec "$target_dir/aarch64-apple-darwin/$profile/scarlet" "$@" \
    "$project/.scarlet/images/initramfs-aarch64-full.cpio" \
    "$project/.scarlet/images/rootfs-aarch64-full.ext2"
