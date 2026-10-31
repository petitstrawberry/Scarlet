//! Anonymous, seekable Linux memfd backed by shared physical pages.

use alloc::{sync::Arc, vec::Vec};
use core::any::Any;

use crate::{
    arch::Trapframe,
    environment::PAGE_SIZE,
    fs::{FileMetadata, FilePermission, FileType, SeekFrom},
    mem::page::{allocate_raw_pages, free_raw_pages},
    object::capability::{
        ControlOps, FileObject, MemoryMappingInfo, MemoryMappingOps, ReadyInterest,
        SelectWaitOutcome, Selectable, StreamError, StreamOps,
    },
    sync::{IrqRwSpinLock, Mutex},
    vm::addr::{phys_to_virt, virt_to_phys},
};

struct MemfdState {
    position: u64,
    seals: u32,
}

/// Sparse memfd backing. Linux reserves file capacity at `ftruncate`; physical
/// pages become resident only when the file is written or a mapping faults.
struct MemfdBacking {
    state: IrqRwSpinLock<MemfdBackingState>,
}

struct MemfdBackingState {
    size: usize,
    pages: Vec<Option<u64>>,
    retired_pages: Vec<u64>,
}

impl MemfdBacking {
    fn new() -> Result<Self, &'static str> {
        Ok(Self {
            state: IrqRwSpinLock::new(MemfdBackingState {
                size: 0,
                pages: Vec::new(),
                retired_pages: Vec::new(),
            }),
        })
    }

    fn size(&self) -> usize {
        self.state.read().size
    }

    fn resize(&self, new_size: usize) -> Result<(), &'static str> {
        let new_pages = new_size.div_ceil(PAGE_SIZE);
        let mut state = self.state.write();
        if new_pages > state.pages.len() {
            let additional = new_pages - state.pages.len();
            state
                .pages
                .try_reserve(additional)
                .map_err(|_| "memfd page table allocation failed")?;
            state.pages.resize(new_pages, None);
        } else if new_pages < state.pages.len() {
            // Existing shared mappings may still hold these PTEs. Retire the
            // pages until the final backing reference disappears instead of
            // freeing memory that a mapping can still translate.
            let retired = state.pages.split_off(new_pages);
            for page in retired {
                if let Some(paddr) = page {
                    state.retired_pages.push(paddr);
                }
            }
        }
        state.size = new_size;
        Ok(())
    }

    fn range_is_valid(&self, offset: usize, length: usize) -> bool {
        if length == 0 {
            return false;
        }
        let state = self.state.read();
        if offset >= state.size {
            return false;
        }
        offset
            .checked_add(length)
            .is_some_and(|end| end <= state.size.div_ceil(PAGE_SIZE) * PAGE_SIZE)
    }

    fn read_range(&self, offset: usize, buffer: &mut [u8]) -> usize {
        let state = self.state.read();
        let count = buffer.len().min(state.size.saturating_sub(offset));
        let mut copied = 0;
        while copied < count {
            let logical = offset + copied;
            let page_index = logical / PAGE_SIZE;
            let in_page = logical % PAGE_SIZE;
            let chunk = (PAGE_SIZE - in_page).min(count - copied);
            match state.pages.get(page_index).copied().flatten() {
                Some(paddr) => unsafe {
                    core::ptr::copy_nonoverlapping(
                        (phys_to_virt(paddr) as *const u8).add(in_page),
                        buffer.as_mut_ptr().add(copied),
                        chunk,
                    );
                },
                None => buffer[copied..copied + chunk].fill(0),
            }
            copied += chunk;
        }
        copied
    }

    fn write_range(&self, offset: usize, buffer: &[u8]) -> Result<usize, StreamError> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let limit = offset
            .checked_add(buffer.len())
            .ok_or(StreamError::InvalidArgument)?;
        let mut state = self.state.write();
        if limit > state.size {
            return Err(StreamError::InvalidArgument);
        }
        let mut copied = 0;
        while copied < buffer.len() {
            let logical = offset + copied;
            let page_index = logical / PAGE_SIZE;
            let in_page = logical % PAGE_SIZE;
            let chunk = (PAGE_SIZE - in_page).min(buffer.len() - copied);
            let slot = state
                .pages
                .get_mut(page_index)
                .ok_or(StreamError::InvalidArgument)?;
            if slot.is_none() {
                let pages = allocate_raw_pages(1);
                if pages.is_null() {
                    return Err(StreamError::NoSpace);
                }
                *slot = Some(virt_to_phys(pages as usize));
            }
            let paddr = slot.ok_or(StreamError::NoSpace)?;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buffer.as_ptr().add(copied),
                    (phys_to_virt(paddr) as *mut u8).add(in_page),
                    chunk,
                );
            }
            copied += chunk;
        }
        Ok(copied)
    }

    fn resolve_page(
        &self,
        offset: usize,
    ) -> Result<u64, crate::object::capability::memory_mapping::ResolveFaultError> {
        use crate::object::capability::memory_mapping::ResolveFaultError;
        let page_index = offset / PAGE_SIZE;
        if offset % PAGE_SIZE != 0 {
            return Err(ResolveFaultError::Invalid);
        }
        let mut state = self.state.write();
        if offset >= state.size {
            return Err(ResolveFaultError::Unmapped);
        }
        let slot = state
            .pages
            .get_mut(page_index)
            .ok_or(ResolveFaultError::Unmapped)?;
        if let Some(paddr) = *slot {
            return Ok(paddr);
        }
        let pages = allocate_raw_pages(1);
        if pages.is_null() {
            return Err(ResolveFaultError::Invalid);
        }
        let paddr = virt_to_phys(pages as usize);
        *slot = Some(paddr);
        Ok(paddr)
    }

    #[cfg(test)]
    fn resident_pages(&self) -> usize {
        self.state
            .read()
            .pages
            .iter()
            .filter(|page| page.is_some())
            .count()
    }
}

impl Drop for MemfdBacking {
    fn drop(&mut self) {
        let (pages, retired_pages) = {
            let mut state = self.state.write();
            (
                core::mem::take(&mut state.pages),
                core::mem::take(&mut state.retired_pages),
            )
        };
        for paddr in pages.into_iter().flatten().chain(retired_pages.into_iter()) {
            unsafe { free_raw_pages(phys_to_virt(paddr) as *mut crate::mem::page::Page, 1) };
        }
    }
}

const F_SEAL_SEAL: u32 = 1;
const F_SEAL_SHRINK: u32 = 2;
const F_SEAL_GROW: u32 = 4;
const F_SEAL_FUTURE_WRITE: u32 = 0x10;

fn sealed() -> StreamError {
    crate::fs::FileSystemError::new(
        crate::fs::FileSystemErrorKind::InvalidOperation,
        "Memfd operation is sealed",
    )
    .into()
}

pub(super) struct MemfdFile {
    backing: Arc<MemfdBacking>,
    operation: Mutex<()>,
    state: IrqRwSpinLock<MemfdState>,
}

impl MemfdFile {
    pub(super) fn new(allow_sealing: bool) -> Result<Self, &'static str> {
        Ok(Self {
            backing: Arc::new(MemfdBacking::new()?),
            operation: Mutex::new(()),
            state: IrqRwSpinLock::new(MemfdState {
                position: 0,
                seals: if allow_sealing { 0 } else { F_SEAL_SEAL },
            }),
        })
    }

    pub(super) fn seals(&self) -> u32 {
        self.state.read().seals
    }

    pub(super) fn add_seals(&self, seals: u32) -> Result<(), StreamError> {
        // F_SEAL_WRITE still requires accounting for existing writable VMAs.
        // FUTURE_WRITE instead captures a ceiling when admitting each new VMA.
        if seals & !(F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_FUTURE_WRITE) != 0 {
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
        Ok(self.backing.read_range(offset, buffer))
    }

    fn write_locked(&self, offset: usize, buffer: &[u8]) -> Result<usize, StreamError> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.seals() & F_SEAL_FUTURE_WRITE != 0 {
            return Err(sealed());
        }
        let end = offset
            .checked_add(buffer.len())
            .filter(|end| *end <= i64::MAX as usize && end.checked_add(PAGE_SIZE - 1).is_some())
            .ok_or(StreamError::InvalidArgument)?;
        let old_size = self.backing.size();
        if end > old_size && self.seals() & F_SEAL_GROW != 0 {
            return Err(sealed());
        }
        if end > old_size {
            self.backing.resize(end).map_err(|_| StreamError::NoSpace)?;
        }
        self.backing.write_range(offset, buffer)?;
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
    fn mapping_owner_for_range(
        &self,
        offset: usize,
        length: usize,
    ) -> Option<Arc<dyn MemoryMappingOps>> {
        if !self.backing.range_is_valid(offset, length) {
            return None;
        }
        Some(Arc::new(MemfdMapping {
            backing: self.backing.clone(),
            offset,
            length,
        }))
    }

    fn new_mapping_permissions(&self, is_shared: bool) -> usize {
        if is_shared && self.state.read().seals & F_SEAL_FUTURE_WRITE != 0 {
            usize::MAX & !2
        } else {
            usize::MAX
        }
    }

    fn supports_permission_changes(&self) -> bool {
        true
    }

    fn supports_shared_permission_changes(&self) -> bool {
        true
    }

    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        if !self.backing.range_is_valid(offset, length) {
            return Err("Memfd mapping range exceeded");
        }
        // Zero is the owner-backed demand-mapping sentinel. Physical pages are
        // resolved one at a time so file capacity does not consume all RAM.
        Ok(MemoryMappingInfo::new(0, 0x7, true))
    }

    fn supports_mmap(&self) -> bool {
        true
    }

    fn mmap_owner_name(&self) -> alloc::string::String {
        alloc::string::String::from("memfd")
    }

    fn can_extend_vma_on_fault(&self) -> bool {
        false
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
        let _ = (access, page_idx, vm_start);
        Err(crate::object::capability::memory_mapping::ResolveFaultError::Unmapped)
    }
}

/// A mapping keeps its file offset across fork, partial unmap and FD closure.
struct MemfdMapping {
    backing: Arc<MemfdBacking>,
    offset: usize,
    length: usize,
}

impl MemoryMappingOps for MemfdMapping {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        if offset
            .checked_add(length)
            .is_none_or(|end| end > self.length)
        {
            return Err("Memfd mapping range exceeded");
        }
        let file_offset = self
            .offset
            .checked_add(offset)
            .ok_or("Memfd offset overflow")?;
        if !self.backing.range_is_valid(file_offset, length) {
            return Err("Memfd mapping range exceeded");
        }
        Ok(MemoryMappingInfo::new(0, 0x7, true))
    }

    fn supports_permission_changes(&self) -> bool {
        true
    }

    fn supports_shared_permission_changes(&self) -> bool {
        true
    }

    fn resolve_fault(
        &self,
        _access: &crate::object::capability::memory_mapping::AccessKind,
        page_idx: usize,
        _vm_start: usize,
    ) -> Result<
        crate::object::capability::memory_mapping::ResolveFaultResult,
        crate::object::capability::memory_mapping::ResolveFaultError,
    > {
        use crate::object::capability::memory_mapping::{ResolveFaultError, ResolveFaultResult};
        let offset = page_idx
            .checked_mul(PAGE_SIZE)
            .and_then(|offset| self.offset.checked_add(offset))
            .ok_or(ResolveFaultError::Invalid)?;
        let paddr_page_base = self.backing.resolve_page(offset)?;
        Ok(ResolveFaultResult {
            paddr_page_base,
            is_tail: false,
        })
    }

    fn is_page_resident(&self, page_idx: usize) -> Option<bool> {
        let offset = page_idx.checked_mul(PAGE_SIZE)?.checked_add(self.offset)?;
        let page_index = offset / PAGE_SIZE;
        Some(
            self.backing
                .state
                .read()
                .pages
                .get(page_index)
                .is_some_and(Option::is_some),
        )
    }

    fn mmap_owner_name(&self) -> alloc::string::String {
        alloc::string::String::from("memfd")
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
            2 => self.backing.size() as u64,
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
        let old_size = self.backing.size();
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
        Ok(())
    }

    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::RegularFile,
            size: self.backing.size(),
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
    use crate::abi::linux::generic::{errno, fs, LinuxAbi};
    use crate::object::KernelObject;
    use crate::task::{clear_mock_current_task, set_mock_current_task, Task, TaskType};
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
    fn memfd_capacity_is_sparse_until_a_page_is_accessed() {
        use crate::object::capability::memory_mapping::{AccessKind, AccessOp, MemoryMappingOps};

        let file = MemfdFile::new(true).unwrap();
        file.truncate(256 * 1024 * 1024).unwrap();
        assert_eq!(file.backing.resident_pages(), 0);

        let owner = file.mapping_owner_for_range(0, PAGE_SIZE).unwrap();
        let access = AccessKind {
            op: AccessOp::Store,
            vaddr: 0,
            size: Some(1),
        };
        let first = owner.resolve_fault(&access, 0, 0).unwrap().paddr_page_base;
        let second = owner.resolve_fault(&access, 0, 0).unwrap().paddr_page_base;
        assert_eq!(first, second);
        assert_eq!(file.backing.resident_pages(), 1);
    }

    #[test_case]
    fn memfd_writes_crossing_pages_preserve_bytes() {
        let file = MemfdFile::new(true).unwrap();
        file.truncate((2 * PAGE_SIZE) as u64).unwrap();
        file.write_at((PAGE_SIZE - 2) as u64, b"cross-page")
            .unwrap();

        let mut bytes = [0u8; 10];
        assert_eq!(
            file.read_at((PAGE_SIZE - 2) as u64, &mut bytes).unwrap(),
            10
        );
        assert_eq!(&bytes, b"cross-page");
        assert_eq!(file.backing.resident_pages(), 2);
    }

    #[test_case]
    fn memfd_mapping_includes_the_partial_final_page() {
        use crate::object::capability::memory_mapping::{AccessKind, AccessOp, MemoryMappingOps};

        let file = MemfdFile::new(true).unwrap();
        file.truncate((PAGE_SIZE - 1) as u64).unwrap();
        file.write_at((PAGE_SIZE - 4) as u64, b"tail").unwrap();

        let owner = file.mapping_owner_for_range(0, PAGE_SIZE).unwrap();
        let access = AccessKind {
            op: AccessOp::Load,
            vaddr: PAGE_SIZE - 1,
            size: Some(1),
        };
        let before = owner.resolve_fault(&access, 0, 0).unwrap();
        let bytes = unsafe {
            core::slice::from_raw_parts(
                phys_to_virt(before.paddr_page_base) as *const u8,
                PAGE_SIZE,
            )
        };
        assert_eq!(&bytes[PAGE_SIZE - 4..], b"tail");
    }

    #[test_case]
    fn memfd_fcntl_duplicate_and_ftruncate_observe_shared_seals_and_errno() {
        use crate::environment::{PAGE_SIZE, USER_STACK_END};
        let task = Arc::new(Task::new("memfd-seals".into(), 1, TaskType::User));
        let code = USER_STACK_END - PAGE_SIZE;
        task.allocate_stack_pages(code, 1).unwrap();
        // RISC-V's syscall return reads the instruction to advance PC.
        for index in 0..8 {
            crate::library::std::usercopy::copy_to_user(
                &task,
                code + index * 4,
                &0x73u32.to_ne_bytes(),
            )
            .unwrap();
        }
        set_mock_current_task(task.clone());
        let mut abi = LinuxAbi::default();
        let file = Arc::new(MemfdFile::new(true).unwrap());
        file.truncate(32).unwrap();
        let handle = task.handle_table.insert(KernelObject::File(file)).unwrap();
        let fd = abi.allocate_fd(handle).unwrap();
        abi.set_file_status_flags(fd, 2).unwrap();
        let mut tf = Trapframe::new();
        tf.set_pc(code as u64);
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
