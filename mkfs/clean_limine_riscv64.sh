#!/bin/sh

cd "$(dirname "$0")" || exit 1

rm -f dist/limine-riscv64-boot.img dist/limine-riscv64.conf
rm -rf dist/limine-build-riscv64-* 
