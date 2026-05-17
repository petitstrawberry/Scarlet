use alloc::string::String;
use alloc::vec::Vec;

use crate::library::std::usercopy::copy_from_user;

#[derive(Debug, PartialEq)]
pub enum StringConversionError {
    NullPointer,
    ExceedsMaxLength,
    Utf8Error,
    TranslationError,
    TooManyStrings,
}

/// Convert a C string pointer to a Rust String
pub fn cstring_to_string(
    cstr_ptr: *const u8,
    max_len: usize,
) -> Result<(String, usize), StringConversionError> {
    if cstr_ptr.is_null() {
        return Err(StringConversionError::NullPointer);
    }
    if max_len == 0 {
        return Ok((String::new(), 0));
    }

    let mut len = 0;
    while len < max_len && unsafe { *cstr_ptr.add(len) } != 0 {
        len += 1;
    }

    if len == max_len {
        return Err(StringConversionError::ExceedsMaxLength);
    }

    let bytes = unsafe { alloc::slice::from_raw_parts(cstr_ptr, len) };
    match String::from_utf8(bytes.to_vec()) {
        Ok(string) => Ok((string, len)),
        Err(_) => Err(StringConversionError::Utf8Error),
    }
}

/// Parse a null-terminated C string from user space using task's VM manager
pub fn parse_c_string_from_userspace(
    task: &crate::task::Task,
    ptr: usize,
    max_len: usize,
) -> Result<String, StringConversionError> {
    if ptr == 0 {
        return Err(StringConversionError::NullPointer);
    }
    if max_len == 0 {
        return Ok(String::new());
    }

    let mut bytes = Vec::new();
    let mut found_nul = false;
    for i in 0..max_len {
        let mut byte = [0u8; 1];
        copy_from_user(task, ptr + i, &mut byte)
            .map_err(|_| StringConversionError::TranslationError)?;
        if byte[0] == 0 {
            found_nul = true;
            break;
        }
        bytes.push(byte[0]);
    }
    if !found_nul {
        return Err(StringConversionError::ExceedsMaxLength);
    }

    String::from_utf8(bytes).map_err(|_| StringConversionError::Utf8Error)
}

/// Parse an array of string pointers (char **) from user space
pub fn parse_string_array_from_userspace(
    task: &crate::task::Task,
    array_ptr: usize,
    max_strings: usize,
    max_string_len: usize,
) -> Result<Vec<String>, StringConversionError> {
    if array_ptr == 0 {
        return Ok(Vec::new());
    }

    let mut strings = Vec::new();
    let mut i = 0;

    loop {
        let mut raw_ptr = [0u8; core::mem::size_of::<usize>()];
        copy_from_user(
            task,
            array_ptr + i * core::mem::size_of::<usize>(),
            &mut raw_ptr,
        )
        .map_err(|_| StringConversionError::TranslationError)?;
        let str_ptr = usize::from_le_bytes(raw_ptr);

        if str_ptr == 0 {
            break;
        }

        if i >= max_strings {
            return Err(StringConversionError::TooManyStrings);
        }

        let string = parse_c_string_from_userspace(task, str_ptr, max_string_len)?;
        strings.push(string);
        i += 1;
    }

    Ok(strings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_cstring_to_string() {
        let cstr = b"Hello, world!\0";
        let res = cstring_to_string(cstr.as_ptr(), cstr.len()).unwrap();
        assert_eq!(res, ("Hello, world!".into(), 13));
    }

    #[test_case]
    fn test_cstring_to_string_empty() {
        let cstr = b"\0";
        let result = cstring_to_string(cstr.as_ptr(), cstr.len()).unwrap();
        assert_eq!(result, ("".into(), 0));
    }

    #[test_case]
    fn test_cstring_to_string_truncated() {
        let cstr = b"Hello\0World\0";
        let result = cstring_to_string(cstr.as_ptr(), 5);
        assert_eq!(result, Err(StringConversionError::ExceedsMaxLength));
    }

    #[test_case]
    fn test_cstring_to_string_utf8_error() {
        let invalid_utf8 = &[0xFF, 0xFE, 0xFD, 0x00]; // Invalid UTF-8 sequence
        let result = cstring_to_string(invalid_utf8.as_ptr(), 4);
        assert_eq!(result, Err(StringConversionError::Utf8Error));
    }

    #[test_case]
    fn test_parse_c_string_from_userspace_null_pointer() {
        // Create a minimal task for testing
        let task = crate::task::new_user_task("test".into(), 1);

        // Test null pointer
        let result = parse_c_string_from_userspace(&task, 0, 100);
        assert_eq!(result, Err(StringConversionError::NullPointer));
    }

    #[test_case]
    fn test_parse_string_array_from_userspace_null_pointer() {
        // Create a minimal task for testing
        let task = crate::task::new_user_task("test".into(), 1);

        // Test null pointer array
        let result = parse_string_array_from_userspace(&task, 0, 10, 100);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
    }
}
