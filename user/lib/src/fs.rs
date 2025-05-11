use crate::utils::str_to_cstr_bytes;
use crate::boxed::Box;
use crate::syscall::{syscall2, syscall3, Syscall};

/// Open a file.
/// 
/// # Arguments
/// * `path` - Path to the file
/// * `flags` - Flags for opening the file
/// 
/// # Return Value
/// - On success: file descriptor
/// - On error: -1
/// 
pub fn open(path: &str, flags: usize) -> i32 {
    let path_ptr = Box::into_raw(str_to_cstr_bytes(path).unwrap().into_boxed_slice()) as *const u8 as usize;
    let res = syscall2(Syscall::Open, path_ptr, flags);
    // Return the result of the syscall
    res as i32
}

/// Close a file.
/// 
/// # Arguments
/// * `fd` - File descriptor
/// 
/// # Return Value
/// - On success: 0
/// - On error: -1
/// 
pub fn close(fd: i32) -> i32 {
    let res = syscall2(Syscall::Close, fd as usize, 0);
    // Return the result of the syscall
    res as i32
}

/// Read from a file.
/// 
/// # Arguments
/// * `fd` - File descriptor
/// * `buf` - Buffer to read into
///
/// # Return Value
/// - On success: number of bytes read
/// - On error: -1
/// 
pub fn read(fd: i32, buf: &mut [u8]) -> i32 {
    let res = syscall3(Syscall::Read, fd as usize, buf.as_mut_ptr() as usize, buf.len());
    // Return the result of the syscall
    res as i32
}

/// Write to a file.
/// 
/// # Arguments
/// * `fd` - File descriptor
/// * `buf` - Buffer to write from
/// 
/// # Return Value
/// - On success: number of bytes written
/// - On error: -1
/// 
pub fn write(fd: i32, buf: &[u8]) -> i32 {
    let res = syscall3(Syscall::Write, fd as usize, buf.as_ptr() as usize, buf.len());
    // Return the result of the syscall
    res as i32
}

/// Seek to a position in a file.
/// 
/// # Arguments
/// * `fd` - File descriptor
/// * `offset` - Offset to seek to
/// * `whence` - Whence for the seek operation
/// 
/// # Return Value
/// - On success: new position in the file
/// - On error: -1
/// 
pub fn lseek(fd: i32, offset: i64, whence: u32) -> i32 {
    let res = syscall3(Syscall::Lseek, fd as usize, offset as usize, whence as usize);
    // Return the result of the syscall
    res as i32
}