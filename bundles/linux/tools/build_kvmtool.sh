#!/bin/bash
set -euo pipefail

# Cross-compile kvmtool (lkvm-static) for RISC-V 64-bit or AArch64.
#
# Prerequisites:
#   - Buildroot toolchain built (run tools/linux/build_buildroot.sh first)
#   - git
#
# Produces:
#   ${PREBUILT_DIR}/bin/lkvm  (statically linked)
#
# Environment variables:
#  ARCH          - target architecture (riscv64 or aarch64), defaults to riscv64
#  BUILDROOT_DIR - Buildroot installation, defaults to /opt/buildroot (riscv64)
#                  or /opt/buildroot-aarch64 (aarch64)
#  PREBUILT_DIR  - output staging directory, defaults to /opt/prebuilt
#  WORKDIR       - working directory for clones, defaults to /opt
#  KVMTOOL_REPO  - git URL for kvmtool, defaults to upstream
#  DTC_REPO      - git URL for dtc (libfdt), defaults to upstream

: "${ARCH:=riscv64}"
: "${PREBUILT_DIR:=/opt/prebuilt}"
: "${WORKDIR:=/opt}"

: "${KVMTOOL_REPO:=https://github.com/kvmtool/kvmtool.git}"
: "${DTC_REPO:=https://git.kernel.org/pub/scm/utils/dtc/dtc.git}"

: "${KVMTOOL_DIR:=${WORKDIR}/kvmtool-${ARCH}}"
: "${DTC_DIR:=${WORKDIR}/dtc-kvmtool-${ARCH}}"

require_supported_host() {
    if [[ "$(uname -s)" != "Darwin" ]]; then
        return
    fi

    cat >&2 <<EOF
Linux ABI support tools are built with Linux-host tooling.
Run this script on Linux, such as scarlet-dev, a Linux VM, or a Linux Nix shell.

Example:
  ARCH=${ARCH} \\
  BUILDROOT_DIR="\$PWD/.scarlet/cache/buildroot-${ARCH}" \\
  PREBUILT_DIR="\$PWD/.scarlet/cache/prebuilt" \\
  WORKDIR="\$PWD/.scarlet/cache" \\
  bash tools/linux/build_kvmtool.sh
EOF
    exit 1
}

case "${ARCH}" in
    riscv64)
        : "${BUILDROOT_DIR:=/opt/buildroot}"
        TOOLCHAIN_PREFIX="riscv64-buildroot-linux-musl"
        TC_BINDIR="${BUILDROOT_DIR}/output/host/bin"
        KVMTOOL_ARCH=riscv
        ;;
    aarch64)
        : "${BUILDROOT_DIR:=/opt/buildroot-aarch64}"
        TOOLCHAIN_PREFIX="aarch64-buildroot-linux-musl"
        TC_BINDIR="${BUILDROOT_DIR}/output/host/bin"
        KVMTOOL_ARCH=arm64
        ;;
    *)
        echo "Unsupported ARCH=${ARCH}. Use riscv64 or aarch64." >&2
        exit 1
        ;;
esac

require_supported_host

if [[ ! -d "${BUILDROOT_DIR}" ]]; then
    echo "Expected buildroot directory at ${BUILDROOT_DIR} not found." >&2
    echo "Run ARCH=${ARCH} tools/linux/build_buildroot.sh first." >&2
    exit 1
fi

toolchain_gcc="${BUILDROOT_DIR}/output/host/bin/${TOOLCHAIN_PREFIX}-gcc"

if [[ ! -x "${toolchain_gcc}" ]]; then
    echo "Buildroot toolchain missing at ${toolchain_gcc}." >&2
    echo "Run ARCH=${ARCH} tools/linux/build_buildroot.sh first." >&2
    exit 1
fi

mkdir -p "${PREBUILT_DIR}/${ARCH}/bin"
export PATH="${TC_BINDIR}:${PATH}"

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

echo "==> Cloning dtc (libfdt)..."
clone_or_update "${DTC_REPO}" "${DTC_DIR}"

echo "==> Cloning kvmtool..."
clone_or_update "${KVMTOOL_REPO}" "${KVMTOOL_DIR}"

echo "==> Building libfdt for ${ARCH}..."
pushd "${DTC_DIR}" >/dev/null
make clean 2>/dev/null || true
make libfdt \
    CC="${TC_BINDIR}/${TOOLCHAIN_PREFIX}-gcc" \
    AR="${TC_BINDIR}/${TOOLCHAIN_PREFIX}-ar" \
    CFLAGS="-fPIC -O2"
popd >/dev/null

if [[ ! -f "${DTC_DIR}/libfdt/libfdt.a" ]]; then
    echo "ERROR: libfdt.a not built" >&2
    exit 1
fi

echo "    libfdt.a ready at ${DTC_DIR}/libfdt/libfdt.a"

echo "==> Building kvmtool (lkvm-static) for ${ARCH}..."
pushd "${KVMTOOL_DIR}" >/dev/null
make clean 2>/dev/null || true

make lkvm-static \
    ARCH="${KVMTOOL_ARCH}" \
    CROSS_COMPILE="${TC_BINDIR}/${TOOLCHAIN_PREFIX}-" \
    LIBFDT_DIR="${DTC_DIR}/libfdt" \
    V=1

if [[ ! -f "lkvm-static" ]]; then
    echo "ERROR: lkvm-static not built" >&2
    exit 1
fi

"${TC_BINDIR}/${TOOLCHAIN_PREFIX}-strip" -o "${PREBUILT_DIR}/${ARCH}/bin/lkvm" lkvm-static
popd >/dev/null

echo ""
echo "==> kvmtool built successfully!"
echo "    Binary: ${PREBUILT_DIR}/${ARCH}/bin/lkvm"
echo "    Architecture: ${ARCH} (static)"
echo ""
echo "    Deploy with: bash tools/linux/deploy_rootfs.sh"
echo "    Then run on Scarlet: lkvm run -k /path/to/Image ..."
