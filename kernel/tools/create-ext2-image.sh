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

# Use mtools to populate the filesystem without mounting (no sudo needed)
echo "Populating test files..."

# Create temp directory for our test files
TEMP_DIR="/tmp/ext2_staging_$$"
mkdir -p "$TEMP_DIR"
mkdir -p "$TEMP_DIR/test_files"
mkdir -p "$TEMP_DIR/empty_dir"
mkdir -p "$TEMP_DIR/documents"
mkdir -p "$TEMP_DIR/bin"

# Create various test files in staging area
echo "Hello, Scarlet!" > "$TEMP_DIR/hello.txt"
echo "This is a test file for ext2 filesystem implementation." > "$TEMP_DIR/readme.txt"

# Create a slightly larger test file
seq 1 100 > "$TEMP_DIR/test_files/numbers.txt"

# Create files with different sizes to test block allocation
echo "Small file" > "$TEMP_DIR/test_files/small.txt"

# Create 1KB file
head -c 1024 /dev/urandom | base64 > "$TEMP_DIR/test_files/1kb.txt"

# Create 4KB file (one block)
head -c 4096 /dev/urandom | base64 > "$TEMP_DIR/test_files/4kb.txt"

# Create files in subdirectories
echo "Document content" > "$TEMP_DIR/documents/doc1.txt"
echo "Binary data test" > "$TEMP_DIR/bin/test_binary"

# Create a file with Japanese text for UTF-8 testing
echo "こんにちは、世界！" > "$TEMP_DIR/japanese.txt"

# Create padding files
for i in {0..3}; do
    echo "File $i" > "$TEMP_DIR/file$i.txt"
done

# Create a simple config file
cat > "$TEMP_DIR/config.ini" << EOF
[settings]
debug=true
log_level=info
max_files=1000

[network]
enabled=true
port=8080
EOF

# Use debugfs to populate the ext2 filesystem without mounting
# Create a script for debugfs commands
DEBUGFS_SCRIPT="/tmp/debugfs_script_$$"
cat > "$DEBUGFS_SCRIPT" << EOF
cd /
mkdir test_files
mkdir empty_dir
mkdir documents
mkdir bin
write $TEMP_DIR/hello.txt hello.txt
write $TEMP_DIR/readme.txt readme.txt
write $TEMP_DIR/japanese.txt japanese.txt
write $TEMP_DIR/config.ini config.ini
cd test_files
write $TEMP_DIR/test_files/numbers.txt numbers.txt
write $TEMP_DIR/test_files/small.txt small.txt
write $TEMP_DIR/test_files/1kb.txt 1kb.txt
write $TEMP_DIR/test_files/4kb.txt 4kb.txt
cd /documents
write $TEMP_DIR/documents/doc1.txt doc1.txt
cd /bin
write $TEMP_DIR/bin/test_binary test_binary
cd /
EOF

# Add file creation commands
for i in {0..3}; do
    echo "write $TEMP_DIR/file$i.txt file$i.txt" >> "$DEBUGFS_SCRIPT"
done

# Populate the filesystem using debugfs
debugfs -w -f "$DEBUGFS_SCRIPT" "$IMAGE_PATH" >/dev/null 2>&1
if [ $? -ne 0 ]; then
    echo "Warning: debugfs population failed, trying alternative method..."
    # Fallback: create a simpler filesystem
    echo "Hello, Scarlet!" | debugfs -w -R "write /dev/stdin hello.txt" "$IMAGE_PATH" >/dev/null 2>&1
fi

# Clean up temporary files
rm -rf "$TEMP_DIR"
rm -f "$DEBUGFS_SCRIPT"
# Clean up mount point directory
rmdir "$MOUNT_POINT" 2>/dev/null || true

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