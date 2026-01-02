#!/bin/sh

# cd to the script directory
cd "$(dirname "$0")" || exit 1

rm dist/rootfs.img 2>/dev/null || true