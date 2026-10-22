# Native atomic rename smoke test

This executable exercises operation 419 and imports the production Files
filesystem adapter. It requires a booted Scarlet system with the standard init
bootstrap, a writable `/tmp`, and the ordinary `/dev` mount.

Build with the Scarlet toolchain from the repository root:

```sh
cargo build --locked --manifest-path tests/native-rename-smoke/Cargo.toml \
  --target aarch64-unknown-scarlet
cargo check --locked --manifest-path tests/native-rename-smoke/Cargo.toml \
  --target riscv64gc-unknown-scarlet
```

Place `rename419-smoke` in `/bin` of a disposable boot image and select it with
`init.exec=/bin/rename419-smoke init.console=/dev/tty0`. Success prints
`SCARLET_RENAME419_SMOKE_OK`; errors or assertions print
`SCARLET_RENAME419_SMOKE_FAIL`. The test creates an exclusive, process-specific
directory under `/tmp` and removes its known fixture entries after success.
The cross-mount test attempts a unique `/dev` destination that must remain absent.

Coverage includes capability probing, invalid pointers/flags, empty/overlong
paths, existing destinations, dangling symlinks, inode and hard-link identity,
writes through an already-open file after rename, cross-mount refusal, and
directory cycle/dot/root rejection. The host adapter separately checks that an
older kernel's unknown-operation result is treated as unsupported, without a
fallback to operation 409 or copy/delete.

The AArch64 test passed under QEMU with the updated kernel on 2026-10-22. This
validates the filesystem primitive and adapter; it is not a Files GUI test or a
hardware performance measurement. Kernel semantic tests are also defined in
`kernel/src/fs/vfs_v2/tests/rename_status_tests.rs`.
