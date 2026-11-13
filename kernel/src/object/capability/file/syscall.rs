//! System calls for FileObject capability
//!
//! This module implements system calls that operate on KernelObjects
//! with FileObject capability (seek, truncate, metadata operations).

use super::SeekFrom;
use crate::arch::Trapframe;
use crate::fs::AbiFileMetadata;
use crate::library::std::usercopy::copy_to_user;
use crate::object::KernelObject;
use crate::task::mytask;

fn stream_errno(error: crate::object::capability::StreamError) -> usize {
    use crate::object::capability::StreamError;
    let errno = match error {
        StreamError::FileSystemError(error) => return crate::fs::vfs_v2::syscall::fs_errno(error),
        StreamError::NotSupported => scarlet_abi::ERRNO_EOPNOTSUPP,
        StreamError::InvalidArgument => scarlet_abi::ERRNO_EINVAL,
        StreamError::PermissionDenied => scarlet_abi::fs::ERRNO_EACCES,
        StreamError::NoSpace => scarlet_abi::fs::ERRNO_ENOSPC,
        StreamError::Interrupted => scarlet_abi::ERRNO_EINTR,
        StreamError::WouldBlock => scarlet_abi::ERRNO_EAGAIN,
        _ => scarlet_abi::ERRNO_EIO,
    };
    (-(errno as isize)) as usize
}

/// FileSetTimes(handle, RawFileTimes*). The open object survives a rename.
pub fn sys_file_set_times(tf: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let (handle, times_ptr) = (tf.get_arg(0), tf.get_arg(1));
    tf.increment_pc_next(&task);
    let Some(object) = u32::try_from(handle)
        .ok()
        .and_then(|handle| task.handle_table.get(handle))
    else {
        return (-(scarlet_abi::ERRNO_EBADF as isize)) as usize;
    };
    let Some(file) = object.as_file() else {
        return (-(scarlet_abi::ERRNO_EBADF as isize)) as usize;
    };
    let times = match crate::fs::vfs_v2::syscall::read_file_times(&task, times_ptr) {
        Ok(times) => times,
        Err(error) => return error,
    };
    match file.set_times(times) {
        Ok(()) => 0,
        Err(error) => stream_errno(error),
    }
}

/// FileSync(handle): write pending file data and metadata to the backing device.
pub fn sys_file_sync(tf: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let handle = tf.get_arg(0);
    tf.increment_pc_next(&task);
    let Some(object) = u32::try_from(handle)
        .ok()
        .and_then(|handle| task.handle_table.get(handle))
    else {
        return (-(scarlet_abi::ERRNO_EBADF as isize)) as usize;
    };
    let Some(file) = object.as_file() else {
        return (-(scarlet_abi::ERRNO_EBADF as isize)) as usize;
    };
    match file.sync() {
        Ok(()) => 0,
        Err(error) => stream_errno(error),
    }
}

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
/// - `handle`: Handle to a file or anonymous pipe KernelObject
/// - `metadata_ptr`: Pointer to an `AbiFileMetadata` structure to fill
///
/// # Returns
///
/// - On success: `0`
/// - On error: `usize::MAX`, including invalid handles and invalid output memory.
///
/// This is a legacy sentinel-result syscall. Do not return encoded errno values
/// here: existing FileError::from_syscall_result callers would treat -EBADF (and
/// other values besides -1) as success and consume an uninitialized record.
pub fn sys_file_metadata(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let handle = trapframe.get_arg(0);
    let metadata_ptr = trapframe.get_arg(1);

    // Increment PC to avoid infinite loop if metadata fails
    trapframe.increment_pc_next(&task);

    // Get KernelObject from handle table
    let kernel_obj = match u32::try_from(handle)
        .ok()
        .and_then(|handle| task.handle_table.get(handle))
    {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid handle
    };

    let metadata = match object_metadata(&kernel_obj) {
        Some(metadata) => metadata,
        None => return usize::MAX,
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

/// Query object state without granting pipes seek/truncate capabilities or
/// manufacturing a regular-file record for an unsupported object.
fn object_metadata(object: &KernelObject) -> Option<AbiFileMetadata> {
    if let Some(file) = object.as_file() {
        return file
            .metadata()
            .ok()
            .map(|metadata| AbiFileMetadata::from_metadata(&metadata));
    }
    let pipe = object.as_pipe()?;
    // Real pipe endpoints share this identity across both ends and duplicates.
    // An implementation without an identity cannot provide this record.
    let file_id = pipe.pipe_id()?;
    let mut permissions = 0;
    if pipe.is_readable() {
        permissions |= crate::fs::ABI_FILE_PERMISSION_READ;
    }
    if pipe.is_writable() {
        permissions |= crate::fs::ABI_FILE_PERMISSION_WRITE;
    }
    Some(AbiFileMetadata {
        file_type: crate::fs::ABI_FILE_TYPE_PIPE,
        file_id,
        permissions,
        // This is a snapshot of queued bytes, not the pipe capacity or a
        // seekable file length. Observing it does not consume data.
        size: pipe.available_bytes() as u64,
        // Anonymous pipes have no directory links or tracked timestamps.
        // Keep those unavailable fields and the reserved word zero.
        ..AbiFileMetadata::default()
    })
}
