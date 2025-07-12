//! System calls for FileObject capability
//! 
//! This module implements system calls that operate on KernelObjects
//! with FileObject capability (seek, truncate, metadata operations).

use crate::arch::Trapframe;
use crate::task::mytask;
use super::SeekFrom;

/// System call for seeking within a file
/// 
/// # Arguments
/// - handle: Handle to the KernelObject (must support FileObject)
/// - offset: Offset for seek operation
/// - whence: Seek origin (0=start, 1=current, 2=end)
/// 
/// # Returns
/// - On success: new position in file
/// - On error: usize::MAX
pub fn sys_file_seek(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    
    let handle = trapframe.get_arg(0) as u32;
    let offset = trapframe.get_arg(1) as i64;
    let whence = trapframe.get_arg(2) as i32;

    // Increment PC to avoid infinite loop if seek fails
    trapframe.increment_pc_next(task);

    // Get KernelObject from handle table
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid handle
    };

    // Check if object supports FileObject operations
    let file = match kernel_obj.as_file() {
        Some(file) => file,
        None => return usize::MAX, // Object doesn't support file operations
    };

    // Convert whence to SeekFrom
    let seek_from = match whence {
        0 => SeekFrom::Start(offset as u64),
        1 => SeekFrom::Current(offset),
        2 => SeekFrom::End(offset),
        _ => return usize::MAX, // Invalid whence
    };

    // Perform seek operation
    match file.seek(seek_from) {
        Ok(new_position) => new_position as usize,
        Err(_) => usize::MAX, // Seek error
    }
}

/// System call for truncating a file
/// 
/// # Arguments
/// - handle: Handle to the KernelObject (must support FileObject)
/// - length: New length of the file
/// 
/// # Returns
/// - On success: 0
/// - On error: usize::MAX
pub fn sys_file_truncate(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    
    let handle = trapframe.get_arg(0) as u32;
    let length = trapframe.get_arg(1) as u64;
    
    // Increment PC to avoid infinite loop if truncate fails
    trapframe.increment_pc_next(task);
    
    // Get KernelObject from handle table
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid handle
    };
    
    // Check if object supports FileObject operations
    let file = match kernel_obj.as_file() {
        Some(file) => file,
        None => return usize::MAX, // Object doesn't support file operations
    };
    
    // Perform truncate operation
    match file.truncate(length) {
        Ok(()) => 0,
        Err(_) => usize::MAX, // Truncate error
    }
}

/// System call for getting file metadata
/// 
/// # Arguments
/// - handle: Handle to the KernelObject (must support FileObject)
/// - metadata_ptr: Pointer to FileMetadata structure to fill
/// 
/// # Returns
/// - On success: 0
/// - On error: usize::MAX
pub fn sys_file_metadata(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    
    let handle = trapframe.get_arg(0) as u32;
    let metadata_ptr = trapframe.get_arg(1);
    
    // Increment PC to avoid infinite loop if metadata fails
    trapframe.increment_pc_next(task);
    
    // Translate the pointer to get access to the metadata structure
    let metadata_vaddr = match task.vm_manager.translate_vaddr(metadata_ptr) {
        Some(addr) => addr as *mut crate::fs::FileMetadata,
        None => return usize::MAX, // Invalid pointer
    };
    
    // Get KernelObject from handle table
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid handle
    };
    
    // Check if object supports FileObject operations
    let file = match kernel_obj.as_file() {
        Some(file) => file,
        None => return usize::MAX, // Object doesn't support file operations
    };
    
    // Get metadata
    match file.metadata() {
        Ok(metadata) => {
            // Write the metadata to user space
            unsafe {
                *metadata_vaddr = metadata;
            }
            0 // Success
        }
        Err(_) => usize::MAX, // Metadata error
    }
}
