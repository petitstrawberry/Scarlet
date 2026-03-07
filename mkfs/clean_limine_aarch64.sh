#!/bin/sh

cd "$(dirname "$0")" || exit 1

rm -f dist/limine-aarch64-boot.img \
    dist/scarlet-kernel-aarch64.bin \
    dist/virt-aarch64-limine.dtb \
    dist/limine-aarch64.conf
rm -rf dist/limine-build-aarch64-* dist/limine-10.*
