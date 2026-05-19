#!/bin/sh

set -eu

cd "$(dirname "$0")" || exit 1

DIST_DIR="dist"
LIMINE_VERSION="${LIMINE_VERSION:-12.3.0}"
LIMINE_SRC_DIR="$DIST_DIR/limine-${LIMINE_VERSION}"
LIMINE_BINARY_URL="${LIMINE_BINARY_URL:-https://github.com/Limine-Bootloader/Limine/releases/download/v${LIMINE_VERSION}/limine-binary.tar.xz}"
case "$LIMINE_VERSION:$LIMINE_BINARY_URL" in
    12.3.0:https://github.com/Limine-Bootloader/Limine/releases/download/v12.3.0/limine-binary.tar.xz)
        LIMINE_BINARY_SHA256="${LIMINE_BINARY_SHA256:-67e11e0a72595e621cc39bfc1572c70149d71700eaeee7b9d0b32ba0effe07ac}"
        ;;
    *)
        LIMINE_BINARY_SHA256="${LIMINE_BINARY_SHA256:-}"
        ;;
esac
BOOT_IMAGE="${BOOT_IMAGE:-$DIST_DIR/limine-aarch64-boot.img}"
KERNEL_ELF="${KERNEL_ELF:-../projects/aarch64-limine-full/target/aarch64-unknown-none-elf/debug/scarlet}"
INITRAMFS_PATH="${INITRAMFS_PATH:-$DIST_DIR/initramfs-aarch64.cpio}"
ROOTFS_IMAGE="${ROOTFS_IMAGE:-$DIST_DIR/rootfs.img}"
CONFIG_PATH="$DIST_DIR/limine-aarch64.conf"
IMAGE_SLACK_MB="${IMAGE_SLACK_MB:-32}"
LIMINE_CMDLINE="${LIMINE_CMDLINE:-}"

require_file() {
    if [ ! -f "$1" ]; then
        echo "Error: required file not found: $1" >&2
        exit 1
    fi
}

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Error: required command not found: $1" >&2
        exit 1
    fi
}

ensure_limine_source() {
    if [ -f "$LIMINE_SRC_DIR/BOOTAA64.EFI" ]; then
        return
    fi

    rm -rf "$LIMINE_SRC_DIR"

    if command -v curl >/dev/null 2>&1 && command -v tar >/dev/null 2>&1; then
        archive="$DIST_DIR/limine-${LIMINE_VERSION}-binary.tar.xz"
        if ! curl -L --fail --output "$archive" "$LIMINE_BINARY_URL"; then
            rm -f "$archive"
        else
            if [ -n "$LIMINE_BINARY_SHA256" ]; then
                if command -v sha256sum >/dev/null 2>&1; then
                    printf '%s  %s\n' "$LIMINE_BINARY_SHA256" "$archive" | sha256sum -c -
                elif command -v shasum >/dev/null 2>&1; then
                    printf '%s  %s\n' "$LIMINE_BINARY_SHA256" "$archive" | shasum -a 256 -c -
                else
                    echo "Error: cannot verify $archive; sha256sum or shasum is required" >&2
                    exit 1
                fi
            fi
            mkdir -p "$LIMINE_SRC_DIR"
            tar -xf "$archive" -C "$LIMINE_SRC_DIR" --strip-components=1
            require_file "$LIMINE_SRC_DIR/BOOTAA64.EFI"
            return
        fi
    fi

    require_command git
    git clone \
        --depth=1 \
        --branch "v${LIMINE_VERSION}-binary" \
        https://github.com/limine-bootloader/limine.git \
        "$LIMINE_SRC_DIR"
}

file_size_bytes() {
    wc -c < "$1" | tr -d ' '
}

align_up_mb() {
    bytes="$1"
    mb=$(((bytes + 1024 * 1024 - 1) / (1024 * 1024)))
    echo $((((mb + 15) / 16) * 16))
}

mkdir -p "$DIST_DIR"

require_command make
require_command mformat
require_command mmd
require_command mcopy

require_file "$KERNEL_ELF"
require_file "$INITRAMFS_PATH"

ensure_limine_source

require_file "$LIMINE_SRC_DIR/BOOTAA64.EFI"

{
    printf '%s\n' 'timeout: 0'
    printf '%s\n' 'serial: yes'
    printf '%s\n' 'verbose: yes'
    printf '\n'
    printf '%s\n' '/Scarlet AArch64 (Limine)'
    printf '%s\n' '    protocol: limine'
    printf '%s\n' '    path: boot():/boot/kernel'
    printf '%s\n' '    module_path: boot():/boot/initramfs-aarch64.cpio'
    printf '%s\n' '    module_string: initramfs'
    if [ -n "$LIMINE_CMDLINE" ]; then
        printf '    cmdline: %s\n' "$LIMINE_CMDLINE"
    else
        printf '%s\n' '    cmdline: console=ttyAMA0'
    fi
} >"$CONFIG_PATH"

payload_bytes=$(file_size_bytes "$LIMINE_SRC_DIR/BOOTAA64.EFI")
payload_bytes=$((payload_bytes + $(file_size_bytes "$CONFIG_PATH")))
payload_bytes=$((payload_bytes + $(file_size_bytes "$KERNEL_ELF")))
payload_bytes=$((payload_bytes + $(file_size_bytes "$INITRAMFS_PATH")))

required_image_size_mb=$(align_up_mb $((payload_bytes + IMAGE_SLACK_MB * 1024 * 1024)))

if [ "${BOOT_IMAGE_SIZE_MB+x}" = x ] && [ -n "${BOOT_IMAGE_SIZE_MB:-}" ]; then
    if [ "$BOOT_IMAGE_SIZE_MB" -lt "$required_image_size_mb" ]; then
        echo "Requested BOOT_IMAGE_SIZE_MB=$BOOT_IMAGE_SIZE_MB is too small; using required size ${required_image_size_mb}MB instead."
        boot_image_size_mb="$required_image_size_mb"
    else
        boot_image_size_mb="$BOOT_IMAGE_SIZE_MB"
    fi
else
    boot_image_size_mb="$required_image_size_mb"
fi

rm -f "$BOOT_IMAGE"
dd if=/dev/zero of="$BOOT_IMAGE" bs=1M count="$boot_image_size_mb" >/dev/null 2>&1
mformat -i "$BOOT_IMAGE" -F -v SCARLET_AA ::
mmd -i "$BOOT_IMAGE" ::/EFI
mmd -i "$BOOT_IMAGE" ::/EFI/BOOT
mmd -i "$BOOT_IMAGE" ::/boot

mcopy -i "$BOOT_IMAGE" "$LIMINE_SRC_DIR/BOOTAA64.EFI" ::/EFI/BOOT/BOOTAA64.EFI
mcopy -i "$BOOT_IMAGE" "$CONFIG_PATH" ::/EFI/BOOT/limine.conf
mcopy -i "$BOOT_IMAGE" "$KERNEL_ELF" ::/boot/kernel
mcopy -i "$BOOT_IMAGE" "$INITRAMFS_PATH" ::/boot/initramfs-aarch64.cpio
mcopy -i "$BOOT_IMAGE" - <<'EOF' ::/startup.nsh
FS0:
EFI\BOOT\BOOTAA64.EFI
EOF

echo "Created Limine AArch64 boot image: $BOOT_IMAGE"
