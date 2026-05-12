#!/bin/bash
set -euo pipefail

# Builds optional user-space programs (green, fbdoom, kvmtool) using the preconfigured toolchain.
# Environment variables:
#  ARCH - target architecture (riscv64 or aarch64), defaults to riscv64

: "${ARCH:=riscv64}"
: "${BUILDROOT_DIR:=/opt/buildroot}"
: "${PREBUILT_DIR:=/opt/prebuilt}"
: "${WORKDIR:=/opt}"

: "${GREEN_REPO:=https://github.com/petitstrawberry/green.git}"
: "${FBDOOM_REPO:=https://github.com/petitstrawberry/fbdoom.git}"
: "${KVMTOOL_REPO:=https://github.com/kvmtool/kvmtool.git}"
: "${DTC_REPO:=https://git.kernel.org/pub/scm/utils/dtc/dtc.git}"

: "${GREEN_DIR:=${WORKDIR}/green-${ARCH}}"
: "${FBDOOM_DIR:=${WORKDIR}/fbdoom-${ARCH}}"
: "${KVMTOOL_DIR:=${WORKDIR}/kvmtool-${ARCH}}"
: "${DTC_DIR:=${WORKDIR}/dtc-kvmtool-${ARCH}}"

if [[ ! -d "${BUILDROOT_DIR}" ]]; then
    echo "Expected buildroot directory at ${BUILDROOT_DIR} not found." >&2
    echo "Run ARCH=${ARCH} tools/linux/build_buildroot.sh first." >&2
    exit 1
fi

# Determine toolchain prefix based on architecture
case "${ARCH}" in
    riscv64)
        TOOLCHAIN_PREFIX="riscv64-buildroot-linux-musl"
        ;;
    aarch64)
        TOOLCHAIN_PREFIX="aarch64-buildroot-linux-musl"
        ;;
    *)
        echo "Unsupported architecture: ${ARCH}" >&2
        exit 1
        ;;
esac

toolchain_gcc="${BUILDROOT_DIR}/output/host/bin/${TOOLCHAIN_PREFIX}-gcc"
if [[ ! -x "${toolchain_gcc}" ]]; then
    echo "Buildroot toolchain missing at ${toolchain_gcc}. Run ARCH=${ARCH} tools/linux/build_buildroot.sh first." >&2
    exit 1
fi

mkdir -p "${PREBUILT_DIR}/${ARCH}/bin" "${PREBUILT_DIR}/${ARCH}/lib"

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

clone_or_update "${GREEN_REPO}" "${GREEN_DIR}"
clone_or_update "${FBDOOM_REPO}" "${FBDOOM_DIR}"

if [[ "${ARCH}" == "riscv64" ]]; then
    clone_or_update "${KVMTOOL_REPO}" "${KVMTOOL_DIR}"
    clone_or_update "${DTC_REPO}" "${DTC_DIR}"
fi

echo "Building user programs for ${ARCH}..."

pushd "${GREEN_DIR}" >/dev/null
make DEBUG=1 BUILDROOT="${BUILDROOT_DIR}"
if [[ -f "${GREEN_DIR}/green" ]]; then
    install -m 755 "${GREEN_DIR}/green" "${PREBUILT_DIR}/${ARCH}/bin/green"
fi
if [[ -d "${GREEN_DIR}/usr/lib" ]]; then
    cp -a "${GREEN_DIR}/usr/lib/." "${PREBUILT_DIR}/${ARCH}/lib/"
fi
popd >/dev/null

pushd "${FBDOOM_DIR}/fbdoom" >/dev/null
make CROSS_COMPILE=${TOOLCHAIN_PREFIX}- V=1
if [[ -f "${FBDOOM_DIR}/fbdoom/fbdoom" ]]; then
    install -m 755 "${FBDOOM_DIR}/fbdoom/fbdoom" "${PREBUILT_DIR}/${ARCH}/bin/fbdoom"
fi
popd >/dev/null

if [[ "${ARCH}" == "riscv64" ]]; then
    echo "Building libfdt for kvmtool..."
    pushd "${DTC_DIR}" >/dev/null
    make clean 2>/dev/null || true
    make libfdt CC="${TOOLCHAIN_PREFIX}-gcc" AR="${TOOLCHAIN_PREFIX}-ar" CFLAGS="-fPIC -O2"
    popd >/dev/null

    echo "Building kvmtool (lkvm-static)..."
    pushd "${KVMTOOL_DIR}" >/dev/null
    make clean 2>/dev/null || true
    make lkvm-static ARCH=riscv CROSS_COMPILE="${TOOLCHAIN_PREFIX}-" LIBFDT_DIR="${DTC_DIR}/libfdt" V=1
    if [[ -f "lkvm-static" ]]; then
        "${TOOLCHAIN_PREFIX}-strip" -o "${PREBUILT_DIR}/${ARCH}/bin/lkvm" lkvm-static
    fi
    popd >/dev/null
fi

echo "User programs for ${ARCH} built and placed in ${PREBUILT_DIR}."
