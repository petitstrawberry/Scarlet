#!/bin/bash
set -euo pipefail

# Cross-compile a minimal Linux guest kernel for kvmtool.
#
# Prerequisites:
#   - Buildroot toolchain built (run bundles/linux/tools/build_buildroot.sh first)
#   - git
#
# Produces:
#   ${PREBUILT_DIR}/${ARCH}/bin/guest-Image  (uncompressed guest kernel)
#
# Environment variables:
#  ARCH          - target architecture (riscv64 or aarch64), defaults to riscv64
#  BUILDROOT_DIR - Buildroot installation, defaults to /opt/buildroot (riscv64)
#                  or /opt/buildroot-aarch64 (aarch64)
#  PREBUILT_DIR  - output staging directory, defaults to bundles/linux/prebuilt
#  WORKDIR       - working directory for clone, defaults to /opt
#  KERNEL_REPO   - git URL for Linux kernel, defaults to stable
#  KERNEL_BRANCH - branch/tag to checkout, defaults to v6.12

: "${ARCH:=riscv64}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUNDLE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
: "${PREBUILT_DIR:=${BUNDLE_DIR}/prebuilt}"
: "${WORKDIR:=/opt}"
: "${KERNEL_REPO:=https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git}"
: "${KERNEL_BRANCH:=v6.12}"

: "${KERNEL_DIR:=${WORKDIR}/linux-guest-${ARCH}}"

require_supported_host() {
    if [[ "$(uname -s)" != "Darwin" ]]; then
        return
    fi

    cat >&2 <<EOF
Linux guest artifacts are built with Linux-host tooling.
Run this script on Linux, such as scarlet-dev, a Linux VM, or a Linux Nix shell.

Example:
  ARCH=${ARCH} \\
  BUILDROOT_DIR="\$PWD/bundles/linux/cache/buildroot-${ARCH}" \\
  PREBUILT_DIR="\$PWD/bundles/linux/prebuilt" \\
  WORKDIR="\$PWD/bundles/linux/cache/work" \\
  bash "${SCRIPT_DIR}/build_guest_kernel.sh"
EOF
    exit 1
}

case "${ARCH}" in
    riscv64)
        : "${BUILDROOT_DIR:=/opt/buildroot}"
        TOOLCHAIN_PREFIX="riscv64-buildroot-linux-musl"
        LINUX_ARCH=riscv
        KERNEL_IMAGE_PATH="arch/${LINUX_ARCH}/boot/Image"
        GUEST_KERNEL_CONFIG="
CONFIG_VIRTUALIZATION=y
CONFIG_KVM_GUEST=y
CONFIG_RISCV_SBI_V01=n
CONFIG_SERIAL_8250=y
CONFIG_SERIAL_8250_CONSOLE=y
CONFIG_SERIAL_OF_PLATFORM=y
CONFIG_VIRTIO_MENU=y
CONFIG_VIRTIO_PCI=y
CONFIG_VIRTIO_BLK=y
CONFIG_VIRTIO_NET=y
CONFIG_VIRTIO_CONSOLE=y
CONFIG_HW_RANDOM=y
CONFIG_HW_RANDOM_VIRTIO=y
CONFIG_9P_FS=y
CONFIG_9P_FS_POSIX_ACL=y
CONFIG_NET_9P=y
CONFIG_NET_9P_VIRTIO=y
CONFIG_DEVTMPFS=y
CONFIG_DEVTMPFS_MOUNT=y
CONFIG_EXT4_FS=y
CONFIG_TTY=y
CONFIG_PRINTK=y
"
        ;;
    aarch64)
        : "${BUILDROOT_DIR:=/opt/buildroot-aarch64}"
        TOOLCHAIN_PREFIX="aarch64-buildroot-linux-musl"
        LINUX_ARCH=arm64
        KERNEL_IMAGE_PATH="arch/${LINUX_ARCH}/boot/Image"
        GUEST_KERNEL_CONFIG="
CONFIG_SERIAL_8250=y
CONFIG_SERIAL_8250_CONSOLE=y
CONFIG_SERIAL_OF_PLATFORM=y
CONFIG_VIRTIO_MENU=y
CONFIG_VIRTIO_PCI=y
CONFIG_VIRTIO_BLK=y
CONFIG_VIRTIO_NET=y
CONFIG_VIRTIO_CONSOLE=y
CONFIG_HW_RANDOM=y
CONFIG_HW_RANDOM_VIRTIO=y
CONFIG_9P_FS=y
CONFIG_9P_FS_POSIX_ACL=y
CONFIG_NET_9P=y
CONFIG_NET_9P_VIRTIO=y
CONFIG_DEVTMPFS=y
CONFIG_DEVTMPFS_MOUNT=y
CONFIG_EXT4_FS=y
CONFIG_TTY=y
CONFIG_PRINTK=y
"
        ;;
    *)
        echo "Unsupported ARCH=${ARCH}. Use riscv64 or aarch64." >&2
        exit 1
        ;;
esac

require_supported_host

if [[ ! -d "${BUILDROOT_DIR}" ]]; then
    echo "Expected buildroot directory at ${BUILDROOT_DIR} not found." >&2
    echo "Run ARCH=${ARCH} bash ${SCRIPT_DIR}/build_buildroot.sh first." >&2
    exit 1
fi

toolchain_gcc="${BUILDROOT_DIR}/output/host/bin/${TOOLCHAIN_PREFIX}-gcc"

if [[ ! -x "${toolchain_gcc}" ]]; then
    echo "Buildroot toolchain missing at ${toolchain_gcc}." >&2
    exit 1
fi

mkdir -p "${PREBUILT_DIR}/${ARCH}/bin"
export PATH="${BUILDROOT_DIR}/output/host/bin:${PATH}"

echo "==> Cloning Linux kernel (${KERNEL_BRANCH})..."
if [[ -d "${KERNEL_DIR}/.git" ]]; then
    git -C "${KERNEL_DIR}" fetch --depth=1 origin "${KERNEL_BRANCH}"
    git -C "${KERNEL_DIR}" checkout FETCH_HEAD
else
    rm -rf "${KERNEL_DIR}"
    git clone --depth=1 --branch "${KERNEL_BRANCH}" "${KERNEL_REPO}" "${KERNEL_DIR}"
fi

echo "==> Configuring guest kernel for ${ARCH}..."
pushd "${KERNEL_DIR}" >/dev/null

make CROSS_COMPILE="${TOOLCHAIN_PREFIX}-" ARCH="${LINUX_ARCH}" defconfig

echo "${GUEST_KERNEL_CONFIG}" >> .config

make CROSS_COMPILE="${TOOLCHAIN_PREFIX}-" ARCH="${LINUX_ARCH}" olddefconfig

echo "==> Building guest kernel..."
make -j"$(nproc)" CROSS_COMPILE="${TOOLCHAIN_PREFIX}-" ARCH="${LINUX_ARCH}" Image

if [[ ! -f "${KERNEL_IMAGE_PATH}" ]]; then
    echo "ERROR: Image not built" >&2
    exit 1
fi

cp "${KERNEL_IMAGE_PATH}" "${PREBUILT_DIR}/${ARCH}/bin/guest-Image"
popd >/dev/null

echo ""
echo "==> Guest kernel built successfully!"
echo "    Image: ${PREBUILT_DIR}/${ARCH}/bin/guest-Image"
echo "    Architecture: ${ARCH}"
echo "    Deploy with: bash ${SCRIPT_DIR}/deploy_rootfs.sh"
echo "    Run with: lkvm run -k /usr/bin/guest-Image ..."
