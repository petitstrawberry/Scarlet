//! Detailed descriptor operations for native C/POSIX adapters.
//!
//! Existing native operations keep their return contracts. These entry points
//! return a nonnegative value or a negated errno and enforce descriptor access.

use alloc::vec::Vec;
use scarlet_abi::fs::*;
use scarlet_abi::{ERRNO_EBADF, ERRNO_EINVAL, ERRNO_EIO, ERRNO_EOPNOTSUPP};

use super::core::VfsFileObject;
use crate::arch::Trapframe;
use crate::fs::FileType;
use crate::library::std::string::parse_c_string_from_userspace;
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::object::KernelObject;
use crate::object::capability::StreamError;
use crate::object::handle::{
    AccessMode, HandleMetadata, HandleTable, HandleType, SpecialSemantics,
};
use crate::task::{Task, mytask};

fn negative(errno: i32) -> usize {
    (-(errno as isize)) as usize
}

fn stream_errno(error: StreamError) -> usize {
    match error {
        StreamError::FileSystemError(error) => super::syscall::fs_errno(error),
        StreamError::NotSupported => negative(ERRNO_EOPNOTSUPP),
        StreamError::InvalidArgument => negative(ERRNO_EINVAL),
        StreamError::PermissionDenied => negative(ERRNO_EACCES),
        StreamError::NoSpace => negative(ERRNO_ENOSPC),
        StreamError::Interrupted => negative(scarlet_abi::ERRNO_EINTR),
        StreamError::WouldBlock => negative(scarlet_abi::ERRNO_EAGAIN),
        StreamError::BrokenPipe => negative(ERRNO_EPIPE),
        StreamError::Closed => negative(ERRNO_EBADF),
        StreamError::SeekError => negative(ERRNO_ESPIPE),
        _ => negative(ERRNO_EIO),
    }
}

fn descriptor(table: &HandleTable, handle: usize) -> Result<(KernelObject, HandleMetadata), usize> {
    u32::try_from(handle)
        .ok()
        .and_then(|handle| table.get_arc_clone_with_metadata(handle))
        .ok_or(negative(ERRNO_EBADF))
}

fn access_flags(mode: AccessMode) -> u32 {
    match mode {
        AccessMode::ReadOnly => VFS_O_RDONLY,
        AccessMode::WriteOnly => VFS_O_WRONLY,
        AccessMode::ReadWrite => VFS_O_RDWR,
    }
}

fn vfs_file(object: &KernelObject) -> Option<&VfsFileObject> {
    object.as_file()?.as_any().downcast_ref()
}

fn check_stream(table: &HandleTable, handle: usize, write: bool) -> Result<KernelObject, usize> {
    let (object, metadata) = descriptor(table, handle)?;
    if matches!(
        (write, metadata.access_mode),
        (true, AccessMode::ReadOnly) | (false, AccessMode::WriteOnly)
    ) {
        return Err(negative(ERRNO_EBADF));
    }
    if let Some(file) = vfs_file(&object) {
        if file
            .get_vfs_entry()
            .node()
            .file_type()
            .map_err(super::syscall::fs_errno)?
            == FileType::Directory
        {
            return Err(negative(ERRNO_EISDIR));
        }
    }
    if object.as_stream().is_none() {
        return Err(negative(ERRNO_EBADF));
    }
    Ok(object)
}

fn open_at(
    task: &Task,
    base: usize,
    path: &str,
    flags: usize,
    mode: usize,
) -> Result<usize, usize> {
    const SUPPORTED: u32 = VFS_O_ACCMODE
        | VFS_O_CREAT
        | VFS_O_EXCL
        | VFS_O_TRUNC
        | VFS_O_APPEND
        | VFS_O_DIRECTORY
        | VFS_O_NOFOLLOW
        | VFS_O_CLOEXEC;
    let flags = u32::try_from(flags).map_err(|_| negative(ERRNO_EINVAL))?;
    if flags & !SUPPORTED != 0 || flags & VFS_O_ACCMODE == VFS_O_ACCMODE {
        return Err(negative(ERRNO_EINVAL));
    }
    if mode & !0o777 != 0 {
        // Set-ID and sticky bits are not yet implemented by the native adapter.
        return Err(negative(ERRNO_EINVAL));
    }
    if path.is_empty() {
        return Err(negative(ERRNO_ENOENT));
    }
    let vfs = task.get_vfs().ok_or(negative(ERRNO_EIO))?;
    let base_object;
    let from = if path.starts_with('/') || base == CURRENT_DIRECTORY {
        None
    } else {
        base_object = descriptor(&task.handle_table, base)?.0;
        let file = vfs_file(&base_object).ok_or(negative(ERRNO_ENOTDIR))?;
        Some((file.get_vfs_entry(), file.get_mount_point()))
    };
    let object = vfs
        .open_at(from, path, flags, mode as u32)
        .map_err(super::syscall::fs_errno)?;
    let metadata = HandleMetadata {
        handle_type: HandleType::Regular,
        access_mode: match flags & VFS_O_ACCMODE {
            VFS_O_WRONLY => AccessMode::WriteOnly,
            VFS_O_RDWR => AccessMode::ReadWrite,
            _ => AccessMode::ReadOnly,
        },
        special_semantics: (flags & VFS_O_CLOEXEC != 0).then_some(SpecialSemantics::CloseOnExec),
    };
    task.handle_table
        .insert_lowest_with_metadata(object, metadata)
        .map(|handle| handle as usize)
        .map_err(|_| negative(ERRNO_EMFILE))
}

pub fn sys_vfs_open_at(tf: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let (base, path, flags, mode) = (tf.get_arg(0), tf.get_arg(1), tf.get_arg(2), tf.get_arg(3));
    tf.increment_pc_next(&task);
    let path = match parse_c_string_from_userspace(&task, path, crate::fs::MAX_PATH_LENGTH) {
        Ok(path) => path,
        Err(error) => return super::syscall::pathname_errno(error),
    };
    open_at(&task, base, &path, flags, mode).unwrap_or_else(|error| error)
}

fn flags(table: &HandleTable, handle: usize) -> Result<usize, usize> {
    let (object, metadata) = descriptor(table, handle)?;
    let append = vfs_file(&object).map_or(0, |file| file.status_flags());
    Ok((access_flags(metadata.access_mode) | append) as usize)
}

pub fn sys_handle_get_flags(tf: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let handle = tf.get_arg(0);
    tf.increment_pc_next(&task);
    flags(&task.handle_table, handle).unwrap_or_else(|error| error)
}

fn set_flags(table: &HandleTable, handle: usize, flags: usize) -> Result<usize, usize> {
    let (object, metadata) = descriptor(table, handle)?;
    let value = u32::try_from(flags).map_err(|_| negative(ERRNO_EINVAL))?;
    if value & !(VFS_O_ACCMODE | VFS_O_APPEND) != 0
        || value & VFS_O_ACCMODE != access_flags(metadata.access_mode)
    {
        return Err(negative(ERRNO_EINVAL));
    }
    if let Some(file) = vfs_file(&object) {
        if value & VFS_O_APPEND != 0 && !file.inner().supports_append() {
            return Err(negative(ERRNO_EOPNOTSUPP));
        }
        file.set_append(value & VFS_O_APPEND != 0);
    } else if value & VFS_O_APPEND != 0 {
        return Err(negative(ERRNO_EOPNOTSUPP));
    }
    Ok(0)
}

pub fn sys_handle_set_flags(tf: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let (handle, flags) = (tf.get_arg(0), tf.get_arg(1));
    tf.increment_pc_next(&task);
    set_flags(&task.handle_table, handle, flags).unwrap_or_else(|error| error)
}

pub fn sys_handle_close_with_status(tf: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let handle = tf.get_arg(0);
    tf.increment_pc_next(&task);
    match u32::try_from(handle)
        .ok()
        .and_then(|handle| task.handle_table.remove(handle))
    {
        Some(_) => 0,
        None => negative(ERRNO_EBADF),
    }
}

fn duplicate(table: &HandleTable, handle: usize) -> Result<usize, usize> {
    let (object, mut metadata) = u32::try_from(handle)
        .ok()
        .and_then(|handle| table.clone_for_dup(handle))
        .ok_or(negative(ERRNO_EBADF))?;
    if metadata.special_semantics == Some(SpecialSemantics::CloseOnExec) {
        metadata.special_semantics = None;
    }
    table
        .insert_lowest_with_metadata(object, metadata)
        .map(|handle| handle as usize)
        .map_err(|_| negative(ERRNO_EMFILE))
}

pub fn sys_handle_duplicate_with_status(tf: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let handle = tf.get_arg(0);
    tf.increment_pc_next(&task);
    duplicate(&task.handle_table, handle).unwrap_or_else(|error| error)
}

pub fn sys_handle_get_descriptor_flags(tf: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let handle = tf.get_arg(0);
    tf.increment_pc_next(&task);
    match descriptor(&task.handle_table, handle) {
        Ok((_, metadata)) => {
            usize::from(metadata.special_semantics == Some(SpecialSemantics::CloseOnExec))
        }
        Err(error) => error,
    }
}

pub fn sys_handle_set_descriptor_flags(tf: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let (handle, value) = (tf.get_arg(0), tf.get_arg(1));
    tf.increment_pc_next(&task);
    if value > 1 {
        return negative(ERRNO_EINVAL);
    }
    let Ok(handle) = u32::try_from(handle) else {
        return negative(ERRNO_EBADF);
    };
    match task.handle_table.set_close_on_exec(handle, value != 0) {
        Ok(()) => 0,
        Err("Invalid handle") => negative(ERRNO_EBADF),
        Err(_) => negative(ERRNO_EOPNOTSUPP),
    }
}

/// Preflight writable pages before consuming bytes or moving a seek position.
/// Concurrent unmapping can still race the eventual copy, as for other usercopy.
fn writable(task: &Task, address: usize, count: usize) -> Result<(), usize> {
    if count == 0 {
        return Ok(());
    }
    let end = address
        .checked_add(count - 1)
        .filter(|_| address != 0)
        .ok_or(negative(ERRNO_EFAULT))?;
    let page_mask = crate::environment::PAGE_SIZE - 1;
    let mut current = address;
    loop {
        task.vm_manager
            .translate_to_kva_for_write(current)
            .ok_or(negative(ERRNO_EFAULT))?;
        if current | page_mask >= end {
            break;
        }
        current = (current | page_mask) + 1;
    }
    Ok(())
}

fn transfer(
    task: &Task,
    handle: usize,
    address: usize,
    count: usize,
    write: bool,
) -> Result<usize, usize> {
    let object = check_stream(&task.handle_table, handle, write)?;
    if count > isize::MAX as usize {
        return Err(negative(ERRNO_EINVAL));
    }
    if count == 0 {
        return Ok(0);
    }
    // Bounded allocations permit legal short I/O instead of trusting a caller's
    // byte count as a kernel heap allocation size.
    let count = count.min(64 * 1024);
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(count)
        .map_err(|_| negative(ERRNO_ENOMEM))?;
    buffer.resize(count, 0);
    let stream = object.as_stream().ok_or(negative(ERRNO_EBADF))?;
    if write {
        copy_from_user(task, address, &mut buffer).map_err(|_| negative(ERRNO_EFAULT))?;
        let n = stream.write(&buffer).map_err(stream_errno)?;
        if n > count {
            return Err(negative(ERRNO_EIO));
        }
        Ok(n)
    } else {
        writable(task, address, count)?;
        let n = match stream.read(&mut buffer) {
            Ok(n) => n,
            Err(StreamError::EndOfStream) => 0,
            Err(error) => return Err(stream_errno(error)),
        };
        if n > count {
            return Err(negative(ERRNO_EIO));
        }
        copy_to_user(task, address, &buffer[..n]).map_err(|_| negative(ERRNO_EFAULT))?;
        Ok(n)
    }
}

fn stream_syscall(tf: &mut Trapframe, write: bool) -> usize {
    let task = mytask().unwrap();
    let (handle, address, count) = (tf.get_arg(0), tf.get_arg(1), tf.get_arg(2));
    tf.increment_pc_next(&task);
    transfer(&task, handle, address, count, write).unwrap_or_else(|error| error)
}

pub fn sys_stream_read_with_status(tf: &mut Trapframe) -> usize {
    stream_syscall(tf, false)
}
pub fn sys_stream_write_with_status(tf: &mut Trapframe) -> usize {
    stream_syscall(tf, true)
}

fn seek(table: &HandleTable, handle: usize, offset: i64, whence: usize) -> Result<u64, usize> {
    let (object, _) = descriptor(table, handle)?;
    if whence > 2 {
        return Err(negative(ERRNO_EINVAL));
    }
    let file = object.as_file().ok_or(negative(ERRNO_ESPIPE))?;
    match file.seek_signed(offset, whence as u32) {
        Err(StreamError::NotSupported) | Err(StreamError::SeekError) => Err(negative(ERRNO_ESPIPE)),
        result => result.map_err(stream_errno),
    }
}

pub fn sys_file_seek_with_status(tf: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let handle = tf.get_arg(0);
    let offset = crate::syscall::u64_arg(tf, 1) as i64;
    let whence_arg = 1 + scarlet_abi::native_scalar::U64_WORDS;
    let (whence, output) = (tf.get_arg(whence_arg), tf.get_arg(whence_arg + 1));
    tf.increment_pc_next(&task);
    let result = (|| {
        // Validate handle/whence before the userspace pointer.
        descriptor(&task.handle_table, handle)?;
        if whence > 2 {
            return Err(negative(ERRNO_EINVAL));
        }
        writable(&task, output, 8)?;
        let position = seek(&task.handle_table, handle, offset, whence)?;
        copy_to_user(&task, output, &position.to_ne_bytes()).map_err(|_| negative(ERRNO_EFAULT))?;
        Ok(0)
    })();
    result.unwrap_or_else(|error| error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::ToString, sync::Arc};

    fn task() -> Task {
        let task = crate::task::new_user_task("native-descriptors".to_string(), 1);
        task.set_vfs(Arc::new(super::super::VfsManager::new()));
        task
    }

    #[test_case]
    fn detailed_descriptor_access_is_checked_before_zero_count_and_bad_pointers() {
        let task = task();
        let ro = open_at(
            &task,
            CURRENT_DIRECTORY,
            "/ro",
            (VFS_O_CREAT | VFS_O_RDONLY) as usize,
            0o600,
        )
        .unwrap();
        let wo = open_at(
            &task,
            CURRENT_DIRECTORY,
            "/wo",
            (VFS_O_CREAT | VFS_O_WRONLY) as usize,
            0o600,
        )
        .unwrap();
        assert_eq!(transfer(&task, ro, 0, 0, true), Err(negative(ERRNO_EBADF)));
        assert_eq!(transfer(&task, wo, 0, 0, false), Err(negative(ERRNO_EBADF)));
        assert_eq!(
            transfer(&task, usize::MAX, 0, 0, true),
            Err(negative(ERRNO_EBADF))
        );
        assert_eq!(transfer(&task, ro, 0, 0, false), Ok(0));
        assert_eq!(transfer(&task, wo, 0, 0, true), Ok(0));
        assert_eq!(
            transfer(&task, wo, 0, usize::MAX, true),
            Err(negative(ERRNO_EINVAL))
        );
        assert_eq!(transfer(&task, wo, 0, 1, true), Err(negative(ERRNO_EFAULT)));
        let directory =
            open_at(&task, CURRENT_DIRECTORY, "/", VFS_O_DIRECTORY as usize, 0).unwrap();
        assert_eq!(
            transfer(&task, directory, 0, 0, false),
            Err(negative(ERRNO_EISDIR))
        );
    }

    #[test_case]
    fn detailed_duplicate_shares_offset_append_and_clears_close_on_exec() {
        let task = task();
        let fd = open_at(
            &task,
            CURRENT_DIRECTORY,
            "/dup",
            (VFS_O_CREAT | VFS_O_RDWR | VFS_O_CLOEXEC) as usize,
            0o600,
        )
        .unwrap();
        let other = duplicate(&task.handle_table, fd).unwrap();
        assert_eq!(
            task.handle_table
                .get_metadata(other as u32)
                .unwrap()
                .special_semantics,
            None
        );
        assert_eq!(
            task.handle_table
                .get_metadata(fd as u32)
                .unwrap()
                .special_semantics,
            Some(SpecialSemantics::CloseOnExec)
        );
        check_stream(&task.handle_table, fd, true)
            .unwrap()
            .as_stream()
            .unwrap()
            .write(b"abc")
            .unwrap();
        assert_eq!(seek(&task.handle_table, other, 0, 1), Ok(3));
        set_flags(&task.handle_table, fd, (VFS_O_RDWR | VFS_O_APPEND) as usize).unwrap();
        assert_eq!(
            flags(&task.handle_table, other),
            Ok((VFS_O_RDWR | VFS_O_APPEND) as usize)
        );
        seek(&task.handle_table, other, 0, 0).unwrap();
        check_stream(&task.handle_table, other, true)
            .unwrap()
            .as_stream()
            .unwrap()
            .write(b"d")
            .unwrap();
        assert_eq!(seek(&task.handle_table, fd, 0, 1), Ok(4));
        assert_eq!(
            set_flags(&task.handle_table, fd, VFS_O_RDONLY as usize),
            Err(negative(ERRNO_EINVAL))
        );
    }

    #[test_case]
    fn detailed_openat_rejects_bad_base_and_flags_and_ignores_base_for_absolute_path() {
        let task = task();
        let fd = open_at(
            &task,
            usize::MAX - 1,
            "/file",
            (VFS_O_CREAT | VFS_O_RDWR) as usize,
            0o600,
        )
        .unwrap();
        assert_eq!(
            open_at(&task, usize::MAX - 1, "child", 0, 0),
            Err(negative(ERRNO_EBADF))
        );
        assert_eq!(
            open_at(&task, fd, "child", 0, 0),
            Err(negative(ERRNO_ENOTDIR))
        );
        assert_eq!(
            open_at(&task, CURRENT_DIRECTORY, "/file", 3, 0),
            Err(negative(ERRNO_EINVAL))
        );
        assert_eq!(
            open_at(&task, CURRENT_DIRECTORY, "/file", 0x8000_0000, 0),
            Err(negative(ERRNO_EINVAL))
        );
        assert_eq!(
            seek(&task.handle_table, fd, 0, 3),
            Err(negative(ERRNO_EINVAL))
        );
        assert_eq!(
            seek(&task.handle_table, usize::MAX, 0, 0),
            Err(negative(ERRNO_EBADF))
        );
    }

    #[test_case]
    fn detailed_open_and_dup_choose_lowest_unused_descriptor() {
        let task = task();
        let first = open_at(
            &task,
            CURRENT_DIRECTORY,
            "/file",
            (VFS_O_CREAT | VFS_O_RDWR) as usize,
            0o600,
        )
        .unwrap();
        let low = duplicate(&task.handle_table, first).unwrap();
        let high = duplicate(&task.handle_table, first).unwrap();
        assert!(low < high);
        task.handle_table.remove(low as u32).unwrap();
        task.handle_table.remove(high as u32).unwrap();
        assert_eq!(duplicate(&task.handle_table, first).unwrap(), low);
        assert_eq!(
            open_at(&task, CURRENT_DIRECTORY, "/file", VFS_O_RDWR as usize, 0).unwrap(),
            high
        );
        task.handle_table
            .set_close_on_exec(first as u32, true)
            .unwrap();
        assert_eq!(
            task.handle_table
                .get_metadata(first as u32)
                .unwrap()
                .special_semantics,
            Some(SpecialSemantics::CloseOnExec)
        );
        task.handle_table
            .set_close_on_exec(first as u32, false)
            .unwrap();
        assert_eq!(
            task.handle_table
                .get_metadata(first as u32)
                .unwrap()
                .special_semantics,
            None
        );
    }

    #[test_case]
    fn detailed_append_rejects_unsupported_cpio_backend_before_setting_flags() {
        let task = task();
        let mut archive = Vec::new();
        for (name, mode, data) in [
            ("file", 0o100644, &b"data"[..]),
            ("TRAILER!!!", 0, &b""[..]),
        ] {
            archive.extend_from_slice(b"070701");
            for field in [
                1usize,
                mode,
                0,
                0,
                1,
                0,
                data.len(),
                0,
                0,
                0,
                0,
                name.len() + 1,
                0,
            ] {
                archive.extend_from_slice(alloc::format!("{field:08x}").as_bytes());
            }
            archive.extend_from_slice(name.as_bytes());
            archive.push(0);
            while archive.len() % 4 != 0 {
                archive.push(0);
            }
            archive.extend_from_slice(data);
            while archive.len() % 4 != 0 {
                archive.push(0);
            }
        }
        let cpio =
            super::super::drivers::cpiofs::CpioFS::new("descriptor-cpio".to_string(), &archive)
                .unwrap();
        let vfs = task.get_vfs().unwrap();
        vfs.create_dir("/archive").unwrap();
        vfs.mount(cpio, "/archive", 0).unwrap();
        assert_eq!(
            open_at(
                &task,
                CURRENT_DIRECTORY,
                "/archive/file",
                VFS_O_APPEND as usize,
                0
            ),
            Err(negative(ERRNO_EOPNOTSUPP))
        );
        let fd = open_at(&task, CURRENT_DIRECTORY, "/archive/file", 0, 0).unwrap();
        assert_eq!(
            set_flags(&task.handle_table, fd, VFS_O_APPEND as usize),
            Err(negative(ERRNO_EOPNOTSUPP))
        );
        assert_eq!(flags(&task.handle_table, fd), Ok(0));
        // The scoped signed-seek ABI deliberately reports unsupported backends.
        assert_eq!(
            seek(&task.handle_table, fd, 0, 0),
            Err(negative(ERRNO_ESPIPE))
        );
        let mut data = [0; 4];
        assert_eq!(
            check_stream(&task.handle_table, fd, false)
                .unwrap()
                .as_stream()
                .unwrap()
                .read(&mut data)
                .unwrap(),
            4
        );
        assert_eq!(&data, b"data");
    }
}
