#!/bin/bash
set -euo pipefail

# Build a bootable KVM guest image: guest kernel + guest initramfs.
#
# Prerequisites:
#   - Buildroot built (run bundles/linux/tools/build_buildroot.sh first)
#   - Buildroot toolchain available (for guest kernel cross-compilation)
#
# Produces:
#   ${PREBUILT_DIR}/${ARCH}/bin/guest-Image             (uncompressed guest kernel)
#   ${PREBUILT_DIR}/${ARCH}/bin/guest-initramfs.cpio.gz  (compressed cpio initramfs)
#
# Environment variables:
#  ARCH          - target architecture (riscv64 or aarch64), defaults to riscv64
#  BUILDROOT_DIR - Buildroot installation (auto-detected if unset)
#  PREBUILT_DIR  - output staging directory, defaults to bundles/linux/prebuilt
#  WORKDIR       - working directory for clones, defaults to /opt
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
  bash "${SCRIPT_DIR}/build_guest_image.sh"
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

TC_BINDIR="${BUILDROOT_DIR}/output/host/bin"
toolchain_gcc="${TC_BINDIR}/${TOOLCHAIN_PREFIX}-gcc"

if [[ ! -x "${toolchain_gcc}" ]]; then
    echo "Buildroot toolchain missing at ${toolchain_gcc}." >&2
    exit 1
fi

export PATH="${TC_BINDIR}:${PATH}"
mkdir -p "${PREBUILT_DIR}/${ARCH}/bin"

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

echo "    guest-Image -> ${PREBUILT_DIR}/${ARCH}/bin/guest-Image"

echo "==> Building guest initramfs..."

ROOTFS_TAR="${PREBUILT_DIR}/${ARCH}/rootfs.tar"
if [[ ! -f "${ROOTFS_TAR}" ]]; then
    echo "ERROR: ${ROOTFS_TAR} not found. Run ARCH=${ARCH} bash ${SCRIPT_DIR}/build_buildroot.sh first." >&2
    exit 1
fi

INITRAMFS_STAGING="${WORKDIR}/guest-initramfs-staging-${ARCH}"
rm -rf "${INITRAMFS_STAGING}"
mkdir -p "${INITRAMFS_STAGING}"

tar -xf "${ROOTFS_TAR}" -C "${INITRAMFS_STAGING}"

mkdir -p "${INITRAMFS_STAGING}/dev"
rm -f \
    "${INITRAMFS_STAGING}/dev/console" \
    "${INITRAMFS_STAGING}/dev/null" \
    "${INITRAMFS_STAGING}/dev/tty" \
    "${INITRAMFS_STAGING}/dev/zero" \
    "${INITRAMFS_STAGING}/dev/random" \
    "${INITRAMFS_STAGING}/dev/urandom"
mknod -m 600 "${INITRAMFS_STAGING}/dev/console" c 5 1
mknod -m 666 "${INITRAMFS_STAGING}/dev/null" c 1 3
mknod -m 666 "${INITRAMFS_STAGING}/dev/tty" c 5 0
mknod -m 666 "${INITRAMFS_STAGING}/dev/zero" c 1 5
mknod -m 666 "${INITRAMFS_STAGING}/dev/random" c 1 8
mknod -m 666 "${INITRAMFS_STAGING}/dev/urandom" c 1 9

pushd "${INITRAMFS_STAGING}" >/dev/null
find . -print0 | cpio --null -ov --format=newc 2>/dev/null | gzip -9 > "${PREBUILT_DIR}/${ARCH}/bin/guest-initramfs.cpio.gz"
popd >/dev/null

rm -rf "${INITRAMFS_STAGING}"

echo "    guest-initramfs.cpio.gz -> ${PREBUILT_DIR}/${ARCH}/bin/guest-initramfs.cpio.gz"

echo ""
echo "==> Guest image built successfully!"
echo "    Deploy with: bash ${SCRIPT_DIR}/deploy_rootfs.sh"
echo "    Run on Scarlet: lkvm run -k /usr/bin/guest-Image -i /usr/bin/guest-initramfs.cpio.gz -p 'console=ttyS0 rdinit=/sbin/init' --console serial -n mode=none -m 512"
