use alloc::vec::Vec;

#[repr(C)]
pub struct DescriptorTable {
    pub descriptors: Vec<Descriptor>,
}

#[repr(C)]
pub struct Descriptor {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}