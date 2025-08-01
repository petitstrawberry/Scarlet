#!/bin/sh

# cd to the script directory
cd "$(dirname "$0")" || exit 1

mkdir -p initramfs/system/scarlet/bin
cp ../user/bin/dist/* initramfs/system/scarlet/bin/

mkdir -p dist
cd initramfs || exit 1
find . | cpio -o -H newc > ../dist/initramfs.cpio