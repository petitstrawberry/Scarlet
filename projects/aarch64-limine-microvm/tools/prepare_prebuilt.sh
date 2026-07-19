#!/bin/sh
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_DIR="$(cd "$PROJECT_DIR/../.." && pwd)"

PREBUILT_DIR="${SCARLET_MICROVM_PREBUILT_DIR:-$PROJECT_DIR/prebuilt}"
GUEST_BIN_DIR="${SCARLET_MICROVM_GUEST_BIN_DIR:-}"

FIRECRACKER_VERSION="${SCARLET_FIRECRACKER_VERSION:-1.15.1}"
FIRECRACKER_ARCHIVE="firecracker-v${FIRECRACKER_VERSION}-aarch64.tgz"
FIRECRACKER_URL="${SCARLET_FIRECRACKER_URL:-https://github.com/firecracker-microvm/firecracker/releases/download/v${FIRECRACKER_VERSION}/${FIRECRACKER_ARCHIVE}}"
FIRECRACKER_CACHE_DIR="${SCARLET_FIRECRACKER_CACHE_DIR:-$PROJECT_DIR/.scarlet/cache/firecracker}"
FIRECRACKER_ARCHIVE_SHA256="${SCARLET_FIRECRACKER_ARCHIVE_SHA256:-00654ac1e702a22744121ea9f10a4f792ebd7c3a744cba587dfac9fcb79b41a5}"

target_bin="$PREBUILT_DIR/system/linux-aarch64/usr/bin"
mkdir -p "$target_bin"

find_guest_bin_dir() {
    if [ -n "$GUEST_BIN_DIR" ] && [ -f "$GUEST_BIN_DIR/guest-Image" ] && [ -f "$GUEST_BIN_DIR/guest-initramfs.cpio.gz" ]; then
        printf '%s\n' "$GUEST_BIN_DIR"
        return 0
    fi

    for candidate in \
        "$REPO_DIR/bundles/linux/rootfs/linux-aarch64/usr/bin" \
        "/opt/prebuilt/aarch64/bin"
    do
        if [ -f "$candidate/guest-Image" ] && [ -f "$candidate/guest-initramfs.cpio.gz" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done

    return 1
}

guest_bin_dir="$(find_guest_bin_dir || true)"
if [ -z "$guest_bin_dir" ]; then
    echo "AArch64 guest kernel artifacts were not found." >&2
    echo "Run ARCH=aarch64 bundles/linux/tools/build_buildroot.sh and ARCH=aarch64 bundles/linux/tools/build_guest_image.sh," >&2
    echo "or set SCARLET_MICROVM_GUEST_BIN_DIR to a directory containing guest-Image and guest-initramfs.cpio.gz." >&2
    exit 1
fi

echo "Copying guest artifacts from $guest_bin_dir"
cp "$guest_bin_dir/guest-Image" "$target_bin/guest-Image"
cp "$guest_bin_dir/guest-initramfs.cpio.gz" "$target_bin/guest-initramfs.cpio.gz"
chmod +x "$target_bin/guest-Image" "$target_bin/guest-initramfs.cpio.gz"

if [ ! -x "$target_bin/firecracker" ]; then
    if ! command -v curl >/dev/null 2>&1; then
        echo "curl is required to download Firecracker" >&2
        exit 1
    fi
    if ! command -v tar >/dev/null 2>&1; then
        echo "tar is required to extract Firecracker" >&2
        exit 1
    fi

    mkdir -p "$FIRECRACKER_CACHE_DIR"
    archive="$FIRECRACKER_CACHE_DIR/$FIRECRACKER_ARCHIVE"
    if [ ! -f "$archive" ]; then
        echo "Downloading Firecracker v$FIRECRACKER_VERSION for aarch64"
        curl -fL "$FIRECRACKER_URL" -o "$archive"
    fi

    if command -v shasum >/dev/null 2>&1; then
        actual_sha256="$(shasum -a 256 "$archive" | awk '{print $1}')"
    elif command -v sha256sum >/dev/null 2>&1; then
        actual_sha256="$(sha256sum "$archive" | awk '{print $1}')"
    else
        echo "shasum or sha256sum is required to verify Firecracker" >&2
        exit 1
    fi

    if [ "$actual_sha256" != "$FIRECRACKER_ARCHIVE_SHA256" ]; then
        echo "Firecracker archive checksum mismatch: $archive" >&2
        echo "expected: $FIRECRACKER_ARCHIVE_SHA256" >&2
        echo "actual:   $actual_sha256" >&2
        exit 1
    fi

    extract_dir="$FIRECRACKER_CACHE_DIR/extract-v$FIRECRACKER_VERSION"
    rm -rf "$extract_dir"
    mkdir -p "$extract_dir"
    tar -xzf "$archive" -C "$extract_dir"

    binary="$(find "$extract_dir" -type f -name "firecracker-v${FIRECRACKER_VERSION}-aarch64" | head -n 1)"
    if [ -z "$binary" ]; then
        echo "Firecracker binary not found in $archive" >&2
        exit 1
    fi

    cp "$binary" "$target_bin/firecracker"
    chmod +x "$target_bin/firecracker"
fi

echo "Prepared microvm prebuilt artifacts in $PREBUILT_DIR"
