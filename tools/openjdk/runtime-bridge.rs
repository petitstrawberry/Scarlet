// SPDX-License-Identifier: MIT
use std::ffi::{c_char, CString};

/// Expose an immutable process-lifetime snapshot to OpenJDK's POSIX consumers.
#[unsafe(no_mangle)]
pub extern "C" fn scarlet_jvm_environment() -> *mut *mut c_char {
    let mut entries = Vec::new();
    for (key, value) in std::env::vars_os() {
        let mut pair = key.as_encoded_bytes().to_vec();
        pair.push(b'=');
        pair.extend_from_slice(value.as_encoded_bytes());
        if let Ok(value) = CString::new(pair) { entries.push(value.into_raw()); }
    }
    entries.push(std::ptr::null_mut());
    Box::into_raw(entries.into_boxed_slice()).cast()
}
