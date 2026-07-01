#!/bin/sh
set -e

BASE="$(cd "$(dirname "$0")" && pwd)"
UBOOT="$BASE/u-boot"
M1N1="$BASE/m1n1"

IMAGE="${UBOOT}/u-boot-nodtb.bin"
PAYLOADS="$M1N1/payloads"

docker run --rm -v "$UBOOT":/work -w /work ubuntu:24.04 bash -c '
  apt-get update -qq && apt-get install -y -qq build-essential bc bison flex libssl-dev libgnutls28-dev python3 device-tree-compiler >/dev/null 2>&1 &&
  make apple_m1_defconfig &&
  make -j$(nproc)
'

cp "$IMAGE" "$PAYLOADS/u-boot-nodtb.bin"
gzip -kf "$PAYLOADS/u-boot-nodtb.bin"

for machine in "$@"; do
  echo "Generating boot-${machine}.bin..."
  python3 "$M1N1/make-boot.py" "$machine"
done
