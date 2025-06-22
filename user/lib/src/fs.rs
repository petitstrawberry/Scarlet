use crate::utils::str_to_cstr_bytes;
use crate::boxed::Box;
use crate::syscall::{syscall2, syscall3, syscall4, Syscall};

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
    let _ = unsafe { Box::from_raw(path_ptr as *mut u8) }; // Free the allocated memory
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

/// Bind mount a directory or file to another location.
/// 
/// # Arguments
/// * `source` - Source path to bind
/// * `target` - Target mount point
/// * `readonly` - Whether the bind mount should be read-only
/// 
/// # Return Value
/// - On success: 0
/// - On error: -1
/// 
pub fn bind_mount(source: &str, target: &str, readonly: bool) -> i32 {
    let source_ptr = Box::into_raw(str_to_cstr_bytes(source).unwrap().into_boxed_slice()) as *const u8 as usize;
    let target_ptr = Box::into_raw(str_to_cstr_bytes(target).unwrap().into_boxed_slice()) as *const u8 as usize;
    let flags = if readonly { 1 } else { 0 };
    
    let res = syscall3(Syscall::BindMount, source_ptr, target_ptr, flags);
    
    // Free the allocated memory
    let _ = unsafe { Box::from_raw(source_ptr as *mut u8) };
    let _ = unsafe { Box::from_raw(target_ptr as *mut u8) };
    
    res as i32
}

/// Create an overlay mount with upper and lower directories.
/// 
/// # Arguments
/// * `upperdir` - Upper directory for writes (optional)
/// * `lowerdirs` - Lower directories for reads (in priority order)
/// * `target` - Target mount point
/// 
/// # Return Value
/// - On success: 0
/// - On error: -1
/// 
pub fn overlay_mount(upperdir: Option<&str>, lowerdirs: &[&str], target: &str) -> i32 {
    use crate::vec::Vec;
    
    let upperdir_ptr = if let Some(upper) = upperdir {
        Box::into_raw(str_to_cstr_bytes(upper).unwrap().into_boxed_slice()) as *const u8 as usize
    } else {
        0 // null pointer
    };
    
    let target_ptr = Box::into_raw(str_to_cstr_bytes(target).unwrap().into_boxed_slice()) as *const u8 as usize;
    
    // Prepare lower directories array
    let mut lowerdir_boxes: Vec<Box<[u8]>> = Vec::new();
    let mut lowerdir_ptrs = Vec::new();
    
    for lowerdir in lowerdirs {
        let boxed_bytes = str_to_cstr_bytes(lowerdir).unwrap().into_boxed_slice();
        let ptr = boxed_bytes.as_ptr() as usize;
        lowerdir_ptrs.push(ptr);
        lowerdir_boxes.push(boxed_bytes);
    }
    
    let lowerdirs_array_ptr = lowerdir_ptrs.as_ptr() as usize;
    
    let res = syscall4(
        Syscall::OverlayMount,
        upperdir_ptr,
        lowerdirs.len(),
        lowerdirs_array_ptr,
        target_ptr
    );
    
    // Free allocated memory
    if upperdir_ptr != 0 {
        let _ = unsafe { Box::from_raw(upperdir_ptr as *mut u8) };
    }
    let _ = unsafe { Box::from_raw(target_ptr as *mut u8) };
    // lowerdir_boxes will be automatically dropped
    
    res as i32
}