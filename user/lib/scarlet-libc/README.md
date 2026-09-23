# Scarlet Native libc

`scarlet-libc` implements a growing C runtime in Rust for Scarlet's native
AArch64 and RV64 targets. The static C ABI now includes ordinary and aligned
allocation, thread-local `errno`, byte strings, integer conversion, descriptor
I/O, unbuffered streams and integer/string formatted output, plus pathname
operations, clocks, entropy, sorting/searching, file timestamps and sync,
plus process-private pthread lifecycle and synchronization.
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

`qsort` uses allocation-free heapsort and `bsearch` searches sorted records.
`strspn`, `strcspn`, `strpbrk`, `strtok`/`strtok_r`, and ASCII
`strcasecmp`/`strncasecmp` extend the byte-string surface. `strtok` keeps its
continuation in Rust thread-local storage; `strtok_r` uses caller-owned state.
Integer absolute-value functions and IEEE `fabs`/`fabsf` are available; this is
not a general math library, and binary128 `long double` has no `fabsl` export.

`open`, `openat`, `creat`, `close`, `read`, `write`, `lseek` and `dup` use the
kernel's status-preserving descriptor operations. Duplicates share offset and
append state, while close-on-exec remains descriptor-local. `fcntl` supports
`F_GETFD`, `F_SETFD`, `F_GETFL` and `F_SETFL`; the last changes append while
preserving access mode. Unsupported flags and commands fail explicitly. The
kernel checks descriptor access and provides atomic append in ext2 and tmpfs.
Creation modes do not establish Unix ownership, umask or permission enforcement.

`pread`/`pwrite` perform positioned I/O without changing the shared offset;
`pwrite` ignores append. `ftruncate` preserves the offset, including through
`dup`, and zeroes bytes exposed by reextension. Ext2 truncation now preserves
sparse holes with bounded writeback buffers and supports its unsigned 32-bit
file-size range. Growth does not allocate the gap. Shrinking retains allocated
blocks until final inode deletion; `i_blocks` counts those data and indirect
blocks. Device I/O failure does not have complete rollback guarantees. Nonblocking `flock` implements advisory shared/exclusive locks
on regular ext2/tmpfs files. Lock ownership follows the open description, so
closing one duplicate does not release the remaining duplicate's lock. Blocking
acquisition and other filesystems return `ENOTSUP`; POSIX record locks are absent.

`unlink` and `rmdir` use a type-checked Native removal operation. Ext2 currently
returns `EBUSY` for last-link removal while an open description exists; tmpfs
retains the unlinked node until its references disappear. Ext2 `rmdir` returns
`ENOTEMPTY` for nonempty directories and `ENOTSUP` before mutation for empty
directories: retained-cwd lifetime is not solved, so removal remains unsupported.
`getcwd` uses Rust std
and writes the NUL-terminated result only after checking capacity. A null buffer
requests `malloc` storage; `getcwd(NULL, 0)` chooses the required capacity.

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

## Clocks, entropy and assertions

`time`, `gettimeofday`, `clock_gettime` and `clock_getres` expose realtime and
monotonic Native clocks, quantized to microseconds. An unavailable realtime
clock reports `EIO`. `nanosleep` validates the Native timer range and uses Rust
std sleep; the current backend does not report interruption, and the remainder
is left unchanged. Calendar conversion and timezone handling remain absent.

`getrandom` with flags zero and `getentropy` require a registered entropy
source and never accept the kernel's emergency PRNG. The latter fills at most
256 bytes. `GRND_RANDOM` and `GRND_NONBLOCK` return `ENOTSUP`; other flag bits
return `EINVAL`. Native entropy failures currently map to `EIO`, may leave
partial output, and are not proof that entropy hardware is available.

`assert.h` supports repeated inclusion after changing `NDEBUG`. Failed
assertions print a diagnostic without stdio buffering and terminate through
`abort`. Native `abort` exits the whole process with status 134; it does not
deliver `SIGABRT`, run destructors, or establish POSIX signal semantics.

## Threads and synchronization

`pthread.h` exposes thread creation, join/detach, identity, stack/detach
attributes, once initialization, thread-specific keys and process-private
mutexes/condition variables. Threads and blocking use the matching Rust std
runtime. Pthread functions return error numbers directly and preserve errno.
C thread handles use process-unique Rust thread identities; a completed join
consumes the handle. `pthread_once` publishes initialization to all callers.
Thread-specific destructors may reinstall values, with at most four rounds;
key deletion suppresses destruction and does not reuse a stale key identity.

Mutexes support normal, error-checking and recursive types. Static initializers
are all-zero. Condition waits release the mutex and reacquire it before return,
including timeout; absolute deadlines can use realtime or monotonic clocks.
Process-shared attributes return `ENOTSUP`. The public objects contain registry
identities, and internal ownership keeps blocked operations alive. There is no
`pthread_exit`, cancellation/cleanup API, robust mutex, priority protocol,
rwlock, barrier, scheduling or atfork support yet.

Native stack attributes require a 4096-byte multiple of at least 64 KiB; the
default is 2 MiB. Registry allocation is fallible, but Rust std thread creation
still contains infallible allocations, so complete physical-OOM recovery is
not established. This subset is not full POSIX threads conformance.

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

With the matching Scarlet cross compiler/sysroot selected as `rustc`, build the
library for the desired target:

```sh
cargo build --manifest-path user/lib/scarlet-libc/Cargo.toml \
  --release --target aarch64-unknown-scarlet
```

Use `riscv64gc-unknown-scarlet` for RV64. A successful archive build does not
establish C startup or guest compatibility. The [C startup builder](tests/build_c_startup.py)
links a plain C constructor and `main` with the matching CRT and libc, then
checks the ELF and archive members:

```sh
python3 user/lib/scarlet-libc/tests/build_c_startup.py \
  --target aarch64-unknown-scarlet \
  --sysroot "$(rustc --print sysroot)" \
  --libc user/lib/scarlet-libc/target/aarch64-unknown-scarlet/release/libscarlet_c.a \
  --output /tmp/scarlet-c-startup-aarch64
```

Choose a fresh output directory and adjust the archive path if using
`CARGO_TARGET_DIR`. The output is local build data, not repository source. The
[Native TLS](../../../docs/abi/native-tls.md),
[filesystem](../../../docs/abi/native-filesystem.md), and
[descriptor](../../../docs/abi/native-descriptors.md) documents define the
runtime contracts that a matching guest image must provide.
