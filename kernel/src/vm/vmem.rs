#[derive(Debug, Clone, Copy)]
pub struct VirtualMemoryMap {
    pub pmarea: MemoryArea,
    pub vmarea: MemoryArea,
}

impl VirtualMemoryMap {
    pub fn new(pmarea: MemoryArea, vmarea: MemoryArea) -> Self {
        VirtualMemoryMap {
            pmarea,
            vmarea,
        }
    }

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


