/// Represents a mapping between physical and virtual memory areas.
///
/// This structure defines the relationship between a physical memory area
/// and its corresponding virtual memory area in the kernel's memory management system.
///
/// # Fields
///
/// * `pmarea` - The physical memory area that is being mapped
/// * `vmarea` - The virtual memory area where the physical memory is mapped to
#[derive(Debug, Clone, Copy)]
pub struct VirtualMemoryMap {
    pub pmarea: MemoryArea,
    pub vmarea: MemoryArea,
    pub permissions: usize,
}

impl VirtualMemoryMap {
    /// Creates a new virtual memory map with the given physical and virtual memory areas.
    /// 
    /// # Arguments
    /// * `pmarea` - The physical memory area to map
    /// * `vmarea` - The virtual memory area to map to
    /// * `permissions` - The permissions to set for the virtual memory area
    /// 
    /// # Returns
    /// A new virtual memory map with the given physical and virtual memory areas.
    pub fn new(pmarea: MemoryArea, vmarea: MemoryArea, permissions: usize) -> Self {
        VirtualMemoryMap {
            pmarea,
            vmarea,
            permissions,
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

#[derive(Debug, Clone, Copy)]
pub struct MemoryArea {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum VirtualMemoryPermission {
    Read = 0x01,
    Write = 0x02,
    Execute = 0x04,
}

impl From<usize> for VirtualMemoryPermission {
    fn from(value: usize) -> Self {
        match value {
            0x01 => VirtualMemoryPermission::Read,
            0x02 => VirtualMemoryPermission::Write,
            0x04 => VirtualMemoryPermission::Execute,
            _ => panic!("Invalid permission value: {}", value),
        }
    }
}

impl VirtualMemoryPermission {
    pub fn contained_in(&self, permissions: usize) -> bool {
        permissions & (*self as usize) != 0
    }
}