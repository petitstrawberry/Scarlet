//! Native filesystem extensions used by Rust std and C/POSIX adapters.
//!
//! These operations return nonnegative success values or a negated errno.
//! Existing filesystem syscalls retain their original -1 error contract.

/// Maximum native pathname storage, including the NUL terminator.
pub const PATH_MAX: usize = 1024;
/// Use the current working directory for a relative VfsSetTimes pathname.
pub const CURRENT_DIRECTORY: usize = usize::MAX;

/// Open flags for `VfsOpenAt`. Unlisted flag bits are rejected.
pub const VFS_O_RDONLY: u32 = 0;
pub const VFS_O_WRONLY: u32 = 1;
pub const VFS_O_RDWR: u32 = 2;
pub const VFS_O_ACCMODE: u32 = 3;
pub const VFS_O_CREAT: u32 = 0x40;
pub const VFS_O_EXCL: u32 = 0x80;
pub const VFS_O_TRUNC: u32 = 0x200;
pub const VFS_O_APPEND: u32 = 0x400;
pub const VFS_O_DIRECTORY: u32 = 0x10000;
pub const VFS_O_NOFOLLOW: u32 = 0x20000;
pub const VFS_O_CLOEXEC: u32 = 0x80000;

pub const ERRNO_ENOMEM: i32 = 12;
pub const ERRNO_EMFILE: i32 = 24;
pub const ERRNO_ESPIPE: i32 = 29;
pub const ERRNO_EPIPE: i32 = 32;

pub const ERRNO_ENOENT: i32 = 2;
pub const ERRNO_EACCES: i32 = 13;
pub const ERRNO_EFAULT: i32 = 14;
pub const ERRNO_EBUSY: i32 = 16;
pub const ERRNO_EEXIST: i32 = 17;
pub const ERRNO_EXDEV: i32 = 18;
pub const ERRNO_ENOTDIR: i32 = 20;
pub const ERRNO_EISDIR: i32 = 21;
pub const ERRNO_ENOSPC: i32 = 28;
pub const ERRNO_EROFS: i32 = 30;
pub const ERRNO_ERANGE: i32 = 34;
pub const ERRNO_ENAMETOOLONG: i32 = 36;
pub const ERRNO_ENOTEMPTY: i32 = 39;
pub const ERRNO_ELOOP: i32 = 40;
pub const ERRNO_EOVERFLOW: i32 = 75;

pub const FILE_TIMES_VERSION: u32 = 1;
pub const FILE_TIMES_ACCESSED: u32 = 1;
pub const FILE_TIMES_MODIFIED: u32 = 2;
/// Do not follow the final symbolic link in VfsSetTimes.
pub const FILE_TIMES_NOFOLLOW: usize = 1;

/// Partial timestamp update, with seconds since the Unix epoch.
///
/// Unselected timestamps are left unchanged. Current native metadata has
/// second precision; adapters must truncate subsecond input, never round up.
/// A filesystem must reject unrepresentable values before changing either time.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawFileTimes {
    pub version: u32,
    pub flags: u32,
    pub accessed: u64,
    pub modified: u64,
}

impl RawFileTimes {
    pub const fn valid(&self) -> bool {
        self.version == FILE_TIMES_VERSION
            && self.flags & !(FILE_TIMES_ACCESSED | FILE_TIMES_MODIFIED) == 0
    }
}

const _: () = {
    assert!(core::mem::size_of::<RawFileTimes>() == 24);
    assert!(core::mem::offset_of!(RawFileTimes, accessed) == 8);
    assert!(core::mem::offset_of!(RawFileTimes, modified) == 16);
};
