# Apple AVD Cortex-M3 Firmware

This crate is a Scarlet-owned Rust `no_std` firmware for the Apple AVD
Cortex-M3. It applies AVD revision/tier tunables, enables decode IRQ delivery,
forwards DONE/ERROR interrupts to the kernel, and reports panic/debug state.
H.264 parsing and instruction stream generation stay in the kernel driver.

Build a variant with:

```sh
cargo build --release --no-default-features --features v3-t0
```

Convert the ELF to a raw firmware image with:

```sh
llvm-objcopy -O binary \
  target/thumbv7m-none-eabi/release/apple-avd-fw \
  avd-fw-v3-t0.bin
```

Supported feature names:

```text
v2-t0
v3-t0
v3-t1
v4-t0
v5-t0
v5-t1
```
