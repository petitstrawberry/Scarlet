#!/bin/sh

# cd to the script directory
cd "$(dirname "$0")" || exit 1

rm -f dist/initramfs-*.cpio
rm -rf initramfs/bin