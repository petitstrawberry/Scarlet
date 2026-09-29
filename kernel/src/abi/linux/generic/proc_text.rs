//! Small read-only proc/sys text files in the Linux ABI view.

use alloc::{format, string::String, sync::Arc};
use core::{any::Any, arch::asm};

use crate::{
    abi::linux::generic::{
        LinuxAbi, errno,
        fs::{FD_CLOEXEC, O_CLOEXEC, O_DIRECTORY},
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
    task::Task,
};

#[cfg(target_arch = "aarch64")]
fn cpu_list() -> String {
    let mut list = String::new();
    crate::sched::scheduler::for_each_online_cpu(|id| {
        if !list.is_empty() {
            list.push(',');
        }
        list.push_str(&format!("{id}"));
    });
    list.push('\n');
    list
}

#[cfg(target_arch = "aarch64")]
fn cpu_info() -> String {
    let midr: u64;
    // SAFETY: the kernel executes at EL1 and MIDR_EL1 is read-only.
    unsafe { asm!("mrs {value}, midr_el1", value = out(reg) midr, options(nomem, nostack)) };
    let hwcap = crate::arch::aarch64::cpu_features::userspace_capabilities().hwcap;
    let mut features = String::new();
    for (bit, name) in [
        (0, "fp"),
        (1, "asimd"),
        (3, "aes"),
        (4, "pmull"),
        (5, "sha1"),
        (6, "sha2"),
        (7, "crc32"),
        (8, "atomics"),
        (9, "fphp"),
        (10, "asimdhp"),
        (21, "sha512"),
    ] {
        if hwcap & (1u64 << bit) != 0 {
            if !features.is_empty() {
                features.push(' ');
            }
            features.push_str(name);
        }
    }
    let mut content = String::new();
    crate::sched::scheduler::for_each_online_cpu(|id| {
        content.push_str(&format!(
            "processor\t: {id}\nFeatures\t: {features}\nCPU implementer\t: 0x{:02x}\nCPU architecture: 8\nCPU variant\t: 0x{:x}\nCPU part\t: 0x{:03x}\nCPU revision\t: {}\n\n",
            (midr >> 24) & 0xff, (midr >> 20) & 0xf, (midr >> 4) & 0xfff, midr & 0xf,
        ));
    });
    content
}

const CPU_DIRECTORY: &str = "/sys/devices/system/cpu";
const CPU_FILES: &[&str] = &["online", "possible", "present"];

fn text_content(path: &str) -> Option<String> {
    #[cfg(target_arch = "aarch64")]
    return Some(match path {
        "/proc/cpuinfo" => cpu_info(),
        // Use scheduler state, not a fixed count or the caller's affinity.
        "/sys/devices/system/cpu/online"
        | "/sys/devices/system/cpu/possible"
        | "/sys/devices/system/cpu/present" => cpu_list(),
        "/proc/sys/fs/inotify/max_user_watches" => String::from("8192\n"),
        _ => return None,
    });
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = path;
        None
    }
}

fn is_cpu_directory(path: &str) -> bool {
    cfg!(target_arch = "aarch64") && path.trim_end_matches('/') == CPU_DIRECTORY
}

fn metadata(path: &str, size: usize, directory: bool) -> FileMetadata {
    // Stable across open/stat, distinct for each synthetic entry.
    let file_id = path
        .trim_end_matches('/')
        .bytes()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    FileMetadata {
        file_type: if directory {
            FileType::Directory
        } else {
            FileType::RegularFile
        },
        size,
        permissions: FilePermission {
            read: true,
            write: false,
            execute: directory,
        },
        created_time: 0,
        modified_time: 0,
        accessed_time: 0,
        file_id,
        link_count: if directory { 2 } else { 1 },
    }
}

pub(super) fn path_metadata(path: &str) -> Option<FileMetadata> {
    if is_cpu_directory(path) {
        Some(metadata(path, 0, true))
    } else {
        text_content(path).map(|content| metadata(path, content.len(), false))
    }
}

/// Resolve relative openat/stat/access on the directory used by util-linux.
pub(super) fn directory_path(file: &dyn FileObject) -> Option<&str> {
    let file = file.as_any().downcast_ref::<ProcTextFile>()?;
    file.directory.then_some(file.path.as_str())
}

pub(super) fn open_proc_text_file(
    abi: &mut LinuxAbi,
    task: &Task,
    path: &str,
    flags: i32,
) -> Option<usize> {
    let directory = is_cpu_directory(path);
    let content = if directory {
        String::new()
    } else {
        text_content(path)?
    };
    if flags & 3 != 0 {
        return Some(errno::to_result(errno::EACCES));
    }
    if flags & O_DIRECTORY != 0 && !directory {
        return Some(errno::to_result(errno::ENOTDIR));
    }
    let file = Arc::new(ProcTextFile {
        position: IrqRwSpinLock::new(0),
        content,
        path: String::from(path.trim_end_matches('/')),
        directory,
    });
    let handle = match task.handle_table.insert(KernelObject::File(file)) {
        Ok(handle) => handle,
        Err(_) => return Some(errno::to_result(errno::ENFILE)),
    };
    let fd = match abi.allocate_fd(handle) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = task.handle_table.remove(handle);
            return Some(errno::to_result(errno::EMFILE));
        }
    };
    if flags & O_CLOEXEC != 0 {
        let _ = abi.set_fd_flags(fd, FD_CLOEXEC);
    }
    Some(fd)
}

struct ProcTextFile {
    position: IrqRwSpinLock<usize>,
    content: String,
    path: String,
    directory: bool,
}

impl StreamOps for ProcTextFile {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut position = self.position.write();
        if self.directory {
            let name = match *position {
                0 => ".",
                1 => "..",
                n => match CPU_FILES.get(n - 2) {
                    Some(name) => name,
                    None => return Ok(0),
                },
            };
            let path = match name {
                "." => self.path.clone(),
                ".." => String::from("/sys/devices/system"),
                _ => format!("{}/{}", self.path, name),
            };
            let entry = DirectoryEntry::from_internal(&DirectoryEntryInternal {
                name: String::from(name),
                file_type: if *position < 2 {
                    FileType::Directory
                } else {
                    FileType::RegularFile
                },
                size: 0,
                file_id: metadata(&path, 0, *position < 2).file_id,
                metadata: None,
            });
            let size = core::mem::size_of::<DirectoryEntry>();
            if buffer.len() < size {
                return Err(StreamError::InvalidArgument);
            }
            // Same internal directory record consumed by read_linux_dirents_to_user.
            let bytes = unsafe {
                core::slice::from_raw_parts((&entry as *const DirectoryEntry).cast::<u8>(), size)
            };
            buffer[..size].copy_from_slice(bytes);
            *position += 1;
            return Ok(size);
        }
        let n = self.read_at(*position as u64, buffer)?;
        *position += n;
        Ok(n)
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::PermissionDenied)
    }
}

impl ControlOps for ProcTextFile {}

impl MemoryMappingOps for ProcTextFile {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        Err("cannot map proc text file")
    }
    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for ProcTextFile {
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

impl FileObject for ProcTextFile {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut position = self.position.write();
        let next = match whence {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::Current(offset) => *position as i128 + offset as i128,
            SeekFrom::End(offset) => {
                let end = if self.directory {
                    CPU_FILES.len() + 2
                } else {
                    self.content.len()
                };
                end as i128 + offset as i128
            }
        };
        if next < 0 || next > usize::MAX as i128 {
            return Err(StreamError::InvalidArgument);
        }
        *position = next as usize;
        Ok(*position as u64)
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, StreamError> {
        if self.directory {
            return Err(StreamError::InvalidArgument);
        }
        let start = usize::try_from(offset).map_err(|_| StreamError::InvalidArgument)?;
        let bytes = self.content.as_bytes();
        if start >= bytes.len() {
            return Ok(0);
        }
        let n = buffer.len().min(bytes.len() - start);
        buffer[..n].copy_from_slice(&bytes[start..start + n]);
        Ok(n)
    }

    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(metadata(&self.path, self.content.len(), self.directory))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
