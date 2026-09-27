//! Anonymous, seekable Linux memfd backed by shared physical pages.

use core::any::Any;

use crate::{
    arch::Trapframe,
    environment::PAGE_SIZE,
    fs::{FileMetadata, FilePermission, FileType, SeekFrom},
    ipc::{SharedMemory, SharedMemoryObject},
    object::capability::{
        ControlOps, FileObject, MemoryMappingInfo, MemoryMappingOps, ReadyInterest,
        SelectWaitOutcome, Selectable, StreamError, StreamOps,
    },
    sync::{IrqRwSpinLock, Mutex},
    vm::addr::phys_to_virt,
};

struct MemfdState {
    size: usize,
    position: u64,
    seals: u32,
}

const F_SEAL_SEAL: u32 = 1;
const F_SEAL_SHRINK: u32 = 2;
const F_SEAL_GROW: u32 = 4;

fn sealed() -> StreamError {
    crate::fs::FileSystemError::new(
        crate::fs::FileSystemErrorKind::InvalidOperation,
        "Memfd operation is sealed",
    )
    .into()
}

pub(super) struct MemfdFile {
    backing: SharedMemory,
    operation: Mutex<()>,
    state: IrqRwSpinLock<MemfdState>,
}

impl MemfdFile {
    pub(super) fn new(allow_sealing: bool) -> Result<Self, &'static str> {
        Ok(Self {
            backing: SharedMemory::new(PAGE_SIZE, 0x3)?,
            operation: Mutex::new(()),
            state: IrqRwSpinLock::new(MemfdState {
                size: 0,
                position: 0,
                seals: if allow_sealing { 0 } else { F_SEAL_SEAL },
            }),
        })
    }

    pub(super) fn seals(&self) -> u32 {
        self.state.read().seals
    }

    pub(super) fn add_seals(&self, seals: u32) -> Result<(), StreamError> {
        // WRITE/FUTURE_WRITE require writable-VMA accounting. Reject them
        // until that enforcement exists rather than accepting an ineffective seal.
        if seals & !(F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW) != 0 {
            return Err(StreamError::InvalidArgument);
        }
        let _guard = crate::object::capability::file::lock_file_operation(&self.operation)?;
        let mut state = self.state.write();
        if state.seals & F_SEAL_SEAL != 0 {
            return Err(sealed());
        }
        state.seals |= seals;
        Ok(())
    }

    fn copy_out(&self, offset: usize, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let size = self.state.read().size;
        let count = buffer.len().min(size.saturating_sub(offset));
        if count == 0 {
            return Ok(0);
        }
        let backing = self
            .backing
            .pin_range(offset, count)
            .map_err(|_| StreamError::IoError)?;
        // SAFETY: pin_range holds the contiguous backing stable for this copy.
        unsafe {
            core::ptr::copy_nonoverlapping(
                (phys_to_virt(backing.paddr()) as *const u8).add(offset),
                buffer.as_mut_ptr(),
                count,
            );
        }
        self.backing.unpin_range();
        Ok(count)
    }

    fn write_locked(&self, offset: usize, buffer: &[u8]) -> Result<usize, StreamError> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let end = offset
            .checked_add(buffer.len())
            .filter(|end| *end <= i64::MAX as usize && end.checked_add(PAGE_SIZE - 1).is_some())
            .ok_or(StreamError::InvalidArgument)?;
        let old_size = self.state.read().size;
        if end > old_size && self.seals() & F_SEAL_GROW != 0 {
            return Err(sealed());
        }
        if end > self.backing.size() {
            self.backing.resize(end).map_err(|_| StreamError::NoSpace)?;
        }
        let backing = self
            .backing
            .pin_range(0, end)
            .map_err(|_| StreamError::IoError)?;
        // Newly exposed bytes must read as zero, even after shrink and regrow.
        // SAFETY: the pin covers [0, end), and the backing is stable.
        unsafe {
            let base = phys_to_virt(backing.paddr()) as *mut u8;
            if offset > old_size {
                core::ptr::write_bytes(base.add(old_size), 0, offset - old_size);
            }
            core::ptr::copy_nonoverlapping(buffer.as_ptr(), base.add(offset), buffer.len());
        }
        self.backing.unpin_range();
        self.state.write().size = old_size.max(end);
        Ok(buffer.len())
    }
}

impl StreamOps for MemfdFile {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let _guard = crate::object::capability::file::lock_file_operation(&self.operation)?;
        let position = self.state.read().position;
        let offset = usize::try_from(position).map_err(|_| StreamError::InvalidArgument)?;
        let n = self.copy_out(offset, buffer)?;
        self.state.write().position = position + n as u64;
        Ok(n)
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        let _guard = crate::object::capability::file::lock_file_operation(&self.operation)?;
        let position = self.state.read().position;
        let offset = usize::try_from(position).map_err(|_| StreamError::InvalidArgument)?;
        let n = self.write_locked(offset, buffer)?;
        self.state.write().position = position + n as u64;
        Ok(n)
    }
}

impl ControlOps for MemfdFile {}

impl MemoryMappingOps for MemfdFile {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        self.backing.get_mapping_info(offset, length)
    }

    fn on_mapped(&self, vaddr: usize, paddr: u64, length: usize, offset: usize) {
        self.backing.on_mapped(vaddr, paddr, length, offset);
    }

    fn on_unmapped(&self, vaddr: usize, length: usize) {
        self.backing.on_unmapped(vaddr, length);
    }

    fn supports_mmap(&self) -> bool {
        self.backing.supports_mmap()
    }

    fn mmap_owner_name(&self) -> alloc::string::String {
        self.backing.mmap_owner_name()
    }

    fn can_extend_vma_on_fault(&self) -> bool {
        self.backing.can_extend_vma_on_fault()
    }

    fn resolve_fault(
        &self,
        access: &crate::object::capability::memory_mapping::AccessKind,
        page_idx: usize,
        vm_start: usize,
    ) -> Result<
        crate::object::capability::memory_mapping::ResolveFaultResult,
        crate::object::capability::memory_mapping::ResolveFaultError,
    > {
        self.backing.resolve_fault(access, page_idx, vm_start)
    }
}

impl Selectable for MemfdFile {
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

impl FileObject for MemfdFile {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let (offset, kind) = match whence {
            SeekFrom::Start(offset) => {
                let position = i64::try_from(offset).map_err(|_| StreamError::InvalidArgument)?;
                (position, 0)
            }
            SeekFrom::Current(offset) => (offset, 1),
            SeekFrom::End(offset) => (offset, 2),
        };
        self.seek_signed(offset, kind)
    }

    fn seek_signed(&self, offset: i64, whence: u32) -> Result<u64, StreamError> {
        let _guard = crate::object::capability::file::lock_file_operation(&self.operation)?;
        let mut state = self.state.write();
        let base = match whence {
            0 => 0,
            1 => state.position,
            2 => state.size as u64,
            _ => return Err(StreamError::InvalidArgument),
        };
        let position = crate::object::capability::file::checked_seek_position(base, offset)?;
        state.position = position;
        Ok(position)
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let _guard = crate::object::capability::file::lock_file_operation(&self.operation)?;
        let offset = usize::try_from(offset).map_err(|_| StreamError::InvalidArgument)?;
        self.copy_out(offset, buffer)
    }

    fn write_at(&self, offset: u64, buffer: &[u8]) -> Result<usize, StreamError> {
        let _guard = crate::object::capability::file::lock_file_operation(&self.operation)?;
        let offset = usize::try_from(offset).map_err(|_| StreamError::InvalidArgument)?;
        self.write_locked(offset, buffer)
    }

    fn truncate(&self, size: u64) -> Result<(), StreamError> {
        let size = usize::try_from(size)
            .ok()
            .filter(|size| *size <= i64::MAX as usize && size.checked_add(PAGE_SIZE - 1).is_some())
            .ok_or(StreamError::InvalidArgument)?;
        let _guard = crate::object::capability::file::lock_file_operation(&self.operation)?;
        let old_size = self.state.read().size;
        let seals = self.seals();
        if (size < old_size && seals & F_SEAL_SHRINK != 0)
            || (size > old_size && seals & F_SEAL_GROW != 0)
        {
            return Err(sealed());
        }
        if size == old_size {
            return Ok(());
        }
        self.backing
            .resize(size)
            .map_err(|_| StreamError::NoSpace)?;
        if size > old_size {
            let backing = self
                .backing
                .pin_range(old_size, size - old_size)
                .map_err(|_| StreamError::IoError)?;
            // SAFETY: the pinned backing includes every newly exposed byte.
            unsafe {
                core::ptr::write_bytes(
                    (phys_to_virt(backing.paddr()) as *mut u8).add(old_size),
                    0,
                    size - old_size,
                );
            }
            self.backing.unpin_range();
        }
        self.state.write().size = size;
        Ok(())
    }

    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::RegularFile,
            size: self.state.read().size,
            permissions: FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: self as *const Self as usize as u64,
            link_count: 0,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::linux::generic::{LinuxAbi, errno, fs};
    use crate::object::KernelObject;
    use crate::task::{Task, TaskType, clear_mock_current_task, set_mock_current_task};
    use alloc::sync::Arc;

    fn assert_sealed<T: core::fmt::Debug>(result: Result<T, StreamError>) {
        assert!(matches!(result, Err(StreamError::FileSystemError(error))
            if error.kind == crate::fs::FileSystemErrorKind::InvalidOperation));
    }

    #[test_case]
    fn memfd_size_seals_block_resize_but_allow_writes_inside_the_file() {
        let file = MemfdFile::new(true).unwrap();
        file.truncate(16).unwrap();
        file.add_seals(F_SEAL_SHRINK).unwrap();
        assert_sealed(file.truncate(8));
        file.truncate(32).unwrap();
        file.add_seals(F_SEAL_GROW | F_SEAL_SEAL).unwrap();
        assert_sealed(file.truncate(64));
        assert_sealed(file.truncate(0));
        file.truncate(32).unwrap();
        assert_sealed(file.write_at(31, b"xx"));
        assert_eq!(file.write_at(30, b"ok").unwrap(), 2);
        let mut bytes = [0; 2];
        assert_eq!(file.read_at(30, &mut bytes).unwrap(), 2);
        assert_eq!(&bytes, b"ok");
        assert_sealed(file.add_seals(0));
        assert_eq!(file.metadata().unwrap().size, 32);
        assert_eq!(file.seals(), 7);
    }

    #[test_case]
    fn memfd_default_seal_and_unsupported_bits_are_not_silently_accepted() {
        let file = MemfdFile::new(false).unwrap();
        assert_eq!(file.seals(), F_SEAL_SEAL);
        assert_sealed(file.add_seals(F_SEAL_GROW));
        file.truncate(32).unwrap();
        file.truncate(0).unwrap();
        let file = MemfdFile::new(true).unwrap();
        assert!(matches!(
            file.add_seals(8),
            Err(StreamError::InvalidArgument)
        ));
        assert_eq!(file.seals(), 0);
    }

    #[test_case]
    fn memfd_fcntl_duplicate_and_ftruncate_observe_shared_seals_and_errno() {
        let task = Arc::new(Task::new("memfd-seals".into(), 1, TaskType::Kernel));
        set_mock_current_task(task.clone());
        let mut abi = LinuxAbi::default();
        let file = Arc::new(MemfdFile::new(true).unwrap());
        file.truncate(32).unwrap();
        let handle = task.handle_table.insert(KernelObject::File(file)).unwrap();
        let fd = abi.allocate_fd(handle).unwrap();
        abi.set_file_status_flags(fd, 2).unwrap();
        let mut tf = Trapframe::new();
        tf.set_arg(0, fd);
        tf.set_arg(1, 0); // F_DUPFD
        tf.set_arg(2, 10);
        let duplicate = fs::sys_fcntl(&mut abi, &mut tf);
        assert_eq!(duplicate, 10);
        tf.set_arg(1, 1033);
        tf.set_arg(2, 7);
        assert_eq!(fs::sys_fcntl(&mut abi, &mut tf), 0);
        tf.set_arg(0, duplicate);
        tf.set_arg(1, 1034);
        assert_eq!(fs::sys_fcntl(&mut abi, &mut tf), 7);
        tf.set_arg(1, 16);
        assert_eq!(
            fs::sys_ftruncate(&mut abi, &mut tf),
            errno::to_result(errno::EPERM)
        );
        tf.set_arg(1, 64);
        assert_eq!(
            fs::sys_ftruncate(&mut abi, &mut tf),
            errno::to_result(errno::EPERM)
        );
        tf.set_arg(1, 32);
        assert_eq!(fs::sys_ftruncate(&mut abi, &mut tf), 0);
        clear_mock_current_task();
    }
}
