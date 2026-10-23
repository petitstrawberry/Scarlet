//! System calls for StreamOps capability
//!
//! This module implements system calls that operate on KernelObjects
//! with StreamOps capability (read/write operations).

use crate::arch::Trapframe;
use crate::library::std::usercopy::copy_from_user;
use crate::task::mytask;

const EAGAIN: i32 = 11;
const EINTR: i32 = 4;
const ENOMEM: i32 = 12;
// Large userspace reads must not require an equally large contiguous kernel
// allocation (e.g. rustc's shared library is over 100 MiB). Stream reads may
// return a prefix; read_exact/read_to_end continue from the returned offset.
const MAX_READ_BUFFER: usize = 64 * 1024;

fn io_buffer(count: usize) -> Result<alloc::vec::Vec<u8>, usize> {
    let mut buffer = alloc::vec::Vec::new();
    buffer
        .try_reserve_exact(count)
        .map_err(|_| (-ENOMEM) as usize)?;
    buffer.resize(count, 0);
    Ok(buffer)
}

fn stream_error_result(error: super::StreamError) -> usize {
    match error {
        super::StreamError::WouldBlock => (-EAGAIN) as usize,
        super::StreamError::Interrupted => (-EINTR) as usize,
        _ => usize::MAX,
    }
}

/// System call for reading from a KernelObject with StreamOps capability
///
/// # Arguments
/// - handle: Handle to the KernelObject
/// - buffer_ptr: Pointer to the buffer to read into
/// - count: Number of bytes to read
///
/// # Returns
/// - On success: number of bytes read
/// - On a recognized stream error: a negative errno encoded as `usize`
/// - On any other error: `usize::MAX`
pub fn sys_stream_read(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let handle = trapframe.get_arg(0) as u32;
    let buf_vaddr = trapframe.get_arg(1);
    let count = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if read fails
    trapframe.increment_pc_next(&task);

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
    let mut kernel_buf = match io_buffer(count.min(MAX_READ_BUFFER)) {
        Ok(buffer) => buffer,
        Err(error) => return error,
    };
    let bytes_read = match stream.read(&mut kernel_buf) {
        Ok(n) => n,
        Err(error) => return stream_error_result(error),
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
/// - On a recognized stream error: a negative errno encoded as `usize`
/// - On any other error: usize::MAX
pub fn sys_stream_write(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let handle = trapframe.get_arg(0) as u32;
    let buf_vaddr = trapframe.get_arg(1);
    let count = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if write fails
    trapframe.increment_pc_next(&task);

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
    // Keep write boundaries intact for message-oriented streams. An oversized
    // request can fail allocation, but must not abort the kernel.
    let mut buffer = match io_buffer(count) {
        Ok(buffer) => buffer,
        Err(error) => return error,
    };
    if copy_from_user(&task, buf_vaddr, &mut buffer).is_err() {
        return usize::MAX;
    }

    match stream.write(&buffer) {
        Ok(bytes_written) => bytes_written,
        Err(error) => stream_error_result(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{EAGAIN, EINTR, ENOMEM, io_buffer, stream_error_result};
    use crate::object::capability::StreamError;

    #[test_case]
    fn stream_error_result_preserves_retryable_errors() {
        assert_eq!(
            stream_error_result(StreamError::WouldBlock),
            (-EAGAIN) as usize
        );
        assert_eq!(
            stream_error_result(StreamError::Interrupted),
            (-EINTR) as usize
        );
        assert_eq!(stream_error_result(StreamError::IoError), usize::MAX);
    }

    #[test_case]
    fn oversized_io_allocation_returns_an_error() {
        // A malformed count must fail without panicking or entering a device.
        assert_eq!(io_buffer(usize::MAX), Err((-ENOMEM) as usize));
    }
}
