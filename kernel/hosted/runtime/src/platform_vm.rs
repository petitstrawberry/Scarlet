//! Host VM operations used by executable publication.

use std::{ffi::c_void, io, ptr};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    static mach_task_self_: libc::mach_port_t;
    fn mach_vm_allocate(
        task: libc::mach_port_t,
        address: *mut u64,
        length: u64,
        flags: libc::c_int,
    ) -> libc::kern_return_t;
    fn mach_vm_protect(
        task: libc::mach_port_t,
        address: u64,
        length: u64,
        set_maximum: libc::c_int,
        protection: libc::c_int,
    ) -> libc::kern_return_t;
}

#[cfg(target_os = "macos")]
pub(super) fn reserve(address: usize, len: usize) -> io::Result<*mut c_void> {
    let mut reserved = address as u64;
    // FIXED without OVERWRITE cannot clobber the worker's existing VM.
    let result = unsafe { mach_vm_allocate(mach_task_self_, &mut reserved, len as u64, 0) };
    if result != libc::KERN_SUCCESS {
        return Err(io::Error::other(format!(
            "Cannot reserve guest range at {address:#x}: Mach error {result}"
        )));
    }
    Ok(ptr::with_exposed_provenance_mut(reserved as usize))
}

#[cfg(target_os = "linux")]
pub(super) fn reserve(address: usize, len: usize) -> io::Result<*mut c_void> {
    let requested = ptr::with_exposed_provenance_mut(address);
    let mapped = unsafe {
        libc::mmap(
            requested,
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_FIXED_NOREPLACE,
            -1,
            0,
        )
    };
    if mapped == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    if mapped != requested {
        // Older kernels can silently ignore MAP_FIXED_NOREPLACE.
        unsafe { libc::munmap(mapped, len) };
        return Err(io::Error::other("Cannot reserve the exact guest address"));
    }
    Ok(mapped)
}

#[cfg(target_os = "macos")]
pub(super) fn limit_permissions(base: *mut c_void, len: usize, native: i32) -> io::Result<()> {
    let result = unsafe { mach_vm_protect(mach_task_self_, base as u64, len as u64, 1, native) };
    if result != libc::KERN_SUCCESS {
        return Err(io::Error::other(format!(
            "Cannot bound guest mapping permissions: Mach error {result}"
        )));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn limit_permissions(_: *mut c_void, _: usize, _: i32) -> io::Result<()> {
    // Linux's mprotect changes the current permissions. Worker code owns
    // subsequent changes through the mapping coordinator.
    Ok(())
}

/// Synchronize code written to a host mapping before native execution.
///
/// # Safety
/// The range must belong to an accessible mapping.
pub unsafe fn synchronize_instructions(start: *const c_void, len: usize) {
    unsafe extern "C" {
        #[cfg(target_os = "macos")]
        fn sys_icache_invalidate(start: *const c_void, len: usize);
        #[cfg(target_os = "linux")]
        fn __clear_cache(start: *const c_void, end: *const c_void);
    }
    #[cfg(target_os = "macos")]
    unsafe {
        sys_icache_invalidate(start, len)
    };
    #[cfg(target_os = "linux")]
    unsafe {
        __clear_cache(start, start.byte_add(len))
    };
}
