#!/bin/sh
set -eu

if [ "$(uname -s)" != Linux ] || [ "$(uname -m)" != aarch64 ] ||
    [ "$(getconf PAGE_SIZE)" != 4096 ]; then
    echo "scarlet: this console project requires AArch64 Linux with 4096-byte pages" >&2
    exit 1
fi

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

exec "$target_dir/aarch64-unknown-linux-gnu/$profile/scarlet" "$@" \
    "$project/.scarlet/images/initramfs-aarch64-user-linux-minimum.cpio" \
    "$project/.scarlet/images/rootfs-aarch64-user-linux-minimum.ext2"
