//! Memory mapping operations capability module
//! 
//! This module provides the MemoryMappingOps trait for objects that support
//! memory mapping operations like mmap and munmap.

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