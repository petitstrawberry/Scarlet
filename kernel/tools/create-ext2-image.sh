#!/bin/bash

# Create ext2 disk image for testing
# This script creates an ext2 filesystem image that can be used for testing
# with the virtio-blk device driver in QEMU

# Configuration
IMAGE_SIZE="64M"  # 64MB image
IMAGE_NAME="ext2-test.img"
MOUNT_POINT="/tmp/ext2_mount_$$"

# Get script directory and kernel directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KERNEL_DIR="$(dirname "$SCRIPT_DIR")"
IMAGE_PATH="$KERNEL_DIR/$IMAGE_NAME"

echo "Creating fresh ext2 test image: $IMAGE_PATH"

# Clean up any existing image and mount point
rm -f "$IMAGE_PATH"
mkdir -p "$MOUNT_POINT"

# Create blank image file
dd if=/dev/zero of="$IMAGE_PATH" bs=1M count=64 2>/dev/null
if [ $? -ne 0 ]; then
    echo "Error: Failed to create image file"
    exit 1
fi

# Create ext2 filesystem
mkfs.ext2 -F -L "SCARLET" "$IMAGE_PATH" >/dev/null 2>&1
if [ $? -ne 0 ]; then
    echo "Error: Failed to create ext2 filesystem"
    rm -f "$IMAGE_PATH"
    exit 1
fi

# Mount the filesystem to populate it
sudo mount -o loop "$IMAGE_PATH" "$MOUNT_POINT"
if [ $? -ne 0 ]; then
    echo "Error: Failed to mount filesystem"
    rm -f "$IMAGE_PATH"
    rmdir "$MOUNT_POINT"
    exit 1
fi

echo "Populating test files..."

# Create various test files
echo "Hello, Scarlet!" | sudo tee "$MOUNT_POINT/hello.txt" >/dev/null
echo "This is a test file for ext2 filesystem implementation." | sudo tee "$MOUNT_POINT/readme.txt" >/dev/null

# Create test directory structure
sudo mkdir -p "$MOUNT_POINT/test_files"
sudo mkdir -p "$MOUNT_POINT/empty_dir"
sudo mkdir -p "$MOUNT_POINT/documents"
sudo mkdir -p "$MOUNT_POINT/bin"

# Create a slightly larger test file
seq 1 100 | sudo tee "$MOUNT_POINT/test_files/numbers.txt" >/dev/null

# Create files with different sizes to test block allocation
echo "Small file" | sudo tee "$MOUNT_POINT/test_files/small.txt" >/dev/null

# Create 1KB file
head -c 1024 /dev/urandom | base64 | sudo tee "$MOUNT_POINT/test_files/1kb.txt" >/dev/null

# Create 4KB file (one block)
head -c 4096 /dev/urandom | base64 | sudo tee "$MOUNT_POINT/test_files/4kb.txt" >/dev/null

# Create files in subdirectories
echo "Document content" | sudo tee "$MOUNT_POINT/documents/doc1.txt" >/dev/null
echo "Binary data test" | sudo tee "$MOUNT_POINT/bin/test_binary" >/dev/null

# Create a file with Japanese text for UTF-8 testing
echo "こんにちは、世界！" | sudo tee "$MOUNT_POINT/japanese.txt" >/dev/null

# Create padding files
for i in {0..3}; do
    echo "File $i" | sudo tee "$MOUNT_POINT/file$i.txt" >/dev/null
done

# Create a simple config file
sudo tee "$MOUNT_POINT/config.ini" >/dev/null << EOF
[settings]
debug=true
log_level=info
max_files=1000

[network]
enabled=true
port=8080
EOF

# Sync and unmount
sudo sync
sudo umount "$MOUNT_POINT"
rmdir "$MOUNT_POINT"

echo "Fresh ext2 test image ready: $IMAGE_PATH ($(ls -lh "$IMAGE_PATH" | awk '{print $5}'))"

# Verify the image
echo "Verifying filesystem..."
fsck.ext2 -f -n "$IMAGE_PATH" 2>/dev/null
if [ $? -eq 0 ]; then
    echo "Filesystem verification: OK"
else
    echo "Warning: Filesystem verification failed"
fi

echo ""
echo "Ready for testing!"