#!/bin/bash
set -euo pipefail

# Builds Buildroot using the repository-provided configuration.
# Environment variables:
#  ARCH - target architecture (riscv64 or aarch64), defaults to riscv64
#  BUILDROOT_DIR - Buildroot tree; defaults to the Docker path for ARCH
#  PREBUILT_DIR - artifact staging directory, defaults to /opt/prebuilt
#  IMAGES_DIR - Buildroot images directory, defaults to output/images
#  MAKE_JOBS - Buildroot parallelism, defaults to $(nproc)

: "${ARCH:=riscv64}"
: "${PREBUILT_DIR:=/opt/prebuilt}"
: "${IMAGES_DIR:=output/images}"

require_supported_host() {
    if [[ "$(uname -s)" != "Darwin" ]]; then
        return
    fi

    cat >&2 <<EOF
Buildroot artifacts are generated on Linux hosts only.
macOS can still build Scarlet itself, but Buildroot's host tools and generated
toolchains should be produced in scarlet-dev, a Linux VM, or a Linux Nix shell.

Example with repository-local paths:
  ARCH=${ARCH} \\
  BUILDROOT_DIR="\$PWD/.scarlet/cache/buildroot-${ARCH}" \\
  PREBUILT_DIR="\$PWD/.scarlet/cache/prebuilt" \\
  bash tools/linux/build_buildroot.sh
EOF
    exit 1
}

case "${ARCH}" in
    riscv64)
        : "${BUILDROOT_DIR:=/opt/buildroot}"
        ;;
    aarch64)
        : "${BUILDROOT_DIR:=/opt/buildroot-aarch64}"
        ;;
    *)
        echo "Unsupported architecture: ${ARCH}" >&2
        exit 1
        ;;
esac

require_supported_host
: "${MAKE_JOBS:=$(nproc)}"

build_riscv64() {
    if [[ ! -d "${BUILDROOT_DIR}" ]]; then
        echo "Expected buildroot directory at ${BUILDROOT_DIR} not found." >&2
        exit 1
    fi

    pushd "${BUILDROOT_DIR}" >/dev/null
    make -j "${MAKE_JOBS}"

    if [[ ! -d "${IMAGES_DIR}" ]]; then
        echo "Images directory ${BUILDROOT_DIR}/${IMAGES_DIR} missing after build." >&2
        exit 1
    fi

    pushd "${IMAGES_DIR}" >/dev/null
    rm -rf linux-riscv64
    mkdir linux-riscv64
    if [[ ! -f rootfs.tar ]]; then
        echo "rootfs.tar not found in ${BUILDROOT_DIR}/${IMAGES_DIR}." >&2
        exit 1
    fi

    tar -xf rootfs.tar -C linux-riscv64
    mkdir -p "${PREBUILT_DIR}/riscv64"
    cp rootfs.tar "${PREBUILT_DIR}/riscv64/rootfs.tar"

    popd >/dev/null
    popd >/dev/null
}

build_aarch64() {
    if [[ -d "${BUILDROOT_DIR}/.git" ]]; then
        echo "==> Updating existing Buildroot tree..."
        git -C "${BUILDROOT_DIR}" pull --ff-only || true
    else
        echo "==> Cloning Buildroot..."
        mkdir -p "$(dirname "${BUILDROOT_DIR}")"
        git clone --depth=1 https://github.com/buildroot/buildroot.git "${BUILDROOT_DIR}"
    fi

    pushd "${BUILDROOT_DIR}" >/dev/null

    echo "==> Configuring Buildroot for aarch64..."
    make qemu_aarch64_virt_defconfig

    utils/config --enable BR2_TOOLCHAIN_BUILDROOT_MUSL
    utils/config --enable BR2_TOOLCHAIN_BUILDROOT_CXX
    # GCC 14 libgcc's AArch64 unwinder includes hard-CFR traps and SVE scalar
    # count queries that Scarlet's Linux ABI does not fully support yet.
    # Keep AArch64 userland on GCC 13 until SVE state and BRK/signal delivery
    # are implemented.
    utils/config --enable BR2_GCC_VERSION_13_X
    utils/config --disable BR2_GCC_VERSION_14_X
    utils/config --enable BR2_TARGET_ROOTFS_TAR
    utils/config --disable BR2_TARGET_ROOTFS_EXT2
    utils/config --disable BR2_LINUX_KERNEL
    utils/config --disable BR2_PACKAGE_HOST_QEMU
    utils/config --disable BR2_PACKAGE_HOST_QEMU_SYSTEM_MODE
    utils/config --enable BR2_PACKAGE_CAIRO
    utils/config --enable BR2_PACKAGE_CAIRO_PNG
    utils/config --enable BR2_PACKAGE_CAIRO_ZLIB
    utils/config --enable BR2_PACKAGE_LIBGLIB2
    utils/config --enable BR2_PACKAGE_POPPLER
    utils/config --enable BR2_PACKAGE_SDL
    utils/config --enable BR2_PACKAGE_SDL_FBCON
    utils/config --enable BR2_PACKAGE_WAYLAND
    utils/config --enable BR2_PACKAGE_WAYLAND_PROTOCOLS
    make olddefconfig

    toolchain_gxx="${BUILDROOT_DIR}/output/host/bin/aarch64-buildroot-linux-musl-g++"
    if [[ ! -x "${toolchain_gxx}" ]]; then
        echo "==> C++ toolchain missing; rebuilding Buildroot gcc for aarch64..."
        make host-gcc-final-dirclean gcc-final-dirclean
    fi

    echo "==> Building Buildroot toolchain + rootfs for aarch64 (this takes a while)..."
    make -j "${MAKE_JOBS}"

    if [[ ! -f "${IMAGES_DIR}/rootfs.tar" ]]; then
        echo "ERROR: rootfs.tar not found in ${IMAGES_DIR}." >&2
        exit 1
    fi

    mkdir -p "${PREBUILT_DIR}/aarch64"
    cp "${IMAGES_DIR}/rootfs.tar" "${PREBUILT_DIR}/aarch64/rootfs.tar"

    popd >/dev/null

    toolchain_gcc="${BUILDROOT_DIR}/output/host/bin/aarch64-buildroot-linux-musl-gcc"
    if [[ ! -x "${toolchain_gcc}" ]]; then
        echo "ERROR: toolchain gcc not found at ${toolchain_gcc}" >&2
        exit 1
    fi

    echo ""
    echo "==> Buildroot aarch64 built successfully!"
    echo "    Toolchain: ${toolchain_gcc}"
    echo "    Rootfs:    ${PREBUILT_DIR}/aarch64/rootfs.tar"
}

case "${ARCH}" in
    riscv64)
        build_riscv64
        ;;
    aarch64)
        build_aarch64
        ;;
esac

echo "Buildroot artifacts staged under ${PREBUILT_DIR}."
