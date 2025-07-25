//! Memory mapping operations capability module
//! 
//! This module provides the MemoryMappingOps trait for objects that support
//! memory mapping operations like mmap and munmap.

pub mod syscall;

pub use syscall::{sys_memory_map, sys_memory_unmap};

/// Memory mapping operations capability
/// 
/// This trait represents the ability to map object contents into virtual memory.
/// Objects that support memory mapping (like files and devices) should implement
/// this trait to provide mmap/munmap functionality.
pub trait MemoryMappingOps: Send + Sync {
    /// Memory mapping operation
    /// 
    /// Maps the object's content into virtual memory at the specified address.
    /// 
    /// # Arguments
    /// 
    /// * `vaddr` - Virtual address where to map (0 means kernel chooses)
    /// * `length` - Length of the mapping in bytes
    /// * `prot` - Protection flags (PROT_READ, PROT_WRITE, PROT_EXEC)
    /// * `flags` - Mapping flags (MAP_SHARED, MAP_PRIVATE, etc.)
    /// * `offset` - Offset within the object to start mapping from
    /// 
    /// # Returns
    /// 
    /// * `Result<usize, &'static str>` - Virtual address of the mapping on success
    fn mmap(&self, vaddr: usize, length: usize, prot: usize, flags: usize, offset: usize) 
           -> Result<usize, &'static str> {
        let _ = (vaddr, length, prot, flags, offset);
        Err("mmap not supported for this object")
    }
    
    /// Memory unmapping operation
    /// 
    /// Unmaps a previously mapped memory region.
    /// 
    /// # Arguments
    /// 
    /// * `vaddr` - Virtual address of the mapping to unmap
    /// * `length` - Length of the mapping to unmap
    /// 
    /// # Returns
    /// 
    /// * `Result<(), &'static str>` - Success or error message
    fn munmap(&self, vaddr: usize, length: usize) -> Result<(), &'static str> {
        let _ = (vaddr, length);
        Err("munmap not supported for this object")
    }
    
    /// Check if memory mapping is supported
    /// 
    /// # Returns
    /// 
    /// * `bool` - true if this object supports memory mapping
    fn supports_mmap(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;

    // Mock object that implements MemoryMappingOps for testing
    struct MockMappableObject {
        should_fail: bool,
    }

    impl MemoryMappingOps for MockMappableObject {
        fn mmap(&self, vaddr: usize, length: usize, prot: usize, flags: usize, offset: usize) 
               -> Result<usize, &'static str> {
            let _ = (length, prot, flags); // Suppress unused warnings
            if self.should_fail {
                Err("Mock mmap failure")
            } else {
                // Return a mock virtual address
                Ok(if vaddr == 0 { 0x10000000 + offset } else { vaddr })
            }
        }

        fn munmap(&self, vaddr: usize, length: usize) -> Result<(), &'static str> {
            let _ = (vaddr, length); // Suppress unused warnings
            if self.should_fail {
                Err("Mock munmap failure")
            } else {
                Ok(())
            }
        }

        fn supports_mmap(&self) -> bool {
            true
        }
    }

    #[test_case]
    fn test_memory_mapping_ops_trait() {
        // Test the MemoryMappingOps trait implementation
        let mock_obj = MockMappableObject { should_fail: false };
        
        // Test supports_mmap
        assert!(mock_obj.supports_mmap());
        
        // Test successful mmap with different parameters
        let result = mock_obj.mmap(0, 8192, 0x3, 0x2, 1024);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0x10000400); // 0x10000000 + 1024
        
        // Test successful mmap with specified address
        let result = mock_obj.mmap(0x20000000, 8192, 0x3, 0x2, 0);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0x20000000);
        
        // Test successful munmap
        let result = mock_obj.munmap(0x20000000, 8192);
        assert!(result.is_ok());
        
        // Test failed mmap
        let mock_fail_obj = MockMappableObject { should_fail: true };
        let result = mock_fail_obj.mmap(0, 4096, 0x1, 0x1, 0);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Mock mmap failure");
        
        // Test failed munmap
        let result = mock_fail_obj.munmap(0x20000000, 8192);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Mock munmap failure");
    }

    #[test_case]
    fn test_memory_mapping_default_implementation() {
        // Test the default implementation that returns errors
        struct DefaultMappableObject;
        
        impl MemoryMappingOps for DefaultMappableObject {
            // Uses default implementations
        }
        
        let obj = DefaultMappableObject;
        
        // Test default mmap behavior
        let result = obj.mmap(0, 4096, 0x1, 0x1, 0);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "mmap not supported for this object");
        
        // Test default munmap behavior  
        let result = obj.munmap(0, 4096);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "munmap not supported for this object");
        
        // Test default supports_mmap behavior
        assert!(!obj.supports_mmap());
    }
}