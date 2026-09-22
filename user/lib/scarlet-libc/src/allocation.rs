//! C allocations carry their original Rust Layout in a private prefix.
use std::alloc::{Layout, alloc, dealloc};
use std::ffi::{c_int, c_void};
use std::ptr;

use scarlet_abi::ERRNO_EINVAL;

const ALIGN: usize = 16;
const ERRNO_ENOMEM: c_int = 12;

#[derive(Clone, Copy)]
struct Header {
    layout: Layout,
    size: usize,
    offset: usize,
}

fn layout(size: usize, alignment: usize) -> Option<(Layout, usize)> {
    // Keep malloc's fundamental alignment even for weaker aligned_alloc
    // requests. A zero-sized request still owns a distinct, freeable block.
    let payload = Layout::from_size_align(size.max(1), alignment.max(ALIGN)).ok()?;
    Layout::new::<Header>().extend(payload).ok()
}

fn allocate(size: usize, alignment: usize) -> Result<*mut c_void, c_int> {
    let (layout, offset) = layout(size, alignment).ok_or(ERRNO_ENOMEM)?;
    // Keep the std backend one-way: it must not call these C exports. Preserve
    // errno even if the backend changes it; POSIX memalign reports errors in
    // its return value and successful allocations retain the previous error.
    let errno = crate::__errno_location();
    // SAFETY: errno belongs to this thread, layout is nonzero and valid, and
    // the new allocation is not yet exposed. The aligned payload has room
    // for an aligned Header immediately before it, including any padding.
    unsafe {
        let saved_errno = errno.read();
        let base = alloc(layout);
        errno.write(saved_errno);
        if base.is_null() {
            return Err(ERRNO_ENOMEM);
        }
        let result = base.add(offset);
        result.cast::<Header>().sub(1).write(Header {
            layout,
            size,
            offset,
        });
        Ok(result.cast())
    }
}

fn allocation_result(result: Result<*mut c_void, c_int>) -> *mut c_void {
    match result {
        Ok(ptr) => ptr,
        Err(errno) => {
            super::fail(errno);
            ptr::null_mut()
        }
    }
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn malloc(size: usize) -> *mut c_void {
    allocation_result(allocate(size, ALIGN))
}

/// Allocate at a nonzero power-of-two alignment, retaining fundamental
/// alignment. Size need not be a multiple of alignment.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn aligned_alloc(alignment: usize, size: usize) -> *mut c_void {
    if !alignment.is_power_of_two() {
        super::fail(ERRNO_EINVAL);
        return ptr::null_mut();
    }
    allocation_result(allocate(size, alignment))
}

/// Return an error number without modifying errno or memptr on failure.
///
/// # Safety
/// memptr must point to a writable, aligned pointer-sized object.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn posix_memalign(
    memptr: *mut *mut c_void,
    alignment: usize,
    size: usize,
) -> c_int {
    if !alignment.is_power_of_two() || alignment % size_of::<*mut c_void>() != 0 {
        return ERRNO_EINVAL;
    }
    match allocate(size, alignment) {
        Ok(ptr) => {
            // SAFETY: memptr is writable by the caller's contract. Only a
            // successful allocation replaces the caller's original pointer.
            unsafe { memptr.write(ptr) };
            0
        }
        Err(errno) => errno,
    }
}

/// # Safety
/// ptr is null or a live allocation returned by this library's allocation API.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    let errno = crate::__errno_location();
    // SAFETY: the allocation contract guarantees the initialized private
    // header, original base, and original Layout, also for aligned requests.
    unsafe {
        let header = ptr.cast::<Header>().sub(1).read();
        let base = ptr.cast::<u8>().sub(header.offset);
        let saved_errno = errno.read();
        dealloc(base, header.layout);
        errno.write(saved_errno);
    }
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn calloc(count: usize, size: usize) -> *mut c_void {
    let Some(size) = count.checked_mul(size) else {
        super::fail(ERRNO_ENOMEM);
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
    // Realloc guarantees fundamental alignment, not the old extended alignment.
    unsafe {
        let old_size = ptr.cast::<Header>().sub(1).read().size;
        ptr::copy_nonoverlapping(ptr.cast::<u8>(), result.cast::<u8>(), old_size.min(size));
        free(ptr);
    }
    result
}

/// # Safety
/// ptr is null or a live allocation returned by this library's allocation API.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn reallocarray(ptr: *mut c_void, count: usize, size: usize) -> *mut c_void {
    let Some(size) = count.checked_mul(size) else {
        super::fail(ERRNO_ENOMEM);
        return ptr::null_mut();
    };
    // SAFETY: forwarded from the allocation contract. Overflow returns before
    // realloc, leaving the caller's original allocation live and unchanged.
    unsafe { realloc(ptr, size) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errno() -> c_int {
        // SAFETY: errno is a live cell owned by this thread.
        unsafe { crate::__errno_location().read() }
    }

    fn set_errno(value: c_int) {
        // SAFETY: errno is a live cell owned by this thread.
        unsafe { crate::__errno_location().write(value) };
    }

    #[test]
    fn allocations_align_zero_and_preserve_data() {
        unsafe {
            set_errno(71);
            let p = calloc(13, 3).cast::<u8>();
            assert!(!p.is_null());
            assert_eq!(errno(), 71);
            assert_eq!(p as usize % ALIGN, 0);
            assert_eq!(std::slice::from_raw_parts(p, 39), &[0; 39]);
            p.write(42);
            let p = realloc(p.cast(), 100).cast::<u8>();
            assert!(!p.is_null());
            assert_eq!(p.read(), 42);
            assert_eq!(errno(), 71);
            assert!(realloc(p.cast(), usize::MAX).is_null());
            assert_eq!(errno(), ERRNO_ENOMEM);
            assert_eq!(p.read(), 42);
            free(p.cast());
            assert_eq!(errno(), ERRNO_ENOMEM);
            free(ptr::null_mut());
            assert_eq!(errno(), ERRNO_ENOMEM);
            set_errno(71);
            assert!(calloc(usize::MAX, 2).is_null());
            assert_eq!(errno(), ERRNO_ENOMEM);
        }
    }

    #[test]
    fn fundamental_and_extended_alignment_survive_free_and_realloc() {
        unsafe {
            for alignment in [1, 2, 4, 8, 16, 32, 64, 4096] {
                // Non-multiple sizes are permitted by the corrected C contract.
                for size in [1, alignment, alignment + 3] {
                    set_errno(72);
                    let p = aligned_alloc(alignment, size).cast::<u8>();
                    assert!(!p.is_null());
                    assert_eq!(p as usize % alignment.max(ALIGN), 0);
                    assert_eq!(errno(), 72);
                    for i in 0..size {
                        p.add(i).write(i as u8);
                    }
                    let grown = realloc(p.cast(), size + 137).cast::<u8>();
                    assert!(!grown.is_null());
                    assert_eq!(grown as usize % ALIGN, 0);
                    for i in 0..size {
                        assert_eq!(grown.add(i).read(), i as u8);
                    }
                    let shrunk = reallocarray(grown.cast(), 1, size).cast::<u8>();
                    assert!(!shrunk.is_null());
                    for i in 0..size {
                        assert_eq!(shrunk.add(i).read(), i as u8);
                    }
                    free(shrunk.cast());
                    assert_eq!(errno(), 72);
                    let reused = aligned_alloc(alignment, size);
                    assert!(!reused.is_null());
                    assert_eq!(reused as usize % alignment.max(ALIGN), 0);
                    free(reused);
                    assert_eq!(errno(), 72);
                }
            }
        }
    }

    #[test]
    fn invalid_alignment_and_impossible_layouts_report_errors() {
        for alignment in [0, 3, 7, 24, usize::MAX] {
            set_errno(73);
            assert!(aligned_alloc(alignment, 64).is_null());
            assert_eq!(errno(), ERRNO_EINVAL);
        }
        for (alignment, size) in [
            (1, usize::MAX),
            (16, isize::MAX as usize),
            (4096, isize::MAX as usize - 4095),
            (1usize << (usize::BITS - 1), 0),
        ] {
            set_errno(73);
            assert!(aligned_alloc(alignment, size).is_null());
            assert_eq!(errno(), ERRNO_ENOMEM);
        }
    }

    #[test]
    fn posix_memalign_preserves_errno_and_failure_output() {
        unsafe {
            let sentinel = malloc(1);
            assert!(!sentinel.is_null());
            for alignment in [0, 1, 2, 3, size_of::<*mut c_void>() / 2, 24, usize::MAX] {
                let mut output = sentinel;
                set_errno(74);
                assert_eq!(posix_memalign(&mut output, alignment, 31), ERRNO_EINVAL);
                assert_eq!(output, sentinel);
                assert_eq!(errno(), 74);
            }
            for (alignment, size) in [(64, usize::MAX), (1usize << (usize::BITS - 1), 1)] {
                let mut output = sentinel;
                set_errno(74);
                assert_eq!(posix_memalign(&mut output, alignment, size), ERRNO_ENOMEM);
                assert_eq!(output, sentinel);
                assert_eq!(errno(), 74);
            }
            for alignment in [size_of::<*mut c_void>(), 16, 64, 4096] {
                let mut output = sentinel;
                set_errno(74);
                assert_eq!(posix_memalign(&mut output, alignment, 31), 0);
                assert!(!output.is_null());
                assert_eq!(output as usize % alignment.max(ALIGN), 0);
                assert_eq!(errno(), 74);
                ptr::write_bytes(output.cast::<u8>(), 0xa5, 31);
                free(output);
                assert_eq!(errno(), 74);
            }
            free(sentinel);
        }
    }

    #[test]
    fn reallocarray_checks_multiplication_and_preserves_failed_input() {
        unsafe {
            let p = aligned_alloc(4096, 19).cast::<u8>();
            assert!(!p.is_null());
            ptr::write_bytes(p, 0xa5, 19);
            for (count, size) in [(usize::MAX, 2), (2, usize::MAX), (1, usize::MAX)] {
                set_errno(75);
                assert!(reallocarray(p.cast(), count, size).is_null());
                assert_eq!(errno(), ERRNO_ENOMEM);
                assert_eq!(std::slice::from_raw_parts(p, 19), &[0xa5; 19]);
            }
            let p = reallocarray(p.cast(), 31, 3).cast::<u8>();
            assert!(!p.is_null());
            assert_eq!(std::slice::from_raw_parts(p, 19), &[0xa5; 19]);
            free(p.cast());
            assert!(reallocarray(ptr::null_mut(), usize::MAX, 2).is_null());
            assert_eq!(errno(), ERRNO_ENOMEM);
            let p = reallocarray(ptr::null_mut(), 7, 3);
            assert!(!p.is_null());
            free(p);
        }
    }

    #[test]
    fn zero_size_allocations_are_freeable_and_reallocation_frees() {
        unsafe {
            set_errno(76);
            for p in [
                malloc(0),
                calloc(0, usize::MAX),
                calloc(usize::MAX, 0),
                aligned_alloc(4096, 0),
                realloc(ptr::null_mut(), 0),
                reallocarray(ptr::null_mut(), 0, usize::MAX),
            ] {
                assert!(!p.is_null());
                assert_eq!(errno(), 76);
                free(p);
                assert_eq!(errno(), 76);
            }
            let mut p = ptr::null_mut();
            assert_eq!(posix_memalign(&mut p, 4096, 0), 0);
            assert!(!p.is_null());
            assert_eq!(p as usize % 4096, 0);
            assert_eq!(errno(), 76);
            assert!(realloc(p, 0).is_null());
            assert_eq!(errno(), 76);
            let p = malloc(1);
            assert!(!p.is_null());
            assert!(reallocarray(p, usize::MAX, 0).is_null());
            assert_eq!(errno(), 76);
        }
    }
}
