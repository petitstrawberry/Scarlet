#!/bin/bash

# Create FAT32 disk image for testing
# This script creates a FAT32 filesystem image that can be used for testing
# with the virtio-blk device driver in QEMU

# Configuration
IMAGE_SIZE="64M"  # 64MB image
IMAGE_NAME="fat32-test.img"
TEST_FILES_DIR="test_files"

# Get script directory and kernel directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KERNEL_DIR="$(dirname "$SCRIPT_DIR")"
IMAGE_PATH="$KERNEL_DIR/$IMAGE_NAME"

echo "Creating fresh FAT32 test image: $IMAGE_PATH"

# Clean up any existing image
rm -f "$IMAGE_PATH"

# Create blank image file
dd if=/dev/zero of="$IMAGE_PATH" bs=1M count=64 2>/dev/null
if [ $? -ne 0 ]; then
    echo "Error: Failed to create image file"
    exit 1
fi

# Create FAT32 filesystem
mkfs.fat -F 32 -n "SCARLET" "$IMAGE_PATH" >/dev/null
if [ $? -ne 0 ]; then
    echo "Error: Failed to create FAT32 filesystem"
    rm -f "$IMAGE_PATH"
    exit 1
fi

# Instead of mounting, use mtools to create files directly
echo "Populating test files using mtools..."

# Create various test files
echo "Hello, Scarlet!" > /tmp/hello.txt
mcopy -i "$IMAGE_PATH" /tmp/hello.txt ::hello.txt

echo "This is a test file for FAT32 filesystem implementation." > /tmp/readme.txt
mcopy -i "$IMAGE_PATH" /tmp/readme.txt ::readme.txt

# Create test directory structure
mmd -i "$IMAGE_PATH" ::test_files
mmd -i "$IMAGE_PATH" ::empty_dir
mmd -i "$IMAGE_PATH" ::documents
mmd -i "$IMAGE_PATH" ::bin

# Create a slightly larger test file
seq 1 100 > /tmp/numbers.txt
mcopy -i "$IMAGE_PATH" /tmp/numbers.txt ::test_files/numbers.txt

# Create files with different sizes to test cluster allocation
echo "Small file" > /tmp/small.txt
mcopy -i "$IMAGE_PATH" /tmp/small.txt ::test_files/small.txt

head -c 1024 /dev/urandom | base64 > /tmp/1kb.txt
mcopy -i "$IMAGE_PATH" /tmp/1kb.txt ::test_files/1kb.txt

head -c 4096 /dev/urandom | base64 > /tmp/4kb.txt
mcopy -i "$IMAGE_PATH" /tmp/4kb.txt ::test_files/4kb.txt

# Create files in subdirectories
echo "Document content" > /tmp/doc1.txt
mcopy -i "$IMAGE_PATH" /tmp/doc1.txt ::documents/doc1.txt

echo "Binary data test" > /tmp/test_binary
mcopy -i "$IMAGE_PATH" /tmp/test_binary ::bin/test_binary

# Create a file with Japanese text for UTF-8 testing
echo "こんにちは、世界！" > /tmp/japanese.txt
mcopy -i "$IMAGE_PATH" /tmp/japanese.txt ::japanese.txt

# Create a simple config file
cat << EOF > /tmp/config.ini
[settings]
debug=true
log_level=info
max_files=1000

[network]
enabled=true
port=8080
EOF
mcopy -i "$IMAGE_PATH" /tmp/config.ini ::config.ini

# Clean up temporary files
rm -f /tmp/hello.txt /tmp/readme.txt /tmp/numbers.txt /tmp/small.txt /tmp/1kb.txt /tmp/4kb.txt
rm -f /tmp/doc1.txt /tmp/test_binary /tmp/japanese.txt /tmp/config.ini
rmdir "$MOUNT_POINT"

echo "Fresh FAT32 test image ready: $IMAGE_PATH ($(ls -lh "$IMAGE_PATH" | awk '{print $5}'))"

# Verify the image
echo "Verifying filesystem..."
fsck.fat -v "$IMAGE_PATH" 2>/dev/null
if [ $? -eq 0 ]; then
    echo "Filesystem verification: OK"
else
    echo "Warning: Filesystem verification failed"
fi

echo ""
echo "Ready for testing!"
