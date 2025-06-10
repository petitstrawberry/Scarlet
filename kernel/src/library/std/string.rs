use alloc::string::String;

#[derive(Debug, PartialEq)]
pub enum StringConversionError {
    NullPointer,
    ExceedsMaxLength,
    Utf8Error,
}
pub fn cstring_to_string(cstr_ptr: *const u8, max_len: usize) -> Result<(String, usize), StringConversionError> {
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

    if len > max_len {
        return Err(StringConversionError::ExceedsMaxLength);
    }

    Ok((
        String::from_utf8(unsafe { alloc::slice::from_raw_parts(cstr_ptr, len) }.to_vec())
        .unwrap_or_else(|_| String::new()),
        len,
    ))
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
        assert_eq!(result, Ok(("Hello".into(), 5)));
    }
}