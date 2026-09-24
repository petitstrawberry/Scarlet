//! File and anonymous mappings through Scarlet's Native VM syscalls.

use std::ffi::{c_int, c_void};

use scarlet_abi::{ERRNO_EBADF, ERRNO_EINVAL, ERRNO_EIO, Syscall};

const MAP_SHARED: c_int = 0x01;
const MAP_PRIVATE: c_int = 0x02;
const MAP_FIXED: c_int = 0x10;
const MAP_ANONYMOUS: c_int = 0x20;
const PAGE_SIZE: usize = 4096;

/// # Safety
/// On success, the returned region may be accessed according to `prot` until
/// it is unmapped. The caller must uphold the usual mmap ownership rules.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mmap(
    addr: *mut c_void,
    length: usize,
    prot: c_int,
    flags: c_int,
    fd: c_int,
    offset: i64,
) -> *mut c_void {
    let mapping_kind = flags & (MAP_SHARED | MAP_PRIVATE);
    let anonymous = flags & MAP_ANONYMOUS != 0;
    let valid_flags = MAP_SHARED | MAP_PRIVATE | MAP_FIXED | MAP_ANONYMOUS;
    if length == 0
        || length > usize::MAX - (PAGE_SIZE - 1)
        || mapping_kind != MAP_SHARED && mapping_kind != MAP_PRIVATE
        || flags & !valid_flags != 0
        || prot & !0x7 != 0
        || offset < 0
        || offset as usize % PAGE_SIZE != 0
        || flags & MAP_FIXED != 0 && (addr as usize) % PAGE_SIZE != 0
    {
        crate::fail(ERRNO_EINVAL);
        return usize::MAX as *mut c_void;
    }
    if !anonymous && fd < 0 {
        crate::fail(ERRNO_EBADF);
        return usize::MAX as *mut c_void;
    }
    if anonymous && (fd != -1 || offset != 0) {
        crate::fail(ERRNO_EINVAL);
        return usize::MAX as *mut c_void;
    }

    // SAFETY: Scalar arguments are validated above; the kernel owns and
    // validates the requested address and file handle.
    let result = unsafe {
        scarlet_sys::syscall6(
            Syscall::MemoryMap,
            if anonymous { 0 } else { fd as usize },
            addr as usize,
            length,
            prot as usize,
            flags as usize,
            offset as usize,
        )
    };
    if result == usize::MAX {
        crate::fail(ERRNO_EIO);
    }
    result as *mut c_void
}

/// # Safety
/// `addr` and `length` must describe a live mapping obtained from mmap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn munmap(addr: *mut c_void, length: usize) -> c_int {
    if addr.is_null() || (addr as usize) % PAGE_SIZE != 0 || length == 0 {
        return crate::fail(ERRNO_EINVAL);
    }
    // SAFETY: The kernel validates the mapping and range.
    let result = unsafe { scarlet_sys::syscall2(Syscall::MemoryUnmap, addr as usize, length) };
    if result == usize::MAX {
        crate::fail(ERRNO_EIO)
    } else {
        0
    }
}
