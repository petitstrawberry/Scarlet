# Native Files worker smoke

This fixture imports the production Files worker, ZIP reader and folder-size
code. It catches a native-only regression: Scarlet's default 64 KiB thread stack
can overflow while constructing a deflate reader. The production worker reserves
a bounded 2 MiB stack and reports thread creation errors instead of panicking.

```sh
cargo build --manifest-path tests/native-files-smoke/Cargo.toml \
  --target aarch64-unknown-scarlet
```

Run `files-worker-smoke` only in a disposable Scarlet boot with writable `/tmp`.
The executable exclusively creates a process-specific fixture directory, browses
the embedded deterministic deflated ZIP in a production worker, calculates a
folder's logical size in another worker, then removes its known fixture entries.
Success prints `SCARLET_FILES_WORKER_SMOKE_OK`. A separate failure marker identifies
errors/assertions. Apply a bounded boot timeout so a stack regression fails
instead of hanging a test run.

This covers native archive browsing and worker completion. It does not cover the
Files window, extraction durability, picker IPC or all filesystem operations.

The fixture passed on AArch64 Scarlet/QEMU on 2026-10-22: success marker, exit 0,
18.038 seconds. Runtime evidence is retained at
`/workspace/scratch/native-boot/files-worker-qemu/result.json` and `serial.log`.
