//! Linux extended-attribute ABI over native inode storage.
//! The current Linux identity model is root-only. Security labels are opaque;
//! ACLs and capability semantics are deliberately not advertised.
use super::{LinuxAbi, errno};
use crate::{
    arch::Trapframe,
    fs::vfs_v2::{
        core::{VfsFileObject, VfsNode},
        manager::PathResolutionOptions,
        xattr,
    },
    library::std::{
        string::{StringConversionError, parse_c_string_from_userspace},
        usercopy::{copy_from_user, copy_to_user},
    },
    task::{Task, mytask},
};
use alloc::{sync::Arc, vec, vec::Vec};

fn error(error: xattr::Error) -> usize {
    match error {
        xattr::Error::Unsupported => errno::EOPNOTSUPP,
        xattr::Error::Missing => errno::ENODATA,
        xattr::Error::Exists => errno::EEXIST,
        xattr::Error::InvalidName => errno::EINVAL,
        xattr::Error::TooLarge => errno::E2BIG,
        xattr::Error::NoSpace => errno::ENOSPC,
    }
}

fn name(task: &Task, address: usize) -> Result<Vec<u8>, usize> {
    let mut name = Vec::new();
    for i in 0..=xattr::MAX_NAME {
        let mut byte = [0];
        copy_from_user(
            task,
            address.checked_add(i).ok_or(errno::EFAULT)?,
            &mut byte,
        )
        .map_err(|_| errno::EFAULT)?;
        if byte[0] == 0 {
            return if name.is_empty() {
                Err(errno::ERANGE)
            } else {
                Ok(name)
            };
        }
        name.push(byte[0]);
    }
    Err(errno::ERANGE)
}

fn node(
    task: &Task,
    abi: &LinuxAbi,
    target: usize,
    fd: bool,
    no_follow: bool,
) -> Result<Arc<dyn VfsNode>, usize> {
    if fd {
        let handle = abi.get_handle(target).ok_or(errno::EBADF)?;
        let object = task.handle_table.get(handle).ok_or(errno::EBADF)?;
        let file = object.as_file().ok_or(errno::EBADF)?;
        let file = file
            .as_any()
            .downcast_ref::<VfsFileObject>()
            .ok_or(errno::EOPNOTSUPP)?;
        return Ok(file.get_vfs_entry().node());
    }
    let path = parse_c_string_from_userspace(task, target, 4096).map_err(|e| match e {
        StringConversionError::ExceedsMaxLength => errno::ENAMETOOLONG,
        _ => errno::EFAULT,
    })?;
    if path.is_empty() {
        return Err(errno::ENOENT);
    }
    let vfs = task.get_vfs().ok_or(errno::EIO)?;
    let (entry, _) = vfs
        .resolve_path_with_options(&path, &PathResolutionOptions { no_follow })
        .map_err(|e| errno::from_fs_error(&e))?;
    Ok(entry.node())
}

fn posix_acl_name(name: &[u8]) -> bool {
    matches!(
        name,
        b"system.posix_acl_access" | b"system.posix_acl_default"
    )
}

fn supported_name(node: &dyn VfsNode, name: &[u8], write: bool) -> Result<(), usize> {
    if name.starts_with(b"user.") && name.len() > 5 {
        if !matches!(
            node.file_type().map_err(|e| errno::from_fs_error(&e))?,
            crate::fs::FileType::RegularFile | crate::fs::FileType::Directory
        ) {
            return Err(if write { errno::EPERM } else { errno::ENODATA });
        }
        Ok(())
    } else if (name.starts_with(b"trusted.") && name.len() > 8)
        || name == b"security.selinux"
        || posix_acl_name(name)
    {
        Ok(())
    } else {
        Err(errno::EOPNOTSUPP)
    }
}

#[cfg(test)]
mod tests {
    #[test_case]
    fn recognizes_only_the_native_posix_acl_attributes() {
        assert!(super::posix_acl_name(b"system.posix_acl_access"));
        assert!(super::posix_acl_name(b"system.posix_acl_default"));
        assert!(!super::posix_acl_name(b"system.posix_acl_other"));
        assert!(!super::posix_acl_name(b"system.admin"));
    }
}

fn writable(node: &dyn VfsNode) -> Result<(), usize> {
    if node
        .filesystem()
        .and_then(|fs| fs.upgrade())
        .is_some_and(|fs| fs.is_read_only())
    {
        Err(errno::EROFS)
    } else {
        Ok(())
    }
}

fn output(task: &Task, address: usize, size: usize, value: &[u8]) -> Result<usize, usize> {
    if size == 0 {
        return Ok(value.len());
    }
    if size < value.len() {
        return Err(errno::ERANGE);
    }
    copy_to_user(task, address, value).map_err(|_| errno::EFAULT)?;
    Ok(value.len())
}

#[derive(Clone, Copy)]
enum Operation {
    Set,
    Get,
    List,
    Remove,
}
fn dispatch(
    abi: &mut LinuxAbi,
    tf: &mut Trapframe,
    op: Operation,
    fd: bool,
    no_follow: bool,
) -> usize {
    let Some(task) = mytask() else {
        return errno::to_result(errno::EIO);
    };
    let a = [
        tf.get_arg(0),
        tf.get_arg(1),
        tf.get_arg(2),
        tf.get_arg(3),
        tf.get_arg(4),
    ];
    tf.increment_pc_next(&task);
    let result = (|| {
        // Validate bounded request data before any filesystem mutation.
        let mode = match (op, a[4] as u32) {
            (Operation::Set, 0) => xattr::SetMode::Upsert,
            (Operation::Set, 1) => xattr::SetMode::Create,
            (Operation::Set, 2) => xattr::SetMode::Replace,
            (Operation::Set, _) => return Err(errno::EINVAL),
            _ => xattr::SetMode::Upsert,
        };
        let attribute = if matches!(op, Operation::List) {
            Vec::new()
        } else {
            name(&task, a[1])?
        };
        let value = if matches!(op, Operation::Set) {
            if a[3] > xattr::MAX_VALUE {
                return Err(errno::E2BIG);
            }
            let mut value = vec![0; a[3]];
            copy_from_user(&task, a[2], &mut value).map_err(|_| errno::EFAULT)?;
            value
        } else {
            Vec::new()
        };
        let node = node(&task, abi, a[0], fd, no_follow)?;
        if !matches!(op, Operation::List) {
            supported_name(
                node.as_ref(),
                &attribute,
                matches!(op, Operation::Set | Operation::Remove),
            )?;
        }
        if matches!(op, Operation::Get) {
            if let Some(value) = abi.extensions.get_xattr(&task, node.as_ref(), &attribute) {
                return output(&task, a[2], a[3], &value?);
            }
        } else if matches!(op, Operation::Set | Operation::Remove) {
            writable(node.as_ref())?;
            if let Some(result) = abi.extensions.change_xattr(&task, node.as_ref(), &attribute, if matches!(op,Operation::Set) {Some(&value)}else {None}, mode) {
                return result.map(|_| 0);
            }
        }
        match op {
            Operation::Set => {
                writable(node.as_ref())?;
                node.set_xattr(&attribute, &value, mode).map_err(error)?;
                Ok(0)
            }
            Operation::Get => output(
                &task,
                a[2],
                a[3],
                &node.get_xattr(&attribute).map_err(error)?,
            ),
            Operation::List => {
                let mut list = Vec::new();
                for name in node.list_xattrs().map_err(error)? {
                    if supported_name(node.as_ref(), &name, false).is_ok() {
                        list.extend_from_slice(&name);
                        list.push(0);
                    }
                }
                if list.len() > xattr::MAX_VALUE {
                    return Err(errno::E2BIG);
                }
                output(&task, a[1], a[2], &list)
            }
            Operation::Remove => {
                writable(node.as_ref())?;
                node.remove_xattr(&attribute).map_err(error)?;
                Ok(0)
            }
        }
    })();
    result.unwrap_or_else(errno::to_result)
}

macro_rules! syscall {
    ($name:ident, $op:ident, $fd:expr, $nofollow:expr) => {
        pub fn $name(abi: &mut LinuxAbi, tf: &mut Trapframe) -> usize {
            dispatch(abi, tf, Operation::$op, $fd, $nofollow)
        }
    };
}
syscall!(sys_setxattr, Set, false, false);
syscall!(sys_lsetxattr, Set, false, true);
syscall!(sys_fsetxattr, Set, true, false);
syscall!(sys_getxattr, Get, false, false);
syscall!(sys_lgetxattr, Get, false, true);
syscall!(sys_fgetxattr, Get, true, false);
syscall!(sys_listxattr, List, false, false);
syscall!(sys_llistxattr, List, false, true);
syscall!(sys_flistxattr, List, true, false);
syscall!(sys_removexattr, Remove, false, false);
syscall!(sys_lremovexattr, Remove, false, true);
syscall!(sys_fremovexattr, Remove, true, false);
