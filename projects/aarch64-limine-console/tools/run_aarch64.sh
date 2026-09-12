#!/usr/bin/env bash
# Reuse the full distribution runner with this project's isolated images.
set -euo pipefail
console_project="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export SCARLET_QEMU_PROJECT_DIR="${SCARLET_QEMU_PROJECT_DIR:-$console_project}"
export SCARLET_QEMU_BOOT_IMAGE="${SCARLET_QEMU_BOOT_IMAGE:-$SCARLET_QEMU_PROJECT_DIR/.scarlet/images/limine-aarch64-console.img}"
export SCARLET_QEMU_ROOTFS_IMAGE="${SCARLET_QEMU_ROOTFS_IMAGE:-$SCARLET_QEMU_PROJECT_DIR/.scarlet/images/rootfs-aarch64-console.ext2}"
exec bash "$console_project/../aarch64-limine-full/tools/run_aarch64.sh" "$@"
