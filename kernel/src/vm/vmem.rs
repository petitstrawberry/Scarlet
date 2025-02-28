#[derive(Debug, Clone, Copy)]
pub struct VirtualMemoryMap {
    pub pmarea: MemoryArea,
    pub vmarea: MemoryArea,
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryArea {
    pub start: usize,
    pub end: usize,
}


