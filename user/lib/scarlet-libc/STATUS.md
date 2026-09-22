# Support and acceptance status

The implementation will remain in Rust, using Scarlet Rust std as its backend.
The goal is broad C/POSIX compatibility and musl-level quality; `no_std` and
removing std are not requirements. This is a direction, not a claim of present
parity or POSIX conformance. The current archive adapts Scarlet Rust std and
Native syscalls to a C ABI.

## Implemented surface

| Surface | Current behavior | Existing checks |
| --- | --- | --- |
| `malloc`, `calloc`, `realloc`, `free` | At least 16-byte alignment; checked size arithmetic; zeroing; failed realloc preserves the old allocation. Zero-size allocation requests a freeable minimum block; `realloc(p, 0)` frees non-null `p` and returns null. Success and free preserve errno. Builtin substitution is disabled in the implementing crate. | Host tests; C guest mixed-allocation fragmentation/reallocation stress; optimized overflow and forced null-backend checks. |
| `aligned_alloc`, `posix_memalign`, `reallocarray` | Power-of-two aligned allocation; non-multiple sizes supported. `posix_memalign` also requires pointer-size alignment and preserves errno/output on failure. `reallocarray` checks multiplication and retains the old block on failure. All results support ordinary free/realloc. | Host boundary, zero-size, alignment and reuse tests; C guest checks; deterministic backend failure checks for all three APIs. |
| `errno`, `__errno_location` | Per-thread writable `int` in the versioned [Native TLS header](../../../docs/abi/native-tls.md), prepared before constructors or thread entry. Access needs no allocation or syscall; std `last_os_error` reads the same slot. | Host isolation tests; AArch64 guest constructor, fresh-thread and TLS-destructor checks with heap allocation denied; C syscall-error checks. |
| Plain C startup fixture | The matching CRT initializes native TLS and calls a C constructor and `main`; the directly linked static archive includes Rust std. | [Builder](tests/build_c_startup.py) audits inputs, symbols and ELF; AArch64/HVF guest executes [C fixture](tests/c_main.c) and exits 43, with persisted marker and exit-status validation. This is not installed SDK acceptance. |
| `realpath` | Existing UTF-8 paths; caller buffer or library allocation; VFS resolution follows symlinks before `..`. | C/Rust guest checks for missing, empty, dangling, cyclic, non-directory and trailing-slash paths; nested tmpfs mount paths. |
| `futimens`, `utimensat` | Seconds precision; NOW/OMIT/null times; dirfd-relative paths; absolute paths ignore dirfd; final-link nofollow. | C guest flag/fd/time errors and resulting inode metadata; Rust checks updates after rename and ext2 overflow without partial updates. |
| `fsync`, `fdatasync` | Native file sync; `fdatasync` currently uses the same full sync operation. | Guest file/data/timestamp checks and independent ext2 inode extraction after shutdown. Power-loss durability is not established by this test. |
| Byte strings and memory | Comparisons, copies, concatenation, bounded scans, searches, `strdup`/`strndup`, stable `strerror` storage. The matching compiler-builtins supplies `memcpy`, `memmove`, `memset`, `memcmp` and `strlen`. | Host boundary, unsigned-byte and ownership tests; [C fixture](tests/strings.c) calls the actual symbols with builtins disabled and passes on AArch64/HVF. |
| C-locale classification and integer conversion | ASCII `ctype` operations; `strtol`/`strtoul`/`strtoll`/`strtoull` plus decimal convenience functions. C17/POSIX prefixes, no C23 binary prefix. Overflow consumes valid digits, saturates and reports `ERANGE`. | Host all-byte/EOF, radix, signed/unsigned limit and end-pointer checks; C fixture passes on AArch64/HVF. |
| Native descriptor I/O | `open`/`openat`/`creat`, `close`, `read`/`write`, `lseek`, `dup`; four `fcntl` commands for descriptor flags and access/append state. Kernel enforces descriptor access and owns shared offset/append state. | [C fixture](tests/descriptor.c) passes relative/absolute paths, flag errors, access, duplication, append and errno checks on ext2/tmpfs in AArch64/HVF. See the [descriptor ABI](../../../docs/abi/native-descriptors.md). |
| Unbuffered stdio | Locked opaque `FILE`, standard streams, open/close/flush, block/character/line I/O, seek/tell, indicators and one-byte pushback. `fdopen` retains the descriptor on failure. | Host mode-parser checks and [C fixture](tests/stdio.c) for file state, partial items, ownership, append and pushback; AArch64/HVF passes on ext2/tmpfs. |
| Formatted output | `printf`, `fprintf`, `sprintf`, `snprintf` and all four `v` variants; integer/string/character/pointer formatting, width, precision and integer length modifiers. Unsupported float/wide/positional/grouping/`%n` formats return `ENOTSUP`. | Host comparisons with the host libc, truncation/overflow and forwarded-varargs tests; C tests pass arguments through register and stack areas in AArch64/HVF. RV64 has cross-build evidence only. |
| Upstream zlib consumer | All 15 unmodified zlib 1.3.2 core/gzip sources compile and statically link against the matching CRT and std-backed libc. | [Pinned builder](../../../tools/native-rustc/consumer-zlib/README.md) records both target builds, hashes and ELF audits. AArch64/HVF passes compression/gzip/error-path acceptance with exit 47 and the required stdout marker. |
| C headers | Partial `ctype.h`, `errno.h`, `fcntl.h`, `limits.h`, `stdio.h`, `stdlib.h`, `string.h`, `time.h`, `unistd.h`, `sys/types.h`, `sys/stat.h`; LP64 layouts. | [Header checks](tests/check_headers.py) compile standalone/repeated inclusion, C11/C++11 linkage and layouts for AArch64/RV64 without host libc headers. |

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

The [errno and C startup evidence](../../../tools/native-rustc/evidence/2026-09-22-libc-errno-aarch64.json)
records AArch64/HVF passes for allocation-free errno through constructor,
thread entry and destructor paths, deterministic null allocation results, and
the plain C fixture. The final run also passes native filesystem checks, full
native compilation and proc macros with a matching staged runtime. RV64 has
cross-build and ELF-audit checks for the probe, loader, static libc and C fixture;
guest execution on RV64 remains outstanding.

The [string/descriptor/stdio and zlib evidence](../../../tools/native-rustc/evidence/2026-09-22-libc-zlib-aarch64.json)
records AArch64/HVF `FULL_PASS`: the new C fixtures pass on ext2 and tmpfs,
zlib returns 47 with its required stdout marker, and the direct-C startup,
native filesystem, full native compilation and proc-macro gates all pass.
The complete release kernel suite passes all 1293 tests, including 20 added
regressions and the fix for seeking with a detached ext2 node. Host checks pass
33 libc tests, 27 ABI tests, three probe tests, 21 Python tests and 56 header
checks. RV64 has cross-build and ELF-audit evidence only. The published bundle
is unchanged; Cargo and installed SDK acceptance remain outstanding.

## Limits that callers must account for

- **Runtime integration:** allocation and TLS use Rust std and its CRT by
  design. The toolchain must supply and initialize the matching backend.
  Keep this dependency one-way: the std primitives used by scarlet-libc must
  not call back into these same C exports. Allocations from another libc
  instance are not interchangeable. Every runtime-bearing executable, loader
  and DSO must agree on the TLS header; old runtime artifacts must be rebuilt
  or replaced together. Reentering startup on the same thread preserves the
  existing header, errno and Rust TLS namespace list.
- **Error paths and exhaustion:** the errno slot is preallocated at startup or
  before clone. Errno access adds no allocation, including when an allocation
  backend returns null.
  Missing or incompatible TLS aborts; custom entry points and foreign thread
  creators must establish the runtime contract. Creating the mapping itself
  can fail before application startup. Deterministic null-backend tests do not
  establish behavior under physical memory exhaustion, signal interruption or
  all destructor orders. Signal-safe errno access is not established.
- **Paths:** UTF-8 and a 1024-byte maximum are Native ABI constraints. Arbitrary
  non-NUL Unix pathname bytes are not currently supported.
- **Time:** subsecond input is truncated; pre-epoch timestamps are rejected;
  ext2 timestamps are limited to unsigned 32-bit seconds. These are explicit
  implementation limits, not a general POSIX timestamp guarantee.
- **Permissions:** ownership/credential and permission enforcement semantics
  still need definition and implementation. Creation forwards low `0777` mode
  bits; the metadata model and tmpfs do not preserve full owner/group/other
  permission classes. There is no umask contract. Checking a descriptor's
  read/write access mode does not establish Unix access-control behavior.
- **Descriptors and streams:** `fcntl` currently implements only `F_GETFD`,
  `F_SETFD`, `F_GETFL` and `F_SETFL`, with append as the only mutable status
  flag. There is no nonblocking or record-lock interface. Stdio is unbuffered;
  buffering controls, scanf, wide streams and the remaining stdio surface are
  absent. Floating-point, wide, positional, grouping and `%n` output formats
  report `ENOTSUP`; this is a supported subset, not printf conformance.
- **Concurrency and persistence:** normal ext2 writes, timestamp updates,
  truncation and fsync share an inode mutex. Full mmap/unlink and namespace
  concurrency, pinned-page invalidation and crash consistency still need
  separate acceptance tests and fixes; these results do not establish them.
- **Coverage:** partial headers are not general-purpose C SDK headers. The
  remaining strings/conversions and descriptor APIs, general locale, math,
  process and signal APIs, locks, pthreads, sockets and name resolution remain
  incomplete or absent. Existing kernel or Rust APIs do not imply matching C
  exports. Cargo, curl, libgit2 and SQLite acceptance remains outstanding.

## Acceptance gates toward a complete C runtime

These gates apply to the Rust implementation; passing a smoke test is not a
substitute for declaring a standards baseline and checking its requirements.

1. **C startup and ABI.** The bounded direct C constructor/main fixture and
   versioned native TLS layout are implemented. Specify the complete
   AArch64/RV64 calling conventions, public type layouts, errno/flag values,
   CRT, environment, initialization and
   exit behavior. Link and run ordinary C `main` programs using the installed
   SDK, with the Scarlet Rust backend linked and initialized internally, no
   Rust source wrapper, and no host libraries or headers. Audit symbols and
   reproduce the SDK from pinned sources.
2. **Memory and thread behavior.** Use std allocation and thread facilities
   where they satisfy the C contract, extending the backend or using Native
   primitives where necessary. `errno` must remain available without allocating
   on the error path; recoverable C errors must not become Rust panics or aborts.
   Current tests cover odd sizes, extended alignment, overflow, null backend
   results and selected TLS lifetime paths. Exercise physical exhaustion,
   concurrent allocation, interruption and destructor order on both targets.
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
   libgit2 and curl tests against the SDK. The pinned zlib consumer now builds
   for both targets and passes in AArch64/HVF; RV64 guest execution, exhaustive
   upstream tests and installed SDK acceptance remain gates.
   Then run Cargo on an offline workspace
   with a path dependency, build script and proc macro, and execute its output.
   Record AArch64 and RV64 guest results for the shipped artifacts; automate
   regressions, preserve logs/hashes, and verify the installed bundle separately
   from private development builds.
