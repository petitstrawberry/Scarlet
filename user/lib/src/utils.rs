use crate::vec::Vec;
extern crate alloc;
pub use alloc::string::*;

/// Converts a Rust string slice (`&str`) into a null-terminated C-style string represented as a `Vec<u8>`.
/// 
/// # Arguments
/// 
/// * `s` - A string slice to be converted.
///
/// # Returns
///
/// * `Ok(Vec<u8>)` - A vector containing the bytes of the input string, followed by a null terminator.
/// * `Err(())` - An error if the input string contains a null byte (`\0`), as null bytes are not allowed in C-style strings.
///
/// # Error Handling
///
/// If this function returns `Err(())`, the caller should sanitize the input string to remove null bytes before calling the function again.
pub fn str_to_cstr_bytes(s: &str) -> Result<Vec<u8>, ()> {
    if s.as_bytes().contains(&0) {
        return Err(()); // Error if there is a null byte inside
    }
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0); // Null terminator
    Ok(v)
}