//! C allocations carry their Rust Layout in a private, aligned prefix.
use std::alloc::{Layout, alloc, dealloc};
use std::ffi::c_void;
use std::ptr;

const ALIGN: usize = 16;
const PREFIX: usize = 16;
const ENOMEM: i32 = 12;

fn layout(size: usize) -> Option<Layout> {
    Layout::from_size_align(size.max(1).checked_add(PREFIX)?, ALIGN).ok()
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn malloc(size: usize) -> *mut c_void {
    let Some(layout) = layout(size) else {
        super::fail(ENOMEM);
        return ptr::null_mut();
    };
    // SAFETY: layout is nonzero and valid; allocation is not yet exposed.
    unsafe {
        let base = alloc(layout);
        if base.is_null() {
            super::fail(ENOMEM);
            return ptr::null_mut();
        }
        base.cast::<usize>().write(size);
        base.add(PREFIX).cast()
    }
}

/// # Safety
/// ptr is null or a live allocation returned by this library's allocation API.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: the allocation contract guarantees the initialized private prefix.
    unsafe {
        let base = ptr.cast::<u8>().sub(PREFIX);
        let size = base.cast::<usize>().read();
        dealloc(base, layout(size).unwrap_unchecked());
    }
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn calloc(count: usize, size: usize) -> *mut c_void {
    let Some(size) = count.checked_mul(size) else {
        super::fail(ENOMEM);
        return ptr::null_mut();
    };
    let result = malloc(size);
    if !result.is_null() {
        // SAFETY: malloc returned at least size writable bytes.
        unsafe { ptr::write_bytes(result.cast::<u8>(), 0, size) };
    }
    result
}

/// # Safety
/// ptr is null or a live allocation returned by this library's allocation API.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    if ptr.is_null() {
        return malloc(size);
    }
    if size == 0 {
        // SAFETY: forwarded from the allocation contract.
        unsafe { free(ptr) };
        return ptr::null_mut();
    }
    let result = malloc(size);
    if result.is_null() {
        return result;
    }
    // SAFETY: distinct live allocations; copying preserves the common prefix.
    unsafe {
        let old_size = ptr.cast::<u8>().sub(PREFIX).cast::<usize>().read();
        ptr::copy_nonoverlapping(ptr.cast::<u8>(), result.cast::<u8>(), old_size.min(size));
        free(ptr);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocations_align_zero_and_preserve_data() {
        unsafe {
            let p = calloc(13, 3).cast::<u8>();
            assert!(!p.is_null());
            assert_eq!(p as usize % ALIGN, 0);
            assert_eq!(std::slice::from_raw_parts(p, 39), &[0; 39]);
            p.write(42);
            let p = realloc(p.cast(), 100).cast::<u8>();
            assert_eq!(p.read(), 42);
            assert!(realloc(p.cast(), usize::MAX).is_null());
            assert_eq!(p.read(), 42);
            free(p.cast());
            free(ptr::null_mut());
            assert!(calloc(usize::MAX, 2).is_null());
        }
    }
}
