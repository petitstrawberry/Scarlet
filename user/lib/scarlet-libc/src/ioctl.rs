//! Minimal C ioctl translation for Native socket controls.

use std::ffi::{c_int, c_ulong};

use scarlet_abi::{ERRNO_EBADF, ERRNO_EIO, SCTL_SOCKET_SET_NONBLOCK, Syscall, fs::ERRNO_EFAULT};

const FIONBIO: c_ulong = 0x5421;
const ENOTTY: c_int = 25;

/// # Safety
/// FIONBIO requires a readable pointer to int in the variadic argument list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ioctl(fd: c_int, request: c_ulong, mut args: ...) -> c_int {
    if fd < 0 {
        return crate::fail(ERRNO_EBADF);
    }
    if request != FIONBIO {
        return crate::fail(ENOTTY);
    }
    // SAFETY: FIONBIO's C contract requires a pointer to int argument.
    let value = unsafe { args.arg::<*const c_int>() };
    if value.is_null() {
        return crate::fail(ERRNO_EFAULT);
    }
    // SAFETY: The caller supplies a readable int at value.
    let enabled = unsafe { *value != 0 };
    // SAFETY: Socket control accepts a raw descriptor and a scalar boolean.
    let result = unsafe {
        scarlet_sys::syscall3(
            Syscall::HandleControl,
            fd as usize,
            SCTL_SOCKET_SET_NONBLOCK as usize,
            usize::from(enabled),
        )
    };
    if result == usize::MAX {
        crate::fail(ERRNO_EIO)
    } else {
        0
    }
}
