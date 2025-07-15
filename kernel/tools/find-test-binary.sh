#!/bin/bash

# Script to find the most recent test binary for VSCode debugging

KERNEL_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$KERNEL_DIR"

# Find the most recent test binary (newer than 1 minute to ensure it's from current test run)
TEST_BINARY=$(find target/riscv64gc-unknown-none-elf/debug/deps -name "kernel-*" -type f -executable -newermt "1 minute ago" 2>/dev/null | head -1)

if [ -n "$TEST_BINARY" ]; then
    echo "$KERNEL_DIR/$TEST_BINARY"
else
    # Fallback: find any test binary
    TEST_BINARY=$(find target/riscv64gc-unknown-none-elf/debug/deps -name "kernel-*" -type f -executable 2>/dev/null | head -1)
    if [ -n "$TEST_BINARY" ]; then
        echo "$KERNEL_DIR/$TEST_BINARY"
    else
        echo "Test binary not found" >&2
        exit 1
    fi
fi
