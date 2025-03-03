extern crate alloc;
use alloc::vec::Vec;

use crate::arch::vm::{alloc_virtual_address_space, get_page_table, mmu::PageTable};

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

#[cfg(test)]
use crate::vm::vmem::MemoryArea;

#[test_case]
fn test_new_virtual_memory_manager() {
    let vmm = VirtualMemoryManager::new();
    assert_eq!(vmm.get_asid(), 0);
}

#[test_case]
fn test_set_and_get_asid() {
    let mut vmm = VirtualMemoryManager::new();
    vmm.set_asid(42);
    assert_eq!(vmm.get_asid(), 42);
}

#[test_case]
fn test_add_and_get_memory_map() {
    let mut vmm = VirtualMemoryManager::new();
    let vma = MemoryArea { start: 0x1000, end: 0x2000 };
    let map = VirtualMemoryMap { vmarea: vma, pmarea: vma };
    vmm.add_memory_map(map);
    assert_eq!(vmm.get_memory_map(0).unwrap().vmarea.start, 0x1000);
}

#[test_case]
fn test_remove_memory_map() {
    let mut vmm = VirtualMemoryManager::new();
    let vma = MemoryArea { start: 0x1000, end: 0x2000 };
    let map = VirtualMemoryMap { vmarea: vma, pmarea: vma };
    vmm.add_memory_map(map);
    let removed_map = vmm.remove_memory_map(0).unwrap();
    assert_eq!(removed_map.vmarea.start, 0x1000);
    assert!(vmm.get_memory_map(0).is_none());
}

#[test_case]
fn test_search_memory_map() {
    let mut vmm = VirtualMemoryManager::new();
    let vma1 = MemoryArea { start: 0x1000, end: 0x2000 };
    let map1 = VirtualMemoryMap { vmarea: vma1, pmarea: vma1 };
    let vma2 = MemoryArea { start: 0x3000, end: 0x4000 };
    let map2 = VirtualMemoryMap { vmarea: vma2, pmarea: vma2 };
    vmm.add_memory_map(map1);
    vmm.add_memory_map(map2);
    let found_map = vmm.search_memory_map(0x3500).unwrap();
    assert_eq!(found_map.vmarea.start, 0x3000);
}

#[test_case]
fn test_get_root_page_table() {
    let mut vmm = VirtualMemoryManager::new();
    let asid = alloc_virtual_address_space();
    vmm.set_asid(asid);
    let page_table = vmm.get_root_page_table();
    assert!(page_table.is_some());
}
