#!/bin/bash

# Helper script to connect GDB to QEMU debug session

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KERNEL_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "Connecting GDB to QEMU debug session..."
echo "Make sure QEMU is running in debug mode first!"
echo ""
echo "Available commands:"
echo "  (gdb) b main                 # Set breakpoint at main"
echo "  (gdb) c                      # Continue execution"
echo "  (gdb) info registers         # Show register values"
echo "  (gdb) bt                     # Show backtrace"
echo ""

# Check if test mode
if [ "$1" = "--test" ]; then
    echo "Test mode: Looking for test binary..."
    cd "$KERNEL_DIR"
    
    # Find the most recent test binary
    TEST_BINARY=$(find target/riscv64gc-unknown-none-elf/debug/deps -name "*Scarlet*" -type f -executable -newer target/riscv64gc-unknown-none-elf/debug/kernel 2>/dev/null | head -1)
    
    if [ -n "$TEST_BINARY" ]; then
        echo "Using test binary: $TEST_BINARY"
        gdb-multiarch "$TEST_BINARY" \
            -ex "target remote :12345" \
            -ex "set confirm off"
    else
        echo "Test binary not found. Please run 'cargo make debug-test' first."
        exit 1
    fi
else
    echo "Regular kernel mode"
    KERNEL_BINARY="$KERNEL_DIR/target/riscv64gc-unknown-none-elf/debug/kernel"
    
    if [ -f "$KERNEL_BINARY" ]; then
        gdb-multiarch "$KERNEL_BINARY" \
            -ex "target remote :12345" \
            -ex "set confirm off"
    else
        echo "Kernel binary not found. Please run 'cargo make debug' first."
        exit 1
    fi
fi
