//! System calls for StreamOps capability
//!
//! This module implements system calls that operate on KernelObjects
//! with StreamOps capability (read/write operations).

use crate::arch::Trapframe;
use crate::library::std::usercopy::copy_from_user;
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
    let buf_vaddr = trapframe.get_arg(1);
    let count = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if read fails
    trapframe.increment_pc_next(task);

    // Get KernelObject from handle table
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX,
    };

    // Check if object supports StreamOps
    let stream = match kernel_obj.as_stream() {
        Some(stream) => stream,
        None => return usize::MAX, // Object doesn't support stream operations
    };

    // Allocate kernel buffer and read into it
    let mut kernel_buf = alloc::vec![0u8; count];
    let bytes_read = match stream.read(&mut kernel_buf) {
        Ok(n) => n,
        Err(super::StreamError::WouldBlock) => {
            // Return EAGAIN error code (negative value indicates error)
            return (-(11i32)) as usize;
        }
        Err(_) => return usize::MAX,
    };

    // Copy to user space using copy_to_user (handles page boundaries)
    if bytes_read > 0 {
        use crate::library::std::usercopy::copy_to_user;
        if copy_to_user(&task, buf_vaddr, &kernel_buf[..bytes_read]).is_err() {
            return usize::MAX;
        }
    }

    bytes_read
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
    let buf_vaddr = trapframe.get_arg(1);
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

    // Copy from user space before writing so buffers crossing page boundaries
    // are handled correctly.
    let mut buffer = alloc::vec![0u8; count];
    if copy_from_user(task, buf_vaddr, &mut buffer).is_err() {
        return usize::MAX;
    }

    match stream.write(&buffer) {
        Ok(bytes_written) => bytes_written,
        Err(_) => usize::MAX, // Write error
    }
}
