//! Linux `/proc/<pid>/oom_score_adj` adapter for native reclaim protection.

use alloc::{
    format,
    string::String,
    sync::{Arc, Weak},
};
use core::any::Any;

use crate::{
    abi::linux::generic::{
        LinuxAbi, errno,
        fs::{FD_CLOEXEC, O_CLOEXEC, O_DIRECTORY},
    },
    arch::Trapframe,
    fs::{FileMetadata, FilePermission, FileSystemError, FileSystemErrorKind, FileType, SeekFrom},
    object::{
        KernelObject,
        capability::{
            ControlOps, FileObject, MemoryMappingInfo, MemoryMappingOps, ReadyInterest,
            SelectWaitOutcome, Selectable, StreamError, StreamOps,
        },
    },
    shield::identity::MANAGE_RECLAIM,
    sync::IrqRwSpinLock,
    task::{Task, TaskState, reclaim_preference::UpdateError},
};

const FIELD: &str = "oom_score_adj";
const LINUX_SCORE_ADJ_MIN: i32 = -1000;
const LINUX_SCORE_ADJ_MAX: i32 = 1000;
const MAX_WRITE_BYTES: usize = 64;

fn score_to_protection(score: i32) -> u16 {
    ((LINUX_SCORE_ADJ_MAX - score) as u32 * u32::from(u16::MAX) / 2000) as u16
}

fn protection_to_score(protection: u16) -> i32 {
    let scaled = u32::from(protection) * 2000 + u32::from(u16::MAX) / 2;
    LINUX_SCORE_ADJ_MAX - (scaled / u32::from(u16::MAX)) as i32
}

fn live(task: &Task) -> bool {
    !matches!(task.get_state(), TaskState::Zombie | TaskState::Terminated)
}

fn process_leader(group_id: usize, thread_id: Option<usize>) -> Result<Arc<Task>, usize> {
    let named = crate::sched::scheduler::get_task_by_id(group_id).ok_or(errno::ENOENT)?;
    if !live(&named) {
        return Err(errno::ENOENT);
    }
    let leader_id = named.get_thread_group_id();
    let leader = crate::sched::scheduler::get_task_by_id(leader_id).ok_or(errno::ENOENT)?;
    if leader.get_thread_group_id() != leader_id || !live(&leader) {
        return Err(errno::ENOENT);
    }
    if let Some(thread_id) = thread_id {
        if group_id != leader_id {
            return Err(errno::ENOENT);
        }
        let thread = crate::sched::scheduler::get_task_by_id(thread_id).ok_or(errno::ENOENT)?;
        if thread.get_thread_group_id() != leader_id || !live(&thread) {
            return Err(errno::ENOENT);
        }
    }
    Ok(leader)
}

/// Resolve every spelling at which Linux exposes this process-level control.
/// Numeric IDs are interpreted in the caller's namespace. The returned task is
/// the thread-group leader, regardless of whether the path named a member.
fn path_target(caller: &Task, path: &str) -> Option<Result<Arc<Task>, usize>> {
    let path = path.trim_end_matches('/');
    let name = path
        .strip_prefix("/proc/")?
        .strip_suffix(&format!("/{FIELD}"))?;
    if name == "self" {
        return Some(process_leader(caller.get_thread_group_id(), None));
    }
    if name == "thread-self" {
        return Some(process_leader(
            caller.get_thread_group_id(),
            Some(caller.get_id()),
        ));
    }

    let (group, thread) = if let Some((group, thread)) = name.split_once("/task/") {
        if group.is_empty() || thread.is_empty() || thread.contains('/') {
            return Some(Err(errno::ENOENT));
        }
        (group, Some(thread))
    } else {
        if name.is_empty() || name.contains('/') {
            return Some(Err(errno::ENOENT));
        }
        (name, None)
    };
    let namespace = caller.get_namespace();
    let resolve_group = |name: &str| match name {
        "self" | "thread-self" => Some(caller.get_thread_group_id()),
        name => name
            .parse::<usize>()
            .ok()
            .and_then(|id| namespace.resolve_global_id(id)),
    };
    let Some(group) = resolve_group(group) else {
        return Some(Err(errno::ENOENT));
    };
    let thread = match thread {
        Some("self" | "thread-self") => Some(caller.get_id()),
        Some(thread) => {
            let Some(thread) = thread
                .parse::<usize>()
                .ok()
                .and_then(|id| namespace.resolve_global_id(id))
            else {
                return Some(Err(errno::ENOENT));
            };
            Some(thread)
        }
        None => None,
    };
    Some(process_leader(group, thread))
}

pub(super) fn is_path(path: &str) -> bool {
    let path = path.trim_end_matches('/');
    path.strip_prefix("/proc/")
        .and_then(|name| name.strip_suffix(&format!("/{FIELD}")))
        .is_some()
}

fn stale_target() -> StreamError {
    StreamError::FileSystemError(FileSystemError::new(
        FileSystemErrorKind::NotFound,
        "process is no longer live",
    ))
}

fn authority_error() -> StreamError {
    StreamError::FileSystemError(FileSystemError::new(
        FileSystemErrorKind::InvalidOperation,
        "reclaim preference change is not permitted",
    ))
}

fn current_target(file: &ProcReclaimFile) -> Result<Arc<Task>, StreamError> {
    let target = file.target.upgrade().ok_or_else(stale_target)?;
    if target.get_thread_group_id() != file.process_id || !live(&target) {
        return Err(stale_target());
    }
    Ok(target)
}

fn target_text(file: &ProcReclaimFile) -> Result<String, StreamError> {
    let target = current_target(file)?;
    let preference = *target.reclaim_preference.read();
    let score = protection_to_score(preference.protection());
    Ok(format!("{score}\n"))
}

fn may_change(caller: &Task, target: &Task) -> bool {
    let caller = caller.credentials.read().clone();
    let owner = target.credentials.read().identity().principal;
    caller.permits(MANAGE_RECLAIM) || caller.identity().principal == owner
}

fn update(file: &ProcReclaimFile, bytes: &[u8]) -> Result<usize, StreamError> {
    if file.access == 0 {
        return Err(StreamError::PermissionDenied);
    }
    if bytes.is_empty() || bytes.len() > MAX_WRITE_BYTES {
        return Err(StreamError::InvalidArgument);
    }
    let text = core::str::from_utf8(bytes)
        .map_err(|_| StreamError::InvalidArgument)?
        .trim();
    if text.is_empty() {
        return Err(StreamError::InvalidArgument);
    }
    let score = text
        .parse::<i32>()
        .map_err(|_| StreamError::InvalidArgument)?;
    if !(LINUX_SCORE_ADJ_MIN..=LINUX_SCORE_ADJ_MAX).contains(&score) {
        return Err(StreamError::InvalidArgument);
    }
    let protection = score_to_protection(score);

    let target = current_target(file)?;
    let caller = crate::task::mytask().ok_or(StreamError::PermissionDenied)?;
    if !live(&caller) {
        return Err(stale_target());
    }
    let caller_credentials = caller.credentials.read().clone();
    let owner = target.credentials.read().identity().principal;
    target
        .reclaim_preference
        .write()
        .update(protection, &caller_credentials, owner)
        .map_err(|error| match error {
            UpdateError::PermissionDenied => authority_error(),
        })?;
    Ok(bytes.len())
}

struct ProcReclaimFile {
    position: IrqRwSpinLock<usize>,
    target: Weak<Task>,
    process_id: usize,
    access: i32,
}

impl StreamOps for ProcReclaimFile {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut position = self.position.write();
        let n = self.read_at(*position as u64, buffer)?;
        *position += n;
        Ok(n)
    }

    fn write_empty(&self) -> Result<usize, StreamError> {
        Err(StreamError::InvalidArgument)
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        update(self, buffer)
    }
}

impl ControlOps for ProcReclaimFile {}

impl MemoryMappingOps for ProcReclaimFile {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        Err("cannot map a proc control file")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for ProcReclaimFile {
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

impl FileObject for ProcReclaimFile {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut position = self.position.write();
        let end = target_text(self)?.len();
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

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if self.access == 1 {
            return Err(StreamError::PermissionDenied);
        }
        let content = target_text(self)?;
        let start = usize::try_from(offset).map_err(|_| StreamError::InvalidArgument)?;
        let bytes = content.as_bytes();
        if start >= bytes.len() {
            return Ok(0);
        }
        let count = buffer.len().min(bytes.len() - start);
        buffer[..count].copy_from_slice(&bytes[start..start + count]);
        Ok(count)
    }

    fn write_at(&self, offset: u64, buffer: &[u8]) -> Result<usize, StreamError> {
        let _ = offset;
        update(self, buffer)
    }

    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::RegularFile,
            size: target_text(self)?.len(),
            permissions: FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: self.process_id as u64,
            link_count: 1,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub(super) fn open(abi: &mut LinuxAbi, caller: &Task, path: &str, flags: i32) -> Option<usize> {
    let target = match path_target(caller, path)? {
        Ok(target) => target,
        Err(error) => return Some(errno::to_result(error)),
    };
    if flags & 3 == 3 {
        return Some(errno::to_result(errno::EACCES));
    }
    if flags & O_DIRECTORY != 0 {
        return Some(errno::to_result(errno::ENOTDIR));
    }
    if flags & 3 != 0 && !may_change(caller, &target) {
        return Some(errno::to_result(errno::EACCES));
    }
    let process_id = target.get_thread_group_id();
    let file = Arc::new(ProcReclaimFile {
        position: IrqRwSpinLock::new(0),
        target: Arc::downgrade(&target),
        process_id,
        access: flags & 3,
    });
    let handle = match caller.handle_table.insert(KernelObject::File(file)) {
        Ok(handle) => handle,
        Err(_) => return Some(errno::to_result(errno::ENFILE)),
    };
    let fd = match abi.allocate_fd(handle) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = caller.handle_table.remove(handle);
            return Some(errno::to_result(errno::EMFILE));
        }
    };
    if flags & O_CLOEXEC != 0 {
        let _ = abi.set_fd_flags(fd, FD_CLOEXEC);
    }
    Some(fd)
}

pub(super) fn text_content(path: &str) -> Option<String> {
    let caller = crate::task::mytask()?;
    let target = match path_target(&caller, path)? {
        Ok(target) => target,
        Err(_) => return None,
    };
    let protection = target.reclaim_preference.read().protection();
    Some(format!("{}\n", -(protection as i32)))
}

pub(super) fn path_metadata(path: &str) -> Option<FileMetadata> {
    let caller = crate::task::mytask()?;
    let target = match path_target(&caller, path)? {
        Ok(target) => target,
        Err(_) => return None,
    };
    let content = format!(
        "{}\n",
        -(target.reclaim_preference.read().protection() as i32)
    );
    Some(FileMetadata {
        file_type: FileType::RegularFile,
        size: content.len(),
        permissions: FilePermission {
            read: true,
            write: true,
            execute: false,
        },
        created_time: 0,
        modified_time: 0,
        accessed_time: 0,
        file_id: target.get_thread_group_id() as u64,
        link_count: 1,
    })
}

pub(super) fn access(caller: &Task, path: &str, mode: i32) -> Option<Result<(), usize>> {
    if !is_path(path) {
        return None;
    }
    if mode & 1 != 0 {
        return Some(Err(errno::EACCES));
    }
    let target = match path_target(caller, path)? {
        Ok(target) => target,
        Err(error) => return Some(Err(error)),
    };
    if mode & 2 != 0 && !may_change(caller, &target) {
        return Some(Err(errno::EACCES));
    }
    Some(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sched::scheduler::{get_task_by_id, register_task, reset},
        shield::identity::{Group, Identities, Principal},
        task::{TaskType, clear_mock_current_task, set_mock_current_task},
    };
    use alloc::sync::Arc;

    fn task(name: &str, principal: u32, privileged: bool) -> Arc<Task> {
        let mut task = Task::new(name.into(), 0, TaskType::User);
        task.credentials
            .write()
            .set_principals(Identities::all(Principal(principal)))
            .unwrap();
        task.credentials
            .write()
            .set_groups(Identities::all(Group(principal)))
            .unwrap();
        if !privileged {
            task.credentials.write().set_authority(0, 0).unwrap();
        }
        let id = register_task(task);
        get_task_by_id(id).unwrap()
    }

    #[test_case]
    fn fd_reads_current_process_state_and_validates_text_and_authority() {
        reset();
        let caller = task("oom-self", 1000, false);
        set_mock_current_task(caller.clone());
        let path = format!("/proc/{}/oom_score_adj", caller.get_namespace_id());
        assert_eq!(
            text_content("/proc/self/oom_score_adj").as_deref(),
            Some("0\n")
        );
        assert_eq!(
            text_content("/proc/thread-self/oom_score_adj").as_deref(),
            Some("0\n")
        );
        let mut abi = LinuxAbi::default();
        let fd = open(&mut abi, &caller, &path, 2).unwrap();
        assert!(fd < 256);
        let object = caller
            .handle_table
            .get(abi.get_handle(fd).unwrap())
            .unwrap();
        let file = object.as_file().unwrap();

        assert!(file.write(b"1001").is_err());
        assert!(file.write(b"not-an-integer").is_err());
        assert!(file.write(b"-1000").is_err());
        assert_eq!(file.write(b"1000\n").unwrap(), 5);
        let mut value = [0u8; 8];
        assert_eq!(file.read_at(0, &mut value).unwrap(), 5);
        assert_eq!(&value[..5], b"1000\n");
        assert_eq!(file.read(&mut value).unwrap(), 5);
        assert_eq!(file.write(b"500").unwrap(), 3);
        assert_eq!(file.write_at(4096, b"400").unwrap(), 3);
        let mut value = [0u8; 8];
        assert_eq!(file.read_at(0, &mut value).unwrap(), 4);
        assert_eq!(&value[..4], b"400\n");

        let other = task("oom-other-owner", 2000, false);
        set_mock_current_task(other.clone());
        assert_eq!(access(&other, &path, 2), Some(Err(errno::EACCES)));
        assert_eq!(
            open(&mut abi, &other, &path, 1),
            Some(errno::to_result(errno::EACCES))
        );

        let privileged = Arc::new(Task::new("oom-authority".into(), 0, TaskType::User));
        set_mock_current_task(privileged.clone());
        assert_eq!(access(&privileged, &path, 2), Some(Ok(())));
        let fd = open(&mut abi, &privileged, &path, 1).unwrap();
        let object = privileged
            .handle_table
            .get(abi.get_handle(fd).unwrap())
            .unwrap();
        assert_eq!(object.as_file().unwrap().write(b"-1000").unwrap(), 5);
        assert_eq!(caller.reclaim_preference.read().protection(), u16::MAX);
        clear_mock_current_task();
        reset();
    }

    #[test_case]
    fn stale_targets_reject_fd_writes_without_retargeting() {
        reset();
        let caller = task("oom-caller", 1000, false);
        let target = task("oom-target", 1000, false);
        set_mock_current_task(caller.clone());
        let file = ProcReclaimFile {
            position: IrqRwSpinLock::new(0),
            target: Arc::downgrade(&target),
            process_id: target.get_thread_group_id(),
            access: 1,
        };
        target.set_state(TaskState::Zombie);
        assert!(file.write(b"1000").is_err());
        assert!(file.write_at(4096, b"1000").is_err());
        clear_mock_current_task();
        reset();
    }

    #[test_case]
    fn numeric_thread_path_resolves_to_its_process_leader() {
        reset();
        let leader = task("oom-process-leader", 1000, false);
        let mut member = Task::new("oom-thread".into(), 0, TaskType::User);
        member
            .credentials
            .write()
            .set_principals(Identities::all(Principal(1000)))
            .unwrap();
        member.credentials.write().set_authority(0, 0).unwrap();
        member.set_thread_group_id(leader.get_id());
        let member_id = register_task(member);
        let member = get_task_by_id(member_id).unwrap();
        let path = format!("/proc/{}/oom_score_adj", member.get_namespace_id());
        assert_eq!(
            path_target(&member, &path).unwrap().unwrap().get_id(),
            leader.get_id()
        );
        reset();
    }

    #[test_case]
    fn linux_scores_round_trip_through_the_native_weight() {
        for score in LINUX_SCORE_ADJ_MIN..=LINUX_SCORE_ADJ_MAX {
            assert_eq!(protection_to_score(score_to_protection(score)), score);
        }
    }
}
