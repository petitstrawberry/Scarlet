#!/usr/bin/env bash
set -euo pipefail

variants=(v2-t0 v3-t0 v3-t1 v4-t0 v5-t0 v5-t1)
objcopy_bin="${LLVM_OBJCOPY:-llvm-objcopy}"

for variant in "${variants[@]}"; do
  cargo build -Zbuild-std=core --release --no-default-features --features "$variant"
  "$objcopy_bin" -O binary \
    target/thumbv7m-none-eabi/release/apple-avd-fw \
    "avd-fw-${variant}.bin"
done
