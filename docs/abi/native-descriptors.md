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
entries, including dangling symlinks. Creation is not rolled back if a later
filesystem-open or descriptor-table insertion fails.

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
descriptor support, advisory locking, nonblocking flag control, or signal-safe
stdio.
