#!/usr/bin/env bash
# Collect octox user binaries (those starting with "_")
# Default source: /opt/octox/target/riscv64gc-unknown-none-elf/release
# Default dest: mkfs/rootfs/system/octox-riscv64

set -euo pipefail

SRC=${1:-/opt/octox/target/riscv64gc-unknown-none-elf/release}
DEST=${2:-$(dirname "$0")/../mkfs/rootfs/system/octox-riscv64}

# Resolve DEST to repo-root relative path
DEST=$(realpath -m "$DEST")

if [ ! -d "$SRC" ]; then
  echo "Source directory not found: $SRC"
  echo "If you built octox in a container, run the script from the host where /opt/octox is mounted, or pass the built path as the first argument." >&2
  exit 1
fi

mkdir -p "$DEST"

echo "Collecting octox user binaries from: $SRC"
shopt -s nullglob
copied=0
for f in "$SRC"/_*; do
  if [ -f "$f" ] && [ -x "$f" ]; then
    base=$(basename "$f")
    newname="${base#_}"
    echo "  -> copying $base -> $newname"
    # Copy while preserving mode/timestamps. Use --preserve for portability.
    cp --preserve=mode,timestamps "$f" "$DEST/$newname"
    # Ensure executable bit is kept (some filesystems may drop it when renaming)
    if [ -x "$f" ]; then
      chmod +x "$DEST/$newname" || true
    fi
    copied=$((copied+1))
  fi
done

if [ "$copied" -eq 0 ]; then
  echo "No executable files starting with '_' found in $SRC"
  exit 2
fi

echo "Copied $copied files to $DEST"
echo "You can now build Docker (files will be included in the build context)."
