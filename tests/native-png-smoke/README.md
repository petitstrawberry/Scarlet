# Native Screenshot PNG worker smoke test

This fixture imports the production Screenshot pixel/export helper and named
2 MiB worker builder. It creates an exclusive temporary directory, exports a
cropped PNG, decodes and compares every pixel, checks existing-file refusal,
replaces an annotated PNG, and verifies an invalid destination fails. It removes
its known output and directory after success.

Build with the Scarlet toolchain:

```sh
cargo build --locked --manifest-path tests/native-png-smoke/Cargo.toml \
  --target aarch64-unknown-scarlet
```

Run the executable as `/bin/screenshot-png-smoke` in a disposable boot image
with standard init bootstrap and writable `/tmp`. Success prints
`SCARLET_PNG_WORKER_SMOKE_OK`; errors print `SCARLET_PNG_WORKER_SMOKE_FAIL`.
This validates the codec/export helper on the explicit worker stack, not native
screen capture or Vellum's separate PDF rendering queue.

The AArch64 fixture passed under QEMU on 2026-10-22 with
`SCARLET_PNG_WORKER_SMOKE_OK` (8.896 seconds). This establishes successful PNG
work on the explicit stack; it does not measure the minimum stack requirement.
