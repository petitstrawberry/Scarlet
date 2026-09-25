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
}

pub(super) struct MemfdFile {
    backing: SharedMemory,
    operation: Mutex<()>,
    state: IrqRwSpinLock<MemfdState>,
}

impl MemfdFile {
    pub(super) fn new() -> Result<Self, &'static str> {
        Ok(Self {
            backing: SharedMemory::new(PAGE_SIZE, 0x3)?,
            operation: Mutex::new(()),
            state: IrqRwSpinLock::new(MemfdState {
                size: 0,
                position: 0,
            }),
        })
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
