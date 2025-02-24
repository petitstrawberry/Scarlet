#!/bin/bash

echo Starting qemu...

qemu-system-riscv64 \
    -machine virt \
    -bios default \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    -kernel $1
