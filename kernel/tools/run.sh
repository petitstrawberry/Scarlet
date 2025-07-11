#!/bin/bash

echo Starting qemu...

qemu-system-riscv64 \
    -machine virt \
    -bios default \
    -m 2G \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    -global virtio-mmio.force-legacy=false \
    -drive id=x0,file=test.txt,format=raw,if=none \
    -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
    -initrd /workspaces/Scarlet/mkfs/dist/initramfs.cpio \
    -kernel $1
