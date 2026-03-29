#!/usr/bin/env bash
set -euo pipefail

# Extract Windows ARM64 system DLLs from the Windows ISO into the ABI rootfs.
#
# Prerequisites:
#   - 7z and wimextract (wimtools) must be installed
#   - The Windows ARM64 ISO must exist at WORKSPACE_ROOT
#
# Environment variables:
#   ISO_PATH  - path to the Windows ARM64 ISO (default: Win11_25H2_English_Arm64_v2.iso in workspace root)
#   WIM_INDEX - WIM image index to extract from (default: 1)
#   DEST_DIR  - deployment target (default: mkfs/rootfs/system/windows-arm64)

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

ISO_PATH="${ISO_PATH:-${WORKSPACE_ROOT}/Win11_25H2_English_Arm64_v2.iso}"
WIM_INDEX="${WIM_INDEX:-1}"
DEST_DIR="${DEST_DIR:-${WORKSPACE_ROOT}/mkfs/rootfs/system/windows-aarch64}"

DLLS=(
  "System32/ntdll.dll"
  "System32/kernel32.dll"
  "System32/ucrtbase.dll"
  "System32/msvcrt.dll"
  "System32/secur32.dll"
  "System32/cryptbase.dll"
  "System32/apphelp.dll"
  "System32/bcrypt.dll"
  "System32/rpcrt4.dll"
  "System32/advapi32.dll"
)

if [ ! -f "$ISO_PATH" ]; then
  echo "Error: Windows ARM64 ISO not found at $ISO_PATH"
  exit 1
fi

if ! command -v wimextract &>/dev/null; then
  echo "Error: wimextract not found. Install wimtools."
  exit 1
fi

echo "Deploying Windows ARM64 system DLLs"
echo "  ISO:      $ISO_PATH"
echo "  WIM index: $WIM_INDEX"
echo "  Target:   $DEST_DIR"

WIM_CACHE="/tmp/win11_iso_cache"
mkdir -p "$WIM_CACHE"

if [ ! -f "$WIM_CACHE/install.wim" ]; then
  echo "Extracting install.wim from ISO..."
  mkdir -p "$WIM_CACHE"
  7z x -o"$WIM_CACHE" "$ISO_PATH" sources/install.wim -y -bso0
fi

DLL_DIR="$DEST_DIR/System32"
mkdir -p "$DLL_DIR"

DLL_ARGS=()
for dll in "${DLLS[@]}"; do
  DLL_ARGS+=("$dll")
done

echo "Extracting ${#DLLS[@]} DLLs from install.wim..."
wimextract "$WIM_CACHE/install.wim" "$WIM_INDEX" "${DLL_ARGS[@]}" --dest-dir="$DLL_DIR" 2>&1 | grep -E "^(Done|Error)" || true

echo ""
echo "Deployed DLLs:"
ls -lhS "$DLL_DIR"/*.dll 2>/dev/null || echo "  (no DLLs found)"
echo ""
echo "Done: $DEST_DIR"
