//! Linux proc directories backed by native process, descriptor, and thread views.

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

#[derive(Clone)]
struct ProcDirectoryEntry {
    name: String,
    file_type: FileType,
}

fn proc_path(path: &str) -> &str {
    path.trim_end_matches('/')
}

fn process_path_target(task: &Task, path: &str) -> Option<Arc<Task>> {
    let selector = proc_path(path).strip_prefix("/proc/")?;
    if selector.is_empty() || selector.contains('/') {
        return None;
    }
    let id = match selector {
        "self" => task.get_thread_group_id(),
        "thread-self" => task.get_id(),
        name => task.get_namespace().resolve_global_id(name.parse().ok()?)?,
    };
    crate::sched::scheduler::get_task_by_id(id)
}

fn is_process_path(path: &str) -> bool {
    let Some(selector) = proc_path(path).strip_prefix("/proc/") else {
        return false;
    };
    !selector.is_empty()
        && !selector.contains('/')
        && (matches!(selector, "self" | "thread-self")
            || selector.bytes().all(|byte| byte.is_ascii_digit()))
}

fn process_directory_entries(caller: &Task, task: &Task) -> Vec<ProcDirectoryEntry> {
    let mut entries = alloc::vec![
        ProcDirectoryEntry {
            name: String::from("status"),
            file_type: FileType::RegularFile,
        },
        ProcDirectoryEntry {
            name: String::from("stat"),
            file_type: FileType::RegularFile,
        },
    ];
    if task.get_id() == task.get_thread_group_id() {
        entries.push(ProcDirectoryEntry {
            name: String::from("task"),
            file_type: FileType::Directory,
        });
    }
    if caller.vm_manager.shares_address_space(&task.vm_manager) {
        entries.push(ProcDirectoryEntry {
            name: String::from("maps"),
            file_type: FileType::RegularFile,
        });
    }
    entries
}

fn directory_metadata(file_id: u64, entries: &[ProcDirectoryEntry]) -> FileMetadata {
    let subdirectories = entries
        .iter()
        .filter(|entry| matches!(&entry.file_type, FileType::Directory))
        .count();
    FileMetadata {
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
        file_id,
        link_count: 2usize.saturating_add(subdirectories).min(u32::MAX as usize) as u32,
    }
}

fn live_threads(task: &Task) -> Vec<usize> {
    crate::sched::scheduler::get_task_pool()
        .task_ids_snapshot()
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

/// Resolve only a member of the named thread group, using the caller's PID
/// namespace for both IDs. Proc text fields share this directory resolution.
pub(super) fn thread_directory_target(caller: &Task, path: &str) -> Option<Arc<Task>> {
    let (_, thread_id) = task_path_target(caller, path)?;
    crate::sched::scheduler::get_task_by_id(thread_id?)
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
    if !is_task_path(path) {
        return None;
    }
    let Some((target, thread_id)) = task_path_target(caller, path) else {
        return Some(errno::to_result(errno::ENOENT));
    };
    if let Some(id) = thread_id {
        let target = crate::sched::scheduler::get_task_by_id(id)?;
        return Some(open_directory(
            abi,
            caller,
            proc_path(path),
            flags,
            process_directory_entries(caller, &target),
            id as u64,
        ));
    }
    let namespace = caller.get_namespace();
    let entries = live_threads(&target)
        .into_iter()
        .filter_map(|id| namespace.resolve_local_id(id))
        .map(|id| ProcDirectoryEntry {
            name: format!("{id}"),
            file_type: FileType::Directory,
        })
        .collect();
    Some(open_directory(
        abi,
        caller,
        proc_path(path),
        flags,
        entries,
        target.get_thread_group_id() as u64,
    ))
}

/// Android's procinfo opens fields relative to `/proc/<pid>`. Keep this
/// synthetic directory restricted to processes already visible in the
/// caller's native task namespace, matching the status policy.
pub(super) fn open_process_directory(
    abi: &mut LinuxAbi,
    caller: &Task,
    path: &str,
    flags: i32,
) -> Option<usize> {
    if !is_process_path(path) {
        return None;
    }
    let Some(target) = process_path_target(caller, path) else {
        return Some(errno::to_result(errno::ENOENT));
    };
    let entries = process_directory_entries(caller, &target);
    let canonical_path = caller
        .get_namespace()
        .resolve_local_id(target.get_id())
        .map(|pid| format!("/proc/{pid}"))
        .unwrap_or_else(|| String::from(proc_path(path)));
    Some(open_directory(
        abi,
        caller,
        &canonical_path,
        flags,
        entries,
        target.get_id() as u64,
    ))
}

/// Metadata for synthetic process and task directories. They are visible only
/// through Linux ABI stat calls and are never mounted in VFS.
pub(super) fn self_task_metadata(task: &Task, path: &str) -> Option<Option<FileMetadata>> {
    task_path_metadata(task, path)
}

pub(super) fn task_path_metadata(caller: &Task, path: &str) -> Option<Option<FileMetadata>> {
    if let Some((target, thread_id)) = task_path_target(caller, path) {
        let entries = if let Some(id) = thread_id {
            let thread = crate::sched::scheduler::get_task_by_id(id)?;
            process_directory_entries(caller, &thread)
        } else {
            let namespace = caller.get_namespace();
            live_threads(&target)
                .into_iter()
                .filter_map(|id| namespace.resolve_local_id(id))
                .map(|id| ProcDirectoryEntry {
                    name: format!("{id}"),
                    file_type: FileType::Directory,
                })
                .collect()
        };
        return Some(Some(directory_metadata(
            thread_id.unwrap_or_else(|| target.get_thread_group_id()) as u64,
            &entries,
        )));
    }
    if is_task_path(path) {
        return Some(None);
    }
    if !is_process_path(path) {
        return None;
    }
    let Some(target) = process_path_target(caller, path) else {
        return Some(None);
    };
    Some(Some(directory_metadata(
        target.get_id() as u64,
        &process_directory_entries(caller, &target),
    )))
}

/// Resolve fields opened relative to a synthetic proc directory.
pub(super) fn directory_path(file: &dyn FileObject) -> Option<&str> {
    file.as_any()
        .downcast_ref::<ProcIdDirectory>()
        .map(|directory| directory.path.as_str())
}

pub(super) fn is_self_fd_directory(path: &str) -> bool {
    matches!(
        path.trim_end_matches('/'),
        "/proc/self/fd" | "/proc/thread-self/fd"
    )
}

pub(super) fn open_self_fd_directory(abi: &mut LinuxAbi, task: &Task, flags: i32) -> usize {
    let entries = abi
        .allocated_fds()
        .into_iter()
        .map(|fd| ProcDirectoryEntry {
            name: format!("{fd}"),
            file_type: FileType::SymbolicLink(String::new()),
        })
        .collect();
    open_directory(abi, task, "/proc/self/fd", flags, entries, 1)
}

fn open_directory(
    abi: &mut LinuxAbi,
    task: &Task,
    path: &str,
    flags: i32,
    entries: Vec<ProcDirectoryEntry>,
    file_id: u64,
) -> usize {
    if flags & 3 != 0 {
        return errno::to_result(errno::EISDIR);
    }
    let directory = Arc::new(ProcIdDirectory {
        path: String::from(path),
        entries,
        position: IrqRwSpinLock::new(0),
        file_id,
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
        let Some((identity, _)) =
            crate::network::NetworkManager::get_manager().socket_handle_identity(socket.as_arc())
        else {
            return Some(Err(errno::ENOENT));
        };
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
    path: String,
    entries: Vec<ProcDirectoryEntry>,
    position: IrqRwSpinLock<usize>,
    file_id: u64,
}

impl StreamOps for ProcIdDirectory {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut position = self.position.write();
        let (name, file_type) = match *position {
            0 => (String::from("."), FileType::Directory),
            1 => (String::from(".."), FileType::Directory),
            n => match self.entries.get(n - 2) {
                Some(entry) => (entry.name.clone(), entry.file_type.clone()),
                None => return Ok(0),
            },
        };
        let entry = DirectoryEntry::from_internal(&DirectoryEntryInternal {
            name,
            file_type,
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
        let end = self.entries.len().saturating_add(2);
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
        Ok(directory_metadata(self.file_id, &self.entries))
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
        let object = leader
            .handle_table
            .get(abi.get_handle(fd).unwrap())
            .unwrap();
        let file = object.as_file().unwrap();
        let directory = file.as_any().downcast_ref::<ProcIdDirectory>().unwrap();
        let mut entries: Vec<usize> = directory
            .entries
            .iter()
            .map(|entry| entry.name.parse().unwrap())
            .collect();
        entries.sort_unstable();
        let mut expected = alloc::vec![leader.get_namespace_id(), members[0].get_namespace_id()];
        expected.sort_unstable();
        assert_eq!(entries, expected);
        assert_eq!(directory.metadata().unwrap().link_count, 4);
        assert_eq!(
            self_task_metadata(&leader, "/proc/self/task")
                .unwrap()
                .unwrap()
                .link_count,
            4
        );
        let blocked_path = format!("/proc/self/task/{}", members[0].get_namespace_id());
        assert!(
            self_task_metadata(&leader, &blocked_path)
                .unwrap()
                .is_some()
        );
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
        assert_eq!(process_directory.entries.len(), 2);
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
                    .unwrap()
                    .is_none()
            );
        }

        drop(object);
        drop(members);
        drop(leader);
        reset();
    }

    #[test_case]
    fn process_directory_exposes_only_available_fields_and_supports_directory_io() {
        use crate::sched::scheduler::{get_task_by_id, register_task, reset};
        use crate::task::{TaskType, namespace::TaskNamespace};

        reset();
        let namespace = TaskNamespace::new_root("proc-process-test".into());
        namespace.allocate_task_id_for(usize::MAX);
        let task = Task::new("ProcProcess".into(), 0, TaskType::User);
        task.set_namespace(namespace.clone());
        let task_id = register_task(task);
        let task = get_task_by_id(task_id).unwrap();
        let pid = task.get_namespace_id();
        let path = format!("/proc/{pid}");

        let mut abi = LinuxAbi::default();
        let fd = open_process_directory(&mut abi, &task, &path, O_CLOEXEC).unwrap();
        assert_eq!(abi.get_fd_flags(fd), Some(FD_CLOEXEC));
        let handle = abi.get_handle(fd).unwrap();
        let object = task.handle_table.get(handle).unwrap();
        let file = object.as_file().unwrap();
        assert_eq!(directory_path(file), Some(path.as_str()));
        let opened_metadata = file.metadata().unwrap();
        let path_metadata = task_path_metadata(&task, &path).unwrap().unwrap();
        assert_eq!(opened_metadata.file_type, FileType::Directory);
        assert_eq!(opened_metadata.file_id, path_metadata.file_id);
        assert_eq!(opened_metadata.link_count, path_metadata.link_count);

        let mut names = Vec::new();
        let mut storage = [0u8; size_of::<DirectoryEntry>()];
        while file.read(&mut storage).unwrap() != 0 {
            let entry =
                unsafe { core::ptr::read_unaligned(storage.as_ptr().cast::<DirectoryEntry>()) };
            names.push(String::from(entry.name_str().unwrap()));
        }
        assert_eq!(
            names.iter().map(String::as_str).collect::<Vec<_>>(),
            alloc::vec![".", "..", "status", "stat", "task", "maps"]
        );

        let other = Task::new("OtherProcess".into(), 0, TaskType::User);
        other.set_namespace(namespace);
        let other_id = register_task(other);
        let other = get_task_by_id(other_id).unwrap();
        let other_entries = process_directory_entries(&task, &other);
        assert!(!other_entries.iter().any(|entry| entry.name == "maps"));

        drop(object);
        drop(other);
        drop(task);
        reset();
    }

    #[test_case]
    fn proc_thread_directories_keep_thread_identity_and_validate_group_membership() {
        use crate::sched::scheduler::{get_task_by_id, register_task, reset};
        use crate::task::{TaskType, clear_mock_current_task, set_mock_current_task};

        reset();
        let leader_id = register_task(Task::new("ProcLeader".into(), 0, TaskType::Kernel));
        let leader = get_task_by_id(leader_id).unwrap();
        let mut member = Task::new("ProcMember".into(), 0, TaskType::Kernel);
        member.set_namespace(leader.get_namespace());
        member.set_thread_group_id(leader_id);
        let member_id = register_task(member);
        let member = get_task_by_id(member_id).unwrap();
        let tid = member.get_namespace_id();
        let root = format!("/proc/{tid}");
        let nested = format!("/proc/{}/task/{tid}", leader.get_namespace_id());
        let mut abi = LinuxAbi::default();
        set_mock_current_task(member.clone());
        for (path, canonical) in [
            ("/proc/thread-self", root.as_str()),
            (root.as_str(), root.as_str()),
            (nested.as_str(), nested.as_str()),
        ] {
            let fd = open_process_directory(&mut abi, &member, path, 0)
                .or_else(|| open_task_directory(&mut abi, &member, path, 0))
                .unwrap();
            let object = member
                .handle_table
                .get(abi.get_handle(fd).unwrap())
                .unwrap();
            let file = object.as_file().unwrap();
            assert_eq!(directory_path(file), Some(canonical));
            assert_eq!(file.metadata().unwrap().file_id, member_id as u64);
            assert_eq!(
                task_path_metadata(&member, path).unwrap().unwrap().file_id,
                member_id as u64
            );
            let field = format!("{canonical}/status");
            let status_fd =
                super::super::proc_text::open_proc_text_file(&mut abi, &member, &field, 0).unwrap();
            let status_object = member
                .handle_table
                .get(abi.get_handle(status_fd).unwrap())
                .unwrap();
            let mut bytes = [0u8; 512];
            let len = status_object.as_file().unwrap().read(&mut bytes).unwrap();
            let text = core::str::from_utf8(&bytes[..len]).unwrap();
            assert!(text.contains(&format!("Pid:\t{tid}\n")));
            assert!(text.contains(&format!("Tgid:\t{}\n", leader.get_namespace_id())));
        }
        let other_id = register_task(Task::new("OtherProc".into(), 0, TaskType::Kernel));
        let other = get_task_by_id(other_id).unwrap();
        let wrong_group = format!("/proc/{}/task/{tid}", other.get_namespace_id());
        assert!(thread_directory_target(&member, &wrong_group).is_none());
        assert!(
            super::super::proc_text::open_proc_text_file(
                &mut abi,
                &member,
                &format!("{wrong_group}/status"),
                0
            )
            .is_none()
        );
        clear_mock_current_task();
        drop(other);
        drop(member);
        drop(leader);
        reset();
    }

    #[test_case]
    fn process_directory_rejects_missing_and_foreign_namespace_pids() {
        use crate::sched::scheduler::{get_task_by_id, register_task, reset};
        use crate::task::{TaskType, namespace::TaskNamespace};

        reset();
        let caller_namespace = TaskNamespace::new_root("proc-caller".into());
        let caller = Task::new("ProcCaller".into(), 0, TaskType::User);
        caller.set_namespace(caller_namespace);
        let caller_id = register_task(caller);
        let caller = get_task_by_id(caller_id).unwrap();

        let mut abi = LinuxAbi::default();
        let missing = String::from("/proc/999999");
        assert_eq!(
            open_process_directory(&mut abi, &caller, &missing, 0),
            Some(errno::to_result(errno::ENOENT))
        );
        assert!(matches!(task_path_metadata(&caller, &missing), Some(None)));

        let foreign_namespace = TaskNamespace::new_root("proc-foreign".into());
        // Leave the caller with no mapping for the foreign process's local ID.
        foreign_namespace.allocate_task_id_for(usize::MAX);
        let foreign = Task::new("ForeignProcess".into(), 0, TaskType::User);
        foreign.set_namespace(foreign_namespace);
        let foreign_id = register_task(foreign);
        let foreign = get_task_by_id(foreign_id).unwrap();
        let foreign_pid = foreign.get_namespace_id();
        let foreign_path = format!("/proc/{foreign_pid}");
        assert_eq!(caller.get_namespace().resolve_global_id(foreign_pid), None);
        assert_eq!(
            open_process_directory(&mut abi, &caller, &foreign_path, 0),
            Some(errno::to_result(errno::ENOENT))
        );
        assert!(matches!(
            task_path_metadata(&caller, &foreign_path),
            Some(None)
        ));

        drop(foreign);
        drop(caller);
        reset();
    }
}
