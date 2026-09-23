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
| Upstream zlib consumer | The unmodified zlib 1.3.2 core/gzip sources were statically linked against the matching CRT and std-backed libc. | A bounded AArch64 guest run covered compression, gzip and error paths. Repeat with the current SDK before treating this as release support. |
| Positioned I/O, truncation and locks | `pread`/`pwrite` preserve the shared offset; `pwrite` ignores append. `ftruncate` preserves offset. Nonblocking `flock` on regular ext2/tmpfs files follows open-description lifetime. | [C fixture](tests/positioned.c) covers bounds, access, sparse/zero-tail behavior, duplicate offsets and lock contention; passes on AArch64/HVF. |
| Path removal and cwd | Type-checked `unlink`/`rmdir`; `getcwd` checks capacity before output and supports a malloc-owned null-buffer result. Ext2 last-link removal of an open file returns `EBUSY`; empty-directory removal returns `ENOTSUP`, nonempty returns `ENOTEMPTY`. | [C fixture](tests/path.c) covers symlink/trailing-slash/type errors, live duplicate lifetime, replacement and cwd bounds; passes on AArch64/HVF. |
| Sorting and byte algorithms | Allocation-free heapsort, binary search, spans, tokenizers, ASCII case-insensitive comparisons and integer absolute values. `strtok` uses Rust TLS. | Host shape/boundary/thread tests and [C fixture](tests/algorithms.c); passes on AArch64/HVF. |
| Clocks, entropy and termination | Realtime/monotonic clocks, validated sleep, entropy-only random bytes, IEEE `fabs`/`fabsf`, repeatable `assert.h`; abort exits the process with status 134. | Host validation helpers and [C fixture](tests/runtime.c), plus child assertion/abort checks; passes on AArch64/HVF. |
| Upstream SQLite consumer | A separate Native VFS uses positioned I/O, exclusive nonblocking locks, rollback journals and memory temp storage. | Bounded AArch64 guest checks covered create/verify/process-exit recovery on ext2 and tmpfs. Unix-VFS, WAL, power-loss durability and RV64 guest support are not established. |
| POSIX thread subset | Rust std-backed create/join/detach, identity, stack/detach attributes, once, thread-specific keys with four destructor rounds; process-private normal/error-checking/recursive mutexes and timed condition variables. Direct error returns preserve errno. | Host concurrency/lifetime tests and C ABI fixtures cover this subset. Cancellation, robust/shared mutexes, rwlocks and full POSIX behavior remain outside it. |
| C headers | Partial `pthread.h`, `assert.h`, `ctype.h`, `errno.h`, `fcntl.h`, `limits.h`, `math.h`, `stdio.h`, `stdlib.h`, `string.h`, `strings.h`, `time.h`, `unistd.h`, `sys/file.h`, `sys/random.h`, `sys/time.h`, `sys/types.h`, `sys/stat.h`; LP64 layouts. | [Header checks](tests/check_headers.py) compile standalone/repeated inclusion, C11/C++11 linkage and layouts for AArch64/RV64 without host libc headers. |

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
  flag. Nonblocking whole-file `flock` is limited to regular ext2/tmpfs files;
  blocking acquisition and POSIX record locks remain absent. Stdio is unbuffered;
  buffering controls, scanf, wide streams and the remaining stdio surface are
  absent. Floating-point, wide, positional, grouping and `%n` output formats
  report `ENOTSUP`; this is a supported subset, not printf conformance.
- **Clock, entropy and termination contracts:** clocks are microsecond-quantized,
  with realtime availability required. Sleep has no EINTR/remainder behavior.
  Entropy requests use registered hardware/providers only; nonzero recognized
  flags are unsupported and the legacy failure sentinel maps to EIO. A failed
  fill may alter part of the buffer. Abort terminates with exit 134 rather than
  signal delivery. These are explicit limits, not full POSIX behavior.
- **Truncation and unlink:** ext2 truncate uses bounded writeback, preserves
  holes and the open offset, and accepts its unsigned 32-bit size range.
  Shrink retains blocks, counted in `i_blocks`, until final inode deletion;
  complete rollback after device I/O failure is not established.
  Last-link removal of an
  open ext2 file returns EBUSY; full Unix unlink lifetime remains outstanding.
  Ext2 rmdir reports ENOTEMPTY for nonempty directories and EOPNOTSUPP before
  mutation for empty directories because retained-cwd lifetime is unresolved.
  Deterministic allocation errors do not establish physical OOM behavior.
- **Concurrency and persistence:** normal ext2 writes, timestamp updates,
  truncation and fsync share an inode mutex. Full mmap/unlink and namespace
  concurrency, pinned-page invalidation and crash consistency still need
  separate acceptance tests and fixes; these results do not establish them.
- **Coverage:** partial headers are not general-purpose C SDK headers. The
  remaining strings/conversions and descriptor APIs, general locale, math,
  process and signal APIs, locks, pthreads, sockets and name resolution remain
  incomplete or absent. Existing kernel or Rust APIs do not imply matching C
  exports. Cargo, curl, libgit2 and SQLite Unix-VFS acceptance remain outstanding.
  The separate Native SQLite VFS intentionally serializes readers, omits WAL,
  mmap and loadable extensions. Process-exit hot-journal recovery passes on ext2/tmpfs; it does not
  establish power-loss safety or all crash-recovery cases.

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
   SQLite Native VFS create/reopen, forced-exit hot-journal recovery and
   extracted-database exact-content checks pass on AArch64/HVF. Its supported
   subset does not replace Unix-VFS, full upstream-suite or RV64 acceptance.
   Then run Cargo on an offline workspace
   with a path dependency, build script and proc macro, and execute its output.
   Record AArch64 and RV64 guest results for the shipped artifacts; automate
   regressions, retain logs and hashes as CI artifacts, and verify the installed bundle separately
   from private development builds.
