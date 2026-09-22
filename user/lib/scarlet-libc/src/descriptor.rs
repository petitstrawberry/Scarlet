//! C descriptor operations on the status-preserving Scarlet Native ABI.
//!
//! The kernel owns open-file state, including the current offset and append
//! flag. No shadow descriptor table or temporary Rust File owns a C handle.

use std::ffi::c_int;
#[cfg(target_os = "scarlet")]
use std::ffi::{c_char, c_uint, c_void};

#[cfg(any(test, target_os = "scarlet"))]
use scarlet_abi::{ERRNO_EBADF, ERRNO_EINVAL, fs::ERRNO_EFAULT};
#[cfg(target_os = "scarlet")]
use scarlet_abi::{Syscall, fs::CURRENT_DIRECTORY};

pub const O_ACCMODE: c_int = scarlet_abi::fs::VFS_O_ACCMODE as c_int;
pub const O_RDONLY: c_int = scarlet_abi::fs::VFS_O_RDONLY as c_int;
pub const O_WRONLY: c_int = scarlet_abi::fs::VFS_O_WRONLY as c_int;
pub const O_RDWR: c_int = scarlet_abi::fs::VFS_O_RDWR as c_int;
pub const O_CREAT: c_int = scarlet_abi::fs::VFS_O_CREAT as c_int;
pub const O_EXCL: c_int = scarlet_abi::fs::VFS_O_EXCL as c_int;
pub const O_TRUNC: c_int = scarlet_abi::fs::VFS_O_TRUNC as c_int;
pub const O_APPEND: c_int = scarlet_abi::fs::VFS_O_APPEND as c_int;
pub const O_DIRECTORY: c_int = scarlet_abi::fs::VFS_O_DIRECTORY as c_int;
pub const O_NOFOLLOW: c_int = scarlet_abi::fs::VFS_O_NOFOLLOW as c_int;
pub const O_CLOEXEC: c_int = scarlet_abi::fs::VFS_O_CLOEXEC as c_int;
pub const F_GETFD: c_int = 1;
pub const F_SETFD: c_int = 2;
pub const F_GETFL: c_int = 3;
pub const F_SETFL: c_int = 4;
pub const FD_CLOEXEC: c_int = 1;

#[cfg(any(test, target_os = "scarlet"))]
fn transfer_arguments(fd: c_int, null_buffer: bool, count: usize) -> Result<(), c_int> {
    if fd < 0 {
        Err(ERRNO_EBADF)
    } else if count > isize::MAX as usize {
        Err(ERRNO_EINVAL)
    } else if null_buffer && count != 0 {
        Err(ERRNO_EFAULT)
    } else {
        Ok(())
    }
}

/// Decode only the additive syscalls, whose errors are always negative errno.
#[cfg(any(test, target_os = "scarlet"))]
fn result(value: usize) -> Result<usize, c_int> {
    if value > isize::MAX as usize {
        Err((value as isize).wrapping_neg() as c_int)
    } else {
        Ok(value)
    }
}

#[cfg(target_os = "scarlet")]
fn descriptor_result(value: usize) -> c_int {
    match result(value) {
        Ok(value) => value as c_int,
        Err(error) => crate::fail(error),
    }
}

/// Open a C pathname with an explicit mode (also used by stdio).
///
/// # Safety
/// `path` must reference a NUL-terminated readable C string.
#[cfg(target_os = "scarlet")]
pub(crate) unsafe fn open_impl(path: *const c_char, flags: c_int, mode: u32) -> c_int {
    // SAFETY: the path contract is forwarded from the caller.
    unsafe { openat_impl(crate::AT_FDCWD, path, flags, mode) }
}

#[cfg(target_os = "scarlet")]
unsafe fn openat_impl(fd: c_int, path: *const c_char, flags: c_int, mode: u32) -> c_int {
    if path.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }
    // An absolute pathname ignores fd. Preserve an invalid negative fd as an
    // out-of-range handle instead of accidentally turning -1 into cwd.
    let base = if fd == crate::AT_FDCWD {
        CURRENT_DIRECTORY
    } else {
        fd as u32 as usize
    };
    // SAFETY: the caller supplies path; remaining arguments are ABI scalars.
    descriptor_result(unsafe {
        scarlet_sys::syscall4(
            Syscall::VfsOpenAt,
            base,
            path as usize,
            flags as usize,
            mode as usize,
        )
    })
}

/// # Safety
/// `path` must be a C string. With O_CREAT, a promoted mode_t argument is required.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn open(path: *const c_char, flags: c_int, mut args: ...) -> c_int {
    let mode = if flags & O_CREAT != 0 {
        // SAFETY: O_CREAT requires a mode_t (unsigned int) variadic argument.
        unsafe { args.arg::<c_uint>() }
    } else {
        0
    };
    // SAFETY: forwarded from this function's caller.
    unsafe { open_impl(path, flags, mode) }
}

/// # Safety
/// `path` must be a C string. With O_CREAT, a promoted mode_t argument is required.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn openat(
    fd: c_int,
    path: *const c_char,
    flags: c_int,
    mut args: ...
) -> c_int {
    let mode = if flags & O_CREAT != 0 {
        // SAFETY: O_CREAT requires a mode_t (unsigned int) variadic argument.
        unsafe { args.arg::<c_uint>() }
    } else {
        0
    };
    // SAFETY: forwarded from this function's caller.
    unsafe { openat_impl(fd, path, flags, mode) }
}

/// # Safety
/// `path` must point to a NUL-terminated readable string.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn creat(path: *const c_char, mode: u32) -> c_int {
    // SAFETY: forwarded from this function's caller.
    unsafe { open_impl(path, O_WRONLY | O_CREAT | O_TRUNC, mode) }
}

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn close(fd: c_int) -> c_int {
    if fd < 0 {
        return crate::fail(ERRNO_EBADF);
    }
    // SAFETY: closing the C descriptor transfers no Rust-owned resource.
    descriptor_result(unsafe { scarlet_sys::syscall1(Syscall::HandleCloseWithStatus, fd as usize) })
}

/// # Safety
/// `buffer` must be writable for count bytes (unless count is zero).
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn read(fd: c_int, buffer: *mut c_void, count: usize) -> isize {
    if let Err(error) = transfer_arguments(fd, buffer.is_null(), count) {
        return crate::fail(error) as isize;
    }
    // SAFETY: the C caller owns the buffer and supplies its valid length.
    match result(unsafe {
        scarlet_sys::syscall3(
            Syscall::StreamReadWithStatus,
            fd as usize,
            buffer as usize,
            count,
        )
    }) {
        Ok(count) => count as isize,
        Err(error) => crate::fail(error) as isize,
    }
}

/// # Safety
/// `buffer` must be readable for count bytes (unless count is zero).
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize {
    if let Err(error) = transfer_arguments(fd, buffer.is_null(), count) {
        return crate::fail(error) as isize;
    }
    // SAFETY: the C caller owns the buffer and supplies its valid length.
    match result(unsafe {
        scarlet_sys::syscall3(
            Syscall::StreamWriteWithStatus,
            fd as usize,
            buffer as usize,
            count,
        )
    }) {
        Ok(count) => count as isize,
        Err(error) => crate::fail(error) as isize,
    }
}

#[cfg(any(test, target_os = "scarlet"))]
fn positioned_arguments(
    fd: c_int,
    null_buffer: bool,
    count: usize,
    offset: i64,
) -> Result<(), c_int> {
    transfer_arguments(fd, null_buffer, count)?;
    if offset < 0 {
        return Err(ERRNO_EINVAL);
    }
    if count as u64 > (i64::MAX - offset) as u64 {
        return Err(scarlet_abi::fs::ERRNO_EOVERFLOW);
    }
    Ok(())
}

/// Read at an absolute offset without changing the shared descriptor cursor.
///
/// # Safety
/// `buffer` must be writable for `count` bytes unless count is zero.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pread(fd: c_int, buffer: *mut c_void, count: usize, offset: i64) -> isize {
    if let Err(error) = positioned_arguments(fd, buffer.is_null(), count, offset) {
        return crate::fail(error) as isize;
    }
    // SAFETY: the C caller supplies a writable buffer; offset is nonnegative.
    match result(unsafe {
        scarlet_sys::syscall4(
            Syscall::FileReadAtWithStatus,
            fd as usize,
            buffer as usize,
            count,
            offset as usize,
        )
    }) {
        Ok(count) => count as isize,
        Err(error) => crate::fail(error) as isize,
    }
}

/// Write at an absolute offset, ignoring O_APPEND and preserving the cursor.
///
/// # Safety
/// `buffer` must be readable for `count` bytes unless count is zero.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pwrite(
    fd: c_int,
    buffer: *const c_void,
    count: usize,
    offset: i64,
) -> isize {
    if let Err(error) = positioned_arguments(fd, buffer.is_null(), count, offset) {
        return crate::fail(error) as isize;
    }
    // SAFETY: the C caller supplies a readable buffer; offset is nonnegative.
    match result(unsafe {
        scarlet_sys::syscall4(
            Syscall::FileWriteAtWithStatus,
            fd as usize,
            buffer as usize,
            count,
            offset as usize,
        )
    }) {
        Ok(count) => count as isize,
        Err(error) => crate::fail(error) as isize,
    }
}

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn ftruncate(fd: c_int, length: i64) -> c_int {
    if fd < 0 {
        return crate::fail(ERRNO_EBADF);
    }
    if length < 0 {
        return crate::fail(ERRNO_EINVAL);
    }
    // SAFETY: this syscall takes only scalars and retains ownership of fd.
    descriptor_result(unsafe {
        scarlet_sys::syscall2(
            Syscall::FileTruncateWithStatus,
            fd as usize,
            length as usize,
        )
    })
}

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn flock(fd: c_int, operation: c_int) -> c_int {
    if fd < 0 {
        return crate::fail(ERRNO_EBADF);
    }
    // SAFETY: the kernel validates the scalar operation and descriptor.
    descriptor_result(unsafe {
        scarlet_sys::syscall2(Syscall::FileLock, fd as usize, operation as usize)
    })
}

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn lseek(fd: c_int, offset: i64, whence: c_int) -> i64 {
    if fd < 0 {
        return crate::fail(ERRNO_EBADF) as i64;
    }
    if !(0..=2).contains(&whence) || (whence == 0 && offset < 0) {
        return crate::fail(ERRNO_EINVAL) as i64;
    }
    let mut position = 0u64;
    // SAFETY: the result is exclusively writable for the duration of the call.
    // The crate supports only 64-bit C ABIs, so offset occupies one register.
    let status = unsafe {
        scarlet_sys::syscall4(
            Syscall::FileSeekWithStatus,
            fd as usize,
            offset as usize,
            whence as usize,
            &mut position as *mut _ as usize,
        )
    };
    match result(status) {
        Ok(_) if position <= i64::MAX as u64 => position as i64,
        Ok(_) => crate::fail(scarlet_abi::fs::ERRNO_EOVERFLOW) as i64,
        Err(error) => crate::fail(error) as i64,
    }
}

#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub extern "C" fn dup(fd: c_int) -> c_int {
    if fd < 0 {
        return crate::fail(ERRNO_EBADF);
    }
    // SAFETY: the new handle is returned to C; Rust does not own either one.
    descriptor_result(unsafe {
        scarlet_sys::syscall1(Syscall::HandleDuplicateWithStatus, fd as usize)
    })
}

#[cfg(target_os = "scarlet")]
fn descriptor_flags(fd: c_int) -> Result<c_int, c_int> {
    if fd < 0 {
        return Err(ERRNO_EBADF);
    }
    // SAFETY: this query has no pointer arguments or ownership effects.
    result(unsafe { scarlet_sys::syscall1(Syscall::HandleGetFlags, fd as usize) })
        .map(|flags| flags as c_int)
}

/// Query the access mode without changing errno (fdopen reports failures).
#[cfg(target_os = "scarlet")]
pub(crate) fn descriptor_access(fd: c_int) -> Result<c_int, c_int> {
    descriptor_flags(fd).map(|flags| flags & O_ACCMODE)
}

/// Enable append on the shared open-file description without changing errno.
#[cfg(target_os = "scarlet")]
pub(crate) fn set_append(fd: c_int) -> Result<(), c_int> {
    let flags = descriptor_flags(fd)?;
    // SAFETY: flags are scalar ABI values from the same descriptor query.
    result(unsafe {
        scarlet_sys::syscall2(
            Syscall::HandleSetFlags,
            fd as usize,
            (flags | O_APPEND) as usize,
        )
    })
    .map(|_| ())
}

/// Query descriptor/access flags or set close-on-exec / shared append state.
///
/// # Safety
/// F_SETFD and F_SETFL require one promoted int argument. Other commands take
/// no argument. This initial implementation supports the four commands above.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fcntl(fd: c_int, command: c_int, mut args: ...) -> c_int {
    if fd < 0 {
        return crate::fail(ERRNO_EBADF);
    }
    match command {
        F_GETFD => {
            // SAFETY: the query has no pointers or ownership effects.
            descriptor_result(unsafe {
                scarlet_sys::syscall1(Syscall::HandleGetDescriptorFlags, fd as usize)
            })
        }
        F_SETFD => {
            // SAFETY: this command requires one promoted int argument.
            let flags = unsafe { args.arg::<c_int>() } & FD_CLOEXEC;
            // SAFETY: the syscall consumes only scalar descriptor flags.
            descriptor_result(unsafe {
                scarlet_sys::syscall2(
                    Syscall::HandleSetDescriptorFlags,
                    fd as usize,
                    flags as usize,
                )
            })
        }
        F_GETFL => match descriptor_flags(fd) {
            Ok(flags) => flags,
            Err(error) => crate::fail(error),
        },
        F_SETFL => {
            // SAFETY: this command requires one promoted int argument.
            let requested = unsafe { args.arg::<c_int>() };
            if requested & !(O_ACCMODE | O_APPEND) != 0 {
                return crate::fail(ERRNO_EINVAL);
            }
            // The access mode is immutable; F_SETFL ignores the requested
            // access bits and preserves the existing open-file description.
            let access = match descriptor_access(fd) {
                Ok(access) => access,
                Err(error) => return crate::fail(error),
            };
            // SAFETY: the flags are scalar ABI values; the descriptor remains open.
            descriptor_result(unsafe {
                scarlet_sys::syscall2(
                    Syscall::HandleSetFlags,
                    fd as usize,
                    (access | (requested & O_APPEND)) as usize,
                )
            })
        }
        _ => crate::fail(ERRNO_EINVAL),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positioned_ranges_reject_negative_and_overflow_without_wrapping() {
        assert_eq!(positioned_arguments(1, true, 0, i64::MAX), Ok(()));
        assert_eq!(positioned_arguments(1, false, 2, i64::MAX - 2), Ok(()));
        assert_eq!(
            positioned_arguments(1, false, 3, i64::MAX - 2),
            Err(scarlet_abi::fs::ERRNO_EOVERFLOW)
        );
        assert_eq!(positioned_arguments(1, false, 0, -1), Err(ERRNO_EINVAL));
        assert_eq!(positioned_arguments(-1, true, 0, 0), Err(ERRNO_EBADF));
    }

    #[test]
    fn transfer_boundaries_allow_zero_length_and_reject_unrepresentable_counts() {
        assert_eq!(transfer_arguments(0, true, 0), Ok(()));
        assert_eq!(transfer_arguments(0, false, isize::MAX as usize), Ok(()));
        assert_eq!(
            transfer_arguments(0, false, isize::MAX as usize + 1),
            Err(ERRNO_EINVAL)
        );
        assert_eq!(transfer_arguments(0, true, 1), Err(ERRNO_EFAULT));
        assert_eq!(transfer_arguments(-1, false, 1), Err(ERRNO_EBADF));
        assert_eq!(transfer_arguments(c_int::MIN, false, 0), Err(ERRNO_EBADF));
    }

    #[test]
    fn status_results_preserve_byte_counts_and_distinct_errors() {
        for count in [0, 1, 4096, isize::MAX as usize] {
            assert_eq!(result(count), Ok(count));
        }
        for error in [
            ERRNO_EBADF,
            ERRNO_EINVAL,
            ERRNO_EFAULT,
            scarlet_abi::ERRNO_EINTR,
        ] {
            assert_eq!(result((-(error as isize)) as usize), Err(error));
        }
    }
}
