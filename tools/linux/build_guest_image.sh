#!/bin/bash
set -euo pipefail

# Build a bootable KVM guest image: guest kernel + guest initramfs.
#
# Prerequisites:
#   - Buildroot built (run tools/linux/build_buildroot.sh or
#     tools/linux/build_buildroot_aarch64.sh first)
#   - Buildroot toolchain available (for guest kernel cross-compilation)
#
# Produces:
#   ${PREBUILT_DIR}/bin/guest-Image             (uncompressed guest kernel)
#   ${PREBUILT_DIR}/bin/guest-initramfs.cpio.gz  (compressed cpio initramfs)
#
# Environment variables:
#  ARCH          - target architecture (riscv64 or aarch64), defaults to riscv64
#  BUILDROOT_DIR - Buildroot installation (auto-detected if unset)
#  PREBUILT_DIR  - output staging directory, defaults to /opt/prebuilt
#  WORKDIR       - working directory for clones, defaults to /opt
#  KERNEL_REPO   - git URL for Linux kernel, defaults to stable
#  KERNEL_BRANCH - branch/tag to checkout, defaults to v6.12

: "${ARCH:=riscv64}"
: "${PREBUILT_DIR:=/opt/prebuilt}"
: "${WORKDIR:=/opt}"
: "${KERNEL_REPO:=https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git}"
: "${KERNEL_BRANCH:=v6.12}"

: "${KERNEL_DIR:=${WORKDIR}/linux-guest-${ARCH}}"

case "${ARCH}" in
    riscv64)
        : "${BUILDROOT_DIR:=/opt/buildroot}"
        TOOLCHAIN_PREFIX="riscv64-buildroot-linux-musl"
        KERNEL_ARCH=riscv
        KERNEL_IMAGE_PATH="arch/riscv/boot/Image"
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
        KERNEL_ARCH=arm64
        KERNEL_IMAGE_PATH="arch/arm64/boot/Image"
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

if [[ ! -d "${BUILDROOT_DIR}" ]]; then
    echo "Expected buildroot directory at ${BUILDROOT_DIR} not found." >&2
    echo "Run tools/linux/build_buildroot.sh or build_buildroot_aarch64.sh first." >&2
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

make CROSS_COMPILE="${TOOLCHAIN_PREFIX}-" ARCH="${KERNEL_ARCH}" defconfig

echo "${GUEST_KERNEL_CONFIG}" >> .config

make CROSS_COMPILE="${TOOLCHAIN_PREFIX}-" ARCH="${KERNEL_ARCH}" olddefconfig

echo "==> Building guest kernel..."
make -j"$(nproc)" CROSS_COMPILE="${TOOLCHAIN_PREFIX}-" ARCH="${KERNEL_ARCH}" Image

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
    echo "ERROR: ${ROOTFS_TAR} not found. Run tools/linux/build_buildroot.sh or build_buildroot_aarch64.sh first." >&2
    exit 1
fi

INITRAMFS_STAGING="${WORKDIR}/guest-initramfs-staging-${ARCH}"
rm -rf "${INITRAMFS_STAGING}"
mkdir -p "${INITRAMFS_STAGING}"

tar -xf "${ROOTFS_TAR}" -C "${INITRAMFS_STAGING}"

pushd "${INITRAMFS_STAGING}" >/dev/null
find . -print0 | cpio --null -ov --format=newc 2>/dev/null | gzip -9 > "${PREBUILT_DIR}/${ARCH}/bin/guest-initramfs.cpio.gz"
popd >/dev/null

rm -rf "${INITRAMFS_STAGING}"

echo "    guest-initramfs.cpio.gz -> ${PREBUILT_DIR}/${ARCH}/bin/guest-initramfs.cpio.gz"

echo ""
echo "==> Guest image built successfully!"
echo "    Deploy with: bash tools/linux/deploy_rootfs.sh"
echo "    Run on Scarlet: lkvm run -k /usr/bin/guest-Image -i /usr/bin/guest-initramfs.cpio.gz -p 'console=ttyS0 rdinit=/sbin/init' --console serial -n mode=none -m 512"
