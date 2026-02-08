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

# Calculate source directory size (for logging/reference)
ROOTFS_SIZE_KB=$(du -sk "$ROOTFS_DIR" | cut -f1)

# Use a fixed 8 GiB ext2 image (can be overridden via EXT2_SIZE_KB_OVERRIDE)
if [ -n "$EXT2_SIZE_KB_OVERRIDE" ]; then
    EXT2_SIZE_KB="$EXT2_SIZE_KB_OVERRIDE"
else
    EXT2_SIZE_KB=$((8 * 1024 * 1024))
fi

echo "Creating ext2 image: $EXT2_IMAGE (${EXT2_SIZE_KB}KB)"
echo "Source rootfs directory size: ${ROOTFS_SIZE_KB}KB"

# Create directory for output if it doesn't exist
mkdir -p "$(dirname "$EXT2_IMAGE")"

# Create ext2 filesystem with optimized parameters
echo "Using block size: ${BLOCK_SIZE} bytes"
dd if=/dev/zero of="$EXT2_IMAGE" bs=$BLOCK_SIZE count=$((EXT2_SIZE_KB * 1024 / BLOCK_SIZE))
# Use smaller inode ratio (2048) to support more files and directories
# Add extra inodes and reserved blocks
mke2fs -F -t ext2 -b $BLOCK_SIZE -i 2048 -m 1 -L "SCARLET_ROOT" "$EXT2_IMAGE"

# Mount and copy files using debugfs (works without loop devices)
echo "Copying files to ext2 image using debugfs..."

# Create a script for debugfs commands
DEBUGFS_SCRIPT=$(mktemp)
# Track created directories to avoid duplicates
CREATED_DIRS_FILE=$(mktemp)

# Function to ensure directory exists in ext2
ensure_directory() {
    local dir_path="$1"
    if [ "$dir_path" != "/" ] && [ "$dir_path" != "" ]; then
        # Check if directory was already created
        if grep -Fxq "$dir_path" "$CREATED_DIRS_FILE" 2>/dev/null; then
            return 0
        fi
        
        # Get parent directory
        local parent_dir=$(dirname "$dir_path")
        # Recursively ensure parent exists
        if [ "$parent_dir" != "/" ] && [ "$parent_dir" != "." ]; then
            ensure_directory "$parent_dir"
        fi
        # Create this directory and mark as created
        echo "mkdir $dir_path" >> "$DEBUGFS_SCRIPT"
        echo "$dir_path" >> "$CREATED_DIRS_FILE"
    fi
}

# Function to add files recursively
add_files_to_debugfs() {
    local src_dir="$1"
    local dest_dir="$2"
    
    # Create directory in ext2 if it doesn't exist (except root)
    if [ "$dest_dir" != "/" ] && [ "$dest_dir" != "" ]; then
        ensure_directory "$dest_dir"
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
            # Ensure parent directory exists before adding file
            local file_parent=$(dirname "$dest_path")
            if [ "$file_parent" != "/" ] && [ "$file_parent" != "." ]; then
                ensure_directory "$file_parent"
            fi
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
rm "$CREATED_DIRS_FILE"

echo "ext2 rootfs created successfully: $EXT2_IMAGE"
