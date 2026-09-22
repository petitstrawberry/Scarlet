# Scarlet Native libc

`scarlet-libc` implements a growing C runtime in Rust for Scarlet's native
AArch64 and RV64 targets. The static C ABI now includes ordinary and aligned
allocation, thread-local `errno`, byte strings, integer conversion, descriptor
I/O, unbuffered streams and integer/string formatted output, plus `realpath`,
file timestamps and sync.
The [support matrix and acceptance gates](STATUS.md) distinguish implemented
behavior from the work needed for a complete C runtime.

The direction is to grow this Rust implementation using Scarlet Rust std as
its backend, with musl-level completeness and robustness as a goal. Allocation
and TLS can continue to use std; `no_std` is not a requirement. The public
contract is the C ABI, with the matching Rust runtime managed internally by the
toolchain. The library is not complete and does not yet supply Cargo's full C
dependency closure. It does not enable `cfg(unix)` or the Linux syscall ABI.

## Current integration contract

- Link one `libscarlet_c.a` into a native 64-bit executable using the matching
  Scarlet Rust toolchain and CRT. Allocation and deallocation must use the same
  library. The bounded C startup fixture links a plain C constructor and `main`
  directly with the CRT and archive, including its Rust backend. An installed
  C SDK and general environment/exit behavior remain acceptance gates.
- The implementing crate uses `#![no_builtins]`: LLVM must preserve its actual
  C export behavior for Rust callers. An optimized `calloc(SIZE_MAX, 2)` call
  must still return null and set `ENOMEM`, as checked inside the guest.
- [Headers](include/) describe only the implemented subset. `errno` is per
  thread; wrappers translate Native negative errors into `-1` or `NULL`.
  The matching runtime initializes the [Native TLS header](../../../docs/abi/native-tls.md)
  before constructors and before a new thread enters user code. Even first
  access to `__errno_location` needs no allocation or syscall. Rust std's
  `std::io::Error::last_os_error` reads the same slot.
- The executable, CRT, libc, loader and Rust DSOs must use the same TLS layout.
  Refresh the entire staged runtime dependency set, including compiler DSOs;
  replacing only the target sysroot does not update every bundled std copy.
  Absent or incompatible TLS causes an abort rather than lazy errno allocation.
- Paths use Scarlet's UTF-8 VFS contract. `PATH_MAX` is 1024 including NUL;
  `realpath` requires an existing path. A null result buffer requests allocation.
- Timestamp precision is one second. Negative epoch seconds return `EOVERFLOW`;
  ext2 also rejects seconds above `UINT32_MAX`. `UTIME_NOW`, `UTIME_OMIT`, relative
  directory handles, and `AT_SYMLINK_NOFOLLOW` are implemented.
- The kernel and toolchain must agree on the
  [Native filesystem extensions](../../../docs/abi/native-filesystem.md) and
  [Native descriptor operations](../../../docs/abi/native-descriptors.md).
  Older filesystem syscalls retain their legacy error contract.

`malloc` and successful zero-size allocations have at least 16-byte alignment
and may be passed to `free`. `calloc` checks multiplication before zeroing;
`realloc` preserves the old block on failure. For non-null `p`, `realloc(p, 0)`
frees it and returns null. Successful allocation and `free` preserve errno.

`aligned_alloc` accepts nonzero power-of-two alignments, including values below
16, and supports sizes that are not multiples of the alignment. Results retain
at least 16-byte alignment. Invalid alignment reports `EINVAL`; arithmetic or
backend allocation failure reports `ENOMEM`. `posix_memalign` additionally
requires alignment to be a multiple of `sizeof(void *)`; it returns an error
number and preserves both errno and the output pointer on failure.
`reallocarray` checks `count * size`, reports `ENOMEM` on overflow and keeps the
old block live. All these allocation APIs use the same prefix metadata and
support ordinary `free` and `realloc`; realloc guarantees fundamental alignment,
not preservation of an earlier extended alignment.

## Strings, descriptors and streams

`string.h` provides byte comparisons, copies, bounded scans, searches,
concatenation, owned duplication and stable C-locale error descriptions.
`memcpy`, `memmove`, `memset`, `memcmp` and `strlen` come from the matching Rust
compiler-builtins runtime; the libc archive does not replace them with recursive
wrappers. `ctype.h` implements the ASCII C locale, independent of plain `char`
signedness. `strtol`, `strtoul`, `strtoll`, `strtoull`, `atoi`, `atol` and `atoll`
use C17/POSIX integer prefix grammar: base zero recognizes decimal, octal and
hexadecimal, without C23 binary prefixes. The `strto*` functions consume all
valid digits on overflow, set `ERANGE` and preserve the required end pointer.

`open`, `openat`, `creat`, `close`, `read`, `write`, `lseek` and `dup` use the
kernel's status-preserving descriptor operations. Duplicates share offset and
append state, while close-on-exec remains descriptor-local. `fcntl` supports
`F_GETFD`, `F_SETFD`, `F_GETFL` and `F_SETFL`; the last changes append while
preserving access mode. Unsupported flags and commands fail explicitly. The
kernel checks descriptor access and provides atomic append in ext2 and tmpfs.
Creation modes do not establish Unix ownership, umask or permission enforcement.

`stdio.h` supplies opaque, internally locked `FILE` streams: `fopen`/`fdopen`,
close/flush, block and character I/O, line I/O, seek/tell, EOF/error indicators,
and one-byte `ungetc`. Output is unbuffered, so there is no pending output to
flush at process exit; `fflush` does not imply `fsync`. `fdopen` takes ownership
only on success and does not truncate an existing descriptor for mode `w`.

The `printf`/`fprintf`/`sprintf`/`snprintf` families and their `v` variants support
integers, byte strings, characters, pointers, flags, widths, precisions and
integer length modifiers. Bounded output counts discarded bytes and terminates
when capacity is nonzero; an unrepresentable return count reports `EOVERFLOW`.
Floating-point, wide, positional, grouping and `%n` conversions report
`ENOTSUP`. There is no scanf family, stream buffering API, floating-point
conversion or general locale implementation yet.

## Build and checks

Run these commands from the Scarlet repository root. Host tests need a nightly
Rust compiler supporting edition 2024 and `c_variadic`; they do not execute
Scarlet syscalls:

```sh
cargo test --manifest-path user/lib/scarlet-libc/Cargo.toml
python3 user/lib/scarlet-libc/tests/check_headers.py --clang clang
```

The [header runner](tests/check_headers.py) compiles every public header alone
and repeatedly, then checks [C11](tests/headers.c) and [C++11](tests/headers.cpp)
ABI declarations for both architectures and both char signedness settings.
It uses only Scarlet and Clang builtin headers, with no host libc fallback.
`SCARLET_PROBE_CC` can select Clang instead of `--clang`. The Nix development
shell sets it to unwrapped Clang so host include and linker flags are not
injected into cross-target checks.

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

The probe links the actual [allocation/filesystem](tests/native.c),
[strings/conversion](tests/strings.c), [descriptor](tests/descriptor.c) and
[stdio](tests/stdio.c) C fixtures and runs them alongside
[Rust filesystem checks](../../../tools/native-rustc/native_fs.rs). Pass the
fresh probe to `tools/native-rustc/run-qemu.py` with `--native-fs --proc-macro`.
Use `--accel hvf` for AArch64 on Apple Silicon. The
[native rustc guide](../../../docs/development/native-rustc.md#native-filesystem-and-initial-libc-2026-09-22)
describes the required kernel, staging tree, bootstrap, and linker inputs.

The probe's `native-fs` build also includes
[deterministic allocator failure checks](../../../tools/native-rustc/allocation_failure.rs).
They deny heap allocation in a constructor, on a fresh thread and during its
TLS destructor, checking errno access, overflow handling, actual null backend
results and `last_os_error`. This tests a failing allocator result, not physical
memory exhaustion or signal delivery.

To build the separate plain C startup fixture with a freshly built archive:

```sh
python3 user/lib/scarlet-libc/tests/build_c_startup.py \
  --target aarch64-unknown-scarlet \
  --sysroot "$(rustc --print sysroot)" \
  --libc user/lib/scarlet-libc/target/aarch64-unknown-scarlet/release/libscarlet_c.a \
  --output /tmp/scarlet-c-startup-aarch64
```

Choose a new output directory and adjust the archive path if using
`CARGO_TARGET_DIR`. Use `--clang` and `--linker` to select Clang and `ld.lld`
explicitly. The builder compiles [c_main.c](tests/c_main.c), links it with
`scarlet-crt0.o` and the archive, and audits archive members, symbols, entry
point, constructor membership and static ELF properties. It records input
hashes and commands; a successful build alone is not a guest pass.

Add the following options to the matching `tools/native-rustc/run-qemu.py`
invocation described in the native rustc guide:

```sh
--storage ext2 --native-fs --proc-macro \
--c-startup-probe /tmp/scarlet-c-startup-aarch64/c-startup-probe
```

The harness executes the C program and requires exit status 43. It validates
both persisted `C_STARTUP_PASS` and `c-startup.status` before setting
`c_startup_verified`. The existing kernel, staging, bootstrap, probe, native
linker and fresh output arguments are still required. For RV64, build with
`riscv64gc-unknown-scarlet` and run the harness with `--arch riscv64`.

## Upstream zlib consumer

The [zlib builder](../../../tools/native-rustc/consumer-zlib/README.md) downloads
or reuses the SHA-256-pinned upstream zlib 1.3.2 release, compiles all 15
unmodified core/gzip C sources, and links a plain C consumer with the matching
CRT and `libscarlet_c.a`. Both AArch64 and RV64 have static cross-build and ELF
audit results. The consumer covers compression, incremental inflate/deflate,
gzip file I/O, integer/string `gzprintf`, descriptor ownership and corruption
errors. It uses Scarlet headers and Clang builtin headers, without a host libc.

```sh
python3 tools/native-rustc/consumer-zlib/build.py \
  --target aarch64-unknown-scarlet \
  --sysroot "$(rustc --print sysroot)" \
  --libc user/lib/scarlet-libc/target/aarch64-unknown-scarlet/release/libscarlet_c.a \
  --output /tmp/scarlet-zlib-aarch64
```

Use a fresh output directory and select `--clang`, `--ar` and `--linker` when
the desired tools are not on PATH. `--source-archive` reuses a local release
archive only when its hash matches. Add these options to the complete matching
QEMU harness invocation:

```sh
--storage ext2 --native-fs --proc-macro \
--c-startup-probe /tmp/scarlet-c-startup-aarch64/c-startup-probe \
--zlib-probe /tmp/scarlet-zlib-aarch64/zlib-probe
```

The harness requires persisted `ZLIB_PASS`, exit status 47 and the exact stdout
line `SCARLET_LIBC_ZLIB_OK` before recording `zlib_verified`. Build success does
not satisfy this guest check. The
[AArch64/HVF milestone](../../../tools/native-rustc/evidence/2026-09-22-libc-zlib-aarch64.json)
passes this gate, all C string/descriptor/stdio fixtures on ext2 and tmpfs,
the direct-C startup fixture, native std compilation/execution and proc macros.
The complete release kernel suite also passes all 1293 tests, including 20
added regressions. RV64 has cross-build and ELF-audit evidence only. The
published bundle remains unchanged, and this does not establish complete
zlib/libc conformance or installed SDK acceptance.

## Recorded milestones

The recorded [2026-09-22 filesystem run](../../../tools/native-rustc/evidence/2026-09-22-native-fs-aarch64.json)
passed on AArch64/HVF with ext2 and tmpfs. RV64 has cross-build evidence, not
filesystem guest execution evidence from that run. Local results validate the
recorded inputs; they do not update or validate an installed distribution bundle.

The [follow-up quality run](../../../tools/native-rustc/evidence/2026-09-22-libc-quality-aarch64.json)
also checks allocation fragmentation, optimized overflow/errno behavior across
threads, renamed directory handles and mount ancestors, and ext2 directory
growth. It includes the release kernel's complete 1273-test HVF result.

The [errno and C startup milestone](../../../tools/native-rustc/evidence/2026-09-22-libc-errno-aarch64.json)
passed on AArch64/HVF: constructor/new-thread/destructor errno access under
allocation denial, null backend allocation failures, the plain C constructor
and `main` returning 43, native filesystem checks, full native compilation and
proc macros. RV64 has probe, loader, static libc and C fixture cross-build and
ELF-audit evidence only; this milestone does not establish RV64 guest execution,
installed SDK acceptance, signal safety or musl parity.
