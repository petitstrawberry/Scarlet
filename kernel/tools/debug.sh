#!/bin/bash

echo Starting qemu...

qemu-system-riscv64 \
    -machine virt \
    -m 2G \
    -bios default \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    -global virtio-mmio.force-legacy=false \
    -drive id=x0,file=test.txt,format=raw,if=none \
    -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
    -gdb tcp::12345 -S \
    -initrd /workspaces/Scarlet/mkfs/dist/initramfs.cpio \
    -kernel /workspaces/Scarlet/kernel/target/riscv64gc-unknown-none-elf/debug/kernel
