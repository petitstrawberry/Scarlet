//! WASI Preview1 core type and constant definitions (minimal subset)
//!
//! This module defines the minimal set of WASI types / constants required
//! for the initial syscall surface we implement (args/env + fd read/write/close + proc_exit).
//! It intentionally keeps scope small; more constants can be appended later.
#![allow(dead_code)]

/// Raw WASI error number type
pub type Errno = u16;

/// A successful result.
pub const ERRNO_SUCCESS: Errno = 0;
pub const ERRNO_BADF: Errno = 8;        // bad file descriptor
pub const ERRNO_INVAL: Errno = 28;      // invalid argument
pub const ERRNO_IO: Errno = 29;         // I/O error
pub const ERRNO_NOMEM: Errno = 48;      // out of memory
pub const ERRNO_NOSYS: Errno = 52;      // function not supported
pub const ERRNO_NOTCAPABLE: Errno = 76; // capability insufficient (provisional)

/// File descriptor number
pub type Fd = u32;
/// Size in bytes (for iov lengths etc.)
pub type Size = u32;
/// Filesize (64-bit)
pub type FileSize = u64;
/// Timestamp (nanoseconds since epoch)
pub type Timestamp = u64;

/// File type subset (enough for stdout/stderr and regular files)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    Unknown = 0,
    RegularFile = 4,
    Directory = 3,
    CharacterDevice = 2,
    BlockDevice = 1,
    Fifo = 6,
    SocketDgram = 7,
    SocketStream = 8,
    Symlink = 10,
}

/// Rights bit flags (minimal). Full WASI defines many more; we start tiny.
/// Using 64-bit to remain compatible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rights(pub u64);

impl Rights {
    pub const READ: Rights = Rights(1 << 0);
    pub const WRITE: Rights = Rights(1 << 1);
    pub const EMPTY: Rights = Rights(0);

    pub fn contains(self, other: Rights) -> bool { (self.0 & other.0) == other.0 }
    pub fn union(self, other: Rights) -> Rights { Rights(self.0 | other.0) }
}

/// File descriptor flags (placeholder subset)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FdFlags(pub u16);

impl FdFlags {
    pub const APPEND: FdFlags = FdFlags(1 << 0);
    pub const DSYNC: FdFlags = FdFlags(1 << 1);
    pub const NONBLOCK: FdFlags = FdFlags(1 << 2);
    pub const SYNC: FdFlags = FdFlags(1 << 3);
    pub const EMPTY: FdFlags = FdFlags(0);
    pub fn contains(self, other: FdFlags) -> bool { (self.0 & other.0) == other.0 }
    pub fn union(self, other: FdFlags) -> FdFlags { FdFlags(self.0 | other.0) }
}

/// Open flags placeholder (currently unused, defined for completeness)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OFlags(pub u16);

impl OFlags {
    pub const CREAT: OFlags = OFlags(1 << 0);
    pub const DIRECTORY: OFlags = OFlags(1 << 1);
    pub const EXCL: OFlags = OFlags(1 << 2);
    pub const TRUNC: OFlags = OFlags(1 << 3);
    pub const EMPTY: OFlags = OFlags(0);
    pub fn contains(self, other: OFlags) -> bool { (self.0 & other.0) == other.0 }
    pub fn union(self, other: OFlags) -> OFlags { OFlags(self.0 | other.0) }
}

/// Convert common internal errors to WASI errno codes (stub mapping for now)
pub fn map_fs_error_to_errno(kind: crate::fs::FileSystemErrorKind) -> Errno {
    use crate::fs::FileSystemErrorKind as K;
    match kind {
        K::NotFound => ERRNO_INVAL, // refine later
        K::PermissionDenied => ERRNO_NOTCAPABLE,
        K::AlreadyExists => ERRNO_INVAL,
        K::IoError => ERRNO_IO,
        // Fallback for kinds not explicitly mapped yet
        _ => ERRNO_IO,
    }
}
