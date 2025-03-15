use core::{arch::asm, mem::transmute};
use core::result::Result;

use crate::vm::vmem::VirtualMemoryPermission;
use crate::{arch::vm::{get_page_table, new_page_table_idx}, vm::vmem::VirtualMemoryMap};

const MAX_PAGING_LEVEL: usize = 3;

#[repr(align(8))]
#[derive(Clone, Copy)]
pub struct PageTableEntry {
    pub entry: u64,
}

impl PageTableEntry {
    pub const fn new() -> Self {
        PageTableEntry { entry: 0 }
    }

    pub fn get_ppn(&self) -> usize {
        (self.entry >> 10) as usize
    }

    pub fn get_flags(&self) -> u64 {
        self.entry & 0xfff
    }

    pub fn is_valid(&self) -> bool {
        self.entry & 1 == 1
    }

    pub fn is_leaf(&self) -> bool {
        let flags = self.entry & 0b1110;
        !(flags == 0)
    }

    pub fn validate(&mut self) {
        self.entry |= 1;
    }

    pub fn invalidate(&mut self) {
        self.entry &= !1;
    }

    pub fn set_ppn(&mut self, ppn: usize) -> &mut Self {
        let mask = 0xFFFFFFFFFFF;
        self.entry &= !(mask << 10);
        self.entry |= (ppn as u64) << 10;
        self
    }

    pub fn set_flags(&mut self, flags: u64) -> &mut Self {
        let mask = 0xff;
        self.entry |= flags & mask;
        self
    }

    pub fn clear_flags(&mut self) -> &mut Self {
        self.entry &= !0xff;
        self
    }

    pub fn writable(&mut self) -> &mut Self {
        self.entry |= 0x4;
        self
    }

    pub fn readable(&mut self) -> &mut Self {
        self.entry |= 0x2;
        self
    }

    pub fn executable(&mut self) -> &mut Self {
        self.entry |= 0x8;
        self
    }
}

#[repr(align(4096))]
pub struct PageTable {
    pub entries: [PageTableEntry; 512],
}

impl PageTable {
    pub const fn new() -> Self {
        PageTable {
            entries: [PageTableEntry::new(); 512],
        }
    }

    pub fn switch(&self, asid: usize) {
        let mode = 9;
        let ppn = self as *const _ as usize >> 12;
        let satp = mode << 60 | asid << 44 | ppn;
        unsafe {
            asm!(
                "
                csrw satp, {0}
                sfence.vma
                ",

                in(reg) satp,
            );
        }
    }

    fn get_next_level_table(&self, index: usize) -> &mut PageTable {
        let addr = self.entries[index].get_ppn() << 12;
        unsafe { transmute(addr) }
    }

    pub fn map_memory_area(&mut self, mmap: VirtualMemoryMap) {
        let mut vaddr = mmap.vmarea.start;
        let mut paddr = mmap.pmarea.start;
        while vaddr + 0xfff <= mmap.vmarea.end {
            self.map(vaddr, paddr, mmap.permissions);
            match vaddr.checked_add(0x1000) {
                Some(addr) => vaddr = addr,
                None => break,
            }
            match paddr.checked_add(0x1000) {
                Some(addr) => paddr = addr,
                None => break,
            }
        }
    }

    /* Only for root page table */
    pub fn map(&mut self, vaddr: usize, paddr: usize, permissions: usize) {
        let vaddr = vaddr & 0xffff_ffff_ffff_f000;
        let paddr = paddr & 0xffff_ffff_ffff_f000;
        for i in (0..=MAX_PAGING_LEVEL).rev() {
            let pagetable = self.walk(vaddr, MAX_PAGING_LEVEL);
            match pagetable {
                Ok((pagetable, level)) => {
                    let vpn = (vaddr >> (12 + 9 * level)) & 0x1ff;
                    let ppn = (paddr >> 12) & 0xfffffffffff;
                    let entry = &mut pagetable.entries[vpn];
                    if VirtualMemoryPermission::Read.contained_in(permissions) {
                        entry.readable();
                    }
                    if VirtualMemoryPermission::Write.contained_in(permissions) {
                        entry.writable();
                    }
                    if VirtualMemoryPermission::Execute.contained_in(permissions) {
                        entry.executable();
                    }
                    entry
                        .set_ppn(ppn)
                        .validate();
                    unsafe { asm!("sfence.vma") };
                    break;
                }
                Err(t) => {
                    let vpn = vaddr >> (12 + 9 * i) & 0x1ff;
                    let entry = &mut t.entries[vpn];
                    let next_table_idx = new_page_table_idx();
                    let next_table = get_page_table(next_table_idx).unwrap();
                    entry
                        .set_ppn(next_table as *const _ as usize >> 12)
                        .validate();
                }
            }
        }
    }

    fn walk(&mut self, vaddr: usize, level: usize) -> Result<(&mut PageTable, usize), &mut PageTable> {
        let vpn = (vaddr >> (12 + 9 * level)) & 0x1ff;
        let entry = &self.entries[vpn];

        if entry.is_leaf() || level == 0 {
            return Ok((self, level));
        }
        
        if !entry.is_valid() {
            return Err(self);
        }

        let next_level_table = self.get_next_level_table(vpn);
        next_level_table.walk(vaddr, level - 1)
    }

    pub fn unmap(&mut self, vaddr: usize) {
        let vaddr = vaddr & 0xffff_ffff_ffff_f000;
        let pagetable = self.walk(vaddr, MAX_PAGING_LEVEL);
        match pagetable {
            Ok((pagetable, level)) => {
                let vpn = (vaddr >> (12 + 9 * level)) & 0x1ff;
                let entry = &mut pagetable.entries[vpn];
                entry.invalidate();
                unsafe { asm!("sfence.vma") };
            }
            Err(_) => {}
        }
    }
}
