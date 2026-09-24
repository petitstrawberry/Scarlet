//! C pathname operations. Rust std supplies the working-directory backend;
//! removal uses the Native ABI's atomic, type-checked directory-entry operation.

#[cfg(any(test, target_os = "scarlet"))]
use std::ffi::{c_char, c_int};

#[cfg(target_os = "scarlet")]
unsafe fn remove_entry(path: *const c_char, directory: bool) -> c_int {
    if path.is_null() {
        return crate::fail(scarlet_abi::fs::ERRNO_EFAULT);
    }
    // SAFETY: the C caller supplies the readable NUL-terminated pathname.
    crate::status(unsafe {
        scarlet_sys::syscall2(
            scarlet_abi::Syscall::VfsRemoveWithStatus,
            path as usize,
            usize::from(directory),
        )
    })
}

/// # Safety
/// `path` must reference a readable NUL-terminated C string.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn unlink(path: *const c_char) -> c_int {
    // SAFETY: forwarded from the caller.
    unsafe { remove_entry(path, false) }
}

/// # Safety
/// `path` must reference a readable NUL-terminated C string.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rmdir(path: *const c_char) -> c_int {
    // SAFETY: forwarded from the caller.
    unsafe { remove_entry(path, true) }
}

/// # Safety
/// Both paths must be readable NUL-terminated strings.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn symlink(target: *const c_char, link_path: *const c_char) -> c_int {
    if target.is_null() || link_path.is_null() {
        return crate::fail(scarlet_abi::fs::ERRNO_EFAULT);
    }
    // Native takes the new pathname before the symlink target.
    crate::status(unsafe {
        scarlet_sys::syscall2(
            scarlet_abi::Syscall::VfsCreateSymlink,
            link_path as usize,
            target as usize,
        )
    })
}

/// # Safety
/// Both paths must be readable NUL-terminated strings.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn link(old: *const c_char, new: *const c_char) -> c_int {
    if old.is_null() || new.is_null() {
        return crate::fail(scarlet_abi::fs::ERRNO_EFAULT);
    }
    crate::status(unsafe {
        scarlet_sys::syscall2(
            scarlet_abi::Syscall::VfsCreateHardlink,
            old as usize,
            new as usize,
        )
    })
}

/// Remove a file, symlink, or empty directory.
///
/// # Safety
/// `path` must reference a readable NUL-terminated C string.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn remove(path: *const c_char) -> c_int {
    if path.is_null() {
        return crate::fail(scarlet_abi::fs::ERRNO_EFAULT);
    }
    let mut metadata = std::mem::MaybeUninit::<crate::stat::Stat>::uninit();
    // SAFETY: path and metadata satisfy lstat's C contract.
    if unsafe { crate::stat::lstat(path, metadata.as_mut_ptr()) } != 0 {
        return -1;
    }
    // SAFETY: lstat initialized the record on success.
    let metadata = unsafe { metadata.assume_init() };
    if metadata.mode & 0o170000 == 0o040000 {
        // SAFETY: the path contract is forwarded.
        unsafe { rmdir(path) }
    } else {
        // SAFETY: the path contract is forwarded.
        unsafe { unlink(path) }
    }
}

/// # Safety
/// Both paths must be readable NUL-terminated strings.
#[cfg(target_os = "scarlet")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rename(old: *const c_char, new: *const c_char) -> c_int {
    use std::ffi::CStr;
    if old.is_null() || new.is_null() {
        return crate::fail(scarlet_abi::fs::ERRNO_EFAULT);
    }
    // SAFETY: both pointers satisfy the C pathname contract.
    let old = unsafe { CStr::from_ptr(old) };
    let new = unsafe { CStr::from_ptr(new) };
    let (Ok(old), Ok(new)) = (old.to_str(), new.to_str()) else {
        return crate::fail(scarlet_abi::ERRNO_EINVAL);
    };
    match std::fs::rename(old, new) {
        Ok(()) => 0,
        Err(error) => crate::fail(error.raw_os_error().unwrap_or(scarlet_abi::ERRNO_EIO)),
    }
}

#[cfg(any(test, target_os = "scarlet"))]
fn cwd_capacity(length: usize, allocate: bool, size: usize) -> Result<usize, c_int> {
    if !allocate && size == 0 {
        return Err(scarlet_abi::ERRNO_EINVAL);
    }
    let required = length.checked_add(1).ok_or(scarlet_abi::fs::ERRNO_ERANGE)?;
    let capacity = if allocate && size == 0 {
        required
    } else {
        size
    };
    if capacity < required {
        Err(scarlet_abi::fs::ERRNO_ERANGE)
    } else {
        Ok(capacity)
    }
}

/// Write the absolute working directory including NUL. A null buffer requests
/// a malloc-owned allocation; size zero then allocates the required size.
/// No bytes are written to an undersized caller buffer.
///
/// # Safety
/// A non-null `buffer` must be writable for `size` bytes. A successful allocated
/// result must be released with this library's free.
#[cfg(any(test, target_os = "scarlet"))]
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn getcwd(buffer: *mut c_char, size: usize) -> *mut c_char {
    // SAFETY: errno belongs to the current thread.
    let saved_errno = unsafe { *crate::__errno_location() };
    if !buffer.is_null() && size == 0 {
        crate::fail(scarlet_abi::ERRNO_EINVAL);
        return std::ptr::null_mut();
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            crate::fail(error.raw_os_error().unwrap_or(scarlet_abi::ERRNO_EIO));
            return std::ptr::null_mut();
        }
    };
    let bytes = cwd.as_os_str().as_encoded_bytes();
    let capacity = match cwd_capacity(bytes.len(), buffer.is_null(), size) {
        Ok(capacity) => capacity,
        Err(error) => {
            crate::fail(error);
            return std::ptr::null_mut();
        }
    };
    let output = if buffer.is_null() {
        crate::allocation::malloc(capacity).cast::<c_char>()
    } else {
        buffer
    };
    if !output.is_null() {
        // SAFETY: capacity validation or malloc provides bytes.len()+1 bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast::<u8>(), bytes.len());
            output.add(bytes.len()).write(0);
            *crate::__errno_location() = saved_errno;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn cwd_buffer_sizes_include_the_terminator() {
        assert_eq!(cwd_capacity(4, false, 5), Ok(5));
        assert_eq!(
            cwd_capacity(4, false, 4),
            Err(scarlet_abi::fs::ERRNO_ERANGE)
        );
        assert_eq!(cwd_capacity(4, false, 0), Err(scarlet_abi::ERRNO_EINVAL));
        assert_eq!(cwd_capacity(4, true, 0), Ok(5));
        assert_eq!(cwd_capacity(4, true, 7), Ok(7));
        assert_eq!(cwd_capacity(4, true, 4), Err(scarlet_abi::fs::ERRNO_ERANGE));
        assert_eq!(
            cwd_capacity(usize::MAX, true, 0),
            Err(scarlet_abi::fs::ERRNO_ERANGE)
        );
    }

    #[test]
    fn cwd_writes_only_after_validation_and_preserves_success_errno() {
        let expected = std::env::current_dir().unwrap();
        let expected = expected.as_os_str().as_encoded_bytes();
        let mut output = vec![42u8; expected.len() + 2];
        unsafe {
            let pointer = output.as_mut_ptr().cast();
            assert!(getcwd(pointer, expected.len()).is_null());
            assert_eq!(*crate::__errno_location(), scarlet_abi::fs::ERRNO_ERANGE);
            assert!(output.iter().all(|byte| *byte == 42));
            *crate::__errno_location() = scarlet_abi::ERRNO_EBADF;
            assert_eq!(getcwd(pointer, expected.len() + 1), pointer);
            assert_eq!(*crate::__errno_location(), scarlet_abi::ERRNO_EBADF);
            assert_eq!(CStr::from_ptr(pointer).to_bytes(), expected);
            assert_eq!(output[expected.len() + 1], 42);
            let allocated = getcwd(std::ptr::null_mut(), 0);
            assert!(!allocated.is_null());
            assert_eq!(CStr::from_ptr(allocated).to_bytes(), expected);
            assert_eq!(*crate::__errno_location(), scarlet_abi::ERRNO_EBADF);
            crate::allocation::free(allocated.cast());
        }
    }
}
