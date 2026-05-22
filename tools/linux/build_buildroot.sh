#!/bin/bash
set -euo pipefail

# Builds Buildroot using the repository-provided configuration.
# Environment variables:
#  ARCH - target architecture (riscv64 or aarch64), defaults to riscv64
#  BUILDROOT_DIR - Buildroot tree; defaults to the Docker path for ARCH
#  PREBUILT_DIR - artifact staging directory, defaults to /opt/prebuilt
#  IMAGES_DIR - Buildroot images directory, defaults to output/images
#  MAKE_JOBS - Buildroot parallelism, defaults to $(nproc)
#  BUILDROOT_VERSION - Buildroot release to use when bootstrapping AArch64

: "${ARCH:=riscv64}"
: "${BUILDROOT_VERSION:=2025.02.6}"
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

download_file() {
    local url="$1"
    local output="$2"

    if command -v curl >/dev/null 2>&1; then
        curl -fL "${url}" -o "${output}"
    elif command -v wget >/dev/null 2>&1; then
        wget -O "${output}" "${url}"
    else
        echo "Neither curl nor wget is available to download ${url}" >&2
        exit 1
    fi
}

ensure_buildroot_release() {
    if [[ -d "${BUILDROOT_DIR}" && -d "${BUILDROOT_DIR}/.git" ]]; then
        local current_version
        current_version="$(make -C "${BUILDROOT_DIR}" -s printvars VARS=BR2_VERSION_FULL 2>/dev/null | sed -n 's/^BR2_VERSION_FULL=//p')"
        if [[ "${current_version}" != "${BUILDROOT_VERSION}" ]]; then
            echo "==> Switching Buildroot from ${current_version:-unknown} to ${BUILDROOT_VERSION}..."
            git -C "${BUILDROOT_DIR}" fetch --depth=1 origin "refs/tags/${BUILDROOT_VERSION}:refs/tags/${BUILDROOT_VERSION}"
            git -C "${BUILDROOT_DIR}" checkout "${BUILDROOT_VERSION}"
        fi
        return
    fi

    if [[ -d "${BUILDROOT_DIR}" ]]; then
        local current_version
        current_version="$(make -C "${BUILDROOT_DIR}" -s printvars VARS=BR2_VERSION_FULL 2>/dev/null | sed -n 's/^BR2_VERSION_FULL=//p')"
        if [[ "${current_version}" != "${BUILDROOT_VERSION}" ]]; then
            echo "Buildroot tree at ${BUILDROOT_DIR} is ${current_version:-unknown}, expected ${BUILDROOT_VERSION}." >&2
            echo "Remove it or set BUILDROOT_DIR to a Buildroot ${BUILDROOT_VERSION} tree." >&2
            exit 1
        fi
        return
    fi

    local archive="buildroot-${BUILDROOT_VERSION}.tar.gz"
    local cache_dir
    cache_dir="$(dirname "${BUILDROOT_DIR}")"

    echo "==> Downloading Buildroot ${BUILDROOT_VERSION}..."
    mkdir -p "${cache_dir}"
    download_file "https://buildroot.org/downloads/${archive}" "${cache_dir}/${archive}"
    tar -xf "${cache_dir}/${archive}" -C "${cache_dir}"
    mv "${cache_dir}/buildroot-${BUILDROOT_VERSION}" "${BUILDROOT_DIR}"
}

configure_common_linux_userland() {
    utils/config --enable BR2_TOOLCHAIN_BUILDROOT_MUSL
    utils/config --enable BR2_TOOLCHAIN_BUILDROOT_CXX
    utils/config --enable BR2_GCC_VERSION_13_X
    utils/config --disable BR2_GCC_VERSION_14_X
    utils/config --enable BR2_TARGET_ROOTFS_TAR
    utils/config --disable BR2_TARGET_ROOTFS_EXT2
    utils/config --disable BR2_LINUX_KERNEL
    utils/config --disable BR2_PACKAGE_HOST_QEMU
    utils/config --disable BR2_PACKAGE_HOST_QEMU_SYSTEM_MODE
    utils/config --enable BR2_ROOTFS_DEVICE_CREATION_DYNAMIC_EUDEV
    utils/config --set-str BR2_ROOTFS_POST_IMAGE_SCRIPT ""
    utils/config --set-str BR2_ROOTFS_POST_SCRIPT_ARGS ""

    utils/config --enable BR2_PACKAGE_CAIRO
    utils/config --enable BR2_PACKAGE_CAIRO_PNG
    utils/config --enable BR2_PACKAGE_CAIRO_ZLIB
    utils/config --enable BR2_PACKAGE_DBUS
    utils/config --enable BR2_PACKAGE_DESKTOP_FILE_UTILS
    utils/config --enable BR2_PACKAGE_EUDEV
    utils/config --enable BR2_PACKAGE_EUDEV_MODULE_LOADING
    utils/config --enable BR2_PACKAGE_EUDEV_ENABLE_HWDB
    utils/config --enable BR2_PACKAGE_FILE
    utils/config --enable BR2_PACKAGE_GUMBO_PARSER
    utils/config --enable BR2_PACKAGE_HICOLOR_ICON_THEME
    utils/config --enable BR2_PACKAGE_JBIG2DEC
    utils/config --enable BR2_PACKAGE_JSON_GLIB
    utils/config --enable BR2_PACKAGE_JPEG
    utils/config --disable BR2_PACKAGE_JPEG_TURBO
    utils/config --enable BR2_PACKAGE_LCMS2
    utils/config --enable BR2_PACKAGE_LIBERATION
    utils/config --enable BR2_PACKAGE_LIBERATION_MONO
    utils/config --enable BR2_PACKAGE_LIBERATION_SANS
    utils/config --enable BR2_PACKAGE_LIBERATION_SERIF
    utils/config --enable BR2_PACKAGE_LIBEVDEV
    utils/config --enable BR2_PACKAGE_LIBGLIB2
    utils/config --enable BR2_PACKAGE_LIBGTK3
    utils/config --enable BR2_PACKAGE_LIBGTK3_WAYLAND
    utils/config --enable BR2_PACKAGE_LIBGTK3_DEMO
    utils/config --disable BR2_PACKAGE_LIBGTK3_BROADWAY
    utils/config --enable BR2_PACKAGE_LIBGTK4
    utils/config --enable BR2_PACKAGE_LIBGTK4_WAYLAND
    utils/config --enable BR2_PACKAGE_LIBGTK4_DEMO
    utils/config --disable BR2_PACKAGE_LIBGTK4_BROADWAY
    utils/config --enable BR2_PACKAGE_LIBINPUT
    utils/config --enable BR2_PACKAGE_LIBJPEG
    utils/config --enable BR2_PACKAGE_LIBPCIACCESS
    utils/config --enable BR2_PACKAGE_LIBSHA1
    utils/config --enable BR2_PACKAGE_LIBTIRPC
    utils/config --enable BR2_PACKAGE_LTRIS
    utils/config --disable BR2_PACKAGE_LTRIS_AUDIO
    utils/config --enable BR2_PACKAGE_LUA
    utils/config --enable BR2_PACKAGE_LUA_5_4
    utils/config --enable BR2_PACKAGE_MESA3D
    utils/config --enable BR2_PACKAGE_MESA3D_DEMOS
    utils/config --enable BR2_PACKAGE_MESA3D_GALLIUM_DRIVER_SWRAST
    utils/config --enable BR2_PACKAGE_MESA3D_GBM
    utils/config --enable BR2_PACKAGE_MESA3D_LLVM
    utils/config --enable BR2_PACKAGE_MESA3D_OPENGL_EGL
    utils/config --enable BR2_PACKAGE_MESA3D_OPENGL_ES
    utils/config --enable BR2_PACKAGE_MICROPYTHON
    utils/config --enable BR2_PACKAGE_MTDEV
    utils/config --enable BR2_PACKAGE_NANO
    utils/config --enable BR2_PACKAGE_NANO_TINY
    utils/config --enable BR2_PACKAGE_OPENJPEG
    utils/config --enable BR2_PACKAGE_POPPLER
    utils/config --enable BR2_PACKAGE_POPPLER_QT5
    utils/config --enable BR2_PACKAGE_QT5
    utils/config --enable BR2_PACKAGE_QT5BASE
    utils/config --enable BR2_PACKAGE_QT5BASE_EXAMPLES
    utils/config --enable BR2_PACKAGE_QT5BASE_GUI
    utils/config --enable BR2_PACKAGE_QT5BASE_LINUXFB
    utils/config --enable BR2_PACKAGE_QT5BASE_NETWORK
    utils/config --enable BR2_PACKAGE_QT5BASE_PRINTSUPPORT
    utils/config --enable BR2_PACKAGE_QT5BASE_TEST
    utils/config --enable BR2_PACKAGE_QT5BASE_WIDGETS
    utils/config --enable BR2_PACKAGE_QT5BASE_XML
    utils/config --enable BR2_PACKAGE_QT5WAYLAND
    utils/config --enable BR2_PACKAGE_SDL
    utils/config --enable BR2_PACKAGE_SDL_FBCON
    utils/config --disable BR2_PACKAGE_SDL_MIXER
    utils/config --enable BR2_PACKAGE_SEATD
    utils/config --enable BR2_PACKAGE_SEATD_BUILTIN
    utils/config --enable BR2_PACKAGE_SHARED_MIME_INFO
    utils/config --enable BR2_PACKAGE_SL
    utils/config --enable BR2_PACKAGE_SQLITE
    utils/config --enable BR2_PACKAGE_TIFF
    utils/config --enable BR2_PACKAGE_TIFF_CCITT
    utils/config --enable BR2_PACKAGE_TIFF_JPEG
    utils/config --enable BR2_PACKAGE_TIFF_LOGLUV
    utils/config --enable BR2_PACKAGE_TIFF_LZW
    utils/config --enable BR2_PACKAGE_TIFF_MDI
    utils/config --enable BR2_PACKAGE_TIFF_NEXT
    utils/config --enable BR2_PACKAGE_TIFF_OLD_JPEG
    utils/config --enable BR2_PACKAGE_TIFF_PACKBITS
    utils/config --enable BR2_PACKAGE_TIFF_PIXARLOG
    utils/config --enable BR2_PACKAGE_TIFF_THUNDER
    utils/config --enable BR2_PACKAGE_TIFF_ZLIB
    utils/config --enable BR2_PACKAGE_WAYLAND
    utils/config --enable BR2_PACKAGE_WAYLAND_PROTOCOLS
    utils/config --enable BR2_PACKAGE_WESTON
    utils/config --enable BR2_PACKAGE_WESTON_DEFAULT_HEADLESS
    utils/config --enable BR2_PACKAGE_WESTON_DEMO_CLIENTS
    utils/config --enable BR2_PACKAGE_WESTON_HEADLESS
    utils/config --enable BR2_PACKAGE_WESTON_SHELL_DESKTOP
    utils/config --disable BR2_PACKAGE_WESTON_SHELL_FULLSCREEN
    utils/config --disable BR2_PACKAGE_WESTON_SHELL_IVI
    utils/config --disable BR2_PACKAGE_WESTON_SHELL_KIOSK
    utils/config --disable BR2_PACKAGE_WESTON_SCREENSHARE
    utils/config --enable BR2_PACKAGE_WESTON_SIMPLE_CLIENTS
    utils/config --enable BR2_PACKAGE_XKEYBOARD_CONFIG
    utils/config --set-str BR2_PACKAGE_WESTON_DEFAULT_COMPOSITOR "headless"
}

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
    ensure_buildroot_release

    pushd "${BUILDROOT_DIR}" >/dev/null

    echo "==> Configuring Buildroot ${BUILDROOT_VERSION} for aarch64..."
    make qemu_aarch64_virt_defconfig
    configure_common_linux_userland
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
