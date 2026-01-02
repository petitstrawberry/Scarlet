#!/bin/bash
set -euo pipefail

# Builds Buildroot using the repository-provided configuration.
# Environment variables:
#  ARCH - target architecture (riscv64 or aarch64), defaults to riscv64

: "${ARCH:=riscv64}"
: "${BUILDROOT_DIR:=/opt/buildroot}"
: "${PREBUILT_DIR:=/opt/prebuilt}"
: "${IMAGES_DIR:=output/images}"
: "${MAKE_JOBS:=$(nproc)}"
: "${CONFIG_SOURCE:=/workspaces/Scarlet/tools/linux/configs/buildroot_config_${ARCH}}"

if [[ ! -d "${BUILDROOT_DIR}" ]]; then
    echo "Expected buildroot directory at ${BUILDROOT_DIR} not found." >&2
    exit 1
fi

if [[ ! -f "${CONFIG_SOURCE}" ]]; then
    echo "Buildroot config not found at ${CONFIG_SOURCE}" >&2
    echo "Available configs:" >&2
    ls -1 /workspaces/Scarlet/tools/linux/configs/buildroot_config_* 2>/dev/null || echo "  (none found)" >&2
    exit 1
fi

echo "Building Buildroot for ${ARCH}"
echo "Using config: ${CONFIG_SOURCE}"

pushd "${BUILDROOT_DIR}" >/dev/null

# Clean to avoid cross-architecture build issues
echo "Cleaning Buildroot build directory..."
make clean

# Copy architecture-specific config
cp "${CONFIG_SOURCE}" .config

# Build
make -j "${MAKE_JOBS}"

if [[ ! -d "${IMAGES_DIR}" ]]; then
    echo "Images directory ${BUILDROOT_DIR}/${IMAGES_DIR} missing after build." >&2
    exit 1
fi

pushd "${IMAGES_DIR}" >/dev/null
rm -rf "linux-${ARCH}"
mkdir "linux-${ARCH}"
if [[ ! -f rootfs.tar ]]; then
    echo "rootfs.tar not found in ${BUILDROOT_DIR}/${IMAGES_DIR}." >&2
    exit 1
fi

tar -xf rootfs.tar -C "linux-${ARCH}"
mkdir -p "${PREBUILT_DIR}"
cp rootfs.tar "${PREBUILT_DIR}/linux-${ARCH}.tar"

popd >/dev/null
popd >/dev/null

echo "Buildroot artifacts for ${ARCH} staged under ${PREBUILT_DIR}/linux-${ARCH}.tar"
