//! C metadata over the Native VFS metadata record.

#[cfg(target_os = "scarlet")]
use std::ffi::c_char;
use std::ffi::c_int;

#[cfg(target_os = "scarlet")]
use scarlet_abi::{ERRNO_EBADF, ERRNO_EINVAL, ERRNO_EIO, Syscall, fs::ERRNO_EFAULT};
use scarlet_abi::{
    FILE_PERMISSION_EXECUTE, FILE_PERMISSION_READ, FILE_PERMISSION_WRITE, FILE_TYPE_BLOCK_DEVICE,
    FILE_TYPE_CHAR_DEVICE, FILE_TYPE_DIRECTORY, FILE_TYPE_PIPE, FILE_TYPE_REGULAR,
    FILE_TYPE_SOCKET, FILE_TYPE_SYMLINK, RawFileMetadata,
};

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Timespec {
    seconds: i64,
    nanoseconds: i64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Stat {
    device: u64,
    special_device: u64,
    inode: u64,
    link_count: u64,
    pub(crate) mode: u32,
    user: u32,
    group: u32,
    padding: u32,
    size: i64,
    accessed: Timespec,
    modified: Timespec,
    changed: Timespec,
    block_size: i64,
    blocks: i64,
}

fn to_stat(raw: RawFileMetadata) -> Result<Stat, c_int> {
    let size = i64::try_from(raw.size).map_err(|_| scarlet_abi::fs::ERRNO_EOVERFLOW)?;
    let file_type = match raw.file_type {
        FILE_TYPE_REGULAR => 0o100000,
        FILE_TYPE_DIRECTORY => 0o040000,
        FILE_TYPE_SYMLINK => 0o120000,
        FILE_TYPE_CHAR_DEVICE => 0o020000,
        FILE_TYPE_BLOCK_DEVICE => 0o060000,
        FILE_TYPE_PIPE => 0o010000,
        FILE_TYPE_SOCKET => 0o140000,
        _ => 0,
    };
    let permissions = (if raw.permissions & FILE_PERMISSION_READ != 0 {
        0o444
    } else {
        0
    }) | (if raw.permissions & FILE_PERMISSION_WRITE != 0 {
        0o222
    } else {
        0
    }) | (if raw.permissions & FILE_PERMISSION_EXECUTE != 0 {
        0o111
    } else {
        0
    });
    let stamp = |seconds: u64| -> Result<Timespec, c_int> {
        Ok(Timespec {
            seconds: i64::try_from(seconds).map_err(|_| scarlet_abi::fs::ERRNO_EOVERFLOW)?,
            nanoseconds: 0,
        })
    };
    Ok(Stat {
        // The Native record has a filesystem-local inode but no mount ID.
        device: 0,
        // Native metadata does not expose a separate device-node number.
        special_device: 0,
        inode: raw.file_id,
        link_count: raw.link_count as u64,
        mode: file_type | permissions,
        user: 1,
        group: 1,
        padding: 0,
        size,
        accessed: stamp(raw.accessed)?,
        modified: stamp(raw.modified)?,
        // Status-change time is unavailable; expose modification time until
        // the Native metadata ABI grows a separate timestamp.
        changed: stamp(raw.modified)?,
        block_size: 4096,
        blocks: size / 512 + i64::from(size % 512 != 0),
    })
}

#[cfg(target_os = "scarlet")]
fn result(value: usize) -> Result<(), c_int> {
    if value == usize::MAX {
        Err(ERRNO_EIO)
    } else if value > isize::MAX as usize {
        Err((value as isize).wrapping_neg() as c_int)
    } else {
        Ok(())
    }
}

/// # Safety
/// `path` is NUL-terminated and `output` points to a writable Stat.
#[cfg(target_os = "scarlet")]
unsafe fn path_stat(path: *const c_char, output: *mut Stat, no_follow: bool) -> c_int {
    if path.is_null() || output.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }
    let mut raw = RawFileMetadata::default();
    // SAFETY: the kernel validates the path and copies a fixed-size record.
    let status = unsafe {
        scarlet_sys::syscall3(
            Syscall::VfsMetadataWithStatus,
            path as usize,
            (&raw mut raw) as usize,
            usize::from(no_follow),
        )
    };
    if let Err(errno) = result(status) {
        return crate::fail(errno);
    }
    let converted = match to_stat(raw) {
        Ok(converted) => converted,
        Err(errno) => return crate::fail(errno),
    };
    // SAFETY: the caller supplies a writable output.
    unsafe { *output = converted };
    0
}

/// # Safety
/// `path` and `output` satisfy the C stat contract.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stat(path: *const c_char, output: *mut Stat) -> c_int {
    unsafe { path_stat(path, output, false) }
}

/// # Safety
/// `path` and `output` satisfy the C lstat contract.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lstat(path: *const c_char, output: *mut Stat) -> c_int {
    unsafe { path_stat(path, output, true) }
}

/// # Safety
/// `output` points to a writable Stat.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fstat(fd: c_int, output: *mut Stat) -> c_int {
    if fd < 0 {
        return crate::fail(ERRNO_EBADF);
    }
    if output.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }
    let mut raw = RawFileMetadata::default();
    // SAFETY: the kernel validates the handle and writes a fixed-size record.
    let status = unsafe {
        scarlet_sys::syscall2(Syscall::FileMetadata, fd as usize, (&raw mut raw) as usize)
    };
    if let Err(errno) = result(status) {
        return crate::fail(errno);
    }
    let converted = match to_stat(raw) {
        Ok(converted) => converted,
        Err(errno) => return crate::fail(errno),
    };
    // SAFETY: caller supplies a writable output.
    unsafe { *output = converted };
    0
}

/// # Safety
/// `path` is a NUL-terminated pathname.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chmod(path: *const c_char, _mode: u32) -> c_int {
    if path.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }
    // Native VFS does not yet expose a permission mutation operation.
    crate::fail(scarlet_abi::ERRNO_EOPNOTSUPP)
}

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn getpgid(_pid: c_int) -> c_int {
    // Scarlet has no process-group capability yet.
    crate::fail(scarlet_abi::ERRNO_EOPNOTSUPP)
}

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn getsid(_pid: c_int) -> c_int {
    crate::fail(scarlet_abi::ERRNO_EOPNOTSUPP)
}

/// # Safety
/// `path` is NUL-terminated.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn access(path: *const c_char, mode: c_int) -> c_int {
    if path.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }
    if mode & !7 != 0 {
        return crate::fail(ERRNO_EINVAL);
    }
    let mut raw = RawFileMetadata::default();
    // SAFETY: the kernel copies metadata for an existing path.
    let status = unsafe {
        scarlet_sys::syscall3(
            Syscall::VfsMetadataWithStatus,
            path as usize,
            (&raw mut raw) as usize,
            0,
        )
    };
    if let Err(errno) = result(status) {
        return crate::fail(errno);
    }
    let requested = (if mode & 4 != 0 {
        FILE_PERMISSION_READ
    } else {
        0
    }) | (if mode & 2 != 0 {
        FILE_PERMISSION_WRITE
    } else {
        0
    }) | (if mode & 1 != 0 {
        FILE_PERMISSION_EXECUTE
    } else {
        0
    });
    if raw.permissions & requested == requested {
        0
    } else {
        crate::fail(scarlet_abi::fs::ERRNO_EACCES)
    }
}

/// # Safety
/// `path` is NUL-terminated.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mkdir(path: *const c_char, mode: u32) -> c_int {
    if path.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }
    // Native mkdir currently creates mode 0755 and cannot honor other modes.
    if mode & 0o7777 != 0o755 {
        return crate::fail(scarlet_abi::ERRNO_EOPNOTSUPP);
    }
    // SAFETY: the kernel reads a NUL-terminated path and returns -errno.
    match result(unsafe {
        scarlet_sys::syscall1(Syscall::VfsCreateDirectoryWithStatus, path as usize)
    }) {
        Ok(()) => 0,
        Err(errno) => crate::fail(errno),
    }
}

/// # Safety
/// `path` is NUL-terminated and `buffer` has `size` writable bytes.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn readlink(path: *const c_char, buffer: *mut c_char, size: usize) -> isize {
    if path.is_null() || buffer.is_null() {
        return crate::fail(ERRNO_EFAULT) as isize;
    }
    if size == 0 || size > isize::MAX as usize {
        return crate::fail(ERRNO_EINVAL) as isize;
    }
    // SAFETY: the kernel copies at most size bytes without a NUL terminator.
    let count = unsafe {
        scarlet_sys::syscall3(Syscall::VfsReadlink, path as usize, buffer as usize, size)
    };
    if count == usize::MAX || count > size {
        crate::fail(ERRNO_EIO) as isize
    } else {
        count as isize
    }
}

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn getpid() -> c_int {
    // SAFETY: process ID query takes no arguments.
    let pid = unsafe { scarlet_sys::syscall0(Syscall::Getpid) };
    c_int::try_from(pid).unwrap_or_else(|_| crate::fail(ERRNO_EIO))
}

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn getppid() -> c_int {
    // SAFETY: process ID query takes no arguments.
    let pid = unsafe { scarlet_sys::syscall0(Syscall::Getppid) };
    c_int::try_from(pid).unwrap_or_else(|_| crate::fail(ERRNO_EIO))
}

// Scarlet has no Unix user database; libc presents a synthetic unprivileged
// identity. Authorization remains governed by Native access controls.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn getuid() -> u32 {
    1
}
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn geteuid() -> u32 {
    1
}
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn getgid() -> u32 {
    1
}
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn getegid() -> u32 {
    1
}

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn fchmod(_fd: c_int, _mode: u32) -> c_int {
    crate::fail(scarlet_abi::ERRNO_EOPNOTSUPP)
}
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn fchown(_fd: c_int, _user: u32, _group: u32) -> c_int {
    crate::fail(scarlet_abi::ERRNO_EOPNOTSUPP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_record_preserves_size_and_inode() {
        let raw = RawFileMetadata {
            size: 4097,
            file_type: FILE_TYPE_REGULAR,
            permissions: FILE_PERMISSION_READ | FILE_PERMISSION_WRITE,
            created: 100,
            modified: 110,
            accessed: 120,
            file_id: 17,
            link_count: 2,
            _reserved: 0,
        };
        let converted = to_stat(raw).unwrap();
        assert_eq!(converted.mode, 0o100666);
        assert_eq!(converted.size, 4097);
        assert_eq!(converted.inode, 17);
        assert_eq!(converted.link_count, 2);
        assert_eq!(converted.blocks, 9);
    }
}
