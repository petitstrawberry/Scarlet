#!/usr/bin/env bash
set -euo pipefail

# Build and deploy Linux rootfs artifacts into the bundle.
#
# Usage:
#   ./prepare.sh [OPTIONS]
#
# Options:
#   --arch ARCH          aarch64 or riscv64 (default: aarch64)
#   --steps STEPS        Comma-separated steps to run (default: all)
#   --buildroot-dir DIR  Buildroot tree location
#   --prebuilt-dir DIR   Artifact staging directory (default: ./cache/prebuilt)
#   --workdir DIR        Checkout/build working directory (default: ./cache/work)
#   --make-jobs N        Parallelism for make (default: $(nproc))
#   -h, --help           Show this help
#
# Steps:
#   buildroot     Build Buildroot toolchain + rootfs tarball
#   guest-image   Build guest kernel + guest initramfs
#   user-programs Build zathura, green, fbdoom, kvmtool user programs
#   mozc          Build Mozc server
#   kvmtool       Build kvmtool (lkvm-static)
#   deploy        Extract rootfs + overlay prebuilt into bundle's rootfs/
#
# Step dependencies:
#   guest-image, user-programs, mozc, kvmtool all require buildroot first.
#   deploy requires buildroot (+ any optional steps for their artifacts).
#
# Examples:
#   ./prepare.sh                                          # everything, aarch64
#   ./prepare.sh --arch riscv64                           # everything, riscv64
#   ./prepare.sh --steps buildroot,deploy                 # only buildroot then deploy
#   ./prepare.sh --steps guest-image                      # only guest image (needs existing buildroot)
#   ./prepare.sh --steps buildroot,user-programs,mozc,deploy --arch aarch64
#
# This script is self-contained: running it with default options produces a
# fully populated rootfs/system/linux-<arch>/ tree that the bundle's copy layer
# picks up during cargo-scarlet image builds.
#
# macOS note: Buildroot and kernel builds require a Linux host. Run inside
# scarlet-dev Docker, a Linux VM, or a Linux Nix shell.

ARCH="${ARCH:-aarch64}"
STEPS="${STEPS:-all}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUNDLE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
CACHE_DIR="${SCARLET_LINUX_CACHE_DIR:-${BUNDLE_DIR}/cache}"
BUILDROOT_DIR="${BUILDROOT_DIR:-${CACHE_DIR}/buildroot-${ARCH}}"
PREBUILT_DIR="${PREBUILT_DIR:-${CACHE_DIR}/prebuilt}"
WORKDIR="${WORKDIR:-${CACHE_DIR}/work}"
MAKE_JOBS="${MAKE_JOBS:-$(nproc 2>/dev/null || echo 4)}"

ALL_STEPS=(buildroot guest-image user-programs mozc kvmtool deploy)

usage() {
    grep '^#' "${BASH_SOURCE[0]}" | head -n -1
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch)         ARCH="$2"; shift 2 ;;
        --steps)        STEPS="$2"; shift 2 ;;
        --buildroot-dir) BUILDROOT_DIR="$2"; shift 2 ;;
        --prebuilt-dir) PREBUILT_DIR="$2"; shift 2 ;;
        --workdir)      WORKDIR="$2"; shift 2 ;;
        --make-jobs)    MAKE_JOBS="$2"; shift 2 ;;
        -h|--help)      usage ;;
        *)              echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

export ARCH BUILDROOT_DIR PREBUILT_DIR WORKDIR MAKE_JOBS

if [[ "$STEPS" == "all" ]]; then
    selected=("${ALL_STEPS[@]}")
else
    IFS=',' read -ra selected <<< "$STEPS"
fi

for step in "${selected[@]}"; do
    echo "=== prepare.sh: step [$step] (arch=$ARCH) ==="
    case "$step" in
        buildroot)
            bash "${SCRIPT_DIR}/build_buildroot.sh"
            ;;
        guest-image)
            bash "${SCRIPT_DIR}/build_guest_image.sh"
            ;;
        user-programs)
            bash "${SCRIPT_DIR}/build_user_programs.sh"
            ;;
        mozc)
            bash "${SCRIPT_DIR}/build_mozc_server.sh"
            ;;
        kvmtool)
            bash "${SCRIPT_DIR}/build_kvmtool.sh"
            ;;
        deploy)
            ARCH="${ARCH}" PREBUILT_DIR="${PREBUILT_DIR}" \
            bash "${SCRIPT_DIR}/deploy_rootfs.sh"
            ;;
        *)
            echo "Unknown step: $step" >&2
            echo "Available: ${ALL_STEPS[*]}" >&2
            exit 1
            ;;
    esac
done

echo "=== prepare.sh: done ==="
echo "Steps run: ${selected[*]}"
echo "Architecture: $ARCH"
echo "Prebuilt dir: $PREBUILT_DIR"
echo "Buildroot dir: $BUILDROOT_DIR"
