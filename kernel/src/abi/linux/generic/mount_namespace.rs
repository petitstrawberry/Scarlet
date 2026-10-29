//! Linux namespace descriptors carry the native MountNamespace capability.
use alloc::{format, string::String};
use crate::{arch::Trapframe, fs::vfs_v2::namespace::{MountNamespace, JoinError},
    object::{KernelObject, handle::SpecialSemantics}, task::Task};
use super::{LinuxAbi, errno, fs::{self, LinuxStat, O_CLOEXEC, O_DIRECTORY, O_NOFOLLOW, FD_CLOEXEC}};

pub(super) fn is_path(path: &str) -> bool {
    matches!(path, "/proc/self/ns/mnt" | "/proc/thread-self/ns/mnt")
}
pub(super) fn link(task: &Task, path: &str) -> Option<String> {
    is_path(path).then(|| format!("mnt:[{}]", task.mount_namespace.read().id()))
}
pub(super) fn object_stat(namespace: &MountNamespace) -> LinuxStat {
    let mut stat = fs::proc_exe_linux_stat(namespace.id(), 0);
    stat.st_dev = 0x6e73; // namespace capability filesystem, distinct from VFS/proc inode domains
    stat.st_mode = fs::S_IFREG | 0o444;
    stat
}
pub(super) fn path_stat(task: &Task, path: &str, nofollow: bool) -> Option<LinuxStat> {
    if !is_path(path) { return None; }
    let namespace = task.mount_namespace.read();
    Some(if nofollow { fs::proc_exe_linux_stat(namespace.id(), format!("mnt:[{}]", namespace.id()).len()) }
        else { object_stat(&namespace) })
}
pub(super) fn open(abi: &mut LinuxAbi, task: &Task, path: &str, flags: i32) -> Option<usize> {
    if !is_path(path) { return None; }
    Some(open_current(abi, task, flags).unwrap_or_else(errno::to_result))
}
fn open_current(abi: &mut LinuxAbi, task: &Task, flags: i32) -> Result<usize, usize> {
    if flags & 3 != 0 || flags & (0x40 | 0x200) != 0 { return Err(errno::EACCES); }
    if flags & O_DIRECTORY != 0 { return Err(errno::ENOTDIR); }
    if flags & O_NOFOLLOW != 0 { return Err(errno::ELOOP); }
    let namespace = task.capture_mount_namespace().map_err(|_| errno::ENOMEM)?;
    let handle = task.handle_table.insert(KernelObject::MountNamespace(namespace)).map_err(|_| errno::EMFILE)?;
    let mut metadata = task.handle_table.get_metadata(handle).ok_or(errno::EBADF)?;
    metadata.special_semantics = if flags & O_CLOEXEC != 0 { Some(SpecialSemantics::CloseOnExec) } else { None };
    task.handle_table.update_metadata(handle, metadata).map_err(|_| errno::EBADF)?;
    match abi.allocate_fd(handle) {
        Ok(fd) => {
            if flags & O_CLOEXEC != 0 { let _ = abi.set_fd_flags(fd, FD_CLOEXEC); }
            Ok(fd)
        }
        Err(_) => { task.handle_table.remove(handle); Err(errno::EMFILE) }
    }
}
pub fn sys_setns(abi: &mut LinuxAbi, frame: &mut Trapframe) -> usize {
    let task = crate::task::mytask().unwrap();
    let fd = frame.get_arg(0) as u32 as usize;
    let kind = frame.get_arg(1) as u32;
    frame.increment_pc_next(&task);
    setns(abi, &task, fd, kind).map_or_else(errno::to_result, |_| 0)
}
pub(super) fn setns(abi: &LinuxAbi, task: &Task, fd: usize, kind: u32) -> Result<(), usize> {
    if kind != 0 && kind != 0x20000 { return Err(errno::EINVAL); }
    let handle = abi.get_handle(fd).ok_or(errno::EBADF)?;
    let namespace = match task.handle_table.get(handle) {
        Some(KernelObject::MountNamespace(namespace)) => namespace,
        Some(_) => return Err(errno::EINVAL), None => return Err(errno::EBADF),
    };
    task.join_mount_namespace(namespace).map_err(|error| match error {
        JoinError::Authority => errno::EPERM,
        JoinError::SharedContext | JoinError::IncompatibleView => errno::EINVAL,
        JoinError::WorkingDirectory => errno::ENOENT,
        JoinError::Context => errno::EIO,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    #[test_case]
    fn namespace_descriptor_has_stable_native_identity_and_checks_type_kind_and_authority() {
        let task = crate::task::new_user_task("linux-namespace-fd".into(), 0);
        task.set_vfs(Arc::new(crate::fs::VfsManager::new()));
        let mut abi = LinuxAbi::default();
        let fd = open_current(&mut abi, &task, 0).unwrap();
        let namespace = task.capture_mount_namespace().unwrap();
        let path = path_stat(&task, "/proc/self/ns/mnt", false).unwrap();
        assert_eq!(path.st_ino, namespace.id());
        assert_eq!(path.st_mode & fs::S_IFMT, fs::S_IFREG);
        assert_eq!(path_stat(&task, "/proc/self/ns/mnt", true).unwrap().st_mode & fs::S_IFMT, fs::S_IFLNK);
        assert_eq!(link(&task, "/proc/self/ns/mnt").unwrap(), format!("mnt:[{}]", namespace.id()));
        assert!(setns(&abi, &task, fd, 0).is_ok());
        assert_eq!(setns(&abi, &task, fd, 1), Err(errno::EINVAL));
        assert_eq!(setns(&abi, &task, usize::MAX, 0), Err(errno::EBADF));
        assert_eq!(open_current(&mut abi, &task, O_NOFOLLOW), Err(errno::ELOOP));
        assert_eq!(open_current(&mut abi, &task, 2), Err(errno::EACCES));
        let file = task.get_vfs().unwrap().open("/", 0).unwrap();
        let handle = task.handle_table.insert(file).unwrap();
        let wrong = abi.allocate_fd(handle).unwrap();
        assert_eq!(setns(&abi, &task, wrong, 0x20000), Err(errno::EINVAL));
        task.credentials.write().set_authority(0, 0).unwrap();
        assert_eq!(setns(&abi, &task, fd, 0x20000), Err(errno::EPERM));
    }
}
