#!/bin/bash

# Test runner for Scarlet userland libraries
# This script builds test binaries and runs them in QEMU

set -e

# Default values
DEBUG_MODE=false
ARCH="riscv64"
LIB_NAME=""

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --debug)
            DEBUG_MODE=true
            shift
            ;;
        --arch)
            ARCH="$2"
            shift 2
            ;;
        --lib)
            LIB_NAME="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--debug] [--arch riscv64|aarch64] [--lib std|ui]"
            exit 1
            ;;
    esac
done

# Validate library name
if [ -z "$LIB_NAME" ]; then
    echo "Error: --lib option is required (std or ui)"
    exit 1
fi

if [ "$LIB_NAME" != "std" ] && [ "$LIB_NAME" != "ui" ]; then
    echo "Error: --lib must be 'std' or 'ui'"
    exit 1
fi

# Find the project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR" && cd .. && pwd)"

# Set paths based on library
if [ "$LIB_NAME" = "std" ]; then
    LIB_DIR="$PROJECT_ROOT/user/lib/std"
    TEST_NAME="std_tests"
elif [ "$LIB_NAME" = "ui" ]; then
    LIB_DIR="$PROJECT_ROOT/user/lib/scarlet-ui"
    TEST_NAME="ui_tests"
fi

echo "========================================"
echo "Scarlet Userland Test Runner"
echo "========================================"
echo "Library: $LIB_NAME"
echo "Architecture: $ARCH"
echo "Debug mode: $DEBUG_MODE"
echo ""

# Set target based on architecture
if [ "$ARCH" = "riscv64" ]; then
    TARGET="riscv64gc-unknown-scarlet-elf-test"
    QEMU_CMD="qemu-system-riscv64"
    QEMU_MACHINE="-machine virt -cpu rv64,v=true,vlen=256"
    QEMU_BIOS="-bios default"
elif [ "$ARCH" = "aarch64" ]; then
    TARGET="aarch64-unknown-scarlet-elf-test"
    QEMU_CMD="qemu-system-aarch64"
    QEMU_MACHINE="-machine virt -cpu cortex-a72"
    QEMU_BIOS="-bios $PROJECT_ROOT/u-boot-aarch64.bin"
else
    echo "Error: Unsupported architecture: $ARCH"
    exit 1
fi

# Build the kernel first
echo "Building kernel for $ARCH..."
cd "$PROJECT_ROOT/kernel"
if [ "$ARCH" = "riscv64" ]; then
    cargo build --target targets/riscv64gc-unknown-none-elf.json
    KERNEL_BINARY="$PROJECT_ROOT/kernel/target/riscv64gc-unknown-none-elf/debug/kernel"
elif [ "$ARCH" = "aarch64" ]; then
    cargo build --target targets/aarch64-unknown-none-elf.json
    KERNEL_BINARY="$PROJECT_ROOT/kernel/target/aarch64-unknown-none-elf/debug/kernel"
fi

if [ ! -f "$KERNEL_BINARY" ]; then
    echo "Error: Kernel binary not found: $KERNEL_BINARY"
    exit 1
fi

# Build the test binary
echo "Building test binary for $LIB_NAME ($ARCH)..."
cd "$LIB_DIR"
cargo test --no-run --features test_harness --target "../../targets/${TARGET}.json"

# Find the test binary
TEST_BINARY=$(find "target/${TARGET}/debug/deps/" -name "${TEST_NAME}-*" -type f ! -name "*.d" ! -name "*.o" | head -1)

if [ ! -f "$TEST_BINARY" ]; then
    echo "Error: Test binary not found in target/${TARGET}/debug/deps/"
    exit 1
fi

echo "Test binary: $TEST_BINARY"

# Create a temporary dist directory for test binary
TEST_DIST_DIR="$PROJECT_ROOT/user/bin/dist/${ARCH}-test"
mkdir -p "$TEST_DIST_DIR"
rm -f "$TEST_DIST_DIR"/*

# Copy test binary as 'init' so it runs automatically
cp "$TEST_BINARY" "$TEST_DIST_DIR/init"
chmod +x "$TEST_DIST_DIR/init"

# Build test initramfs
echo "Building test initramfs..."
cd "$PROJECT_ROOT/mkfs"
mkdir -p initramfs-test/system/scarlet/bin
rm -f initramfs-test/system/scarlet/bin/*
cp "$TEST_DIST_DIR"/* initramfs-test/system/scarlet/bin/

# Create the CPIO archive
cd initramfs-test
mkdir -p ../dist
INITRAMFS_PATH="$PROJECT_ROOT/mkfs/dist/initramfs-${ARCH}-test.cpio"
find . | cpio -o -H newc > "$INITRAMFS_PATH"
cd ..

if [ ! -f "$INITRAMFS_PATH" ]; then
    echo "Error: Failed to create initramfs: $INITRAMFS_PATH"
    exit 1
fi

echo "Initramfs created: $INITRAMFS_PATH"

# Generate test filesystems
echo "Generating test filesystems..."
cd "$PROJECT_ROOT/kernel"
"$PROJECT_ROOT/kernel/tools/create-fat32-image.sh"
"$PROJECT_ROOT/kernel/tools/create-ext2-image.sh"

# Create temporary file for capturing output
TEMP_OUTPUT=$(mktemp)

echo ""
echo "========================================"
echo "Starting QEMU..."
echo "========================================"

# Run QEMU
if [ "$DEBUG_MODE" = true ]; then
    echo "DEBUG MODE: Starting qemu with gdb server..."
    echo "Connect with: gdb $KERNEL_BINARY -ex 'target remote :12345'"
    
    $QEMU_CMD \
        $QEMU_MACHINE \
        $QEMU_BIOS \
        -m 4G \
        -nographic \
        -serial mon:stdio \
        --no-reboot \
        -global virtio-mmio.force-legacy=false \
        -drive id=x0,file="$PROJECT_ROOT/kernel/fat32-test.img",format=raw,if=none \
        -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
        -drive id=x1,file="$PROJECT_ROOT/kernel/ext2-test.img",format=raw,if=none \
        -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.5 \
        -initrd "$INITRAMFS_PATH" \
        -gdb tcp::12345 -S \
        -kernel "$KERNEL_BINARY" | tee "$TEMP_OUTPUT"
else
    $QEMU_CMD \
        $QEMU_MACHINE \
        $QEMU_BIOS \
        -m 4G \
        -nographic \
        -serial mon:stdio \
        --no-reboot \
        -global virtio-mmio.force-legacy=false \
        -drive id=x0,file="$PROJECT_ROOT/kernel/fat32-test.img",format=raw,if=none \
        -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
        -drive id=x1,file="$PROJECT_ROOT/kernel/ext2-test.img",format=raw,if=none \
        -device virtio-blk-device,drive=x1,bus=virtio-mmio-bus.5 \
        -initrd "$INITRAMFS_PATH" \
        -kernel "$KERNEL_BINARY" | tee "$TEMP_OUTPUT"
fi

# Capture QEMU exit code
QEMU_EXIT_CODE=$?

echo ""
echo "========================================"
echo "Test Results"
echo "========================================"

# Check for test failure patterns in output
if grep -q "\[Test Runner\] Test failed" "$TEMP_OUTPUT"; then
    echo "❌ Test failure detected"
    rm -f "$TEMP_OUTPUT"
    exit 1
elif grep -q "\[Test Runner\] All .* tests passed" "$TEMP_OUTPUT"; then
    echo "✅ All tests passed"
    rm -f "$TEMP_OUTPUT"
    exit 0
else
    echo "⚠️  Could not determine test result, QEMU exit code: $QEMU_EXIT_CODE"
    rm -f "$TEMP_OUTPUT"
    exit $QEMU_EXIT_CODE
fi
