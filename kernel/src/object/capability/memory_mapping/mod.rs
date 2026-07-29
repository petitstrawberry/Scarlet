//! Memory mapping capability module
//!
//! This module provides the MemoryMappingOps trait for objects that support
//! memory mapping operations like mmap and munmap.

pub mod anon_owner;
pub mod syscall;

use crate::vm::vmem::MemoryAttribute;

pub use syscall::{sys_memory_map, sys_memory_unmap};

/// Information needed to map a region of an object into virtual memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryMappingInfo {
    /// Physical address backing the mapping.
    pub paddr: usize,
    /// Scarlet virtual-memory permission bits allowed by the object.
    pub permissions: usize,
    /// Whether the mapping is shared with other tasks.
    pub is_shared: bool,
    /// Cacheability/device attribute requested for the mapping.
    pub memory_attribute: MemoryAttribute,
}

impl MemoryMappingInfo {
    /// Creates mapping information for normal cacheable memory.
    ///
    /// # Arguments
    /// * `paddr` - Physical address backing the mapping
    /// * `permissions` - Scarlet virtual-memory permission bits allowed by the object
    /// * `is_shared` - Whether the mapping is shared with other tasks
    ///
    /// # Returns
    /// Mapping information with [`MemoryAttribute::Normal`].
    pub const fn new(paddr: usize, permissions: usize, is_shared: bool) -> Self {
        Self {
            paddr,
            permissions,
            is_shared,
            memory_attribute: MemoryAttribute::Normal,
        }
    }

    /// Returns this mapping information with an explicit memory attribute.
    ///
    /// # Arguments
    /// * `memory_attribute` - Cacheability/device attribute requested for the mapping
    ///
    /// # Returns
    /// The mapping information with the supplied memory attribute.
    pub const fn with_memory_attribute(self, memory_attribute: MemoryAttribute) -> Self {
        Self {
            memory_attribute,
            ..self
        }
    }
}

/// Memory mapping operations capability
///
/// This trait represents the ability to provide memory mapping information
/// and receive notifications about mapping lifecycle events.
/// Objects that support memory mapping (like files and devices) should implement
/// this trait to provide mmap/munmap functionality.
pub trait MemoryMappingOps: Send + Sync {
    /// Get mapping information for a region of the object
    ///
    /// Returns the physical address, permissions, sharing information, and
    /// memory attribute for mapping a region of this object into virtual memory.
    ///
    /// # Arguments
    /// * `offset` - Offset within the object to start mapping from
    /// * `length` - Length of the mapping in bytes
    ///
    /// # Returns
    /// * `Result<MemoryMappingInfo, &'static str>` - Mapping information on success
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str>;

    /// Get mapping information with sharing intent.
    ///
    /// Default implementation ignores `is_shared` and delegates to
    /// `get_mapping_info` for backward compatibility.
    ///
    /// # Arguments
    /// * `offset` - Offset within the object to start mapping from
    /// * `length` - Length of the mapping in bytes
    /// * `is_shared` - Whether the caller requested a shared mapping
    ///
    /// # Returns
    /// Mapping information on success.
    fn get_mapping_info_with(
        &self,
        offset: usize,
        length: usize,
        _is_shared: bool,
    ) -> Result<MemoryMappingInfo, &'static str> {
        self.get_mapping_info(offset, length)
    }

    /// Notification that a mapping has been created
    ///
    /// Called when a mapping of this object has been successfully created
    /// in the virtual memory manager. The object can use this to track
    /// its active mappings.
    ///
    /// # Arguments
    /// * `vaddr` - Virtual address where the mapping was created
    /// * `paddr` - Physical address that was mapped
    /// * `length` - Length of the mapping in bytes
    /// * `offset` - Offset within the object that was mapped
    fn on_mapped(&self, vaddr: usize, paddr: usize, length: usize, offset: usize) {}

    /// Notification that a mapping has been removed
    ///
    /// Called when a mapping of this object has been removed from
    /// the virtual memory manager. The object should clean up any
    /// tracking of this mapping.
    ///
    /// # Arguments
    /// * `vaddr` - Virtual address where the mapping was removed
    /// * `length` - Length of the mapping that was removed
    fn on_unmapped(&self, vaddr: usize, length: usize) {}

    /// Check if memory mapping is supported
    ///
    /// # Returns
    /// * `bool` - true if this object supports memory mapping
    fn supports_mmap(&self) -> bool {
        true
    }

    /// Return whether this object supports private copy-on-write mappings.
    ///
    /// Objects whose mappings must preserve shared device-memory semantics can
    /// reject `MAP_PRIVATE` without affecting their shared mapping capability.
    ///
    /// # Returns
    /// `true` when private mappings are supported.
    fn supports_private_mmap(&self) -> bool {
        true
    }

    /// Diagnostic helper: return a short owner name for logging
    ///
    /// Default implementation returns a generic "object" string. Implementers
    /// (e.g. VfsFileObject) should override to provide more meaningful names
    /// such as file paths.
    ///
    /// # Returns
    /// A short diagnostic name for this mapping owner.
    fn mmap_owner_name(&self) -> alloc::string::String {
        alloc::string::String::from("object")
    }

    /// Return whether this owner may grow an existing VMA on an out-of-range fault.
    ///
    /// Most mappings have a fixed virtual range determined by mmap. Owners should
    /// only return true when their backing object can grow independently after
    /// the VMA was created, and faults beyond the current VMA should make the
    /// VMA cover the newly valid backing range.
    ///
    /// # Returns
    /// `true` when the virtual memory manager may extend the VMA after a
    /// successful out-of-range fault resolution.
    fn can_extend_vma_on_fault(&self) -> bool {
        false
    }

    /// Resolve the physical backing page for a fault on an owner-backed mapping.
    ///
    /// # Arguments
    /// * `access` - Access that caused the fault
    /// * `page_idx` - Page index within the original mapping
    /// * `vm_start` - Original virtual start address for the mapping
    ///
    /// # Returns
    /// The resolved backing page, or an error describing why the fault cannot be resolved.
    fn resolve_fault(
        &self,
        access: &crate::object::capability::memory_mapping::AccessKind,
        page_idx: usize,
        vm_start: usize,
    ) -> core::result::Result<
        crate::object::capability::memory_mapping::ResolveFaultResult,
        crate::object::capability::memory_mapping::ResolveFaultError,
    > {
        let _ = access;
        let _ = page_idx;
        let _ = vm_start;
        Err(crate::object::capability::memory_mapping::ResolveFaultError::Unmapped)
    }

    /// Choose the actual page-table permissions for a resolved fault.
    ///
    /// The virtual memory map keeps the maximum permissions requested by mmap,
    /// while some owners need to install a stricter PTE temporarily. A
    /// framebuffer compatibility mapping, for example, keeps the VM map
    /// writable but resolves read faults as read-only so a later store still
    /// traps and can mark the legacy framebuffer dirty.
    ///
    /// # Arguments
    /// * `access` - Access that caused the fault
    /// * `default_permissions` - Permissions from the virtual memory map
    ///
    /// # Returns
    /// The permissions to install in the page table for this fault.
    fn fault_page_permissions(
        &self,
        access: &crate::object::capability::memory_mapping::AccessKind,
        default_permissions: usize,
    ) -> usize {
        let _ = access;
        default_permissions
    }

    /// Decide whether a private mapping fault must copy the resolved backing page.
    ///
    /// Most private object mappings use the owner as immutable backing storage
    /// and therefore copy on every resolved fault. Fork COW mappings override
    /// this so reads can share the backing page and stores perform the copy.
    ///
    /// # Arguments
    /// * `access` - Access that caused the fault.
    ///
    /// # Returns
    /// `true` when the fault handler must allocate a private page.
    fn private_fault_requires_copy(
        &self,
        access: &crate::object::capability::memory_mapping::AccessKind,
    ) -> bool {
        let _ = access;
        true
    }

    /// Release a range of pages owned by this mapping object.
    ///
    /// # Arguments
    /// * `start_page_idx` - First page index to release
    /// * `page_count` - Number of pages to release
    fn release_pages(&self, _start_page_idx: usize, _page_count: usize) {}

    /// Clone this mapping owner for a forked task.
    ///
    /// # Returns
    /// A cloned mapping owner when the object supports fork-specific ownership.
    fn fork_clone(&self) -> Option<alloc::sync::Arc<dyn MemoryMappingOps>> {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessOp {
    Instruction,
    Load,
    Store,
}

#[derive(Clone, Copy, Debug)]
pub struct AccessKind {
    pub op: AccessOp,
    pub vaddr: usize,
    pub size: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
pub struct ResolveFaultResult {
    pub paddr_page_base: usize,
    pub is_tail: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum ResolveFaultError {
    Invalid,
    Unmapped,
}

#[cfg(test)]
mod tests {
    use crate::sync::IrqRwSpinLock;

    use super::*;

    // Mock object that implements MemoryMappingOps for testing
    struct MockMappableObject {
        should_fail: bool,
        mapped_regions: IrqRwSpinLock<alloc::vec::Vec<(usize, usize)>>, // (vaddr, length)
    }

    impl MockMappableObject {
        fn new(should_fail: bool) -> Self {
            MockMappableObject {
                should_fail,
                mapped_regions: IrqRwSpinLock::new(alloc::vec::Vec::new()),
            }
        }
    }

    impl MemoryMappingOps for MockMappableObject {
        fn get_mapping_info(
            &self,
            offset: usize,
            _length: usize,
        ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
            if self.should_fail {
                Err("Mock get_mapping_info failure")
            } else {
                // Return mock physical address, read/write permissions, not shared
                Ok(MemoryMappingInfo::new(0x80000000 + offset, 0x3, false))
            }
        }

        fn on_mapped(&self, vaddr: usize, _paddr: usize, length: usize, _offset: usize) {
            if !self.should_fail {
                self.mapped_regions.write().push((vaddr, length));
            }
        }

        fn on_unmapped(&self, vaddr: usize, length: usize) {
            if !self.should_fail {
                let mut regions = self.mapped_regions.write();
                if let Some(pos) = regions
                    .iter()
                    .position(|(v, l)| *v == vaddr && *l == length)
                {
                    regions.remove(pos);
                }
            }
        }

        fn supports_mmap(&self) -> bool {
            !self.should_fail
        }

        fn mmap_owner_name(&self) -> alloc::string::String {
            alloc::string::String::from("mock_object")
        }
    }

    #[test_case]
    fn test_memory_mapping_ops_trait() {
        // Test the MemoryMappingOps trait implementation
        let mock_obj = MockMappableObject::new(false);

        // Test supports_mmap
        assert!(mock_obj.supports_mmap());

        // Test successful get_mapping_info
        let result = mock_obj.get_mapping_info(1024, 8192);
        assert!(result.is_ok());
        let info = result.unwrap();
        assert_eq!(info.paddr, 0x80000400); // 0x80000000 + 1024
        assert_eq!(info.permissions, 0x3);
        assert!(!info.is_shared);
        assert_eq!(info.memory_attribute, MemoryAttribute::Normal);

        // Test on_mapped notification
        mock_obj.on_mapped(0x10000000, 0x80000400, 8192, 1024);
        assert_eq!(mock_obj.mapped_regions.read().len(), 1);
        assert_eq!(mock_obj.mapped_regions.read()[0], (0x10000000, 8192));

        // Test on_unmapped notification
        mock_obj.on_unmapped(0x10000000, 8192);
        assert_eq!(mock_obj.mapped_regions.read().len(), 0);
    }

    #[test_case]
    fn test_memory_mapping_failure_cases() {
        // Test failure cases
        let mock_fail_obj = MockMappableObject::new(true);

        // Test supports_mmap returns false for failing object
        assert!(!mock_fail_obj.supports_mmap());

        // Test failed get_mapping_info
        let result = mock_fail_obj.get_mapping_info(0, 4096);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Mock get_mapping_info failure");

        // Test that on_mapped/on_unmapped don't panic for failing object
        mock_fail_obj.on_mapped(0x10000000, 0x80000000, 4096, 0);
        mock_fail_obj.on_unmapped(0x10000000, 4096);
        // Should not crash
    }
}
