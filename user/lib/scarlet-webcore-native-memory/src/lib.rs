//! Native Scarlet object ownership for the WebCore CPU port.
//!
//! Opaque pointers own Box<Handle>, never libc descriptors. The C++ RAII wrapper
//! guarantees pointer provenance/liveness. No raw native handle is exposed to
//! callers and no kernel object is adopted twice.
use scarlet_os::handle::Handle;
use scarlet_os::handle::capability::memory_mapping::{flags, munmap, prot};
use scarlet_os::ipc::{SharedMemory, permissions};
use std::ffi::c_void;

#[unsafe(no_mangle)]
pub extern "C" fn scarlet_webcore_memory_create(size: usize) -> *mut c_void {
    if size == 0 || size > isize::MAX as usize {
        return std::ptr::null_mut();
    }
    match SharedMemory::create(size, permissions::READ_WRITE) {
        Ok(memory) => Box::into_raw(Box::new(memory.into_handle())).cast(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// The pointer must be a live handle owned by this bridge; caller retains it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scarlet_webcore_memory_duplicate(handle: *const c_void) -> *mut c_void {
    let Some(handle) = (unsafe { handle.cast::<Handle>().as_ref() }) else {
        return std::ptr::null_mut();
    };
    match handle.duplicate() {
        Ok(duplicate) => Box::into_raw(Box::new(duplicate)).cast(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Consumes this bridge's opaque pointer exactly once, even on close failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scarlet_webcore_memory_close(handle: *mut c_void) -> i32 {
    if handle.is_null() {
        return 0;
    }
    let handle = unsafe { Box::from_raw(handle.cast::<Handle>()) };
    match (*handle).close() {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Maps a live native shared-memory object. Ownership of the handle is retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scarlet_webcore_memory_map(handle: *const c_void, size: usize, writable: bool) -> *mut c_void {
    if size == 0 || size > isize::MAX as usize {
        return std::ptr::null_mut();
    }
    let Some(handle) = (unsafe { handle.cast::<Handle>().as_ref() }) else {
        return std::ptr::null_mut();
    };
    let Ok(mapping) = handle.as_memory_mapping() else {
        return std::ptr::null_mut();
    };
    let protection = if writable { prot::READ | prot::WRITE } else { prot::READ };
    match unsafe { mapping.mmap(0, size, protection, flags::SHARED, 0) } {
        Ok(address) => address as *mut c_void,
        Err(_) => std::ptr::null_mut(),
    }
}

/// The caller owns the entire mapping and has ended all uses of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scarlet_webcore_memory_unmap(address: *mut c_void, size: usize) -> i32 {
    if address.is_null() || size == 0 {
        return -1;
    }
    match unsafe { munmap(address as usize, size) } {
        Ok(()) => 0,
        Err(_) => -1,
    }
}
