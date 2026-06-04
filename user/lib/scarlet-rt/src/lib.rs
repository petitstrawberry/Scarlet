//! Scarlet Native runtime entry glue.
//!
//! This crate provides the `_start` symbol for Scarlet Native executables.
//! Low-level ABI definitions and syscall assembly live in `scarlet-abi` and
//! `scarlet-sys`.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

use scarlet_sys::{Syscall, syscall1};

unsafe extern "C" {
    fn main(argc: isize, argv: *const *const u8) -> isize;
}

/// Scarlet Native process entry point.
///
/// # Arguments
///
/// * `argc` - Number of command-line arguments.
/// * `argv` - Null-terminated command-line argument pointer array.
///
/// # Safety
///
/// This symbol is entered by the Scarlet kernel loader. The loader must provide
/// `argc` and `argv` according to the Scarlet Native process-start ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start(argc: isize, argv: *const *const u8) -> ! {
    // SAFETY: `_start` is only entered by the Scarlet loader, which sets up
    // `argc`/`argv` before transferring control. Rustc provides the C ABI
    // `main` shim for normal Rust executables.
    let code = unsafe { main(argc, argv) };
    exit(code as i32)
}

/// Exit the current process using Scarlet Native `ExitGroup`.
///
/// # Arguments
///
/// * `code` - Process exit status.
pub fn exit(code: i32) -> ! {
    let _ = syscall1(Syscall::ExitGroup, code as usize);
    loop {
        core::hint::spin_loop();
    }
}

/// Copy bytes between non-overlapping buffers.
///
/// # Arguments
///
/// * `dest` - Destination buffer pointer.
/// * `src` - Source buffer pointer.
/// * `n` - Number of bytes to copy.
///
/// # Returns
///
/// The original `dest` pointer.
///
/// # Safety
///
/// The caller must uphold the C `memcpy` contract: `src` and `dest` must be
/// valid for `n` bytes and must not overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    // SAFETY: The caller guarantees the regions are valid for `n` bytes and do
    // not overlap, matching `copy_nonoverlapping` requirements.
    unsafe {
        core::ptr::copy_nonoverlapping(src, dest, n);
    }
    dest
}

/// Copy bytes between potentially overlapping buffers.
///
/// # Arguments
///
/// * `dest` - Destination buffer pointer.
/// * `src` - Source buffer pointer.
/// * `n` - Number of bytes to copy.
///
/// # Returns
///
/// The original `dest` pointer.
///
/// # Safety
///
/// The caller must uphold the C `memmove` contract: `src` and `dest` must be
/// valid for `n` bytes. The regions may overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memmove(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    // SAFETY: The caller guarantees both regions are valid for `n` bytes.
    // `copy` supports overlapping regions.
    unsafe {
        core::ptr::copy(src, dest, n);
    }
    dest
}

/// Fill a buffer with a byte value.
///
/// # Arguments
///
/// * `dest` - Destination buffer pointer.
/// * `value` - Byte value, passed as a C `int`.
/// * `n` - Number of bytes to write.
///
/// # Returns
///
/// The original `dest` pointer.
///
/// # Safety
///
/// The caller must uphold the C `memset` contract: `dest` must be valid for
/// `n` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(dest: *mut u8, value: i32, n: usize) -> *mut u8 {
    // SAFETY: The caller guarantees `dest` is valid for `n` bytes.
    unsafe {
        core::ptr::write_bytes(dest, value as u8, n);
    }
    dest
}

/// Compare two byte buffers.
///
/// # Arguments
///
/// * `left` - First buffer pointer.
/// * `right` - Second buffer pointer.
/// * `n` - Number of bytes to compare.
///
/// # Returns
///
/// Zero if the buffers are equal, otherwise the signed byte difference at the
/// first differing position.
///
/// # Safety
///
/// The caller must uphold the C `memcmp` contract: `left` and `right` must be
/// valid for `n` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(left: *const u8, right: *const u8, n: usize) -> i32 {
    for index in 0..n {
        // SAFETY: The caller guarantees both buffers are valid for `n` bytes,
        // so every index in this loop is in bounds.
        let left_byte = unsafe { *left.add(index) };
        // SAFETY: Same as above for the right-hand buffer.
        let right_byte = unsafe { *right.add(index) };

        if left_byte != right_byte {
            return left_byte as i32 - right_byte as i32;
        }
    }
    0
}
