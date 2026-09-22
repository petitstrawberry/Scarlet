# Native filesystem extensions for Rust std and libc

The following calls extend the Native ABI. They return a nonnegative success
value or a negated errno. Legacy calls retain their existing `usize::MAX`
failure convention. Scalar values follow the caller's word size; the timestamp
record has the same explicit layout on all architectures.

| Number | Operation | Arguments | Success |
| --- | --- | --- | --- |
| 303 | `FileSetTimes` | handle, `RawFileTimes*` | 0 |
| 304 | `FileSync` | handle | 0 |
| 413 | `VfsCanonicalize` | path, output buffer, capacity | bytes excluding NUL |
| 414 | `VfsSetTimes` | path, `RawFileTimes*`, flags, base directory handle | 0 |
| 415 | `VfsMetadataWithStatus` | path, `RawFileMetadata*`, nofollow (0 or 1) | 0 |
| 416 | `VfsCreateDirectoryWithStatus` | path | 0 |

`VfsSetTimes` uses `usize::MAX` for cwd. Absolute paths ignore the base handle.
Flag 1 prevents following the final symbolic link; other bits are invalid.
An open file handle identifies its inode even after a rename. tmpfs and ext2
implement timestamp changes; other backends return `EOPNOTSUPP` by default.

`RawFileTimes` is defined in `scarlet-abi::fs`: version and selected-field flags
are `u32`, followed by accessed and modified times as `u64` seconds since the
Unix epoch. Version must be 1; flag bits 1 and 2 select accessed and modified
respectively. The record is 24 bytes, aligned to 8 bytes. Omitted timestamps
remain unchanged. Invalid versions/flags fail before mutation, and ext2 checks
both timestamp ranges before writing either field. Its inode writeback shares
the timestamp-update lock and preserves explicit mtime through later fsync.

`VfsCanonicalize` resolves an existing pathname within the current VFS view.
The output is not NUL-terminated; the returned byte count is authoritative.
It follows up to 40 symbolic links across the entire lookup. Empty/missing
paths return `ENOENT`; non-directory components and a trailing slash on a
regular file return `ENOTDIR`; excessive links return `ELOOP`. A buffer that is
too small returns `ERANGE` without writing a partial path. A resolved pathname
too long for the Native `PATH_MAX` returns `ENAMETOOLONG`.

Relative-path anchoring must preserve `..`, `.`, and trailing slashes until the
VFS walker has checked each component. Canonical path reconstruction traverses
both entry parents and mountpoint parents, retaining intervening directories
for nested mounts. This same reconstruction is used by getcwd.

These APIs provide the filesystem behavior used by the initial
[`scarlet-libc`](../../user/lib/scarlet-libc/README.md) C adapters and the patched
Rust std. They do not introduce Linux syscall numbering into the Native ABI or
declare the target to be `cfg(unix)`.
