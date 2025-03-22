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

    pub fn alloc(&mut self) -> u16 {
        let index = self.free_head;
        self.free_head = self.descriptors[index as usize].next;
        index
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_descriptor_table() {
        let mut table = DescriptorTable::new(10);
        table.init();
        assert_eq!(table.free_head, 0);
        assert_eq!(table.descriptors[0].next, 1);
        assert_eq!(table.descriptors[9].next, 0);
    }

    #[test_case]
    fn test_alloc() {
        let mut table = DescriptorTable::new(10);
        table.init();
        let index = table.alloc();
        assert_eq!(index, 0);
        assert_eq!(table.free_head, 1);
    }
}