#!/usr/bin/env bash
set -euo pipefail

# This script extracts the prebuilt rootfs tarball into the mounted workspace.
# Optional environment variables:
#  ARCH       - target architecture (riscv64 or aarch64), defaults to riscv64
#  PREBUILT_DIR - artifact staging directory, defaults to /opt/prebuilt
#  TARGET_UID - if set, chown the deployed files to this UID
#  TARGET_GID - if set, chown the deployed files to this GID

ARCH="${ARCH:-riscv64}"
PREBUILT_DIR="${PREBUILT_DIR:-/opt/prebuilt}"
TAR_SRC="${PREBUILT_DIR}/${ARCH}/rootfs.tar"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
DEST_DIR="${PROJECT_ROOT}/bundles/linux/rootfs/linux-${ARCH}"

if [ ! -f "$TAR_SRC" ]; then
  echo "Error: prebuilt tar not found at $TAR_SRC"
  echo "Available architectures:"
  ls -1 "${PREBUILT_DIR}/${ARCH}"/*.tar 2>/dev/null || echo "  (none found)"
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

PREBUILT_ROOT_DIR="${PREBUILT_DIR}/${ARCH}/root"
if [ -d "$PREBUILT_ROOT_DIR" ]; then
  echo "Deploying prebuilt root overlay: $PREBUILT_ROOT_DIR -> $DEST_DIR"
  rsync -a "$PREBUILT_ROOT_DIR/" "$DEST_DIR/"
fi

if [ -n "${TARGET_UID:-}" ] || [ -n "${TARGET_GID:-}" ]; then
  OWNER_UID=${TARGET_UID:-0}
  OWNER_GID=${TARGET_GID:-0}
  echo "Applying ownership: $OWNER_UID:$OWNER_GID -> $DEST_DIR"
  chown -R "$OWNER_UID":"$OWNER_GID" "$DEST_DIR"
fi

mkdir -p \
  "$DEST_DIR/root/.cache/gtk-3.0/compose" \
  "$DEST_DIR/root/.config/gtk-3.0" \
  "$DEST_DIR/root/.local/share/zathura"

# Deploy any prebuilt binaries from ${PREBUILT_DIR}/${ARCH}/bin -> rootfs /usr/bin
PREBUILT_BIN_DIR="${PREBUILT_DIR}/${ARCH}/bin"
TARGET_BIN_DIR="$DEST_DIR/usr/bin"
if [ -d "$PREBUILT_BIN_DIR" ]; then
  mkdir -p "$TARGET_BIN_DIR"
  for f in "$PREBUILT_BIN_DIR"/*; do
    [ -e "$f" ] || continue
    echo "Deploying prebuilt bin: $f -> $TARGET_BIN_DIR"
    cp -a "$f" "$TARGET_BIN_DIR/"
    chmod +x "$TARGET_BIN_DIR/$(basename "$f")" || true
    if [ -n "${TARGET_UID:-}" ] || [ -n "${TARGET_GID:-}" ]; then
      OWNER_UID=${TARGET_UID:-0}
      OWNER_GID=${TARGET_GID:-0}
      chown "$OWNER_UID":"$OWNER_GID" "$TARGET_BIN_DIR/$(basename "$f")"
    fi
  done
else
  echo "No prebuilt bin directory at $PREBUILT_BIN_DIR (skipping)"
fi

# Deploy any prebuilt library fragments into the rootfs's /usr/lib
PREBUILT_LIB_DIR="${PREBUILT_DIR}/${ARCH}/lib"
if [ -d "$PREBUILT_LIB_DIR" ]; then
  echo "Deploying prebuilt lib tree: $PREBUILT_LIB_DIR -> $DEST_DIR/usr/lib/"
  mkdir -p "$DEST_DIR/usr/lib"
  # copy contents so that prebuilt/lib/libfoo.so -> DEST_DIR/usr/lib/libfoo.so
  rsync -a "$PREBUILT_LIB_DIR/" "$DEST_DIR/usr/lib/"
  if [ -n "${TARGET_UID:-}" ] || [ -n "${TARGET_GID:-}" ]; then
    OWNER_UID=${TARGET_UID:-0}
    OWNER_GID=${TARGET_GID:-0}
    chown -R "$OWNER_UID":"$OWNER_GID" "$DEST_DIR/usr/lib"
  fi
else
  echo "No prebuilt lib directory at $PREBUILT_LIB_DIR (skipping)"
fi

# Deploy any prebuilt shared data into the rootfs's /usr/share
PREBUILT_SHARE_DIR="${PREBUILT_DIR}/${ARCH}/share"
if [ -d "$PREBUILT_SHARE_DIR" ]; then
  echo "Deploying prebuilt share tree: $PREBUILT_SHARE_DIR -> $DEST_DIR/usr/share/"
  mkdir -p "$DEST_DIR/usr/share"
  rsync -a "$PREBUILT_SHARE_DIR/" "$DEST_DIR/usr/share/"
  if [ -n "${TARGET_UID:-}" ] || [ -n "${TARGET_GID:-}" ]; then
    OWNER_UID=${TARGET_UID:-0}
    OWNER_GID=${TARGET_GID:-0}
    chown -R "$OWNER_UID":"$OWNER_GID" "$DEST_DIR/usr/share"
  fi
else
  echo "No prebuilt share directory at $PREBUILT_SHARE_DIR (skipping)"
fi

echo "Deployed: $DEST_DIR"
