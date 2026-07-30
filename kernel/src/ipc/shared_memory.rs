//! Shared memory implementation for inter-process communication
//!
//! This module provides shared memory objects for memory-based communication between processes.
//! Shared memory allows multiple processes to access the same physical memory region,
//! providing efficient data sharing without copying.

use crate::sync::IrqRwSpinLock;
use alloc::{format, string::String, sync::Arc, vec::Vec};

use crate::mem::page::{allocate_raw_pages, free_raw_pages};
use crate::object::capability::memory_mapping::{
    AccessKind, MemoryMappingOps, ResolveFaultError, ResolveFaultResult,
};
use crate::vm::addr::{phys_to_virt, virt_to_phys};
use crate::vm::vmem::VirtualMemoryMap;

const LOG_SHARED_MEMORY_RESIZE: bool = false;

/// Kernel-only description of stable shared-memory backing.
///
/// This is returned only after a shared-memory range has been pinned. Its
/// physical address is never exposed through a userspace ABI.
#[derive(Debug, Clone, Copy)]
pub struct SharedMemoryBacking {
    paddr: usize,
    size: usize,
}

impl SharedMemoryBacking {
    pub(crate) const fn paddr(&self) -> usize {
        self.paddr
    }

    pub(crate) const fn size(&self) -> usize {
        self.size
    }
}

/// Strong shared-memory pin retained by a kernel importer.
///
/// The owner Arc keeps the SharedMemory object alive until after the pin is
/// released, so a consumer never relies on a raw physical address alone.
pub(crate) struct SharedMemoryPin {
    owner: Arc<dyn SharedMemoryObject>,
    backing: SharedMemoryBacking,
}

impl SharedMemoryPin {
    pub(crate) fn new(
        owner: Arc<dyn SharedMemoryObject>,
        offset: usize,
        length: usize,
    ) -> Result<Self, &'static str> {
        let backing = owner.pin_range(offset, length)?;
        Ok(Self { owner, backing })
    }

    pub(crate) const fn backing(&self) -> SharedMemoryBacking {
        self.backing
    }
}

impl Drop for SharedMemoryPin {
    fn drop(&mut self) {
        self.owner.unpin_range();
    }
}

/// Shared memory operations
///
/// This trait extends the base functionality for shared memory objects.
pub trait SharedMemoryObject: MemoryMappingOps + Send + Sync {
    /// Get the size of the shared memory region in bytes
    fn size(&self) -> usize;

    /// Resize the shared memory region (within capacity)
    fn resize(&self, new_size: usize) -> Result<(), &'static str>;

    /// Get a unique identifier for this shared memory object
    fn id(&self) -> String;

    /// Check if the shared memory is still valid
    fn is_valid(&self) -> bool;

    /// Pin a live range so its backing cannot change while imported.
    ///
    /// # Arguments
    ///
    /// * `offset` - Byte offset of the range to validate.
    /// * `length` - Non-zero byte length of the range to validate.
    ///
    /// # Returns
    ///
    /// Kernel-only metadata for the stable backing, or an error when the
    /// object is invalid or the range is outside its current size. Each
    /// successful call must be paired with [`SharedMemoryObject::unpin_range`].
    fn pin_range(&self, offset: usize, length: usize) -> Result<SharedMemoryBacking, &'static str>;

    /// Release one prior shared-memory backing pin.
    ///
    /// # Returns
    ///
    /// Nothing. Calls without a corresponding pin are ignored so importer drop
    /// paths remain safe during error cleanup.
    fn unpin_range(&self);
}

/// Internal state of a shared memory object
struct SharedMemoryState {
    /// Physical address of the shared memory region
    paddr: usize,
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
    stale_pages: Vec<(usize, usize)>,
    /// Whether this object owns the physical memory (should free on drop)
    owns_memory: bool,
}

impl SharedMemoryState {
    fn new(paddr: usize, size: usize, permissions: usize, owns_memory: bool) -> Self {
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
    pub unsafe fn from_paddr(paddr: usize, size: usize, permissions: usize) -> Self {
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
                free_raw_pages(old_ptr, old_pages);
            }
        }

        state.paddr = virt_to_phys(pages as usize);
        state.size = aligned_size;
        state.capacity = aligned_size;

        // if LOG_SHARED_MEMORY_RESIZE {
        //     crate::println!(
        //         "[SharedMemory::resize] reallocated: old_paddr={:#x} new_paddr={:#x} mapping_count={}",
        //         old_paddr,
        //         state.paddr,
        //         state.mapping_count
        //     );
        // }

        // NOTE: マッピングがある場合、古いpmap.pmarea.startは無効になる
        // resolve_faultで動的にpaddrを取得するように修正済み
        // if LOG_SHARED_MEMORY_RESIZE && state.mapping_count > 0 {
        //     crate::println!(
        //         "[SharedMemory::resize] WARNING: {} active mappings exist, their pmarea is now stale!",
        //         state.mapping_count
        //     );
        // }

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
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        let state = self.state.read();

        if !state.valid {
            return Err("Shared memory object is not valid");
        }

        let end = match offset.checked_add(length) {
            Some(end) => end,
            None => {
                return Err("Mapping request exceeds shared memory size");
            }
        };

        if end > state.size {
            return Err("Mapping request exceeds shared memory size");
        }

        // Return physical address (base + offset), permissions, and shared flag.
        let paddr = state
            .paddr
            .checked_add(offset)
            .ok_or("Physical address overflow in shared memory mapping")?;

        Ok(crate::object::capability::MemoryMappingInfo::new(
            paddr,
            state.permissions,
            true,
        ))
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        let mut state = self.state.write();
        state.mapping_count += 1;
    }

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        let mut state = self.state.write();
        if state.mapping_count > 0 {
            state.mapping_count -= 1;
        }
        if state.mapping_count == 0 && !state.stale_pages.is_empty() {
            let stale = core::mem::take(&mut state.stale_pages);
            for (paddr, pages) in stale {
                if pages == 0 {
                    continue;
                }
                let ptr = phys_to_virt(paddr) as *mut crate::mem::page::Page;
                free_raw_pages(ptr, pages);
            }
        }
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
        let state = self.state.read();

        if !state.valid {
            return Err(ResolveFaultError::Invalid);
        }

        // Calculate the page-aligned virtual address
        let page_vaddr = access.vaddr & !(crate::environment::PAGE_SIZE - 1);

        // vmarea範囲チェック（マッピング時のサイズ）
        // NOTE: ftruncateでリサイズされた場合、vmarea.endは古いままなので、
        //       SharedMemoryの現在のsizeも確認する必要がある
        if page_vaddr < vm_start {
            return Err(ResolveFaultError::Unmapped);
        }

        let offset_in_mapping = page_vaddr - vm_start;

        // 現在のSharedMemoryサイズを確認（動的に拡張された可能性がある）
        if offset_in_mapping >= state.size {
            return Err(ResolveFaultError::Unmapped);
        }

        // 動的にpaddrを取得（resizeで変更されている可能性があるため）
        // map.pmarea.startは古い可能性があるので使わない
        let paddr_page_base = state
            .paddr
            .checked_add(offset_in_mapping)
            .ok_or(ResolveFaultError::Invalid)?;

        Ok(ResolveFaultResult {
            paddr_page_base,
            is_tail: false,
        })
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        use crate::environment::PAGE_SIZE;

        let state = self.state.read();
        if state.mapping_count > 0 {
            crate::println!(
                "[SharedMemory::drop] leaking {} pages for {} active mapping(s)",
                (state.capacity + PAGE_SIZE - 1) / PAGE_SIZE,
                state.mapping_count
            );
            return;
        }

        // Only free the physical pages if this object owns them
        if state.owns_memory {
            let num_pages = (state.capacity + PAGE_SIZE - 1) / PAGE_SIZE;
            let pages_ptr = phys_to_virt(state.paddr) as *mut crate::mem::page::Page;
            free_raw_pages(pages_ptr, num_pages);
            for (paddr, pages) in &state.stale_pages {
                if *pages == 0 {
                    continue;
                }
                let pages_ptr = phys_to_virt(*paddr) as *mut crate::mem::page::Page;
                free_raw_pages(pages_ptr, *pages);
            }
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
