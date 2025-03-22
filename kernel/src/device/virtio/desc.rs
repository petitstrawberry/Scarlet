use alloc::vec::Vec;

pub struct DescriptorTable {
    pub descriptors: Vec<Descriptor>,
    pub free_list: Vec<u16>,
    pub used_idx: u16,
    pub next_avail_idx: u16,
}

pub struct Descriptor {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}