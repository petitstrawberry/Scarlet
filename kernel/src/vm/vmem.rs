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
}

impl VirtualMemoryMap {
    /// Creates a new virtual memory map with the given physical and virtual memory areas.
    /// 
    /// # Arguments
    /// * `pmarea` - The physical memory area to map
    /// * `vmarea` - The virtual memory area to map to
    /// 
    /// # Returns
    /// A new virtual memory map with the given physical and virtual memory areas.
    pub fn new(pmarea: MemoryArea, vmarea: MemoryArea) -> Self {
        VirtualMemoryMap {
            pmarea,
            vmarea,
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


