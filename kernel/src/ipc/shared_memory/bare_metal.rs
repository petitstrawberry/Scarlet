use super::{SharedMemoryBacking, SharedMemoryObject};
use crate::mem::page::{allocate_raw_pages, free_raw_pages};
use crate::object::capability::memory_mapping::{
    AccessKind, MappingError, MappingRequest, MemoryMappingInfo, MemoryMappingOps, PreparedMapping,
    ResolveFaultError, ResolveFaultResult,
};
use crate::sync::IrqRwSpinLock;
use crate::vm::addr::{phys_to_virt, virt_to_phys};
use alloc::{format, string::String, sync::Arc, vec::Vec};

/// Internal state of a shared memory object
struct SharedMemoryState {
    /// Physical address of the shared memory region
    paddr: u64,
    /// Size of the shared memory region in bytes
    size: usize,
    /// Allocated capacity of the shared memory region in bytes
    capacity: usize,
    /// Access permissions for the shared memory
    permissions: usize,
    /// Whether this shared memory is still valid
    valid: bool,
    /// Number of active mappings
    mapping_count: usize,
    /// Number of active kernel imports retaining this backing.
    pin_count: usize,
    /// Old allocations kept alive while mappings still exist
    stale_pages: Vec<(u64, usize)>,
    /// Whether this object owns the physical memory (should free on drop)
    owns_memory: bool,
}

impl SharedMemoryState {
    fn new(paddr: u64, size: usize, permissions: usize, owns_memory: bool) -> Self {
        Self {
            paddr,
            size,
            capacity: size,
            permissions,
            valid: true,
            mapping_count: 0,
            pin_count: 0,
            stale_pages: Vec::new(),
            owns_memory,
        }
    }

    fn mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        if !self.valid {
            return Err("Shared memory object is not valid");
        }
        if offset.checked_add(length).is_none_or(|end| end > self.size) {
            return Err("Mapping request exceeds shared memory size");
        }
        let paddr = self
            .paddr
            .checked_add(offset as u64)
            .ok_or("Physical address overflow in shared memory mapping")?;
        Ok(MemoryMappingInfo::new(paddr, self.permissions, true))
    }

    fn resolve_offset(&self, offset: usize) -> Result<ResolveFaultResult, ResolveFaultError> {
        if !self.valid {
            return Err(ResolveFaultError::Invalid);
        }
        if offset >= self.size {
            return Err(ResolveFaultError::Unmapped);
        }
        let paddr_page_base = self
            .paddr
            .checked_add(offset as u64)
            .ok_or(ResolveFaultError::Invalid)?;
        Ok(ResolveFaultResult {
            paddr_page_base,
            is_tail: false,
        })
    }

    fn release_mapping(&mut self) {
        self.mapping_count = self.mapping_count.saturating_sub(1);
        if self.mapping_count == 0 {
            for (paddr, pages) in self.stale_pages.drain(..) {
                let ptr = phys_to_virt(paddr) as *mut crate::mem::page::Page;
                // SAFETY: This retired allocation is no longer mapped or leased.
                unsafe { free_raw_pages(ptr, pages) };
            }
        }
    }
}

/// A shared memory object for inter-process communication
///
/// SharedMemory provides a memory region that can be mapped into multiple
/// processes' address spaces, allowing efficient data sharing without copying.
pub struct SharedMemory {
    /// Shared state of the memory object
    state: Arc<IrqRwSpinLock<SharedMemoryState>>,
    /// Unique identifier for debugging
    id: String,
}

impl SharedMemory {
    /// Create a new shared memory object with the specified size and permissions
    ///
    /// # Arguments
    /// * `size` - Size of the shared memory region in bytes (will be rounded up to page size)
    /// * `permissions` - Access permissions (read/write/execute flags)
    ///
    /// # Returns
    /// A new shared memory object, or an error if allocation fails
    pub fn new(size: usize, permissions: usize) -> Result<Self, &'static str> {
        use crate::environment::PAGE_SIZE;

        if size == 0 {
            return Err("Size must be greater than 0");
        }

        // Calculate number of pages needed (round up)
        let num_pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        let aligned_size = num_pages * PAGE_SIZE;

        // Allocate physical memory for the shared region
        let pages = allocate_raw_pages(num_pages);
        if pages.is_null() {
            return Err("Failed to allocate physical memory for shared memory");
        }
        let paddr = virt_to_phys(pages as usize);

        let state = SharedMemoryState::new(paddr, aligned_size, permissions, true);
        let id = format!("shmem_{:#x}", paddr);

        Ok(Self {
            state: Arc::new(IrqRwSpinLock::new(state)),
            id,
        })
    }

    /// Create a shared memory object from an existing physical address
    ///
    /// # Arguments
    /// * `paddr` - Physical address of the memory region
    /// * `size` - Size of the memory region in bytes
    /// * `permissions` - Access permissions
    ///
    /// # Returns
    /// A new shared memory object wrapping the existing memory
    ///
    /// # Safety
    /// The caller must ensure that the physical address is valid and the size is correct.
    /// The physical memory must remain valid for the lifetime of this object.
    /// This object will NOT free the memory on drop - the caller is responsible for
    /// managing the memory lifetime.
    pub unsafe fn from_paddr(paddr: u64, size: usize, permissions: usize) -> Self {
        let state = SharedMemoryState::new(paddr, size, permissions, false);
        let id = format!("shmem_{:#x}", paddr);

        Self {
            state: Arc::new(IrqRwSpinLock::new(state)),
            id,
        }
    }

    /// Invalidate this shared memory object
    ///
    /// This marks the shared memory as invalid, preventing new mappings.
    /// Existing mappings may continue to work but should be unmapped.
    pub fn invalidate(&self) {
        let mut state = self.state.write();
        state.valid = false;
    }
}

impl SharedMemoryObject for SharedMemory {
    fn size(&self) -> usize {
        self.state.read().size
    }

    fn resize(&self, new_size: usize) -> Result<(), &'static str> {
        use crate::environment::PAGE_SIZE;

        let mut state = self.state.write();
        let aligned_size = if new_size == 0 {
            0
        } else {
            let num_pages = (new_size + PAGE_SIZE - 1) / PAGE_SIZE;
            num_pages * PAGE_SIZE
        };

        if state.pin_count != 0 && aligned_size != state.size {
            return Err("Shared memory cannot resize while imported");
        }

        // 容量内であればサイズ更新のみ
        if aligned_size <= state.capacity {
            state.size = aligned_size;
            return Ok(());
        }

        if !state.owns_memory {
            return Err("Shared memory resize not supported for external memory");
        }

        // 新しいメモリを割り当て
        let num_pages = aligned_size / PAGE_SIZE;
        let pages = allocate_raw_pages(num_pages);
        if pages.is_null() {
            return Err("Failed to allocate physical memory for shared memory resize");
        }

        let old_paddr = state.paddr;
        let copy_size = state.size;

        // 古いデータをコピー
        if copy_size > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    phys_to_virt(state.paddr) as *const u8,
                    pages as *mut u8,
                    copy_size,
                );
            }
        }

        // 古いメモリを解放
        let old_pages = state.capacity / PAGE_SIZE;
        if old_pages > 0 {
            if state.mapping_count > 0 {
                state.stale_pages.push((old_paddr, old_pages));
            } else {
                let old_ptr = phys_to_virt(old_paddr) as *mut crate::mem::page::Page;
                // SAFETY: The state lock protects the owned old allocation;
                // copying is complete and no mapping still refers to it.
                unsafe { free_raw_pages(old_ptr, old_pages) };
            }
        }

        state.paddr = virt_to_phys(pages as usize);
        state.size = aligned_size;
        state.capacity = aligned_size;

        Ok(())
    }

    fn id(&self) -> String {
        self.id.clone()
    }

    fn is_valid(&self) -> bool {
        self.state.read().valid
    }

    fn pin_range(&self, offset: usize, length: usize) -> Result<SharedMemoryBacking, &'static str> {
        if length == 0 {
            return Err("Shared memory import range must be non-empty");
        }
        let mut state = self.state.write();
        if !state.valid || state.paddr == 0 {
            return Err("Shared memory backing is not live");
        }
        let end = offset
            .checked_add(length)
            .ok_or("Shared memory import range overflows")?;
        if end > state.size {
            return Err("Shared memory import range exceeds backing");
        }
        state.pin_count = state
            .pin_count
            .checked_add(1)
            .ok_or("Shared memory import pin count overflows")?;
        Ok(SharedMemoryBacking {
            paddr: state.paddr,
            size: state.size,
        })
    }

    fn unpin_range(&self) {
        let mut state = self.state.write();
        if state.pin_count != 0 {
            state.pin_count -= 1;
        }
    }
}

impl MemoryMappingOps for SharedMemory {
    fn prepare_mapping(&self, request: MappingRequest) -> Result<PreparedMapping, MappingError> {
        use crate::environment::PAGE_SIZE;

        let mut state = self.state.write();
        if !state.valid {
            return Err(MappingError::Unsupported);
        }
        if !request.offset.is_multiple_of(PAGE_SIZE) {
            return Err(MappingError::InvalidRange("Unaligned shared memory offset"));
        }
        let mut length = request.length;
        let info = if request.private_demand {
            None
        } else {
            if request.allow_shorter {
                let available = state.size.saturating_sub(request.offset);
                length = length.min(available / PAGE_SIZE * PAGE_SIZE);
                if length == 0 {
                    return Err(MappingError::InvalidRange(
                        "Mapping request exceeds shared memory size",
                    ));
                }
            }
            Some(
                state
                    .mapping_info(request.offset, length)
                    .map_err(MappingError::InvalidRange)?,
            )
        };
        state.mapping_count += 1;
        drop(state);

        let owner: Arc<dyn MemoryMappingOps> = Arc::new(SharedMemoryMapping {
            state: self.state.clone(),
            offset: request.offset,
            id: self.id.clone(),
        });
        Ok(PreparedMapping {
            length,
            info,
            owner: Some(owner.clone()),
            // The lease is acquired during preparation and lives until the
            // last VMA slice or placement attempt releases its owner.
            notification: Some(owner),
            maximum_permissions: self.new_mapping_permissions(request.is_shared),
        })
    }

    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        self.state.read().mapping_info(offset, length)
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: u64, _length: usize, _offset: usize) {
        self.state.write().mapping_count += 1;
    }

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        self.state.write().release_mapping();
    }

    fn supports_mmap(&self) -> bool {
        self.state.read().valid
    }

    fn mmap_owner_name(&self) -> String {
        self.id.clone()
    }

    fn can_extend_vma_on_fault(&self) -> bool {
        true
    }

    fn resolve_fault(
        &self,
        access: &AccessKind,
        _page_idx: usize,
        vm_start: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        let page_vaddr = access.vaddr & !(crate::environment::PAGE_SIZE - 1);
        let offset = page_vaddr
            .checked_sub(vm_start)
            .ok_or(ResolveFaultError::Unmapped)?;
        self.state.read().resolve_offset(offset)
    }
}

struct SharedMemoryMapping {
    state: Arc<IrqRwSpinLock<SharedMemoryState>>,
    offset: usize,
    id: String,
}

impl MemoryMappingOps for SharedMemoryMapping {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        let offset = self
            .offset
            .checked_add(offset)
            .ok_or("Shared memory offset overflow")?;
        self.state.read().mapping_info(offset, length)
    }

    fn mmap_owner_name(&self) -> String {
        self.id.clone()
    }

    fn can_extend_vma_on_fault(&self) -> bool {
        true
    }

    fn resolve_fault(
        &self,
        _access: &AccessKind,
        page_idx: usize,
        _vm_start: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        let offset = page_idx
            .checked_mul(crate::environment::PAGE_SIZE)
            .and_then(|offset| self.offset.checked_add(offset))
            .ok_or(ResolveFaultError::Invalid)?;
        self.state.read().resolve_offset(offset)
    }
}

impl Drop for SharedMemoryMapping {
    fn drop(&mut self) {
        self.state.write().release_mapping();
    }
}

impl Drop for SharedMemoryState {
    fn drop(&mut self) {
        use crate::environment::PAGE_SIZE;

        if self.mapping_count > 0 {
            crate::println!(
                "[SharedMemory::drop] leaking {} pages for {} untracked mapping(s)",
                self.capacity.div_ceil(PAGE_SIZE),
                self.mapping_count
            );
            return;
        }

        if self.owns_memory {
            let ptr = phys_to_virt(self.paddr) as *mut crate::mem::page::Page;
            // SAFETY: Final drop owns this allocation and its original page count.
            unsafe { free_raw_pages(ptr, self.capacity.div_ceil(PAGE_SIZE)) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test_case]
    fn test_shared_memory_creation() {
        // Test creating a new shared memory object
        let permissions = 0x3; // Read + Write
        let size = 4096;

        // Test with actual allocation
        match SharedMemory::new(size, permissions) {
            Ok(shmem) => {
                assert!(shmem.size() >= size); // Size might be rounded up to page size
                assert!(shmem.is_valid());
                assert!(shmem.supports_mmap());
            }
            Err(e) => {
                // If allocation fails, that's also acceptable in test environment
                crate::println!("SharedMemory::new failed: {}", e);
            }
        }
    }

    #[test_case]
    fn test_shared_memory_from_paddr() {
        // Test creating a shared memory object from an existing physical address
        let paddr = 0x80000000;
        let size = 8192;
        let permissions = 0x3; // Read + Write

        let shmem = unsafe { SharedMemory::from_paddr(paddr, size, permissions) };

        assert_eq!(shmem.size(), size);
        assert!(shmem.is_valid());
        assert!(shmem.supports_mmap());
    }

    #[test_case]
    fn test_shared_memory_mapping_info() {
        let paddr = 0x80000000;
        let size = 4096;
        let permissions = 0x3; // Read + Write

        let shmem = unsafe { SharedMemory::from_paddr(paddr, size, permissions) };

        // Test valid mapping request
        match shmem.get_mapping_info(0, 4096) {
            Ok(info) => {
                assert_eq!(info.paddr, paddr);
                assert_eq!(info.permissions, permissions);
                assert!(info.is_shared); // Shared memory should always be shared
            }
            Err(e) => panic!("Mapping info failed: {}", e),
        }

        // Test mapping with offset
        match shmem.get_mapping_info(1024, 2048) {
            Ok(info) => {
                assert_eq!(info.paddr, paddr + 1024);
                assert_eq!(info.permissions, permissions);
                assert!(info.is_shared);
            }
            Err(e) => panic!("Mapping info with offset failed: {}", e),
        }

        // Test invalid mapping request (exceeds size)
        assert!(shmem.get_mapping_info(0, 8192).is_err());
        assert!(shmem.get_mapping_info(4096, 1).is_err());
    }

    #[test_case]
    fn test_shared_memory_invalidation() {
        let paddr = 0x80000000;
        let size = 4096;
        let permissions = 0x3;

        let shmem = unsafe { SharedMemory::from_paddr(paddr, size, permissions) };

        assert!(shmem.is_valid());
        assert!(shmem.supports_mmap());

        // Invalidate the shared memory
        shmem.invalidate();

        assert!(!shmem.is_valid());
        assert!(!shmem.supports_mmap());

        // Mapping should fail after invalidation
        assert!(shmem.get_mapping_info(0, 4096).is_err());
    }

    #[test_case]
    fn test_shared_memory_mapping_tracking() {
        let paddr = 0x80000000;
        let size = 4096;
        let permissions = 0x3;

        let shmem = unsafe { SharedMemory::from_paddr(paddr, size, permissions) };

        // Initial mapping count should be 0
        assert_eq!(shmem.state.read().mapping_count, 0);

        // Simulate mapping
        shmem.on_mapped(0x10000000, paddr, 4096, 0);
        assert_eq!(shmem.state.read().mapping_count, 1);

        // Simulate another mapping
        shmem.on_mapped(0x20000000, paddr, 4096, 0);
        assert_eq!(shmem.state.read().mapping_count, 2);

        // Simulate unmapping
        shmem.on_unmapped(0x10000000, 4096);
        assert_eq!(shmem.state.read().mapping_count, 1);

        shmem.on_unmapped(0x20000000, 4096);
        assert_eq!(shmem.state.read().mapping_count, 0);
    }

    #[test_case]
    fn prepared_shared_memory_retains_its_old_allocation_and_fault_offset() {
        use crate::environment::PAGE_SIZE;

        let shmem = SharedMemory::new(2 * PAGE_SIZE, 3).unwrap();
        let prepared = shmem
            .prepare_mapping(MappingRequest {
                offset: PAGE_SIZE,
                length: PAGE_SIZE,
                is_shared: true,
                permissions: 3,
                private_demand: false,
                allow_shorter: false,
            })
            .unwrap();
        let old_base = shmem.get_mapping_info(0, PAGE_SIZE).unwrap().paddr;
        assert_eq!(prepared.info.unwrap().paddr, old_base + PAGE_SIZE as u64);

        shmem.resize(3 * PAGE_SIZE).unwrap();
        assert_eq!(shmem.state.read().stale_pages.as_slice(), &[(old_base, 2)]);
        let owner = prepared.owner.as_ref().unwrap();
        let access = AccessKind {
            op: crate::object::capability::memory_mapping::AccessOp::Load,
            vaddr: 0x2000 + PAGE_SIZE,
            size: Some(1),
        };
        let grown = shmem.get_mapping_info(2 * PAGE_SIZE, PAGE_SIZE).unwrap();
        assert_eq!(
            owner
                .resolve_fault(&access, 1, 0x2000)
                .unwrap()
                .paddr_page_base,
            grown.paddr
        );

        drop(prepared);
        assert!(shmem.state.read().stale_pages.is_empty());
    }

    #[test_case]
    fn test_shared_memory_pin_blocks_all_size_changes() {
        let paddr = 0x80000000;
        let size = 8192;
        let permissions = 0x3;
        let shmem = unsafe { SharedMemory::from_paddr(paddr, size, permissions) };

        assert!(shmem.pin_range(0, size).is_ok());
        assert_eq!(shmem.state.read().pin_count, 1);
        assert!(shmem.resize(4096).is_err());
        assert!(shmem.resize(12288).is_err());
        assert_eq!(shmem.resize(size), Ok(()));
        shmem.unpin_range();
        assert_eq!(shmem.state.read().pin_count, 0);
        assert_eq!(shmem.resize(4096), Ok(()));
    }

    #[test_case]
    fn test_shared_memory_pin_validates_import_range() {
        let paddr = 0x80000000;
        let size = 8192;
        let permissions = 0x3;
        let shmem = unsafe { SharedMemory::from_paddr(paddr, size, permissions) };

        assert!(shmem.pin_range(0, 0).is_err());
        assert!(shmem.pin_range(size, 1).is_err());
        assert!(shmem.pin_range(size - 1, 2).is_err());
        assert_eq!(shmem.state.read().pin_count, 0);
    }

    #[test_case]
    fn test_shared_memory_mmap_owner_name() {
        let paddr = 0x80000000;
        let size = 4096;
        let permissions = 0x3;

        let shmem = unsafe { SharedMemory::from_paddr(paddr, size, permissions) };

        let owner_name = shmem.mmap_owner_name();
        assert!(owner_name.contains("shmem"));
        assert!(owner_name.contains(&format!("{:#x}", paddr)));
    }
}
