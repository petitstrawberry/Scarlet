use core::arch::asm;
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
        ((self.entry >> 10) & 0x3ffffffffff) as usize // Mask to get the PPN bits (44 bits)
        // (self.entry >> 10) as usize
    }

    pub fn get_flags(&self) -> u64 {
        self.entry & 0x3ff
    }

    pub fn is_valid(&self) -> bool {
        self.entry & 1 == 1
    }

    pub fn is_leaf(&self) -> bool {
        // An entry is a leaf if it's valid and has R=1 or X=1 (RISC-V spec step 4)
        if !self.is_valid() {
            return false;
        }
        let r_bit = (self.entry >> 1) & 1; // Read bit
        let x_bit = (self.entry >> 3) & 1; // Execute bit
        r_bit == 1 || x_bit == 1
    }

    pub fn validate(&mut self) {
        self.entry |= 1;
    }

    pub fn invalidate(&mut self) {
        self.entry &= !1;
    }

    pub fn set_ppn(&mut self, ppn: usize) -> &mut Self {
        let ppn_mask = 0x3ffffffffff; // Mask for the PPN bits
        let masked_ppn = (ppn as u64) & ppn_mask;  // Mask the PPN to fit in the entry

        self.entry &= !(ppn_mask << 10);  // Clear the PPN bits in the entry
        self.entry |= masked_ppn << 10;   // Set the new PPN bits
        self
    }

    pub fn set_flags(&mut self, flags: u64) -> &mut Self {
         let mask = 0x3ff;
        self.entry |= flags & mask;
        self
    }

    pub fn clear_flags(&mut self) -> &mut Self {
        // Only clear the permission bits (R, W, X, U, G), keep V, A, D and PPN
        self.entry &= !0x3E; // Clear bits 1-5 (R, W, X, U, G)
        self
    }

    pub fn clear_all(&mut self) -> &mut Self {
        self.entry = 0;
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
        // Check if the virtual address is properly canonicalized for Sv48
        let canonical_check = (vaddr >> 47) & 1;
        let upper_bits = (vaddr >> 48) & 0xffff;
        if canonical_check == 1 && upper_bits != 0xffff {
            panic!("Non-canonical virtual address: {:#x}", vaddr);
        } else if canonical_check == 0 && upper_bits != 0 {
            panic!("Non-canonical virtual address: {:#x}", vaddr);
        }
        
        let vaddr = vaddr & 0xffff_ffff_ffff_f000; // Page align
        let paddr = paddr & 0xffff_ffff_ffff_f000;
        
        let pte = match self.walk(vaddr, true, asid) {
            Some(pte) => pte,
            None => panic!("map: walk() couldn't allocate a needed page-table page"),
        };

        // Allow remapping - just update the existing entry
        let ppn = (paddr >> 12) & 0xfffffffffff;
        
        // Clear existing flags before setting new ones
        pte.clear_all();
        
        if VirtualMemoryPermission::Read.contained_in(permissions) {
            pte.readable();
        }
        if VirtualMemoryPermission::Write.contained_in(permissions) {
            pte.writable();
        }
        if VirtualMemoryPermission::Execute.contained_in(permissions) {
            pte.executable();
        }
        if VirtualMemoryPermission::User.contained_in(permissions) {
            pte.accesible_from_user();
        }
        pte.set_ppn(ppn);
        pte.validate();
        unsafe { asm!("sfence.vma") };
    }

    // Find the address of the PTE in page table that corresponds to virtual address vaddr.
    // If alloc == true, create any required page-table pages.
    // Returns None if walk() couldn't allocate a needed page-table page.
    // 
    // The RISC-V Sv48 scheme has four levels of page-table pages.
    // A page-table page contains 512 64-bit PTEs.
    // A 48-bit virtual address is split into five fields:
    //   47..48 -- must be zero.
    //   39..47 -- 9 bits of level-3 index.
    //   30..38 -- 9 bits of level-2 index.
    //   21..29 -- 9 bits of level-1 index.
    //   12..20 -- 9 bits of level-0 index.
    //    0..11 -- 12 bits of byte offset within the page.
    pub fn walk(&mut self, vaddr: usize, alloc: bool, asid: u16) -> Option<&mut PageTableEntry> {
        let mut pagetable = self as *mut PageTable;
        
        // Check if virtual address is within valid canonical range for Sv48
        let canonical_check = (vaddr >> 47) & 1;
        let upper_bits = (vaddr >> 48) & 0xffff;
        if canonical_check == 1 && upper_bits != 0xffff {
            return None;
        } else if canonical_check == 0 && upper_bits != 0 {
            return None;
        }

        unsafe {
            // Walk through levels 3, 2, 1
            for level in (1..=MAX_PAGING_LEVEL).rev() {
                let vpn = (vaddr >> (12 + 9 * level)) & 0x1ff;
                let pte = &mut (*pagetable).entries[vpn];
                
                if pte.is_valid() {
                    // At an intermediate level, a PTE must not be a leaf (no huge page support).
                    if pte.is_leaf() {
                        return None; // Fail because it's an invalid state.
                    }
                    // If not a leaf, it's a pointer to the next level table.
                    pagetable = (pte.get_ppn() << 12) as *mut PageTable;
                } else {
                    if !alloc {
                        return None;
                    }
                    // Allocate a new page table
                    let new_table = new_raw_pagetable(asid);
                    if new_table.is_null() {
                        return None;
                    }
                    pte.clear_all(); // Clear the entry
                    pte.set_ppn(new_table as usize >> 12);
                    pte.validate();
                    pagetable = new_table;
                }
            }
            
            // Return the PTE at level 0
            let vpn = (vaddr >> 12) & 0x1ff;
            Some(&mut (*pagetable).entries[vpn])
        }
    }

    pub fn unmap(&mut self, _asid: u16, vaddr: usize) {
        // Check if the virtual address is properly canonicalized for Sv48
        let canonical_check = (vaddr >> 47) & 1;
        let upper_bits = (vaddr >> 48) & 0xffff;
        if canonical_check == 1 && upper_bits != 0xffff {
            panic!("Non-canonical virtual address: {:#x}", vaddr);
        } else if canonical_check == 0 && upper_bits != 0 {
            panic!("Non-canonical virtual address: {:#x}", vaddr);
        }
        
        let vaddr = vaddr & 0xffff_ffff_ffff_f000; // Page align
        
        match self.walk(vaddr, false, 0) {
            Some(pte) => {
                if pte.is_valid() {
                    pte.clear_all();
                    unsafe { asm!("sfence.vma") };
                }
            }
            None => {
                // Mapping doesn't exist, nothing to unmap
            }
        }
    }

    pub fn unmap_all(&mut self) {
        for i in 0..512 {
            let entry = &mut self.entries[i];
            entry.clear_all();
        }
        // Ensure the TLB flush instruction is not optimized away.
        unsafe { asm!("sfence.vma") };
    }
}
