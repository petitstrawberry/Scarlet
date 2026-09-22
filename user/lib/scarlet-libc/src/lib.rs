//! Initial C/POSIX adapters for the Scarlet Native ABI.
//!
//! This is an incremental libc, not yet a complete C library for Cargo's C
//! dependencies. It uses Scarlet Rust std for allocation and its native TLS ABI for errno;
//! link one copy into a native 64-bit executable using the matching Rust CRT.

#![deny(unsafe_op_in_unsafe_fn)]
#![feature(c_variadic)]
// These exports implement libc. LLVM must not infer builtin allocation
// contracts for their Rust callers (e.g. fold overflowing calloc to non-null).
#![no_builtins]

#[cfg(target_os = "scarlet")]
use std::ffi::c_char;
use std::ffi::{c_int, c_long};

#[cfg(any(test, target_os = "scarlet"))]
use scarlet_abi::ERRNO_EINVAL;
#[cfg(target_os = "scarlet")]
use scarlet_abi::Syscall;
#[cfg(any(test, target_os = "scarlet"))]
use scarlet_abi::fs::*;

pub mod algorithms;
pub mod allocation;
pub mod conversion;
pub mod descriptor;
mod errno;
#[cfg(any(test, target_os = "scarlet"))]
mod formatting;
pub mod path;
pub mod runtime;
#[cfg(any(test, target_os = "scarlet"))]
pub mod stdio;
pub mod strings;
pub use errno::__errno_location;

#[cfg(all(target_os = "scarlet", not(target_pointer_width = "64")))]
compile_error!("the initial Scarlet C ABI supports AArch64 and RV64 only");

pub(crate) fn fail(errno: c_int) -> c_int {
    // SAFETY: only this thread can access its errno cell.
    unsafe { *__errno_location() = errno };
    -1
}

#[cfg(target_os = "scarlet")]
fn status(value: usize) -> c_int {
    if value > isize::MAX as usize {
        fail((value as isize).wrapping_neg() as c_int)
    } else {
        0
    }
}

/// Resolve an existing path, optionally allocating the result with malloc.
///
/// # Safety
/// path must point to a NUL-terminated string. A non-null output must hold
/// PATH_MAX bytes and be exclusively writable for the duration of the call.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
#[cfg(target_os = "scarlet")]
pub unsafe extern "C" fn realpath(path: *const c_char, output: *mut c_char) -> *mut c_char {
    if path.is_null() {
        fail(ERRNO_EINVAL);
        return std::ptr::null_mut();
    }
    let mut buffer = [0u8; PATH_MAX];
    // SAFETY: caller supplies path; buffer is live and exclusively writable.
    let len = unsafe {
        scarlet_sys::syscall3(
            Syscall::VfsCanonicalize,
            path as usize,
            buffer.as_mut_ptr() as usize,
            buffer.len() - 1,
        )
    };
    if len > isize::MAX as usize {
        status(len);
        return std::ptr::null_mut();
    }
    if len == 0 || len >= buffer.len() {
        fail(scarlet_abi::ERRNO_EIO);
        return std::ptr::null_mut();
    }
    let output = if output.is_null() {
        allocation::malloc(len + 1).cast::<c_char>()
    } else {
        output
    };
    if output.is_null() {
        return output;
    }
    // SAFETY: malloc returned len+1 bytes, or the caller provided PATH_MAX.
    unsafe { std::ptr::copy_nonoverlapping(buffer.as_ptr(), output.cast::<u8>(), len + 1) };
    output
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: c_long,
}

pub const UTIME_NOW: c_long = 1_073_741_823;
pub const UTIME_OMIT: c_long = 1_073_741_822;
pub const AT_FDCWD: c_int = -100;
pub const AT_SYMLINK_NOFOLLOW: c_int = 0x100;

#[cfg(any(test, target_os = "scarlet"))]
fn timestamp(value: Timespec, now: u64) -> Result<Option<u64>, c_int> {
    match value.tv_nsec {
        UTIME_OMIT => Ok(None),
        UTIME_NOW => Ok(Some(now)),
        0..1_000_000_000 => u64::try_from(value.tv_sec)
            .map(Some)
            .map_err(|_| ERRNO_EOVERFLOW),
        _ => Err(ERRNO_EINVAL),
    }
}

#[cfg(target_os = "scarlet")]
unsafe fn file_times(times: *const Timespec) -> Result<RawFileTimes, c_int> {
    let pair = if times.is_null() {
        [Timespec {
            tv_sec: 0,
            tv_nsec: UTIME_NOW,
        }; 2]
    } else {
        // SAFETY: the C caller provides two valid, aligned timespec records.
        unsafe { [*times, *times.add(1)] }
    };
    let now = if pair.iter().any(|time| time.tv_nsec == UTIME_NOW) {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| ERRNO_EOVERFLOW)?
            .as_secs()
    } else {
        0
    };
    let accessed = timestamp(pair[0], now)?;
    let modified = timestamp(pair[1], now)?;
    Ok(RawFileTimes {
        version: FILE_TIMES_VERSION,
        flags: if accessed.is_some() {
            FILE_TIMES_ACCESSED
        } else {
            0
        } | if modified.is_some() {
            FILE_TIMES_MODIFIED
        } else {
            0
        },
        accessed: accessed.unwrap_or(0),
        modified: modified.unwrap_or(0),
    })
}

/// # Safety
/// A non-null times pointer must address two valid Timespec records.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
#[cfg(target_os = "scarlet")]
pub unsafe extern "C" fn futimens(fd: c_int, times: *const Timespec) -> c_int {
    // SAFETY: forwarded from the C API contract.
    let times = match unsafe { file_times(times) } {
        Ok(times) => times,
        Err(errno) => return fail(errno),
    };
    // SAFETY: times is a live input record. This operation never closes fd.
    status(unsafe {
        scarlet_sys::syscall2(
            Syscall::FileSetTimes,
            fd as usize,
            &times as *const _ as usize,
        )
    })
}

/// # Safety
/// path must be NUL-terminated; non-null times must address two Timespec records.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
#[cfg(target_os = "scarlet")]
pub unsafe extern "C" fn utimensat(
    fd: c_int,
    path: *const c_char,
    times: *const Timespec,
    flags: c_int,
) -> c_int {
    if flags & !AT_SYMLINK_NOFOLLOW != 0 {
        return fail(ERRNO_EINVAL);
    }
    if path.is_null() {
        return fail(ERRNO_EFAULT);
    }
    // SAFETY: forwarded from the C API contract.
    let times = match unsafe { file_times(times) } {
        Ok(times) => times,
        Err(errno) => return fail(errno),
    };
    // Do not sign-extend fd=-1 into the Native cwd sentinel. The C ABI is
    // 64-bit; invalid negative descriptors remain out-of-range u32 handles.
    let base = if fd == AT_FDCWD {
        CURRENT_DIRECTORY
    } else {
        fd as u32 as usize
    };
    // SAFETY: times is a live input record; path is supplied by the C caller.
    status(unsafe {
        scarlet_sys::syscall4(
            Syscall::VfsSetTimes,
            path as usize,
            &times as *const _ as usize,
            if flags & AT_SYMLINK_NOFOLLOW != 0 {
                FILE_TIMES_NOFOLLOW
            } else {
                0
            },
            base,
        )
    })
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
#[cfg(target_os = "scarlet")]
pub extern "C" fn fsync(fd: c_int) -> c_int {
    // SAFETY: FileSync only flushes an existing descriptor; no ownership transfer.
    status(unsafe { scarlet_sys::syscall1(Syscall::FileSync, fd as usize) })
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
#[cfg(target_os = "scarlet")]
pub extern "C" fn fdatasync(fd: c_int) -> c_int {
    fsync(fd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_times_ignore_seconds_and_validate_nanoseconds() {
        assert_eq!(
            timestamp(
                Timespec {
                    tv_sec: -1,
                    tv_nsec: UTIME_OMIT
                },
                42
            ),
            Ok(None)
        );
        assert_eq!(
            timestamp(
                Timespec {
                    tv_sec: -1,
                    tv_nsec: UTIME_NOW
                },
                42
            ),
            Ok(Some(42))
        );
        assert_eq!(
            timestamp(
                Timespec {
                    tv_sec: 5,
                    tv_nsec: 999_999_999
                },
                42
            ),
            Ok(Some(5))
        );
        assert_eq!(
            timestamp(
                Timespec {
                    tv_sec: 5,
                    tv_nsec: 1_000_000_000
                },
                42
            ),
            Err(ERRNO_EINVAL)
        );
        assert_eq!(
            timestamp(
                Timespec {
                    tv_sec: -1,
                    tv_nsec: 0
                },
                42
            ),
            Err(ERRNO_EOVERFLOW)
        );
    }

    #[test]
    fn errno_is_independent_between_threads() {
        fail(22);
        std::thread::spawn(|| {
            fail(40);
            assert_eq!(unsafe { *__errno_location() }, 40);
        })
        .join()
        .unwrap();
        assert_eq!(unsafe { *__errno_location() }, 22);
    }
}
