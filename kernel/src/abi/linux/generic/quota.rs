//! Live `/proc/net/xt_quota/<name>` views over native packet meters.
use super::{LinuxAbi, errno};
use crate::{
    arch::Trapframe,
    fs::{FileMetadata, FilePermission, FileType, SeekFrom},
    library::std::usercopy::copy_from_user,
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
use alloc::{format, string::String, sync::Arc};
use core::any::Any;

const PREFIX: &str = "/proc/net/xt_quota/";

fn name(path: &str) -> Option<&str> {
    let value = path.strip_prefix(PREFIX)?;
    if value.is_empty()
        || value.len() >= 32
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.as_bytes().contains(&0)
    {
        return None;
    }
    Some(value)
}

fn metadata(name: &str, size: usize) -> FileMetadata {
    let file_id = name.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    FileMetadata {
        file_type: FileType::RegularFile,
        size,
        permissions: FilePermission {
            read: true,
            write: true,
            execute: false,
        },
        created_time: 0,
        modified_time: 0,
        accessed_time: 0,
        file_id,
        link_count: 1,
    }
}

pub(super) fn path_metadata(path: &str) -> Option<FileMetadata> {
    let name = name(path)?;
    let meter = crate::network::NetworkManager::get_manager().traffic_meter(name)?;
    Some(metadata(
        name,
        format!("{}\n", meter.snapshot().remaining).len(),
    ))
}

struct QuotaFile {
    name: String,
    meter: Arc<crate::network::traffic_policy::Meter>,
    position: IrqRwSpinLock<usize>,
    access: i32,
}

impl StreamOps for QuotaFile {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let mut position = self.position.write();
        let content = format!("{}\n", self.meter.snapshot().remaining);
        let start = (*position).min(content.len());
        let count = buffer.len().min(content.len() - start);
        buffer[..count].copy_from_slice(&content.as_bytes()[start..start + count]);
        *position += count;
        Ok(count)
    }
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        if self.access == 0 {
            return Err(StreamError::PermissionDenied);
        }
        let caller = crate::task::mytask().ok_or(StreamError::PermissionDenied)?;
        if !caller
            .credentials
            .read()
            .permits(crate::shield::identity::MANAGE_TRAFFIC)
        {
            return Err(StreamError::PermissionDenied);
        }
        let value = core::str::from_utf8(buffer)
            .map_err(|_| StreamError::InvalidArgument)?
            .trim()
            .parse::<u64>()
            .map_err(|_| StreamError::InvalidArgument)?;
        self.meter
            .reset(value)
            .map_err(|_| StreamError::InvalidArgument)?;
        Ok(buffer.len())
    }
}

impl ControlOps for QuotaFile {}
impl MemoryMappingOps for QuotaFile {
    fn supports_mmap(&self) -> bool {
        false
    }
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("quota files are not mappable")
    }
}
impl Selectable for QuotaFile {
    fn current_ready(&self, interest: ReadyInterest) -> crate::object::capability::ReadySet {
        let mut ready = crate::object::capability::ReadySet::none();
        ready.read = interest.read;
        ready.write = interest.write && self.access != 0;
        ready
    }
    fn wait_until_ready(
        &self,
        _: ReadyInterest,
        _: &mut Trapframe,
        _: Option<u64>,
        _: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}
impl FileObject for QuotaFile {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let size = format!("{}\n", self.meter.snapshot().remaining).len();
        let mut position = self.position.write();
        let next = match whence {
            SeekFrom::Start(value) => value as i128,
            SeekFrom::Current(value) => *position as i128 + value as i128,
            SeekFrom::End(value) => size as i128 + value as i128,
        };
        if next < 0 || next > usize::MAX as i128 {
            return Err(StreamError::InvalidArgument);
        }
        *position = next as usize;
        Ok(*position as u64)
    }
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(metadata(
            &self.name,
            format!("{}\n", self.meter.snapshot().remaining).len(),
        ))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub(super) fn open_proc_quota_file(
    abi: &mut LinuxAbi,
    task: &Task,
    path: &str,
    flags: i32,
) -> Option<usize> {
    let name = name(path)?;
    let Some(meter) = crate::network::NetworkManager::get_manager().traffic_meter(name) else {
        return Some(errno::to_result(errno::ENOENT));
    };
    if flags & 3 == 3 {
        return Some(errno::to_result(errno::EINVAL));
    }
    let file = Arc::new(QuotaFile {
        name: String::from(name),
        meter,
        position: IrqRwSpinLock::new(0),
        access: flags & 3,
    });
    let handle = task.handle_table.insert(KernelObject::File(file)).ok()?;
    match abi.allocate_fd(handle) {
        Ok(fd) => Some(fd),
        Err(_) => {
            let _ = task.handle_table.remove(handle);
            Some(errno::to_result(errno::EMFILE))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn only_one_bounded_component_is_exposed() {
        assert_eq!(name("/proc/net/xt_quota/globalAlert"), Some("globalAlert"));
        assert_eq!(name("/proc/net/xt_quota/../globalAlert"), None);
        assert_eq!(name("/proc/net/xt_quota/sub/globalAlert"), None);
        assert_eq!(name("/tmp/globalAlert"), None);
    }
}
