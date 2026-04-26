#!/bin/bash
# collect_macos_binaries.sh
# Run this on macOS (ARM64) to collect the minimum files needed
# for Scarlet's Darwin ABI testing.
#
# Usage:
#   ./collect_macos_binaries.sh [output_dir]
#
# Then scp the output_dir to your Scarlet workspace:
#   scp -r macos-binaries/ user@linux-host:/path/to/Scarlet/mkfs/rootfs/system/darwin-aarch64/
#
# IMPORTANT: These files are copyrighted by Apple Inc.
# Do NOT redistribute them. For personal development/testing only.

set -e

OUTDIR="${1:-macos-binaries}"
ARCH=$(uname -m)

if [ "$ARCH" != "arm64" ]; then
    echo "ERROR: This script must be run on Apple Silicon (arm64)."
    echo "Current architecture: $ARCH"
    exit 1
fi

echo "=== Collecting macOS binaries for Scarlet Darwin ABI ==="
echo "Output directory: $OUTDIR"
echo ""

# Create directory structure
mkdir -p "$OUTDIR"/{usr/lib,usr/bin,bin,sbin}
mkdir -p "$OUTDIR/System/Library/dyld"

# 1. dyld - the dynamic linker (MOST IMPORTANT)
echo "[1/5] Copying dyld..."
if [ -f /usr/lib/dyld ]; then
    cp /usr/lib/dyld "$OUTDIR/usr/lib/dyld"
    echo "  ✓ /usr/lib/dyld"
else
    echo "  ✗ /usr/lib/dyld not found!"
    exit 1
fi

# 2. dyld shared cache (contains all system libraries)
echo "[2/5] Copying dyld shared cache..."
CACHE=""
for path in \
    "/System/Library/dyld/dyld_shared_cache_arm64e" \
    "/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld/dyld_shared_cache_arm64e" \
    "/System/Library/Caches/com.apple.dyld/dyld_shared_cache_arm64e"; do
    if [ -f "$path" ]; then
        CACHE="$path"
        break
    fi
done

if [ -n "$CACHE" ]; then
    cp "$CACHE" "$OUTDIR/System/Library/dyld/dyld_shared_cache_arm64e"
    CACHE_SIZE=$(du -sh "$CACHE" | cut -f1)
    echo "  ✓ $CACHE ($CACHE_SIZE)"

    # Copy sub-cache files (e.g. dyld_shared_cache_arm64e.01, .02, etc.)
    CACHE_DIR=$(dirname "$CACHE")
    CACHE_BASE=$(basename "$CACHE")
    for subcache in "$CACHE_DIR/${CACHE_BASE}."*; do
        if [ -f "$subcache" ]; then
            SUB_BASE=$(basename "$subcache")
            cp "$subcache" "$OUTDIR/System/Library/dyld/$SUB_BASE"
            SUB_SIZE=$(du -sh "$subcache" | cut -f1)
            echo "  ✓ $SUB_BASE ($SUB_SIZE)"
        fi
    done
else
    echo "  ✗ dyld shared cache not found!"
    echo "  Try: find /System -name 'dyld_shared_cache_arm64e' 2>/dev/null"
fi

# 3. Test binaries
echo "[3/5] Copying test binaries..."
for bin in echo cat ls pwd hostname date true false sh; do
    if [ -f "/bin/$bin" ]; then
        # Resolve symlinks
        REAL=$(realpath "/bin/$bin")
        cp "$REAL" "$OUTDIR/bin/$bin" 2>/dev/null || true
        echo "  ✓ /bin/$bin -> $REAL"
    fi
done

# 4. libSystem symlink (for compatibility)
echo "[4/5] Setting up libSystem..."
# On modern macOS, libSystem is in the shared cache, but the symlink stub may exist
if [ -f /usr/lib/libSystem.B.dylib ]; then
    cp /usr/lib/libSystem.B.dylib "$OUTDIR/usr/lib/libSystem.B.dylib" 2>/dev/null || true
    echo "  ✓ /usr/lib/libSystem.B.dylib"
fi
if [ -L /usr/lib/libSystem.dylib ]; then
    # Create symlink
    ln -sf libSystem.B.dylib "$OUTDIR/usr/lib/libSystem.dylib" 2>/dev/null || true
    echo "  ✓ /usr/lib/libSystem.dylib -> libSystem.B.dylib"
fi

# 5. Collect system lib dylibs (if they exist as individual files)
echo "[5/5] Collecting system libraries..."
SYS_LIB_DIR="/usr/lib/system"
if [ -d "$SYS_LIB_DIR" ]; then
    mkdir -p "$OUTDIR/usr/lib/system"
    for lib in "$SYS_LIB_DIR"/libsystem_*.dylib; do
        if [ -f "$lib" ]; then
            cp "$lib" "$OUTDIR/usr/lib/system/" 2>/dev/null || true
            basename=$(basename "$lib")
            echo "  ✓ $basename"
        fi
    done
else
    echo "  (system lib dir not found - expected on modern macOS, shared cache handles this)"
fi

# Summary
echo ""
echo "=== Summary ==="
echo "Files collected in: $OUTDIR/"
find "$OUTDIR" -type f | head -20
TOTAL=$(du -sh "$OUTDIR" | cut -f1)
FILE_COUNT=$(find "$OUTDIR" -type f | wc -l)
echo ""
echo "Total: $FILE_COUNT files, $TOTAL"
echo ""
echo "Next steps:"
echo "  1. tar czf macos-binaries.tar.gz $OUTDIR/"
echo "  2. scp macos-binaries.tar.gz user@linux-host:/tmp/"
echo "  3. On Linux: tar xzf /tmp/macos-binaries.tar.gz -C /path/to/Scarlet/mkfs/rootfs/system/darwin-aarch64/ --strip-components=1"
