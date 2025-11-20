#!/bin/bash
set -euo pipefail

# Builds optional user-space programs (green, fbdoom) using the preconfigured toolchain.

: "${BUILDROOT_DIR:=/opt/buildroot}"
: "${PREBUILT_DIR:=/opt/prebuilt}"
: "${WORKDIR:=/opt}"

: "${GREEN_REPO:=https://github.com/petitstrawberry/green.git}"
: "${FBDOOM_REPO:=https://github.com/petitstrawberry/fbdoom.git}"

: "${GREEN_DIR:=${WORKDIR}/green}"
: "${FBDOOM_DIR:=${WORKDIR}/fbdoom}"

if [[ ! -d "${BUILDROOT_DIR}" ]]; then
    echo "Expected buildroot directory at ${BUILDROOT_DIR} not found." >&2
    exit 1
fi

toolchain_gcc="${BUILDROOT_DIR}/output/host/bin/riscv64-buildroot-linux-musl-gcc"
if [[ ! -x "${toolchain_gcc}" ]]; then
    echo "Buildroot toolchain missing at ${toolchain_gcc}. Run tools/linux/build_buildroot.sh first." >&2
    exit 1
fi

mkdir -p "${PREBUILT_DIR}/bin" "${PREBUILT_DIR}/lib"

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

pushd "${GREEN_DIR}" >/dev/null
make DEBUG=1 BUILDROOT="${BUILDROOT_DIR}"
if [[ -f "${GREEN_DIR}/green" ]]; then
    install -m 755 "${GREEN_DIR}/green" "${PREBUILT_DIR}/bin/green"
fi
if [[ -d "${GREEN_DIR}/usr/lib" ]]; then
    cp -a "${GREEN_DIR}/usr/lib/." "${PREBUILT_DIR}/lib/"
fi
popd >/dev/null

pushd "${FBDOOM_DIR}/fbdoom" >/dev/null
make CROSS_COMPILE=riscv64-buildroot-linux-musl- V=1
if [[ -f "${FBDOOM_DIR}/fbdoom/fbdoom" ]]; then
    install -m 755 "${FBDOOM_DIR}/fbdoom/fbdoom" "${PREBUILT_DIR}/bin/fbdoom"
fi
popd >/dev/null

echo "User programs built and placed in ${PREBUILT_DIR}."
