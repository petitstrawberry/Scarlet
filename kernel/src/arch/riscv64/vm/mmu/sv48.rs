use core::{arch::asm, mem::transmute};
use core::result::Result;


use crate::arch::vm::new_raw_pagetable;
use crate::environment::PAGE_SIZE;
use crate::vm::vmem::VirtualMemoryPermission;
use crate::{vm::vmem::VirtualMemoryMap};

const MAX_PAGING_LEVEL: usize = 3;

#[repr(align(8))]
#[derive(Clone, Copy, Debug)]
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

    pub fn accesible_from_user(&mut self) -> &mut Self {
        self.entry |= 0x10;
        self
    }
}

#[repr(align(4096))]
#[derive(Debug)]
pub struct PageTable {
    pub entries: [PageTableEntry; 512],
}

impl PageTable {
    pub const fn new() -> Self {
        PageTable {
            entries: [PageTableEntry::new(); 512],
        }
    }

    pub fn switch(&self, asid: u16) {
        let satp = self.get_val_for_satp(asid);
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

    /// Get the value for the satp register.
    /// 
    /// # Note
    /// 
    /// Only for RISC-V (Sv48).
    pub fn get_val_for_satp(&self, asid: u16) -> u64 {
        let asid = asid as usize;
        let mode = 9;
        let ppn = self as *const _ as usize >> 12;
        (mode << 60 | asid << 44 | ppn) as u64
    }

    fn get_next_level_table(&self, index: usize) -> &mut PageTable {
        let addr = self.entries[index].get_ppn() << 12;
        unsafe { transmute(addr) }
    }

    pub fn map_memory_area(&mut self, asid: u16, mmap: VirtualMemoryMap) -> Result<(), &'static str> {
        // Check if the address and size is aligned to PAGE_SIZE
        if mmap.vmarea.start % PAGE_SIZE != 0 || mmap.pmarea.start % PAGE_SIZE != 0 ||
            mmap.vmarea.size() % PAGE_SIZE != 0 || mmap.pmarea.size() % PAGE_SIZE != 0 {
            return Err("Address is not aligned to PAGE_SIZE");
        }

        let mut vaddr = mmap.vmarea.start;
        let mut paddr = mmap.pmarea.start;
        while vaddr + (PAGE_SIZE - 1) <= mmap.vmarea.end {
            self.map(asid, vaddr, paddr, mmap.permissions);
            match vaddr.checked_add(PAGE_SIZE) {
                Some(addr) => vaddr = addr,
                None => break,
            }
            match paddr.checked_add(PAGE_SIZE) {
                Some(addr) => paddr = addr,
                None => break,
            }
        }

        Ok(())
    }

    /* Only for root page table */
    pub fn map(&mut self, asid: u16, vaddr: usize, paddr: usize, permissions: usize) {
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
                    if VirtualMemoryPermission::User.contained_in(permissions) {
                        entry.accesible_from_user();
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
                    let next_table_ptr = unsafe { new_raw_pagetable(asid as u16) };
                    entry
                        .set_ppn(next_table_ptr as usize >> 12)
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

    pub fn unmap_all(&mut self) {
        for i in 0..512 {
            let entry = &mut self.entries[i];
            entry.invalidate();
        }
        // Ensure the TLB flush instruction is not optimized away.
        unsafe { asm!("sfence.vma") };
    }
}
