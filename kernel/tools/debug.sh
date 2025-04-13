#!/bin/bash

echo Starting qemu...

qemu-system-riscv64 \
    -machine virt \
    -m 512M \
    -bios default \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    -gdb tcp::12345 -S \
    -initrd /workspaces/Scarlet/mkfs/dist/initramfs.cpio \
    -kernel /workspaces/Scarlet/kernel/target/riscv64gc-unknown-none-elf/debug/kernel
