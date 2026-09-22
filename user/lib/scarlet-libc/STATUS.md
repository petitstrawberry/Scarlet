# Support and acceptance status

The implementation will remain in Rust, using Scarlet Rust std as its backend.
The goal is broad C/POSIX compatibility and musl-level quality; `no_std` and
removing std are not requirements. This is a direction, not a claim of present
parity or POSIX conformance. The current archive adapts Scarlet Rust std and
Native syscalls to a C ABI.

## Implemented surface

| Surface | Current behavior | Existing checks |
| --- | --- | --- |
| `malloc`, `calloc`, `realloc`, `free` | 16-byte alignment; checked size arithmetic; zeroing; failed realloc preserves the old allocation. `malloc(0)` requests a minimum allocation; `realloc(p, 0)` frees non-null `p` and returns null. Builtin substitution is disabled in the implementing crate. | Host allocation test; C guest odd-size fragmentation/reallocation stress; optimized Rust caller checks overflow returns null and sets `ENOMEM`. |
| `errno`, `__errno_location` | Per-thread writable `int`; wrappers set it on failure. Rust TLS supplies storage. | Host and Scarlet guest thread-isolation tests; C guest checks errno values after syscall failures. |
| `realpath` | Existing UTF-8 paths; caller buffer or library allocation; VFS resolution follows symlinks before `..`. | C/Rust guest checks for missing, empty, dangling, cyclic, non-directory and trailing-slash paths; nested tmpfs mount paths. |
| `futimens`, `utimensat` | Seconds precision; NOW/OMIT/null times; dirfd-relative paths; absolute paths ignore dirfd; final-link nofollow. | C guest flag/fd/time errors and resulting inode metadata; Rust checks updates after rename and ext2 overflow without partial updates. |
| `fsync`, `fdatasync` | Native file sync; `fdatasync` currently uses the same full sync operation. | Guest file/data/timestamp checks and independent ext2 inode extraction after shutdown. Power-loss durability is not established by this test. |
| C headers | Partial `errno.h`, `fcntl.h`, `limits.h`, `stdlib.h`, `time.h`, `unistd.h`, `sys/types.h`, `sys/stat.h`; LP64 layouts. | [Header checks](tests/check_headers.py) compile standalone/repeated inclusion, C11/C++11 linkage and layouts for AArch64/RV64. |

The [recorded filesystem evidence](../../../tools/native-rustc/evidence/2026-09-22-native-fs-aarch64.json)
identifies the exact kernel, sysroot and probe inputs. It establishes guest
execution on AArch64/HVF only, plus RV64 cross compilation. The
[C fixture](tests/native.c), [Rust companion](../../../tools/native-rustc/native_fs.rs),
and [host tests](src/) are executable specifications for their covered cases;
adding a case does not establish a guest pass until a matching run is recorded.
Commands are in the [README](README.md#build-and-checks).

The [follow-up quality evidence](../../../tools/native-rustc/evidence/2026-09-22-libc-quality-aarch64.json)
adds the allocation stress, optimized calloc/errno thread check, renamed cwd and
directory handles, renamed mount ancestor and growing ext2 directory cases.
It also records all 1273 kernel tests passing on HVF, including deterministic
write-publication locking and second-handle fsync persistence regressions.

## Limits that callers must account for

- **Runtime integration:** allocation and TLS use Rust std and its CRT by
  design. The toolchain must supply and initialize the matching backend.
  Keep this dependency one-way: the std primitives used by scarlet-libc must
  not call back into these same C exports. Allocations from another libc
  instance are not interchangeable.
- **Error-path allocation:** first use of `errno` can allocate Rust TLS storage.
  Exhaustion before that initialization can abort while trying to report
  `ENOMEM`. This is a source-identified risk; forced guest exhaustion is not yet
  a recorded acceptance result. Signal-safe errno access is not established.
- **Paths:** UTF-8 and a 1024-byte maximum are Native ABI constraints. Arbitrary
  non-NUL Unix pathname bytes are not currently supported.
- **Time:** subsecond input is truncated; pre-epoch timestamps are rejected;
  ext2 timestamps are limited to unsigned 32-bit seconds. These are explicit
  implementation limits, not a general POSIX timestamp guarantee.
- **Permissions:** ownership/credential and permission enforcement semantics
  still need definition and implementation. Successful file operations here
  do not establish Unix access-control behavior.
- **Concurrency and persistence:** normal ext2 writes, timestamp updates,
  truncation and fsync share an inode mutex. Full mmap/unlink and namespace
  concurrency, pinned-page invalidation and crash consistency still need
  separate acceptance tests and fixes; these results do not establish them.
- **Coverage:** partial headers are not general-purpose C SDK headers. stdio,
  the remaining descriptor APIs, strings/conversions, locale, math, process and
  signal APIs, locks, pthreads, sockets and name resolution are not supplied as
  a complete libc surface. Existing kernel or Rust APIs do not imply matching
  C exports. Cargo, curl, libgit2 and SQLite acceptance remains outstanding.

## Acceptance gates toward a complete C runtime

These gates apply to the Rust implementation; passing a smoke test is not a
substitute for declaring a standards baseline and checking its requirements.

1. **C startup and ABI.** Specify AArch64/RV64 calling conventions,
   public type layouts, errno/flag values, CRT, environment, initialization and
   exit behavior. Link and run ordinary C `main` programs using the installed
   SDK, with the Scarlet Rust backend linked and initialized internally, no
   Rust source wrapper, and no host libraries or headers. Audit symbols and
   reproduce the SDK from pinned sources.
2. **Memory and thread behavior.** Use std allocation and thread facilities
   where they satisfy the C contract, extending the backend or using Native
   primitives where necessary. `errno` must remain available without allocating
   on the error path; recoverable C errors must not become Rust panics or aborts.
   Exercise odd sizes, alignment, overflow, real exhaustion,
   concurrent allocation, TLS lifetime and destructor order on both targets.
3. **Declared C standard surface.** Complete the selected standard's headers
   and runtime families, including stdio/varargs, strings and conversions,
   numeric behavior, time and locale. Run an independent conformance suite;
   track each exception rather than accepting successful compilation as proof.
4. **Filesystem and process contracts.** Define descriptor lifetime/flags,
   `*at` resolution, metadata, ownership/permissions, locking, pipes, process
   creation/exec/wait and signals. Test boundary errors, interruption, races,
   atomic failure and storage writeback with deterministic regressions.
5. **Threads and networking.** Implement and test the selected pthread surface,
   cancellation/cleanup rules, synchronization, socket operations, polling and
   DNS. Verify multithreaded failure paths and lifetime rules, not just wrappers
   around existing Native syscalls.
6. **Real consumers and release evidence.** Build and run native zlib, SQLite,
   libgit2 and curl tests against the SDK. Then run Cargo on an offline workspace
   with a path dependency, build script and proc macro, and execute its output.
   Record AArch64 and RV64 guest results for the shipped artifacts; automate
   regressions, preserve logs/hashes, and verify the installed bundle separately
   from private development builds.
