# Scarlet Native libc

`scarlet-libc` implements a growing C runtime in Rust for Scarlet's native
AArch64 and RV64 targets. The current library is a small static C ABI adapter:
allocation, thread-local `errno`, `realpath`, file timestamps, and file sync.
The [support matrix and acceptance gates](STATUS.md) distinguish implemented
behavior from the work needed for a standalone runtime.

The direction is to grow this Rust implementation toward a `no_std` C runtime,
with musl-level completeness and robustness as a goal. **The current version
still depends on Rust std** for allocation and TLS. It is not a complete libc,
does not yet build Cargo's C dependencies, and does not enable `cfg(unix)` or
the Linux syscall ABI.

## Current integration contract

- Link one `libscarlet_c.a` into a native 64-bit executable using the matching
  Scarlet Rust toolchain and CRT. Allocation and deallocation must use the same
  library. A standalone C startup/link workflow remains an acceptance gate.
- The implementing crate uses `#![no_builtins]`: LLVM must preserve its actual
  C export behavior for Rust callers. An optimized `calloc(SIZE_MAX, 2)` call
  must still return null and set `ENOMEM`, as checked inside the guest.
- [Headers](include/) describe only the implemented subset. `errno` is per
  thread; wrappers translate Native negative errors into `-1` or `NULL`.
- Paths use Scarlet's UTF-8 VFS contract. `PATH_MAX` is 1024 including NUL;
  `realpath` requires an existing path. A null result buffer requests allocation.
- Timestamp precision is one second. Negative epoch seconds return `EOVERFLOW`;
  ext2 also rejects seconds above `UINT32_MAX`. `UTIME_NOW`, `UTIME_OMIT`, relative
  directory handles, and `AT_SYMLINK_NOFOLLOW` are implemented.
- The kernel and toolchain must agree on the
  [Native filesystem extensions](../../../docs/abi/native-filesystem.md).
  Older filesystem syscalls retain their legacy error contract.

## Build and checks

Run these commands from the Scarlet repository root. Host tests need a Rust
compiler supporting edition 2024; they do not execute Scarlet syscalls:

```sh
cargo test --manifest-path user/lib/scarlet-libc/Cargo.toml
python3 user/lib/scarlet-libc/tests/check_headers.py --clang clang
```

The [header runner](tests/check_headers.py) compiles every public header alone
and repeatedly, then checks [C11](tests/headers.c) and [C++11](tests/headers.cpp)
ABI declarations for both architectures and both char signedness settings.
It uses only Scarlet and Clang builtin headers, with no host libc fallback.
`SCARLET_PROBE_CC` can select Clang instead of `--clang`.

With the matching Scarlet cross compiler/sysroot selected as `rustc`, build:

```sh
cargo build --manifest-path user/lib/scarlet-libc/Cargo.toml \
  --release --target aarch64-unknown-scarlet
cargo build --manifest-path tools/native-rustc/Cargo.toml \
  --release --target aarch64-unknown-scarlet --features native-fs
```

RV64 uses `riscv64gc-unknown-scarlet`. The probe needs Clang with that target's
code generator and llvm-ar; select them with `SCARLET_PROBE_CC` and
`SCARLET_PROBE_AR`. Building the archive alone does not establish C startup or
guest compatibility.

The probe links the actual [C fixture](tests/native.c) and runs it alongside
[Rust filesystem checks](../../../tools/native-rustc/native_fs.rs). Pass the
fresh probe to `tools/native-rustc/run-qemu.py` with `--native-fs --proc-macro`.
Use `--accel hvf` for AArch64 on Apple Silicon. The
[native rustc guide](../../../docs/development/native-rustc.md#native-filesystem-and-initial-libc-2026-09-22)
describes the required kernel, staging tree, bootstrap, and linker inputs.

The recorded [2026-09-22 filesystem run](../../../tools/native-rustc/evidence/2026-09-22-native-fs-aarch64.json)
passed on AArch64/HVF with ext2 and tmpfs. RV64 has cross-build evidence, not
filesystem guest execution evidence from that run. Local results validate the
recorded inputs; they do not update or validate an installed distribution bundle.

The [follow-up quality run](../../../tools/native-rustc/evidence/2026-09-22-libc-quality-aarch64.json)
also checks allocation fragmentation, optimized overflow/errno behavior across
threads, renamed directory handles and mount ancestors, and ext2 directory
growth. It includes the release kernel's complete 1273-test HVF result.
