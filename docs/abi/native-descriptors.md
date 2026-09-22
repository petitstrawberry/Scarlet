# Native descriptor operations

These additive operations support the std-backed Scarlet C library. Existing
native handle, stream and VFS operation numbers retain their return contracts.
Every operation below returns a nonnegative success value or a negated errno.

| Number | Operation | Arguments | Success |
| --- | --- | --- | --- |
| 104 | `HandleGetFlags` | descriptor | Access mode and shared append flag |
| 105 | `HandleSetFlags` | descriptor, flags | Zero |
| 106 | `HandleCloseWithStatus` | descriptor | Zero |
| 107 | `HandleDuplicateWithStatus` | descriptor | New descriptor |
| 108 | `HandleGetDescriptorFlags` | descriptor | Close-on-exec bit: zero or one |
| 109 | `HandleSetDescriptorFlags` | descriptor, zero or one | Zero |
| 203 | `StreamReadWithStatus` | descriptor, output, count | Bytes read |
| 204 | `StreamWriteWithStatus` | descriptor, input, count | Bytes written |
| 305 | `FileSeekWithStatus` | descriptor, signed 64-bit offset, whence, output | Zero; writes a `u64` position |
| 306 | `FileTruncateWithStatus` | descriptor, signed 64-bit length | Zero; preserves offset |
| 308 | `FileReadAtWithStatus` | descriptor, output, count, signed 64-bit offset | Bytes read; preserves offset |
| 309 | `FileWriteAtWithStatus` | descriptor, input, count, signed 64-bit offset | Bytes written; ignores append and preserves offset |
| 310 | `FileLock` | descriptor, operation | Zero |
| 418 | `VfsRemoveWithStatus` | NUL-terminated path, directory flag (zero or one) | Zero |
| 417 | `VfsOpenAt` | base, NUL-terminated path, flags, mode | New descriptor |

The signed seek offset uses the existing Native fixed-width scalar convention:
one argument word on 64-bit targets and two consecutive words on RV32. The
output is an eight-byte native-endian value on all targets. Whence is zero for
the beginning, one for the current position and two for EOF. The result must be
between zero and `i64::MAX`; negative positions and invalid whence produce
`EINVAL`, and overflow produces `EOVERFLOW`, without moving the position.
Nonseekable objects produce `ESPIPE`.

## Opening and descriptor lifetime

`VfsOpenAt` uses `scarlet_abi::fs::CURRENT_DIRECTORY` (`usize::MAX`) for cwd.
Relative paths otherwise start from an open VFS directory, including a retained
directory renamed after opening. Absolute paths ignore the base. Missing
descriptors produce `EBADF`; a non-directory base produces `ENOTDIR`.

Supported flag values are `O_RDONLY=0`, `O_WRONLY=1`, `O_RDWR=2`, `O_CREAT=0x40`,
`O_EXCL=0x80`, `O_TRUNC=0x200`, `O_APPEND=0x400`, `O_DIRECTORY=0x10000`,
`O_NOFOLLOW=0x20000` and `O_CLOEXEC=0x80000`. Unknown bits and access mode three
produce `EINVAL`. Creation, namespace resolution, overlay copy-up and truncation
are serialized with other VFS namespace mutations. Ordinary creation opens an
existing file without truncating it; exclusive creation rejects existing final
entries, including dangling symlinks. A lowest-numbered descriptor is reserved
before filesystem mutations, so descriptor exhaustion cannot truncate or create
a file before reporting `EMFILE`. The reservation is released on failure. This
does not promise rollback of every later filesystem-open failure.

Creation accepts only the low permission bits `0777`. They are forwarded to the
filesystem. The current metadata model projects permissions to read/write/execute
booleans; tmpfs cannot distinguish owner, group and other permission classes.
This is not POSIX ownership, umask, or permission enforcement. Set-ID and sticky
bits are rejected. UTF-8 remains required by the native VFS path representation.

Opening and duplicating choose the lowest unused descriptor. Duplicates share
the open-file position and append flag. They preserve access
mode and clear close-on-exec on the new descriptor. `HandleGetFlags` returns only
access and append state; `HandleSetFlags` can change append while requiring the
same access mode. Close-on-exec is descriptor-local and uses operations 108/109.
Legacy metadata can represent only one special semantic; adding close-on-exec to
a legacy descriptor already carrying another semantic returns `EOPNOTSUPP`.

## I/O and append

Read and write check descriptor access before touching a user buffer, including
zero-length operations. Reading a write-only descriptor or writing a read-only
descriptor returns `EBADF`; byte I/O on a VFS directory returns `EISDIR`. Counts
larger than `isize::MAX` produce `EINVAL`. A syscall transfers at most 64 KiB and
may return a shorter count; callers must handle short I/O. User buffers are copied
across page boundaries rather than dereferenced directly. Read destinations and
seek outputs are preflighted, but concurrent unmapping can still race usercopy.

The append flag belongs to the shared VFS open object. Tmpfs and ext2 choose EOF,
write bytes and update the open-file position while holding a shared inode/data
lock. Separate append descriptors therefore use the current shared EOF on every
write, even after a seek. Other filesystem drivers must implement the atomic
append capability; opening or enabling append otherwise returns `EOPNOTSUPP`.
Seek-then-write is not used as a fallback.
The signed seek operation currently has implementations for regular tmpfs and
ext2 files. This does not change legacy seek behavior or claim full POSIX
descriptor support, nonblocking descriptor flag control, or signal-safe stdio.

## Positioned I/O and truncation

Operations 308/309 accept an absolute nonnegative offset using the Native
fixed-width scalar convention: one word on AArch64/RV64, two words on RV32.
Their access, count, usercopy and 64 KiB short-transfer rules match stream I/O.
A range extending beyond `i64::MAX` returns `EOVERFLOW`; negative offsets return
`EINVAL`. Both preserve the shared open-file offset, and positioned writes
ignore append state. They call the file's positioned operations directly,
without temporarily moving the shared cursor.

Operation 306 accepts a nonnegative signed 64-bit length with the same scalar
convention, requires write access, and preserves the shared offset even when
shrinking past it. Reextending exposes zero bytes. Ext2 currently reconstructs
content and limits this operation to 16 MiB, rejecting larger requests before
allocation or mutation; this is not a general maximum for normal writes.
Allocation is fallible. Shrinking currently retains disk/cache allocations;
complete rollback after device I/O failure is not established. No physical
memory-exhaustion or power-loss guarantee is established by these checks.

## Nonblocking whole-file locks

Operation 310 exposes the `flock` subset: `LOCK_SH=1`, `LOCK_EX=2`, `LOCK_NB=4`
and `LOCK_UN=8`. Acquisition requires `LOCK_NB`; blocking acquisition returns
`EOPNOTSUPP`. Invalid combinations return `EINVAL`, incompatible owners return
`EAGAIN`, and unsupported object/filesystem types return `EOPNOTSUPP`. Only
regular ext2 and tmpfs files currently implement this contract.

Locks are advisory and keyed by filesystem/inode identity, not pathname. They
belong to an open file description and are shared by duplicate/fork references;
closing its last reference releases its lock. Independent shared owners may
coexist. A failed shared-to-exclusive upgrade preserves the existing shared
lock. Read/write calls do not enforce these locks. POSIX `fcntl` record locks,
blocking wait/cancellation and cross-protocol locking are not implemented.

## Typed removal

Operation 418 atomically validates and removes the final directory entry under
the namespace mutation guard. Flag zero removes a non-directory; flag one
removes an empty directory. It does not follow a final symlink. Missing paths,
wrong types, nonempty directories and busy roots/mounts retain distinct errors.
UTF-8 and the Native path-length bound still apply. Ext2 currently refuses
last-link removal while an open description remains, returning `EBUSY` instead
of reclaiming live storage; tmpfs retains unlinked nodes until references end.
For ext2 directory removal, nonempty directories return `ENOTEMPTY` and empty
directories return `EOPNOTSUPP` before mutation. Retained cwd/directory-reference
lifetime is not yet implemented safely, so ext2 rmdir is intentionally
unsupported rather than reclaiming referenced directory storage.

## Recorded acceptance

The [AArch64/HVF SQLite milestone](../../tools/native-rustc/evidence/2026-09-22-libc-sqlite-aarch64.json)
and [logs](../../tools/native-rustc/evidence/2026-09-22-libc-sqlite-aarch64/)
record the C positioned-I/O, locking and pathname fixtures passing on ext2 and
tmpfs, alongside eight SQLite create/verify/crash/recover processes and host
verification of the recovered ext2 database. The unfiltered release kernel
suite passes all 1328 tests. Actual guest testing exposed tmpfs unlink retiring
cache data while a descriptor still owned the node; final-node cache/quota
ownership and deferred retirement of pinned pages correct that case, with
three dedicated regressions. RV64/RV32 kernel compilation passes; RV64 C guest
execution, full Unix lifetime semantics and power-loss safety remain unverified.
