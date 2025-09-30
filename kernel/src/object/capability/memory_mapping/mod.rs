//! Memory mapping capability module
//! 
//! This module provides the MemoryMappingOps trait for objects that support
//! memory mapping operations like mmap and munmap.

pub mod syscall;

pub use syscall::{sys_memory_map, sys_memory_unmap};

/// Memory mapping operations capability
/// 
/// This trait represents the ability to provide memory mapping information
/// and receive notifications about mapping lifecycle events.
/// Objects that support memory mapping (like files and devices) should implement
/// this trait to provide mmap/munmap functionality.
pub trait MemoryMappingOps: Send + Sync {
    /// Get mapping information for a region of the object
    /// 
    /// Returns the physical address, permissions, and sharing information
    /// for mapping a region of this object into virtual memory.
    /// 
    /// # Arguments
    /// * `offset` - Offset within the object to start mapping from
    /// * `length` - Length of the mapping in bytes
    /// 
    /// # Returns
    /// * `Result<(usize, usize, bool), &'static str>` - (paddr, permissions, is_shared) on success
    fn get_mapping_info(&self, offset: usize, length: usize) 
                       -> Result<(usize, usize, bool), &'static str>;
    
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

    /// Diagnostic helper: return a short owner name for logging
    ///
    /// Default implementation returns a generic "object" string. Implementers
    /// (e.g. VfsFileObject) should override to provide more meaningful names
    /// such as file paths.
    fn mmap_owner_name(&self) -> alloc::string::String {
        alloc::string::String::from("object")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock object that implements MemoryMappingOps for testing
    struct MockMappableObject {
        should_fail: bool,
        mapped_regions: spin::RwLock<alloc::vec::Vec<(usize, usize)>>, // (vaddr, length)
    }

    impl MockMappableObject {
        fn new(should_fail: bool) -> Self {
            MockMappableObject {
                should_fail,
                mapped_regions: spin::RwLock::new(alloc::vec::Vec::new()),
            }
        }
    }

    impl MemoryMappingOps for MockMappableObject {
        fn get_mapping_info(&self, offset: usize, _length: usize) 
                           -> Result<(usize, usize, bool), &'static str> {
            if self.should_fail {
                Err("Mock get_mapping_info failure")
            } else {
                // Return mock physical address, read/write permissions, not shared
                Ok((0x80000000 + offset, 0x3, false))
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
                if let Some(pos) = regions.iter().position(|(v, l)| *v == vaddr && *l == length) {
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
        let (paddr, permissions, is_shared) = result.unwrap();
        assert_eq!(paddr, 0x80000400); // 0x80000000 + 1024
        assert_eq!(permissions, 0x3);
        assert!(!is_shared);
        
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