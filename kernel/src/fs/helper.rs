use crate::alloc::string::ToString;
use alloc::{string::String, vec::Vec};

use super::MAX_PATH_LENGTH;

pub fn get_path_str(path_ptr: *const u8) -> Result<String, &'static str> {
    // Parse path as a null-terminated C string
    let mut path_bytes = Vec::new();
    let mut i = 0;
    unsafe {
        loop {
            let byte = *path_ptr.add(i);
            if byte == 0 {
                break;
            }
            path_bytes.push(byte);
            i += 1;

            if i > MAX_PATH_LENGTH {
                return Err("Path too long");
            }
        }
    }

    // Convert path bytes to string
    let path_str = match str::from_utf8(&path_bytes) {
        Ok(s) => s,
        Err(_) => return Err("Invalid UTF-8"),
    };
    Ok(path_str.to_string())
}