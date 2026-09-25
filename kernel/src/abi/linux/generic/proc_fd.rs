//! Linux `/proc/self/fd` directory backed by the calling process's descriptors.

use alloc::{format, string::String, vec::Vec};
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

/// Metadata for the proc task directory used by Chromium's thread checks.
/// It is visible only through Linux ABI stat calls, never mounted in VFS.
pub(super) fn self_task_metadata(task: &Task, path: &str) -> Option<Option<FileMetadata>> {
    let path = path.trim_end_matches('/');
    let suffix = path.strip_prefix("/proc/self/task")?;
    let threads: Vec<usize> = crate::sched::scheduler::get_all_task_ids()
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
        .collect();
    let link_count = if suffix.is_empty() {
        2 + threads.len()
    } else {
        let Some(name) = suffix.strip_prefix('/') else {
            return None;
        };
        let Some(local_id) = name.parse::<usize>().ok() else {
            return Some(None);
        };
        if !threads
            .iter()
            .any(|id| task.get_namespace().resolve_local_id(*id) == Some(local_id))
        {
            return Some(None);
        }
        2
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
        file_id: task.get_thread_group_id() as u64,
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
    if flags & 3 != 0 {
        return errno::to_result(errno::EISDIR);
    }
    let directory = alloc::sync::Arc::new(ProcFdDirectory {
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
    *directory.descriptors.write() = abi.allocated_fds();
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

struct ProcFdDirectory {
    position: IrqRwSpinLock<usize>,
    descriptors: IrqRwSpinLock<Vec<usize>>,
}

impl StreamOps for ProcFdDirectory {
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
            file_type: if *position < 2 {
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

impl ControlOps for ProcFdDirectory {}

impl MemoryMappingOps for ProcFdDirectory {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        Err("cannot map /proc/self/fd")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for ProcFdDirectory {
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

impl FileObject for ProcFdDirectory {
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
            link_count: 2,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
