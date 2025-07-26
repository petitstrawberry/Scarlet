use alloc::sync::Weak;
use crate::object::capability::memory_mapping::MemoryMappingOps;

/// Represents a mapping between physical and virtual memory areas.
///
/// This structure defines the relationship between a physical memory area
/// and its corresponding virtual memory area in the kernel's memory management system.
///
/// # Fields
///
/// * `pmarea` - The physical memory area that is being mapped
/// * `vmarea` - The virtual memory area where the physical memory is mapped to
/// * `permissions` - The access permissions for this mapping
/// * `is_shared` - Whether this mapping is shared between processes
/// * `owner` - Optional weak reference to the object that created this mapping (None for anonymous mappings)
#[derive(Debug, Clone)]
pub struct VirtualMemoryMap {
    pub pmarea: MemoryArea,
    pub vmarea: MemoryArea,
    pub permissions: usize,
    pub is_shared: bool,
    pub owner: Option<Weak<dyn MemoryMappingOps>>,
}

impl VirtualMemoryMap {
    /// Creates a new virtual memory map with the given physical and virtual memory areas.
    /// 
    /// # Arguments
    /// * `pmarea` - The physical memory area to map
    /// * `vmarea` - The virtual memory area to map to
    /// * `permissions` - The permissions to set for the virtual memory area
    /// * `is_shared` - Whether this memory map should be shared between tasks
    /// * `owner` - Optional weak reference to the object that created this mapping (None for anonymous mappings)
    /// 
    /// # Returns
    /// A new virtual memory map with the given physical and virtual memory areas.
    pub fn new(pmarea: MemoryArea, vmarea: MemoryArea, permissions: usize, is_shared: bool, owner: Option<Weak<dyn MemoryMappingOps>>) -> Self {
        VirtualMemoryMap {
            pmarea,
            vmarea,
            permissions,
            is_shared,
            owner,
        }
    }

    /// Returns the physical address corresponding to the given virtual address.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to translate
    /// 
    /// # Returns
    /// The physical address corresponding to the given virtual address, if it exists.
    /// If the virtual address is not part of the memory map, `None` is returned.
    pub fn get_paddr(&self, vaddr: usize) -> Option<usize> {
        if self.vmarea.start <= vaddr && vaddr <= self.vmarea.end {
            Some(self.pmarea.start + (vaddr - self.vmarea.start))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryArea {
    pub start: usize,
    pub end: usize,
}

impl MemoryArea {
    /// Creates a new memory area with the given start and end addresses
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
    
    /// Creates a new memory area from a pointer and size
    pub fn from_ptr(ptr: *const u8, size: usize) -> Self {
        let start = ptr as usize;
        let end = if size > 0 { start + size - 1 } else { start };
        Self { start, end }
    }
    
    /// Returns the size of the memory area in bytes
    pub fn size(&self) -> usize {
        if self.start > self.end {
            panic!("Invalid memory area: start > end: {:#x} > {:#x}", self.start, self.end);
        }
        self.end - self.start + 1
    }
    
    /// Returns a slice reference to the memory area
    /// 
    /// # Safety
    /// This function assumes that the start and end of MemoryArea point to valid memory ranges.
    /// If not, undefined behavior may occur.
    /// Therefore, make sure that MemoryArea points to a valid range before using this function.
    /// 
    /// # Returns
    /// 
    /// A slice reference to the memory area
    ///
    pub unsafe fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.start as *const u8, self.size()) }
    }
    
    /// Returns a mutable slice reference to the memory area
    /// 
    /// # Safety
    /// This function assumes that the start and end of MemoryArea point to valid memory ranges.
    /// If not, undefined behavior may occur.
    /// Therefore, make sure that MemoryArea points to a valid range before using this function.
    ///
    /// # Returns
    ///
    /// A mutable slice reference to the memory area
    ///
    pub unsafe fn as_slice_mut(&self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.start as *mut u8, self.size()) }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum VirtualMemoryPermission {
    Read = 0x01,
    Write = 0x02,
    Execute = 0x04,
    User = 0x08,
}

impl From<usize> for VirtualMemoryPermission {
    fn from(value: usize) -> Self {
        match value {
            0x01 => VirtualMemoryPermission::Read,
            0x02 => VirtualMemoryPermission::Write,
            0x04 => VirtualMemoryPermission::Execute,
            0x08 => VirtualMemoryPermission::User,
            _ => panic!("Invalid permission value: {}", value),
        }
    }
}

impl VirtualMemoryPermission {
    pub fn contained_in(&self, permissions: usize) -> bool {
        permissions & (*self as usize) != 0
    }
}

pub enum VirtualMemoryRegion {
    Text,
    Data,
    Bss,
    Heap,
    Stack,
    Guard,
    Unknown,
}

impl VirtualMemoryRegion {
    pub fn default_permissions(&self) -> usize {
        match self {
            VirtualMemoryRegion::Text => VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Execute as usize | VirtualMemoryPermission::User as usize,
            VirtualMemoryRegion::Data => VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize | VirtualMemoryPermission::User as usize,
            VirtualMemoryRegion::Bss => VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize | VirtualMemoryPermission::User as usize,
            VirtualMemoryRegion::Heap => VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize | VirtualMemoryPermission::User as usize,
            VirtualMemoryRegion::Stack => VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize | VirtualMemoryPermission::User as usize,
            VirtualMemoryRegion::Guard => 0, // Any access to the guard page should cause a page fault
            VirtualMemoryRegion::Unknown => panic!("Unknown memory segment"),
        }
    }

    /// Returns whether this memory region should be shared between tasks by default
    pub fn is_shareable(&self) -> bool {
        match self {
            VirtualMemoryRegion::Text => true,  // Text segments can be shared (read-only executable code)
            VirtualMemoryRegion::Data => false, // Data segments should not be shared (writable)
            VirtualMemoryRegion::Bss => false,  // BSS segments should not be shared (writable)
            VirtualMemoryRegion::Heap => false, // Heap should not be shared (writable)
            VirtualMemoryRegion::Stack => false, // Stack should not be shared (writable, task-specific)
            VirtualMemoryRegion::Guard => true,  // Guard pages can be shared (no physical backing)
            VirtualMemoryRegion::Unknown => false, // Unknown segments should not be shared by default
        }
    }
}