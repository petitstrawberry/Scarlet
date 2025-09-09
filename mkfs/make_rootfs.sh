#!/bin/sh

# cd to the script directory
cd "$(dirname "$0")" || exit 1

# Create ext2 image from rootfs directory
ROOTFS_DIR="rootfs"
EXT2_IMAGE="dist/rootfs.img"

# Block size (can be overridden by environment variable)
BLOCK_SIZE=${EXT2_BLOCK_SIZE:-4096}

if [ ! -d "$ROOTFS_DIR" ]; then
    echo "Error: $ROOTFS_DIR directory not found"
    echo "Please create the rootfs directory and populate it with your files"
    exit 1
fi

# Calculate size needed (in KB)
ROOTFS_SIZE_KB=$(du -sk "$ROOTFS_DIR" | cut -f1)
# Add 50% extra space, minimum 100MB
EXT2_SIZE_KB=$((ROOTFS_SIZE_KB * 3 / 2))
if [ $EXT2_SIZE_KB -lt 102400 ]; then
    EXT2_SIZE_KB=102400
fi

echo "Creating ext2 image: $EXT2_IMAGE (${EXT2_SIZE_KB}KB)"

# Create ext2 filesystem with variable block size (default 1024)
echo "Using block size: ${BLOCK_SIZE} bytes"
dd if=/dev/zero of="$EXT2_IMAGE" bs=$BLOCK_SIZE count=$((EXT2_SIZE_KB * 1024 / BLOCK_SIZE))
mke2fs -F -t ext2 -b $BLOCK_SIZE -L "SCARLET_ROOT" "$EXT2_IMAGE"

# Mount and copy files using debugfs (works without loop devices)
echo "Copying files to ext2 image using debugfs..."

# Create a script for debugfs commands
DEBUGFS_SCRIPT=$(mktemp)

# Function to add files recursively
add_files_to_debugfs() {
    local src_dir="$1"
    local dest_dir="$2"
    
    # Create directory in ext2 if it doesn't exist (except root)
    if [ "$dest_dir" != "/" ]; then
        echo "mkdir $dest_dir" >> "$DEBUGFS_SCRIPT"
    fi
    
    # Process each item in source directory (including hidden files)
    for item in "$src_dir"/* "$src_dir"/.[!.]* "$src_dir"/..?*; do
        if [ ! -e "$item" ]; then
            continue  # Skip if no files match pattern
        fi
        
        item_name=$(basename "$item")
        dest_path="$dest_dir/$item_name"
        
        if [ -d "$item" ]; then
            # Recursively add subdirectory
            add_files_to_debugfs "$item" "$dest_path"
        elif [ -f "$item" ]; then
            # Add file
            echo "write $item $dest_path" >> "$DEBUGFS_SCRIPT"
        fi
    done
}

# Add all files from rootfs directory
add_files_to_debugfs "$ROOTFS_DIR" ""

# Execute debugfs commands
debugfs -w -f "$DEBUGFS_SCRIPT" "$EXT2_IMAGE"

# Cleanup
rm "$DEBUGFS_SCRIPT"

echo "ext2 rootfs created successfully: $EXT2_IMAGE"
