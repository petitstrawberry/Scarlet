#!/bin/bash
# Convert ELF kernel to Linux arm64 Image format for U-Boot booti command
# This adds the required header so U-Boot can properly boot the kernel
# and pass DTB address in x0 according to Linux boot protocol

set -e

if [ $# -lt 2 ]; then
    echo "Usage: $0 <input-elf> <output-image>"
    exit 1
fi

INPUT_ELF="$1"
OUTPUT_IMAGE="$2"

# Get entry point from ELF
ENTRY=$(aarch64-linux-gnu-readelf -h "$INPUT_ELF" | grep "Entry point" | awk '{print $4}')
echo "Entry point: $ENTRY"

# Convert to raw binary
aarch64-linux-gnu-objcopy -O binary "$INPUT_ELF" "${OUTPUT_IMAGE}.tmp"

# Get size of binary
SIZE=$(stat -c %s "${OUTPUT_IMAGE}.tmp")
echo "Binary size: $SIZE bytes"

# Create Linux arm64 Image header (64 bytes)
# Reference: Documentation/arm64/booting.rst in Linux kernel
#
# Header format:
# u32 code0;                    /* Executable code (branch instruction) */
# u32 code1;                    /* Executable code */
# u64 text_offset;              /* Image load offset, LE */
# u64 image_size;               /* Effective Image size, LE */
# u64 flags;                    /* Kernel flags, LE */
# u64 res2;                     /* Reserved */
# u64 res3;                     /* Reserved */
# u64 res4;                     /* Reserved */
# u32 magic;                    /* Magic number "ARM\x64" = 0x644d5241 */
# u32 res5;                     /* Reserved (used for PE COFF offset) */

# code0: Branch to offset 0x40 (skip header) - encoded as: b . + 0x40
# ARM64 branch encoding: 0x14000000 | (offset >> 2)
# offset = 0x40 = 64, so immediate = 64/4 = 16 = 0x10
CODE0=$(printf '%08x' $((0x14000010)))

# Create header using python for precise binary control
python3 << PYEOF
import struct
import sys

# code0: branch to skip header (b . + 64)
code0 = 0x14000010
# code1: NOP
code1 = 0xd503201f
# text_offset: kernel load offset from RAM base (0x200000 = 2MB, matches linker script)
# This tells U-Boot to keep kernel at RAM_BASE + 0x200000 = 0x40200000
text_offset = 0x200000
# image_size: size of kernel image (round up to page)
image_size = (($SIZE + 0xFFF) & ~0xFFF) + 64
# flags: little endian, 4K pages, kernel anywhere
flags = 0x0
# magic: ARM\x64 in little endian
magic = 0x644d5241

with open("${OUTPUT_IMAGE}", "wb") as f:
    # Write header (64 bytes)
    f.write(struct.pack('<I', code0))      # code0 (4 bytes)
    f.write(struct.pack('<I', code1))      # code1 (4 bytes)
    f.write(struct.pack('<Q', text_offset)) # text_offset (8 bytes)
    f.write(struct.pack('<Q', image_size))  # image_size (8 bytes)
    f.write(struct.pack('<Q', flags))       # flags (8 bytes)
    f.write(struct.pack('<Q', 0))           # res2 (8 bytes)
    f.write(struct.pack('<Q', 0))           # res3 (8 bytes)
    f.write(struct.pack('<Q', 0))           # res4 (8 bytes)
    f.write(struct.pack('<I', magic))       # magic (4 bytes)
    f.write(struct.pack('<I', 0))           # res5 (4 bytes)
    
    # Append kernel binary
    with open("${OUTPUT_IMAGE}.tmp", "rb") as kernel:
        f.write(kernel.read())

print(f"Created Linux Image: ${OUTPUT_IMAGE} ({image_size} bytes)")
PYEOF

rm -f "${OUTPUT_IMAGE}.tmp"
echo "Done: $OUTPUT_IMAGE"
