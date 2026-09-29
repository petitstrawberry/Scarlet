//! Linux file timestamp updates, including glibc's futimens(fd, NULL) path.
use super::{LinuxAbi, errno};
use crate::{arch::Trapframe, fs::FileTimeUpdate, task::mytask};

const AT_FDCWD: i32 = -100;
const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
const AT_EMPTY_PATH: u32 = 0x1000;
const UTIME_NOW: i64 = (1 << 30) - 1;
const UTIME_OMIT: i64 = (1 << 30) - 2;

fn timestamp(sec: i64, nsec: i64, now: Option<u64>) -> Result<Option<u64>, usize> {
    match nsec {
        UTIME_OMIT => Ok(None),
        UTIME_NOW => now.map(Some).ok_or(errno::EIO),
        0..=999_999_999 => {
            // VFS timestamps have whole-second precision and an unsigned range.
            u64::try_from(sec).map(Some).map_err(|_| errno::EOVERFLOW)
        }
        _ => Err(errno::EINVAL),
    }
}

fn set_times(
    node: &dyn crate::fs::vfs_v2::core::VfsNode,
    times: FileTimeUpdate,
) -> Result<(), usize> {
    if node
        .filesystem()
        .and_then(|fs| fs.upgrade())
        .is_some_and(|fs| fs.is_read_only())
    {
        return Err(errno::EROFS);
    }
    node.set_times(times).map_err(|error| {
        if error.kind == crate::fs::FileSystemErrorKind::NotSupported {
            errno::EOPNOTSUPP
        } else {
            errno::from_fs_error(&error)
        }
    })
}

pub fn sys_utimensat(abi: &mut LinuxAbi, tf: &mut Trapframe) -> usize {
    let Some(task) = mytask() else {
        return errno::to_result(errno::EIO);
    };
    let (dirfd, path_ptr, times_ptr, flags) = (
        tf.get_arg(0) as i32,
        tf.get_arg(1),
        tf.get_arg(2),
        tf.get_arg(3) as u32,
    );
    tf.increment_pc_next(&task);
    let result = (|| -> Result<(), usize> {
        if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
            return Err(errno::EINVAL);
        }
        let now = crate::time::system_time_ns().map(|ns| ns / 1_000_000_000);
        let times = if times_ptr == 0 {
            let now = now.ok_or(errno::EIO)?;
            FileTimeUpdate {
                accessed: Some(now),
                modified: Some(now),
            }
        } else {
            let mut bytes = [0u8; 32];
            crate::library::std::usercopy::copy_from_user(&task, times_ptr, &mut bytes)
                .map_err(|_| errno::EFAULT)?;
            let value = |offset| i64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap());
            FileTimeUpdate {
                accessed: timestamp(value(0), value(8), now)?,
                modified: timestamp(value(16), value(24), now)?,
            }
        };
        // Linux accepts two UTIME_OMIT entries without looking up the file.
        if times.accessed.is_none() && times.modified.is_none() {
            return Ok(());
        }
        if path_ptr == 0 && flags & AT_SYMLINK_NOFOLLOW != 0 {
            return Err(errno::EINVAL);
        }
        let path = if path_ptr == 0 {
            if dirfd == AT_FDCWD {
                return Err(errno::EFAULT);
            }
            None
        } else {
            use crate::library::std::string::{
                StringConversionError, parse_c_string_from_userspace,
            };
            let path =
                parse_c_string_from_userspace(&task, path_ptr, 4096).map_err(
                    |error| match error {
                        StringConversionError::ExceedsMaxLength => errno::ENAMETOOLONG,
                        _ => errno::EFAULT,
                    },
                )?;
            if path.is_empty() {
                if flags & AT_EMPTY_PATH == 0 {
                    return Err(errno::ENOENT);
                }
                None
            } else {
                Some(path)
            }
        };
        let vfs = task.get_vfs().ok_or(errno::EIO)?;
        let options = crate::fs::vfs_v2::manager::PathResolutionOptions {
            no_follow: flags & AT_SYMLINK_NOFOLLOW != 0,
        };
        if dirfd == AT_FDCWD || path.as_ref().is_some_and(|path| path.starts_with('/')) {
            let (entry, _) = vfs
                .resolve_path_with_options(path.as_deref().unwrap_or("."), &options)
                .map_err(|error| errno::from_fs_error(&error))?;
            return set_times(entry.node().as_ref(), times);
        }
        let handle = (dirfd >= 0)
            .then(|| abi.get_handle(dirfd as usize))
            .flatten()
            .ok_or(errno::EBADF)?;
        let object = task.handle_table.get(handle).ok_or(errno::EBADF)?;
        let file = object.as_file().ok_or(errno::EBADF)?;
        let file = file
            .as_any()
            .downcast_ref::<crate::fs::vfs_v2::core::VfsFileObject>()
            .ok_or(if path.is_some() {
                errno::ENOTDIR
            } else {
                errno::EOPNOTSUPP
            })?;
        if let Some(path) = path {
            let entry = file.get_vfs_entry();
            if entry
                .node()
                .file_type()
                .map_err(|error| errno::from_fs_error(&error))?
                != crate::fs::FileType::Directory
            {
                return Err(errno::ENOTDIR);
            }
            let (entry, _) = vfs
                .resolve_path_from_with_options(entry, file.get_mount_point(), &path, &options)
                .map_err(|error| errno::from_fs_error(&error))?;
            set_times(entry.node().as_ref(), times)
        } else {
            // Use the open inode, not its old pathname (rename/unlink safe).
            set_times(file.get_vfs_entry().node().as_ref(), times)
        }
    })();
    match result {
        Ok(()) => 0,
        Err(error) => errno::to_result(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn special_times_ignore_seconds_and_omit_does_not_need_clock() {
        assert_eq!(timestamp(-42, UTIME_NOW, Some(123)), Ok(Some(123)));
        assert_eq!(timestamp(-42, UTIME_OMIT, None), Ok(None));
        assert_eq!(timestamp(0, UTIME_NOW, None), Err(errno::EIO));
    }

    #[test_case]
    fn timestamps_truncate_to_vfs_precision_without_wrapping() {
        assert_eq!(timestamp(123, 999_999_999, None), Ok(Some(123)));
        assert_eq!(timestamp(-1, 0, None), Err(errno::EOVERFLOW));
        assert_eq!(timestamp(123, -1, None), Err(errno::EINVAL));
        assert_eq!(timestamp(123, 1_000_000_000, None), Err(errno::EINVAL));
    }
}
