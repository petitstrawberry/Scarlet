#!/usr/bin/env bash
set -euo pipefail
PROJECT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 "$PROJECT/tools/run.py" \
  --staging "$PROJECT/.scarlet/active/staging" \
  --output "$PROJECT/.scarlet/run" \
  --boot-image "$PROJECT/.scarlet/images/boot.img" \
  --rootfs-image "$PROJECT/.scarlet/images/rootfs.ext2" \
  --timeout "${SCARLET_CROSVM_TIMEOUT:-120}" \
  --success-marker "${SCARLET_CROSVM_SUCCESS_MARKER:-SCARLET_CROSVM_GUEST_OK}"
