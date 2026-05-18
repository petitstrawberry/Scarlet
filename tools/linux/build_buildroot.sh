#!/bin/bash
set -euo pipefail

# Builds Buildroot using the repository-provided configuration.

: "${BUILDROOT_DIR:=/opt/buildroot}"
: "${PREBUILT_DIR:=/opt/prebuilt}"
: "${IMAGES_DIR:=output/images}"
: "${MAKE_JOBS:=$(nproc)}"

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

echo "Buildroot artifacts staged under ${PREBUILT_DIR}."
