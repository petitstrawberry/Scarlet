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

## Lower-layer extraction collisions

`archive-collision-smoke` imports the production archive extractor and runs it
on the same bounded worker stack. Build it with the command above (or select
`--bin archive-collision-smoke`). Stage the bootstrap init at `/init`, this
program at `/bin/archive-collision-smoke`, and the native interpreter at
`/bin/scarlet-ld`. Both executables request that interpreter; without it the
kernel cannot start init, and a serial-only run can appear to hang. Build it
with:

```sh
cargo build --release --manifest-path user/scarlet-ld/Cargo.toml \
  --target aarch64-unknown-scarlet
```

Before booting a disposable guest, package
`/root/archive-collision-smoke/` into the **lower initramfs**:

| Entry | Contents |
| --- | --- |
| `fixture-marker` | `isolated-archive-collision-v1` followed by LF |
| `fixture.zip` | The adjacent checked-in `fixture.zip` |
| `lower-directory/keep.txt` | `keep lower directory` followed by LF |
| `lower-file` | `keep lower file` followed by LF |
| `lower-link` | A symbolic link to `missing-target` |

Do not create these entries at runtime: that would only test upper-layer
collisions and miss the regression. The program checks the marker before
mutating the fixture. It requires rejection of all three lower destinations,
checks their contents remain intact, then extracts to a new directory and
requires rejection of a second extraction there. It retains its output only in
the disposable guest; no recursive cleanup of existing directories is attempted.

Use `tests/native-desktop-smoke/run.py` with
`--program /bin/archive-collision-smoke
--expect-marker SCARLET_ARCHIVE_COLLISION_SMOKE_OK`. This verifies real native
syscalls and extraction, separately from the Files GUI. Keep its QEMU output
directory short (for example under `/tmp`); macOS UNIX socket paths must be
shorter than 104 bytes. Use the firmware paths from the development shell.

Verified on AArch64 Scarlet/QEMU on 2026-10-22: the unchanged kernel accepted
`lower-directory` and failed the fixture; the exclusive-create fix rejected all
existing destinations, preserved their contents, and passed in 6.615 seconds.
The kernel suite also passed all 1546 tests, including lower-layer collisions and
recreation after whiteout. Local evidence is retained under
`artifacts/desktop-quality-20261022/archive-native-{before,fixed}/`.
This verifies the production extraction worker and native filesystem calls;
Files window interaction and persistence after reboot remain separate checks.
