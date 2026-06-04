extern crate alloc;

use alloc::vec::Vec;

/// Converts a Rust string slice into a null-terminated C string byte buffer.
///
/// # Arguments
///
/// * `s` - String slice to convert.
///
/// # Returns
///
/// A byte vector containing `s` followed by a trailing NUL byte.
pub fn str_to_cstr_bytes(s: &str) -> Result<Vec<u8>, ()> {
    if s.as_bytes().contains(&0) {
        return Err(());
    }
    let mut bytes = Vec::with_capacity(s.len() + 1);
    bytes.extend_from_slice(s.as_bytes());
    bytes.push(0);
    Ok(bytes)
}
