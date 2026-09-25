//! Small read-only proc/sys text files in the Linux ABI view.

use alloc::{format, string::String, sync::Arc};
use core::{any::Any, arch::asm};

use crate::{
    abi::linux::generic::{
        LinuxAbi, errno,
        fs::{FD_CLOEXEC, O_CLOEXEC, O_DIRECTORY},
    },
    arch::Trapframe,
    fs::{FileMetadata, FilePermission, FileType, SeekFrom},
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

pub(super) fn open_proc_text_file(
    abi: &mut LinuxAbi,
    task: &Task,
    path: &str,
    flags: i32,
) -> Option<usize> {
    #[cfg(target_arch = "aarch64")]
    let content = match path {
        "/proc/cpuinfo" => cpu_info(),
        "/sys/devices/system/cpu/possible" | "/sys/devices/system/cpu/present" => cpu_list(),
        "/proc/sys/fs/inotify/max_user_watches" => String::from("8192\n"),
        _ => return None,
    };
    #[cfg(not(target_arch = "aarch64"))]
    let content: String = {
        let _ = path;
        return None;
    };

    if flags & 3 != 0 {
        return Some(errno::to_result(errno::EACCES));
    }
    if flags & O_DIRECTORY != 0 {
        return Some(errno::to_result(errno::ENOTDIR));
    }
    let file = Arc::new(ProcTextFile {
        position: IrqRwSpinLock::new(0),
        content,
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
}

impl StreamOps for ProcTextFile {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut position = self.position.write();
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
            SeekFrom::End(offset) => self.content.len() as i128 + offset as i128,
        };
        if next < 0 || next > usize::MAX as i128 {
            return Err(StreamError::InvalidArgument);
        }
        *position = next as usize;
        Ok(*position as u64)
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, StreamError> {
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
        Ok(FileMetadata {
            file_type: FileType::RegularFile,
            size: self.content.len(),
            permissions: FilePermission {
                read: true,
                write: false,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 1,
            link_count: 1,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
