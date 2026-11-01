//! Linux proc directories backed by the calling process's descriptors and threads.

use alloc::{format, string::String, sync::Arc, vec::Vec};
use core::{any::Any, mem::size_of};

use crate::{
    abi::linux::generic::{
        LinuxAbi, errno,
        fs::{FD_CLOEXEC, O_CLOEXEC},
    },
    arch::Trapframe,
    fs::{
        DirectoryEntry, DirectoryEntryInternal, FileMetadata, FilePermission, FileType, SeekFrom,
    },
    object::{
        KernelObject,
        capability::{
            ControlOps, FileObject, MemoryMappingInfo, MemoryMappingOps, ReadyInterest,
            SelectWaitOutcome, Selectable, StreamError, StreamOps,
        },
    },
    sync::IrqRwSpinLock,
    task::{Task, TaskState},
};

fn live_threads(task: &Task) -> Vec<usize> {
    crate::sched::scheduler::get_task_pool().task_ids_snapshot()
        .into_iter()
        .filter(|id| {
            crate::sched::scheduler::get_task_by_id(*id).is_some_and(|candidate| {
                candidate.get_thread_group_id() == task.get_thread_group_id()
                    && !matches!(
                        candidate.get_state(),
                        TaskState::Zombie | TaskState::Terminated
                    )
            })
        })
        .collect()
}

fn task_path_target(task: &Task, path: &str) -> Option<(Arc<Task>, Option<usize>)> {
    let path = path.trim_end_matches('/');
    let namespace = task.get_namespace();
    let self_suffix = path
        .strip_prefix("/proc/self/task")
        .or_else(|| path.strip_prefix("/proc/thread-self/task"));
    let (group_id, thread_id) = if let Some(suffix) = self_suffix {
        if !suffix.is_empty() && !suffix.starts_with('/') {
            return None;
        }
        let thread_id = if suffix.is_empty() {
            None
        } else {
            let name = suffix.strip_prefix('/')?;
            if name.is_empty() || name.contains('/') {
                return None;
            }
            Some(namespace.resolve_global_id(name.parse().ok()?)?)
        };
        (task.get_thread_group_id(), thread_id)
    } else {
        let (pid, suffix) = path.strip_prefix("/proc/")?.split_once("/task")?;
        if pid.contains('/') || (!suffix.is_empty() && !suffix.starts_with('/')) {
            return None;
        }
        let group_id = namespace.resolve_global_id(pid.parse().ok()?)?;
        let thread_id = if suffix.is_empty() {
            None
        } else {
            let name = suffix.strip_prefix('/')?;
            if name.is_empty() || name.contains('/') {
                return None;
            }
            Some(namespace.resolve_global_id(name.parse().ok()?)?)
        };
        (group_id, thread_id)
    };
    let leader = crate::sched::scheduler::get_task_by_id(group_id)?;
    if leader.get_thread_group_id() != group_id {
        return None;
    }
    if thread_id.is_some_and(|id| !live_threads(&leader).contains(&id)) {
        return None;
    }
    Some((leader, thread_id))
}

fn is_task_path(path: &str) -> bool {
    let path = path.trim_end_matches('/');
    if let Some(suffix) = path
        .strip_prefix("/proc/self/task")
        .or_else(|| path.strip_prefix("/proc/thread-self/task"))
    {
        return suffix.is_empty()
            || suffix
                .strip_prefix('/')
                .is_some_and(|name| !name.is_empty() && !name.contains('/'));
    }
    let Some((pid, suffix)) = path
        .strip_prefix("/proc/")
        .and_then(|path| path.split_once("/task"))
    else {
        return false;
    };
    !pid.is_empty()
        && !pid.contains('/')
        && (suffix.is_empty()
            || suffix
                .strip_prefix('/')
                .is_some_and(|name| !name.is_empty() && !name.contains('/')))
}

pub(super) fn open_self_task_directory(abi: &mut LinuxAbi, task: &Task, flags: i32) -> usize {
    open_task_directory(abi, task, "/proc/self/task", flags)
        .unwrap_or_else(|| errno::to_result(errno::ENOENT))
}

/// Open a synthetic task directory reached through either self or an explicit
/// process ID. Android uses `/proc/<pid>/task/<tid>` to validate render-thread
/// ownership from another process, so those names must resolve in the same
/// procfs view as `/proc/<pid>/status`.
pub(super) fn open_task_directory(
    abi: &mut LinuxAbi,
    caller: &Task,
    path: &str,
    flags: i32,
) -> Option<usize> {
    let (target, thread_id) = task_path_target(caller, path)?;
    if thread_id.is_some() {
        return Some(errno::to_result(errno::ENOTDIR));
    }
    let namespace = caller.get_namespace();
    let ids = live_threads(&target)
        .into_iter()
        .filter_map(|id| namespace.resolve_local_id(id))
        .collect();
    Some(open_directory(abi, caller, flags, Some(ids)))
}

/// Metadata for the proc task directory used by Chromium's thread checks.
/// It is visible only through Linux ABI stat calls, never mounted in VFS.
pub(super) fn self_task_metadata(task: &Task, path: &str) -> Option<Option<FileMetadata>> {
    task_path_metadata(task, path)
}

pub(super) fn task_path_metadata(
    caller: &Task,
    path: &str,
) -> Option<Option<FileMetadata>> {
    let Some((target, thread_id)) = task_path_target(caller, path) else {
        return is_task_path(path).then_some(None);
    };
    let link_count = if thread_id.is_some() {
        2
    } else {
        2 + live_threads(&target).len()
    };
    Some(Some(FileMetadata {
        file_type: FileType::Directory,
        size: 0,
        permissions: FilePermission {
            read: true,
            write: false,
            execute: true,
        },
        created_time: 0,
        modified_time: 0,
        accessed_time: 0,
        file_id: thread_id.unwrap_or_else(|| target.get_thread_group_id()) as u64,
        link_count: link_count.min(u32::MAX as usize) as u32,
    }))
}

pub(super) fn is_self_fd_directory(path: &str) -> bool {
    matches!(
        path.trim_end_matches('/'),
        "/proc/self/fd" | "/proc/thread-self/fd"
    )
}

pub(super) fn open_self_fd_directory(abi: &mut LinuxAbi, task: &Task, flags: i32) -> usize {
    open_directory(abi, task, flags, None)
}

fn open_directory(
    abi: &mut LinuxAbi,
    task: &Task,
    flags: i32,
    threads: Option<Vec<usize>>,
) -> usize {
    if flags & 3 != 0 {
        return errno::to_result(errno::EISDIR);
    }
    let directory = alloc::sync::Arc::new(ProcIdDirectory {
        thread_entries: threads.is_some(),
        position: IrqRwSpinLock::new(0),
        descriptors: IrqRwSpinLock::new(Vec::new()),
    });
    let handle = match task
        .handle_table
        .insert(KernelObject::File(directory.clone()))
    {
        Ok(handle) => handle,
        Err(_) => return errno::to_result(errno::ENFILE),
    };
    let fd = match abi.allocate_fd(handle) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = task.handle_table.remove(handle);
            return errno::to_result(errno::EMFILE);
        }
    };
    if flags & O_CLOEXEC != 0 {
        let _ = abi.set_fd_flags(fd, FD_CLOEXEC);
    }
    *directory.descriptors.write() = threads.unwrap_or_else(|| abi.allocated_fds());
    fd
}

pub(super) fn self_fd_link_target(
    abi: &LinuxAbi,
    task: &Task,
    path: &str,
) -> Option<Result<String, usize>> {
    let fd_name = path
        .strip_prefix("/proc/self/fd/")
        .or_else(|| path.strip_prefix("/proc/thread-self/fd/"))?;
    let Some(fd) = fd_name.parse::<usize>().ok() else {
        return Some(Err(errno::ENOENT));
    };
    let Some(handle) = abi.get_handle(fd) else {
        return Some(Err(errno::ENOENT));
    };
    let Some(object) = task.handle_table.get(handle) else {
        return Some(Err(errno::ENOENT));
    };
    if let KernelObject::MountNamespace(namespace) = &object {
        return Some(Ok(format!("mnt:[{}]", namespace.id())));
    }
    #[cfg(feature = "network")]
    if let KernelObject::Socket(socket) = &object {
        let Some((identity, _)) = crate::network::NetworkManager::get_manager()
            .socket_handle_identity(socket.as_arc()) else { return Some(Err(errno::ENOENT)); };
        return Some(Ok(format!("socket:[{identity}]")));
    }
    let path = object
        .as_file()
        .and_then(|file| {
            file.as_any()
                .downcast_ref::<crate::fs::vfs_v2::core::VfsFileObject>()
        })
        .map(|file| file.get_original_path().into())
        .unwrap_or_else(|| format!("anon_inode:[{handle}]"));
    Some(Ok(path))
}

struct ProcIdDirectory {
    thread_entries: bool,
    position: IrqRwSpinLock<usize>,
    descriptors: IrqRwSpinLock<Vec<usize>>,
}

impl StreamOps for ProcIdDirectory {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut position = self.position.write();
        let descriptors = self.descriptors.read();
        let name = match *position {
            0 => String::from("."),
            1 => String::from(".."),
            n => match descriptors.get(n - 2) {
                Some(fd) => format!("{fd}"),
                None => return Ok(0),
            },
        };
        let entry = DirectoryEntry::from_internal(&DirectoryEntryInternal {
            name,
            file_type: if *position < 2 || self.thread_entries {
                FileType::Directory
            } else {
                FileType::SymbolicLink(String::new())
            },
            size: 0,
            file_id: *position as u64 + 1,
            metadata: None,
        });
        if buffer.len() < size_of::<DirectoryEntry>() {
            return Err(StreamError::InvalidArgument);
        }
        let bytes = unsafe {
            core::slice::from_raw_parts(
                (&entry as *const DirectoryEntry).cast::<u8>(),
                size_of::<DirectoryEntry>(),
            )
        };
        buffer[..bytes.len()].copy_from_slice(bytes);
        *position += 1;
        Ok(bytes.len())
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::PermissionDenied)
    }
}

impl ControlOps for ProcIdDirectory {}

impl MemoryMappingOps for ProcIdDirectory {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        Err("cannot map a proc directory")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for ProcIdDirectory {
    fn wait_until_ready(
        &self,
        _interest: ReadyInterest,
        _trapframe: &mut Trapframe,
        _timeout_ns: Option<u64>,
        _min_wait_ns: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}

impl FileObject for ProcIdDirectory {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let end = self.descriptors.read().len().saturating_add(2);
        let mut position = self.position.write();
        let next = match whence {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::Current(offset) => *position as i128 + offset as i128,
            SeekFrom::End(offset) => end as i128 + offset as i128,
        };
        if next < 0 || next > usize::MAX as i128 {
            return Err(StreamError::InvalidArgument);
        }
        *position = next as usize;
        Ok(*position as u64)
    }

    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::Directory,
            size: 0,
            permissions: FilePermission {
                read: true,
                write: false,
                execute: true,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 1,
            link_count: if self.thread_entries {
                2 + self.descriptors.read().len() as u32
            } else {
                2
            },
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn self_task_directory_includes_blocked_threads_with_namespace_local_ids() {
        use crate::sched::scheduler::{get_all_task_ids, get_task_by_id, register_task, reset};
        use crate::task::{BlockedType, TaskType, namespace::TaskNamespace};

        reset();
        let namespace = TaskNamespace::new_root("proc-task-test".into());
        // Ensure directory entries exercise translation rather than global IDs.
        namespace.allocate_task_id_for(usize::MAX);
        let leader = Task::new("ProcLeader".into(), 0, TaskType::Kernel);
        leader.set_namespace(namespace.clone());
        let leader_id = register_task(leader);
        let leader = get_task_by_id(leader_id).unwrap();
        let mut members = Vec::new();
        for state in [
            TaskState::Blocked(BlockedType::Interruptible),
            TaskState::Zombie,
            TaskState::Terminated,
        ] {
            let mut member = Task::new("ProcThread".into(), 0, TaskType::Kernel);
            member.set_namespace(namespace.clone());
            member.set_thread_group_id(leader_id);
            member.set_state(state);
            let id = register_task(member);
            members.push(get_task_by_id(id).unwrap());
        }
        let unrelated = Task::new("OtherProcess".into(), 0, TaskType::Kernel);
        unrelated.set_namespace(namespace.clone());
        let unrelated_id = register_task(unrelated);
        assert!(!get_all_task_ids().contains(&members[0].get_id()));
        let mut ids = live_threads(&leader);
        ids.sort_unstable();
        let mut expected = alloc::vec![leader_id, members[0].get_id()];
        expected.sort_unstable();
        assert_eq!(ids, expected);

        let mut abi = LinuxAbi::default();
        let fd = open_self_task_directory(&mut abi, &leader, 0);
        let object = leader.handle_table.get(abi.get_handle(fd).unwrap()).unwrap();
        let file = object.as_file().unwrap();
        let directory = file.as_any().downcast_ref::<ProcIdDirectory>().unwrap();
        let mut entries = directory.descriptors.read().clone();
        entries.sort_unstable();
        let mut expected = alloc::vec![leader.get_namespace_id(), members[0].get_namespace_id()];
        expected.sort_unstable();
        assert_eq!(entries, expected);
        assert_eq!(directory.metadata().unwrap().link_count, 4);
        assert_eq!(
            self_task_metadata(&leader, "/proc/self/task")
                .unwrap().unwrap().link_count,
            4
        );
        let blocked_path = format!("/proc/self/task/{}", members[0].get_namespace_id());
        assert!(self_task_metadata(&leader, &blocked_path).unwrap().is_some());
        let process_path = format!("/proc/{}/task", leader.get_namespace_id());
        assert_eq!(
            task_path_metadata(&leader, &process_path)
                .unwrap()
                .unwrap()
                .link_count,
            4
        );
        let process_fd = open_task_directory(&mut abi, &leader, &process_path, 0).unwrap();
        let process_object = leader
            .handle_table
            .get(abi.get_handle(process_fd).unwrap())
            .unwrap();
        let process_directory = process_object
            .as_file()
            .unwrap()
            .as_any()
            .downcast_ref::<ProcIdDirectory>()
            .unwrap();
        assert_eq!(process_directory.descriptors.read().len(), 2);
        let process_thread = format!(
            "/proc/{}/task/{}",
            leader.get_namespace_id(),
            members[0].get_namespace_id()
        );
        assert_eq!(
            task_path_metadata(&leader, &process_thread)
                .unwrap()
                .unwrap()
                .file_id,
            members[0].get_id() as u64
        );
        let unrelated_path = format!(
            "/proc/{}/task/{}",
            leader.get_namespace_id(),
            namespace.resolve_local_id(unrelated_id).unwrap()
        );
        assert!(matches!(
            task_path_metadata(&leader, &unrelated_path),
            Some(None)
        ));
        for id in [members[1].get_id(), members[2].get_id(), unrelated_id] {
            let local_id = namespace.resolve_local_id(id).unwrap();
            assert!(
                self_task_metadata(&leader, &format!("/proc/self/task/{local_id}"))
                    .unwrap().is_none()
            );
        }

        drop(object);
        drop(members);
        drop(leader);
        reset();
    }
}
