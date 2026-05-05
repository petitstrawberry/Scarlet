#!/bin/bash
set -euo pipefail

# Cross-compile kvmtool (lkvm-static) for RISC-V 64-bit.
#
# Prerequisites:
#   - Buildroot toolchain built (run tools/linux/build_buildroot.sh first)
#   - git
#
# Produces:
#   ${PREBUILT_DIR}/bin/lkvm  (statically linked)
#
# Environment variables:
#  ARCH          - target architecture (riscv64 only), defaults to riscv64
#  BUILDROOT_DIR - Buildroot installation, defaults to /opt/buildroot
#  PREBUILT_DIR  - output staging directory, defaults to /opt/prebuilt
#  WORKDIR       - working directory for clones, defaults to /opt
#  KVMTOOL_REPO  - git URL for kvmtool, defaults to upstream
#  DTC_REPO      - git URL for dtc (libfdt), defaults to upstream

: "${ARCH:=riscv64}"
: "${BUILDROOT_DIR:=/opt/buildroot}"
: "${PREBUILT_DIR:=/opt/prebuilt}"
: "${WORKDIR:=/opt}"

: "${KVMTOOL_REPO:=https://github.com/kvmtool/kvmtool.git}"
: "${DTC_REPO:=https://git.kernel.org/pub/scm/utils/dtc/dtc.git}"

: "${KVMTOOL_DIR:=${WORKDIR}/kvmtool-${ARCH}}"
: "${DTC_DIR:=${WORKDIR}/dtc-kvmtool-${ARCH}}"

# Only riscv64 is supported by this script
if [[ "${ARCH}" != "riscv64" ]]; then
    echo "kvmtool build script only supports riscv64 for now." >&2
    exit 1
fi

if [[ ! -d "${BUILDROOT_DIR}" ]]; then
    echo "Expected buildroot directory at ${BUILDROOT_DIR} not found." >&2
    echo "Run tools/linux/build_buildroot.sh first." >&2
    exit 1
fi

TOOLCHAIN_PREFIX="riscv64-buildroot-linux-musl"
toolchain_gcc="${BUILDROOT_DIR}/output/host/bin/${TOOLCHAIN_PREFIX}-gcc"

if [[ ! -x "${toolchain_gcc}" ]]; then
    echo "Buildroot toolchain missing at ${toolchain_gcc}." >&2
    echo "Run tools/linux/build_buildroot.sh first." >&2
    exit 1
fi

mkdir -p "${PREBUILT_DIR}/bin"

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
    CC="${TOOLCHAIN_PREFIX}-gcc" \
    AR="${TOOLCHAIN_PREFIX}-ar" \
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

# ARCH=riscv: kvmtool uses 'riscv', not 'riscv64'
make lkvm-static \
    ARCH=riscv \
    CROSS_COMPILE="${TOOLCHAIN_PREFIX}-" \
    LIBFDT_DIR="${DTC_DIR}/libfdt" \
    V=1

if [[ ! -f "lkvm-static" ]]; then
    echo "ERROR: lkvm-static not built" >&2
    exit 1
fi

"${TOOLCHAIN_PREFIX}-strip" -o "${PREBUILT_DIR}/bin/lkvm" lkvm-static
popd >/dev/null

echo ""
echo "==> kvmtool built successfully!"
echo "    Binary: ${PREBUILT_DIR}/bin/lkvm"
echo "    Architecture: ${ARCH} (static)"
echo ""
echo "    Deploy with: bash tools/linux/deploy_rootfs.sh"
echo "    Then run on Scarlet: lkvm run -k /path/to/Image ..."
