#!/bin/bash
set -euo pipefail

# Builds optional user-space programs (zathura, green, fbdoom, kvmtool) using the preconfigured toolchain.
# Environment variables:
#  ARCH - target architecture (riscv64 or aarch64), defaults to riscv64
#  BUILDROOT_DIR - Buildroot tree; defaults to the Docker path for ARCH
#  PREBUILT_DIR - artifact staging directory, defaults to bundles/linux/prebuilt
#  WORKDIR - checkout/build working directory, defaults to /opt

: "${ARCH:=riscv64}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUNDLE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
: "${PREBUILT_DIR:=${BUNDLE_DIR}/prebuilt}"
: "${WORKDIR:=/opt}"

: "${GREEN_REPO:=https://github.com/petitstrawberry/green.git}"
: "${FBDOOM_REPO:=https://github.com/petitstrawberry/fbdoom.git}"
: "${KVMTOOL_REPO:=https://github.com/kvmtool/kvmtool.git}"
: "${DTC_REPO:=https://git.kernel.org/pub/scm/utils/dtc/dtc.git}"

: "${GIRARA_VERSION:=0.4.5}"
: "${ZATHURA_VERSION:=0.5.12}"
: "${ZATHURA_PDF_POPPLER_VERSION:=0.3.2}"
: "${XKEYBOARD_CONFIG_VERSION:=2.38}"
: "${GIRARA_SHA256:=6b7f7993f82796854d5036572b879ffaaf7e0b619d12abdb318ce14757bdda91}"
: "${ZATHURA_SHA256:=e84870fbf96b766b8224a3f3a6ce7ccfa36efa3b6919cc8a2fbf765ea4dfe476}"
: "${ZATHURA_PDF_POPPLER_SHA256:=71abeed51cd1d188cef3dbd4c164758e3c371604756967b23ad176ae53453011}"
: "${XKEYBOARD_CONFIG_SHA256:=0690a91bab86b18868f3eee6d41e9ec4ce6894f655443d490a2184bfac56c872}"
: "${GIRARA_URL:=https://pwmt.org/projects/girara/download/girara-${GIRARA_VERSION}.tar.xz}"
: "${ZATHURA_URL:=https://pwmt.org/projects/zathura/download/zathura-${ZATHURA_VERSION}.tar.xz}"
: "${ZATHURA_PDF_POPPLER_URL:=https://pwmt.org/projects/zathura-pdf-poppler/download/zathura-pdf-poppler-${ZATHURA_PDF_POPPLER_VERSION}.tar.xz}"
: "${XKEYBOARD_CONFIG_URL:=https://www.x.org/releases/individual/data/xkeyboard-config/xkeyboard-config-${XKEYBOARD_CONFIG_VERSION}.tar.xz}"

: "${GREEN_DIR:=${WORKDIR}/green-${ARCH}}"
: "${FBDOOM_DIR:=${WORKDIR}/fbdoom-${ARCH}}"
: "${KVMTOOL_DIR:=${WORKDIR}/kvmtool-${ARCH}}"
: "${DTC_DIR:=${WORKDIR}/dtc-kvmtool-${ARCH}}"
: "${GIRARA_DIR:=${WORKDIR}/girara-${ARCH}}"
: "${ZATHURA_DIR:=${WORKDIR}/zathura-${ARCH}}"
: "${ZATHURA_PDF_POPPLER_DIR:=${WORKDIR}/zathura-pdf-poppler-${ARCH}}"
: "${XKEYBOARD_CONFIG_DIR:=${WORKDIR}/xkeyboard-config-${ARCH}}"

require_supported_host() {
    if [[ "$(uname -s)" != "Darwin" ]]; then
        return
    fi

    cat >&2 <<EOF
Linux ABI user programs are built with the Buildroot host toolchain.
Run this script on Linux, such as scarlet-dev, a Linux VM, or a Linux Nix shell.

Example for aarch64 with repository-local paths:
  ARCH=aarch64 \\
  BUILDROOT_DIR="\$PWD/bundles/linux/cache/buildroot-aarch64" \\
  PREBUILT_DIR="\$PWD/bundles/linux/prebuilt" \\
  WORKDIR="\$PWD/bundles/linux/cache/work" \\
  bash "${SCRIPT_DIR}/build_user_programs.sh"
EOF
    exit 1
}

# Determine toolchain prefix based on architecture
case "${ARCH}" in
    riscv64)
        TOOLCHAIN_PREFIX="riscv64-buildroot-linux-musl"
        KVMTOOL_ARCH="riscv"
        : "${BUILDROOT_DIR:=/opt/buildroot}"
        BUILDROOT_HELP="ARCH=${ARCH} bash ${SCRIPT_DIR}/build_buildroot.sh"
        ;;
    aarch64)
        TOOLCHAIN_PREFIX="aarch64-buildroot-linux-musl"
        KVMTOOL_ARCH="arm64"
        : "${BUILDROOT_DIR:=/opt/buildroot-aarch64}"
        BUILDROOT_HELP="ARCH=${ARCH} bash ${SCRIPT_DIR}/build_buildroot.sh"
        ;;
    *)
        echo "Unsupported architecture: ${ARCH}" >&2
        exit 1
        ;;
esac

require_supported_host

if [[ ! -d "${BUILDROOT_DIR}" ]]; then
    echo "Expected buildroot directory at ${BUILDROOT_DIR} not found." >&2
    echo "Run ${BUILDROOT_HELP} first, or inject BUILDROOT_DIR explicitly." >&2
    exit 1
fi

toolchain_bindir="${BUILDROOT_DIR}/output/host/bin"
toolchain_gcc="${toolchain_bindir}/${TOOLCHAIN_PREFIX}-gcc"
if [[ ! -x "${toolchain_gcc}" ]]; then
    echo "Buildroot toolchain missing at ${toolchain_gcc}. Run ${BUILDROOT_HELP} first." >&2
    exit 1
fi
buildroot_pkg_config="${toolchain_bindir}/pkg-config"
if [[ ! -x "${buildroot_pkg_config}" ]]; then
    echo "Buildroot pkg-config wrapper missing at ${buildroot_pkg_config}. Run ${BUILDROOT_HELP} first." >&2
    exit 1
fi
meson_cross_file="${toolchain_bindir}/../etc/meson/cross-compilation.conf"
if [[ ! -f "${meson_cross_file}" ]]; then
    echo "Buildroot meson cross file missing at ${meson_cross_file}. Run ${BUILDROOT_HELP} first." >&2
    exit 1
fi

mkdir -p "${PREBUILT_DIR}/${ARCH}/bin" "${PREBUILT_DIR}/${ARCH}/lib" "${PREBUILT_DIR}/${ARCH}/share"
export PATH="${toolchain_bindir}:${PATH}"
export PKG_CONFIG="${buildroot_pkg_config}"

clone_or_update() {
    local repo_url="$1"
    local dest_dir="$2"

    if [[ -d "${dest_dir}/.git" ]]; then
        git -C "${dest_dir}" fetch --depth=1 origin
        git -C "${dest_dir}" reset --hard origin/$(git -C "${dest_dir}" rev-parse --abbrev-ref HEAD)
    else
        rm -rf "${dest_dir}"
        git clone --depth=1 "${repo_url}" "${dest_dir}"
    fi
}

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

fetch_tarball() {
    local name="$1"
    local version="$2"
    local url="$3"
    local sha256="$4"
    local dest_dir="$5"
    local archive="${WORKDIR}/${name}-${version}.tar.xz"
    local version_file="${dest_dir}/.scarlet-version"

    if [[ -f "${version_file}" ]] && [[ "$(cat "${version_file}")" == "${version}" ]]; then
        return
    fi

    echo "Fetching ${name} ${version}..."
    rm -rf "${dest_dir}"
    download_file "${url}" "${archive}"
    echo "${sha256}  ${archive}" | sha256sum -c -
    mkdir -p "${dest_dir}"
    tar -xf "${archive}" -C "${dest_dir}" --strip-components=1
    echo "${version}" >"${version_file}"
}

install_meson_project() {
    local name="$1"
    local source_dir="$2"
    local dest_dir="$3"
    shift 3
    local build_dir="${source_dir}/build-${ARCH}"
    local staging_dir="${BUILDROOT_DIR}/output/staging"

    echo "Building ${name}..."
    rm -rf "${build_dir}"
    meson setup "${build_dir}" "${source_dir}" \
        --cross-file "${meson_cross_file}" \
        --prefix=/usr \
        --libdir=lib \
        --buildtype=debugoptimized \
        --default-library=shared \
        "$@"
    ninja -C "${build_dir}"
    DESTDIR="${staging_dir}" ninja -C "${build_dir}" install
    DESTDIR="${dest_dir}" ninja -C "${build_dir}" install
}

install_xkeyboard_config_data() {
    local source_dir="$1"
    local dest_dir="$2"
    local build_dir="${source_dir}/build-${ARCH}"

    echo "Installing xkeyboard-config runtime data..."
    rm -rf "${build_dir}"
    meson setup "${build_dir}" "${source_dir}" \
        --prefix=/usr \
        -Dxkb-base=/usr/share/X11/xkb
    ninja -C "${build_dir}"
    DESTDIR="${dest_dir}" ninja -C "${build_dir}" install
}

copy_zathura_artifacts() {
    local dest_dir="$1"

    if [[ -d "${dest_dir}/usr/bin" ]]; then
        cp -a "${dest_dir}/usr/bin/." "${PREBUILT_DIR}/${ARCH}/bin/"
    fi
    if [[ -d "${dest_dir}/usr/lib" ]]; then
        cp -a "${dest_dir}/usr/lib/." "${PREBUILT_DIR}/${ARCH}/lib/"
    fi
    if [[ -d "${dest_dir}/usr/share" ]]; then
        cp -a "${dest_dir}/usr/share/." "${PREBUILT_DIR}/${ARCH}/share/"
    fi
}

build_zathura_stack() {
    local dest_dir="${PREBUILT_DIR}/${ARCH}/zathura-root"

    rm -f "${PREBUILT_DIR}/${ARCH}/bin/scarlet-pdfview"

    fetch_tarball "girara" "${GIRARA_VERSION}" "${GIRARA_URL}" "${GIRARA_SHA256}" "${GIRARA_DIR}"
    fetch_tarball "zathura" "${ZATHURA_VERSION}" "${ZATHURA_URL}" "${ZATHURA_SHA256}" "${ZATHURA_DIR}"
    fetch_tarball "zathura-pdf-poppler" "${ZATHURA_PDF_POPPLER_VERSION}" "${ZATHURA_PDF_POPPLER_URL}" "${ZATHURA_PDF_POPPLER_SHA256}" "${ZATHURA_PDF_POPPLER_DIR}"
    fetch_tarball "xkeyboard-config" "${XKEYBOARD_CONFIG_VERSION}" "${XKEYBOARD_CONFIG_URL}" "${XKEYBOARD_CONFIG_SHA256}" "${XKEYBOARD_CONFIG_DIR}"

    rm -rf "${dest_dir}"
    install_meson_project "girara" "${GIRARA_DIR}" "${dest_dir}"
    install_meson_project "zathura" "${ZATHURA_DIR}" "${dest_dir}"
    install_meson_project "zathura-pdf-poppler" "${ZATHURA_PDF_POPPLER_DIR}" "${dest_dir}" -Dplugindir=/usr/lib/zathura
    install_xkeyboard_config_data "${XKEYBOARD_CONFIG_DIR}" "${dest_dir}"
    copy_zathura_artifacts "${dest_dir}"
}

if [[ "${ARCH}" == "aarch64" ]]; then
    build_zathura_stack
else
    echo "Skipping zathura stack for ${ARCH}; current PDF GUI target is aarch64."
fi

clone_or_update "${GREEN_REPO}" "${GREEN_DIR}"
clone_or_update "${FBDOOM_REPO}" "${FBDOOM_DIR}"
clone_or_update "${KVMTOOL_REPO}" "${KVMTOOL_DIR}"
clone_or_update "${DTC_REPO}" "${DTC_DIR}"

echo "Building user programs for ${ARCH}..."

pushd "${GREEN_DIR}" >/dev/null
if make DEBUG=1 BUILDROOT="${BUILDROOT_DIR}"; then
    if [[ -f "${GREEN_DIR}/green" ]]; then
        install -m 755 "${GREEN_DIR}/green" "${PREBUILT_DIR}/${ARCH}/bin/green"
    fi
    if [[ -d "${GREEN_DIR}/usr/lib" ]]; then
        cp -a "${GREEN_DIR}/usr/lib/." "${PREBUILT_DIR}/${ARCH}/lib/"
    fi
else
    echo "Warning: green build failed for ${ARCH}; skipping." >&2
fi
popd >/dev/null

pushd "${FBDOOM_DIR}/fbdoom" >/dev/null
if make CROSS_COMPILE="${toolchain_bindir}/${TOOLCHAIN_PREFIX}-" V=1; then
    if [[ -f "${FBDOOM_DIR}/fbdoom/fbdoom" ]]; then
        install -m 755 "${FBDOOM_DIR}/fbdoom/fbdoom" "${PREBUILT_DIR}/${ARCH}/bin/fbdoom"
    fi
else
    echo "Warning: fbdoom build failed for ${ARCH}; skipping." >&2
fi
popd >/dev/null

echo "Building libfdt for kvmtool..."
pushd "${DTC_DIR}" >/dev/null
make clean 2>/dev/null || true
make libfdt CC="${toolchain_gcc}" AR="${toolchain_bindir}/${TOOLCHAIN_PREFIX}-ar" CFLAGS="-fPIC -O2"
popd >/dev/null

echo "Building kvmtool (lkvm-static)..."
pushd "${KVMTOOL_DIR}" >/dev/null
make clean 2>/dev/null || true
make lkvm-static ARCH="${KVMTOOL_ARCH}" CROSS_COMPILE="${toolchain_bindir}/${TOOLCHAIN_PREFIX}-" LIBFDT_DIR="${DTC_DIR}/libfdt" V=1
if [[ -f "lkvm-static" ]]; then
    "${toolchain_bindir}/${TOOLCHAIN_PREFIX}-strip" -o "${PREBUILT_DIR}/${ARCH}/bin/lkvm" lkvm-static
fi
popd >/dev/null

echo "User programs for ${ARCH} built and placed in ${PREBUILT_DIR}."
