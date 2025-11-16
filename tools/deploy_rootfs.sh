#!/usr/bin/env bash
set -euo pipefail

# This script extracts the prebuilt rootfs tarball into the mounted workspace.
# Optional environment variables:
#  TARGET_UID - if set, chown the deployed files to this UID
#  TARGET_GID - if set, chown the deployed files to this GID

TAR_SRC="/opt/prebuilt/linux-riscv64.tar"
DEST_DIR="/workspaces/Scarlet/mkfs/rootfs/system/linux-riscv64"

if [ ! -f "$TAR_SRC" ]; then
  echo "Error: prebuilt tar not found at $TAR_SRC"
  exit 1
fi

mkdir -p "$DEST_DIR"

if [ "$(ls -A "$DEST_DIR")" ]; then
  echo "Warning: $DEST_DIR is not empty, removing existing contents"
  rm -rf -- "$DEST_DIR"/*
fi

echo "Extracting $TAR_SRC -> $DEST_DIR"
tar -xf "$TAR_SRC" -C "$DEST_DIR"

if [ -n "${TARGET_UID:-}" ] || [ -n "${TARGET_GID:-}" ]; then
  UID=${TARGET_UID:-0}
  GID=${TARGET_GID:-0}
  echo "Applying ownership: $UID:$GID -> $DEST_DIR"
  chown -R "$UID":"$GID" "$DEST_DIR"
fi

echo "Deployed: $DEST_DIR"
