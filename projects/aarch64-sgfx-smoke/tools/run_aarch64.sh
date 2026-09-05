#!/usr/bin/env bash
set -eu

SCARLET_QEMU_PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export SCARLET_QEMU_PROJECT_DIR
export SCARLET_QEMU_GPU="${SCARLET_QEMU_GPU:-virtio-gpu-gl-pci}"
export SCARLET_QEMU_SNAPSHOT="${SCARLET_QEMU_SNAPSHOT:-1}"
export SCARLET_QEMU_NET="${SCARLET_QEMU_NET:-0}"
if [ -z "${SCARLET_QEMU_DISPLAY:-}" ]; then
    if [ "$(uname -s)" = "Darwin" ]; then
        export SCARLET_QEMU_DISPLAY="cocoa,gl=on"
    else
        echo "Set SCARLET_QEMU_DISPLAY to an available GL-capable display (see qemu-system-aarch64 -display help)." >&2
        exit 1
    fi
fi
exec "$SCARLET_QEMU_PROJECT_DIR/../aarch64-limine-full/tools/run_aarch64.sh" "$@"
