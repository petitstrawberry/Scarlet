use alloc::vec;
use alloc::vec::Vec;

pub struct DescriptorTable {
    pub descriptors: Vec<Descriptor>,
    pub size: usize,
    pub free_list: Vec<u16>,
    pub used_list: Vec<u16>,
    pub free_head: u16,
}

impl DescriptorTable {
    pub fn new(size: usize) -> Self {
        Self {
            descriptors: vec![Descriptor::default(); size],
            size,
            free_list: (0..size as u16).collect(),
            used_list: Vec::new(),
            free_head: 0,
        }
    }

    pub fn init(&mut self) {
        for i in 0..self.size {
            self.descriptors[i].next = (i + 1) as u16;
        }
        self.descriptors[self.size - 1].next = 0;
        self.free_head = 0;
    }

    pub fn alloc(&mut self) -> Option<u16> {
        if self.free_head == 0 {
            return None;
        }
        let index = self.free_head;
        self.free_head = self.descriptors[index as usize].next;
        Some(index)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Descriptor {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

impl Default for Descriptor {
    fn default() -> Self {
        Self {
            addr: 0,
            len: 0,
            flags: 0x1,
            next: 0,
        }
    }
}