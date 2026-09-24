//! Synthetic single-user passwd lookup for Scarlet's Native identity model.

use std::ffi::{c_char, c_int};

#[repr(C)]
pub struct Passwd {
    name: *mut c_char,
    password: *mut c_char,
    uid: u32,
    gid: u32,
    gecos: *mut c_char,
    home: *mut c_char,
    shell: *mut c_char,
}

/// # Safety
/// `output` and `result` are writable, and `buffer` has `size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getpwuid_r(
    uid: u32,
    output: *mut Passwd,
    buffer: *mut c_char,
    size: usize,
    result: *mut *mut Passwd,
) -> c_int {
    if output.is_null() || result.is_null() || buffer.is_null() {
        return scarlet_abi::ERRNO_EINVAL;
    }
    // SAFETY: the caller supplies writable result storage.
    unsafe { *result = std::ptr::null_mut() };
    if uid != 1 {
        return 0;
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_owned());
    if home.as_bytes().contains(&0) {
        return scarlet_abi::ERRNO_EINVAL;
    }
    let fields: [&[u8]; 5] = [b"scarlet", b"x", b"", home.as_bytes(), b"/bin/sh"];
    let Some(required) = fields
        .iter()
        .try_fold(0usize, |size, field| size.checked_add(field.len() + 1))
    else {
        return scarlet_abi::fs::ERRNO_EOVERFLOW;
    };
    if size < required {
        return scarlet_abi::fs::ERRNO_ERANGE;
    }
    let mut cursor = buffer;
    let mut pointers = [std::ptr::null_mut(); 5];
    for (slot, field) in pointers.iter_mut().zip(fields) {
        *slot = cursor;
        // SAFETY: `required <= size` and cursor only advances within buffer.
        unsafe {
            std::ptr::copy_nonoverlapping(field.as_ptr(), cursor.cast::<u8>(), field.len());
            *cursor.add(field.len()) = 0;
            cursor = cursor.add(field.len() + 1);
        }
    }
    // SAFETY: output and result are writable and receive only pointers into
    // the caller-owned buffer.
    unsafe {
        *output = Passwd {
            name: pointers[0],
            password: pointers[1],
            uid: 1,
            gid: 1,
            gecos: pointers[2],
            home: pointers[3],
            shell: pointers[4],
        };
        *result = output;
    }
    0
}
