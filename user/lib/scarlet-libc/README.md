# Scarlet Native libc bring-up

This is the first, static C ABI layer for native AArch64 and RV64 programs. Its
implemented surface is `malloc`, `calloc`, `realloc`, `free`, thread-local
`errno`, `realpath`, `futimens`, `utimensat`, `fsync`, and `fdatasync`. C headers
live in `include/`. Rust std supplies the allocator and TLS runtime; C/POSIX
operations translate to Scarlet Native syscalls.

This library does **not** yet supply a complete libc. In particular, these
partial headers and functions cannot build curl, libgit2 or SQLite yet. stdio,
the remaining descriptor APIs, locking, pthreads, networking and a defined
ownership/permission model still need implementation or an existing-libc port.
Cargo is not included by this change.

## ABI and behavior

- Link one `libscarlet_c.a` into a 64-bit native executable, using the matching
  Scarlet Rust toolchain's CRT. `malloc` and `free` must come from this same
  library. No shared system Rust std is required.
- C `errno` belongs to the calling thread and is changed on failure. The native
  kernel returns negative error numbers; wrappers return `-1` or `NULL`.
- `realpath` resolves existing paths through VFS, including links before `..`,
  directory checks for `.` and trailing `/`, and paths across nested mounts.
  A null output pointer requests a result allocated with `malloc`.
- Pathnames follow Scarlet's existing UTF-8 VFS contract. `PATH_MAX` is 1024,
  including the terminator. This is not a Linux binary ABI.
- Times are stored at the current native metadata precision of one second;
  subsecond input is truncated. `UTIME_NOW` and `UTIME_OMIT` are supported.
  ext2 rejects seconds beyond `UINT32_MAX` before changing either timestamp.
  Dates before the Unix epoch return `EOVERFLOW`.
- `utimensat` resolves relative paths from the supplied directory handle, or
  cwd for `AT_FDCWD`. Absolute paths ignore that handle. `AT_SYMLINK_NOFOLLOW`
  changes the final link's timestamps. Unknown flags are rejected.
- New syscalls preserve the legacy filesystem calls' `-1` contract. See
  [Native filesystem extensions](../../../docs/abi/native-filesystem.md).

## Build and verification

Build with a Scarlet compiler/sysroot containing the matching native-host
filesystem overlay from scarlet-rust-nix:

```sh
cargo build --manifest-path user/lib/scarlet-libc/Cargo.toml \
  --release --target aarch64-unknown-scarlet
```

`tools/native-rustc` has a `native-fs` Cargo feature. Its build script compiles
`tests/native.c` with Clang against these headers and links it into the guest
probe. `SCARLET_PROBE_CC` and `SCARLET_PROBE_AR` can select Clang and llvm-ar.
The `--native-fs` guest probe checks C calls and Rust std on both ext2 and a
private tmpfs mount, and persists `NATIVE_FS_PASS` only after all checks pass.
See [native rustc verification](../../../docs/development/native-rustc.md).

Host tests exercise allocation alignment, zeroing, realloc failure preserving
the old allocation, timestamp validation and thread-local errno. Actual
syscalls must be checked in a Scarlet guest.
