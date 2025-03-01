extern crate alloc;
use alloc::vec::Vec;

use crate::arch::vm::{get_page_table, mmu::PageTable};

use super::vmem::VirtualMemoryMap;

pub struct VirtualMemoryManager {
    memmap: Vec<VirtualMemoryMap>,
    asid: usize,
}

impl VirtualMemoryManager {
    pub fn new() -> Self {
        VirtualMemoryManager {
            memmap: Vec::new(),
            asid: 0,
        }
    }

    pub fn set_asid(&mut self, asid: usize) {
        self.asid = asid;
    }

    pub fn get_asid(&self) -> usize {
        self.asid
    }

    pub fn add_memory_map(&mut self, map: VirtualMemoryMap) {
        self.memmap.push(map);
    }

    pub fn get_memory_map(&self, idx: usize) -> Option<&VirtualMemoryMap> {
        self.memmap.get(idx)
    }

    pub fn remove_memory_map(&mut self, idx: usize) -> Option<VirtualMemoryMap> {
        if idx < self.memmap.len() {
            Some(self.memmap.remove(idx))
        } else {
            None
        }
    }

    pub fn search_memory_map(&self, vaddr: usize) -> Option<&VirtualMemoryMap> {
        let mut ret = None;
        for map in self.memmap.iter() {
            if map.vmarea.start <= vaddr && vaddr <= map.vmarea.end {
                ret = Some(map);
            }
        }
        ret
    }

    pub fn get_root_page_table(&self) -> Option<&mut PageTable> {
        get_page_table(self.asid)
    }
}