#!/usr/bin/env bash
set -euo pipefail

# This script extracts the prebuilt rootfs tarball into the mounted workspace.
# Optional environment variables:
#  ARCH       - target architecture (riscv64 or aarch64), defaults to riscv64
#  TARGET_UID - if set, chown the deployed files to this UID
#  TARGET_GID - if set, chown the deployed files to this GID

ARCH="${ARCH:-riscv64}"
TAR_SRC="/opt/prebuilt/${ARCH}/rootfs.tar"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
DEST_DIR="${PROJECT_ROOT}/mkfs/rootfs/system/linux-${ARCH}"

if [ ! -f "$TAR_SRC" ]; then
  echo "Error: prebuilt tar not found at $TAR_SRC"
  echo "Available architectures:"
  ls -1 /opt/prebuilt/${ARCH}/*.tar 2>/dev/null || echo "  (none found)"
  exit 1
fi

mkdir -p "$DEST_DIR"

if [ "$(ls -A "$DEST_DIR")" ]; then
  echo "Warning: $DEST_DIR is not empty, removing existing contents"
  rm -rf -- "$DEST_DIR"/*
fi

echo "Deploying prebuilt Linux rootfs for ${ARCH}"
echo "Extracting $TAR_SRC -> $DEST_DIR"
tar -xf "$TAR_SRC" -C "$DEST_DIR"

if [ -n "${TARGET_UID:-}" ] || [ -n "${TARGET_GID:-}" ]; then
  UID=${TARGET_UID:-0}
  GID=${TARGET_GID:-0}
  echo "Applying ownership: $UID:$GID -> $DEST_DIR"
  chown -R "$UID":"$GID" "$DEST_DIR"
fi
# Deploy any prebuilt binaries from /opt/prebuilt/${ARCH}/bin -> rootfs /usr/bin
PREBUILT_BIN_DIR="/opt/prebuilt/${ARCH}/bin"
TARGET_BIN_DIR="$DEST_DIR/usr/bin"
if [ -d "$PREBUILT_BIN_DIR" ]; then
  mkdir -p "$TARGET_BIN_DIR"
  for f in "$PREBUILT_BIN_DIR"/*; do
    [ -e "$f" ] || continue
    echo "Deploying prebuilt bin: $f -> $TARGET_BIN_DIR"
    cp -a "$f" "$TARGET_BIN_DIR/"
    chmod +x "$TARGET_BIN_DIR/$(basename "$f")" || true
    if [ -n "${TARGET_UID:-}" ] || [ -n "${TARGET_GID:-}" ]; then
      UID=${TARGET_UID:-0}
      GID=${TARGET_GID:-0}
      chown "$UID":"$GID" "$TARGET_BIN_DIR/$(basename "$f")"
    fi
  done
else
  echo "No prebuilt bin directory at $PREBUILT_BIN_DIR (skipping)"
fi

# Deploy any prebuilt library fragments under /opt/prebuilt/lib into the rootfs's /usr/lib
PREBUILT_LIB_DIR="/opt/prebuilt/${ARCH}/lib"
if [ -d "$PREBUILT_LIB_DIR" ]; then
  echo "Deploying prebuilt lib tree: $PREBUILT_LIB_DIR -> $DEST_DIR/usr/lib/"
  mkdir -p "$DEST_DIR/usr/lib"
  # copy contents so that prebuilt/lib/libfoo.so -> DEST_DIR/usr/lib/libfoo.so
  rsync -a "$PREBUILT_LIB_DIR/" "$DEST_DIR/usr/lib/"
  if [ -n "${TARGET_UID:-}" ] || [ -n "${TARGET_GID:-}" ]; then
    UID=${TARGET_UID:-0}
    GID=${TARGET_GID:-0}
    chown -R "$UID":"$GID" "$DEST_DIR/usr/lib"
  fi
else
  echo "No prebuilt lib directory at $PREBUILT_LIB_DIR (skipping)"
fi

echo "Deployed: $DEST_DIR"
