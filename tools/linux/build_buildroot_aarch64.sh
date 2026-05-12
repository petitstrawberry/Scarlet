#!/bin/bash
set -euo pipefail

# Build Buildroot toolchain and rootfs for AArch64 (arm64).
#
# Clones a separate Buildroot tree to /opt/buildroot-aarch64 so it does not
# interfere with the existing riscv64 build at /opt/buildroot.
#
# Prerequisites:
#   - Host build tools (gcc, make, etc.)
#   - git
#
# Produces:
#   ${BUILDROOT_DIR}/output/host/bin/aarch64-buildroot-linux-musl-gcc
#   ${PREBUILT_DIR}/linux-aarch64.tar
#
# Environment variables:
#  BUILDROOT_DIR - defaults to /opt/buildroot-aarch64
#  PREBUILT_DIR  - defaults to /opt/prebuilt
#  MAKE_JOBS     - defaults to $(nproc)

: "${BUILDROOT_DIR:=/opt/buildroot-aarch64}"
: "${PREBUILT_DIR:=/opt/prebuilt}"
: "${MAKE_JOBS:=$(nproc)}"

if [[ -d "${BUILDROOT_DIR}/.git" ]]; then
    echo "==> Updating existing Buildroot tree..."
    git -C "${BUILDROOT_DIR}" pull --ff-only || true
else
    echo "==> Cloning Buildroot..."
    git clone --depth=1 https://github.com/buildroot/buildroot.git "${BUILDROOT_DIR}"
fi

pushd "${BUILDROOT_DIR}" >/dev/null

echo "==> Configuring Buildroot for aarch64..."
make qemu_aarch64_virt_defconfig

utils/config --enable BR2_TOOLCHAIN_BUILDROOT_MUSL
utils/config --enable BR2_TARGET_ROOTFS_TAR
utils/config --disable BR2_TARGET_ROOTFS_EXT2
utils/config --disable BR2_LINUX_KERNEL
utils/config --disable BR2_PACKAGE_HOST_QEMU
utils/config --disable BR2_PACKAGE_HOST_QEMU_SYSTEM_MODE
make olddefconfig

echo "==> Building Buildroot toolchain + rootfs for aarch64 (this takes a while)..."
make -j "${MAKE_JOBS}"

IMAGES_DIR="output/images"
if [[ ! -f "${IMAGES_DIR}/rootfs.tar" ]]; then
    echo "ERROR: rootfs.tar not found in ${IMAGES_DIR}." >&2
    exit 1
fi

mkdir -p "${PREBUILT_DIR}/aarch64"
cp "${IMAGES_DIR}/rootfs.tar" "${PREBUILT_DIR}/aarch64/rootfs.tar"

popd >/dev/null

TOOLCHAIN_GCC="${BUILDROOT_DIR}/output/host/bin/aarch64-buildroot-linux-musl-gcc"
if [[ ! -x "${TOOLCHAIN_GCC}" ]]; then
    echo "ERROR: toolchain gcc not found at ${TOOLCHAIN_GCC}" >&2
    exit 1
fi

echo ""
echo "==> Buildroot aarch64 built successfully!"
echo "    Toolchain: ${TOOLCHAIN_GCC}"
echo "    Rootfs:    ${PREBUILT_DIR}/aarch64/rootfs.tar"
echo ""
echo "    Next: ARCH=aarch64 bash tools/linux/build_kvmtool.sh"
echo "    Then: ARCH=aarch64 bash tools/linux/build_guest_image.sh"
