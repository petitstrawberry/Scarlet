//! System calls for FileObject capability
//!
//! This module implements system calls that operate on KernelObjects
//! with FileObject capability (seek, truncate, metadata operations).

use super::SeekFrom;
use crate::arch::Trapframe;
use crate::fs::AbiFileMetadata;
use crate::library::std::usercopy::copy_to_user;
use crate::task::mytask;

/// System call for seeking within a file
///
/// # Arguments
/// - handle: Handle to the KernelObject (must support FileObject)
/// - offset: Offset for seek operation
/// - whence: Seek origin (0=start, 1=current, 2=end)
///
/// # Returns
/// - On success: new position in file
/// - On error: u64::MAX in the Native wide-result registers
pub fn sys_file_seek(trapframe: &mut Trapframe) -> usize {
    let handle = trapframe.get_arg(0) as u32;
    let offset = crate::syscall::u64_arg(trapframe, 1);
    let whence = trapframe.get_arg(1 + scarlet_abi::native_scalar::U64_WORDS);
    let result = (|| {
        let task = mytask()?;
        trapframe.increment_pc_next(&task);
        let object = task.handle_table.get(handle)?;
        let file = object.as_file()?;
        let position = match whence {
            0 => SeekFrom::Start(offset),
            1 => SeekFrom::Current(offset as i64),
            2 => SeekFrom::End(offset as i64),
            _ => return None,
        };
        file.seek(position).ok()
    })();
    crate::syscall::u64_result(trapframe, result.unwrap_or(u64::MAX))
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
    let length = crate::syscall::u64_arg(trapframe, 1);

    // Increment PC to avoid infinite loop if truncate fails
    trapframe.increment_pc_next(&task);

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

/// System call for getting file metadata.
///
/// # Arguments
///
/// - `handle`: Handle to the KernelObject (must support FileObject)
/// - `metadata_ptr`: Pointer to an `AbiFileMetadata` structure to fill
///
/// # Returns
///
/// - On success: `0`
/// - On error: `usize::MAX`
pub fn sys_file_metadata(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let handle = trapframe.get_arg(0) as u32;
    let metadata_ptr = trapframe.get_arg(1);

    // Increment PC to avoid infinite loop if metadata fails
    trapframe.increment_pc_next(&task);

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

    let metadata = match file.metadata() {
        Ok(metadata) => AbiFileMetadata::from_metadata(&metadata),
        Err(_) => return usize::MAX,
    };

    // SAFETY: `metadata` is a plain `repr(C)` byte record and is only read for
    // the duration of this copy into the caller-provided buffer.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&metadata as *const AbiFileMetadata).cast::<u8>(),
            core::mem::size_of::<AbiFileMetadata>(),
        )
    };

    match copy_to_user(&task, metadata_ptr, bytes) {
        Ok(()) => 0,
        Err(_) => usize::MAX,
    }
}
