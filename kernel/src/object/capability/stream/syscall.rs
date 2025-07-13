//! System calls for StreamOps capability
//! 
//! This module implements system calls that operate on KernelObjects
//! with StreamOps capability (read/write operations).

use crate::arch::Trapframe;
use crate::task::mytask;

/// System call for reading from a KernelObject with StreamOps capability
/// 
/// # Arguments
/// - handle: Handle to the KernelObject
/// - buffer_ptr: Pointer to the buffer to read into
/// - count: Number of bytes to read
/// 
/// # Returns
/// - On success: number of bytes read
/// - On error: usize::MAX
pub fn sys_stream_read(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    
    let handle = trapframe.get_arg(0) as u32;
    let buf_ptr = match task.vm_manager.translate_vaddr(trapframe.get_arg(1)) {
        Some(ptr) => ptr as *mut u8,
        None => return usize::MAX, // Invalid buffer pointer
    };
    let count = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if read fails
    trapframe.increment_pc_next(task);

    // Get KernelObject from handle table
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid handle
    };

    // Check if object supports StreamOps
    let stream = match kernel_obj.as_stream() {
        Some(stream) => stream,
        None => return usize::MAX, // Object doesn't support stream operations
    };

    // Perform read operation
    let buffer = unsafe { core::slice::from_raw_parts_mut(buf_ptr, count) };
    match stream.read(buffer) {
        Ok(bytes_read) => bytes_read,
        Err(_) => usize::MAX, // Read error
    }
}

/// System call for writing to a KernelObject with StreamOps capability
/// 
/// # Arguments
/// - handle: Handle to the KernelObject
/// - buffer_ptr: Pointer to the buffer to write from
/// - count: Number of bytes to write
/// 
/// # Returns
/// - On success: number of bytes written
/// - On error: usize::MAX
pub fn sys_stream_write(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };
    
    let handle = trapframe.get_arg(0) as u32;
    let buf_ptr = match task.vm_manager.translate_vaddr(trapframe.get_arg(1)) {
        Some(ptr) => ptr as *const u8,
        None => return usize::MAX, // Invalid buffer pointer
    };
    let count = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if write fails
    trapframe.increment_pc_next(task);

    // Get KernelObject from handle table
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid handle
    };

    // Check if object supports StreamOps
    let stream = match kernel_obj.as_stream() {
        Some(stream) => stream,
        None => return usize::MAX, // Object doesn't support stream operations
    };

    // Perform write operation
    let buffer = unsafe { core::slice::from_raw_parts(buf_ptr, count) };
    match stream.write(buffer) {
        Ok(bytes_written) => bytes_written,
        Err(_) => usize::MAX, // Write error
    }
}
