#!/bin/bash

# cd to the directory of the script
cd "$(dirname "$0")"

cd kernel

cargo test --target riscv64gc-unknown-none-elf | tee /tmp/test_output.log

# Check if the test failed
if (grep -q "running 0 tests" /tmp/test_output.log); then
    echo "No tests were run"
    exit 0
elif (grep -q "Test failed" /tmp/test_output.log); then
    exit 1
else
    exit 0
fi
