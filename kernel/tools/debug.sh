#!/bin/bash

echo Starting qemu...

qemu-system-riscv64 \
    -machine virt \
    -bios default \
    -nographic \
    -serial mon:stdio \
    --no-reboot \
    -gdb tcp::12345 -S \
    -kernel /workspaces/Scarlet/kernel/target/riscv64gc-unknown-none-elf/debug/kernel
