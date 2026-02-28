//! x86_64 virtual memory management
//!
//! Provides page table management and MMU operations for x86_64,
//! including 4-level paging (PML4, PDP, PD, PT).

use core::arch::asm;
use core::ptr;

use crate::arch::x86_64::instruction::{read_cr3, write_cr3};

/// Page size constants
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = 12;

/// Page table entry flags
pub mod flags {
    pub const PRESENT: u64 = 1 << 0;
    pub const WRITABLE: u64 = 1 << 1;
    pub const USER: u64 = 1 << 2;
    pub const WRITETHROUGH: u64 = 1 << 3;
    pub const NO_CACHE: u64 = 1 << 4;
    pub const ACCESSED: u64 = 1 << 5;
    pub const DIRTY: u64 = 1 << 6;
    pub const GLOBAL: u64 = 1 << 8;
    pub const NO_EXECUTE: u64 = 1 << 63;
}

/// Page table entry (PTE)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    /// Create a new page table entry
    pub const fn new() -> Self {
        PageTableEntry(0)
    }

    /// Get the physical address from this entry
    pub fn addr(&self) -> u64 {
        self.0 & 0x000F_FFFFFF_FFF8
    }

    /// Set the physical address for this entry
    pub fn set_addr(&mut self, addr: u64) {
        self.0 = (self.0 & !0x000F_FFFFFF_FFF8) | (addr & 0x000F_FFFFFF_FFF8);
    }

    /// Check if the present flag is set
    pub fn is_present(&self) -> bool {
        self.0 & flags::PRESENT != 0
    }

    /// Set the present flag
    pub fn set_present(&mut self, present: bool) {
        if present {
            self.0 |= flags::PRESENT;
        } else {
            self.0 &= !flags::PRESENT;
        }
    }

    /// Check if the writable flag is set
    pub fn is_writable(&self) -> bool {
        self.0 & flags::WRITABLE != 0
    }

    /// Set the writable flag
    pub fn set_writable(&mut self, writable: bool) {
        if writable {
            self.0 |= flags::WRITABLE;
        } else {
            self.0 &= !flags::WRITABLE;
        }
    }

    /// Check if the user flag is set
    pub fn is_user(&self) -> bool {
        self.0 & flags::USER != 0
    }

    /// Set the user flag
    pub fn set_user(&mut self, user: bool) {
        if user {
            self.0 |= flags::USER;
        } else {
            self.0 &= !flags::USER;
        }
    }

    /// Check if the no-execute flag is set
    pub fn is_no_execute(&self) -> bool {
        self.0 & flags::NO_EXECUTE != 0
    }

    /// Set the no-execute flag
    pub fn set_no_execute(&mut self, no_execute: bool) {
        if no_execute {
            self.0 |= flags::NO_EXECUTE;
        } else {
            self.0 &= !flags::NO_EXECUTE;
        }
    }

    /// Get the raw value
    pub fn value(&self) -> u64 {
        self.0
    }
}

/// Page table (512 entries)
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

impl PageTable {
    /// Create a new zeroed page table
    pub const fn new() -> Self {
        PageTable {
            entries: [PageTableEntry::new(); 512],
        }
    }

    /// Get a reference to an entry
    pub fn get_entry(&self, index: usize) -> Option<&PageTableEntry> {
        self.entries.get(index)
    }

    /// Get a mutable reference to an entry
    pub fn get_entry_mut(&mut self, index: usize) -> Option<&mut PageTableEntry> {
        self.entries.get_mut(index)
    }
}

/// Root page table for x86_64
///
/// Manages the 4-level page table hierarchy.
#[derive(Debug)]
pub struct RootPageTable {
    /// Physical address of the PML4 (top-level page table)
    pml4_paddr: u64,
    /// ASID (address space ID) for this page table
    asid: u16,
}

impl RootPageTable {
    /// Create a new root page table
    ///
    /// # Arguments
    /// * `pml4_paddr` - Physical address of the PML4
    /// * `asid` - Address space ID
    pub fn new(pml4_paddr: u64, asid: u16) -> Self {
        RootPageTable { pml4_paddr, asid }
    }

    /// Get the CR3 value for this page table
    pub fn get_cr3_value(&self) -> u64 {
        self.pml4_paddr
    }

    /// Get the ASID
    pub fn get_asid(&self) -> u16 {
        self.asid
    }

    /// Get the value to store in TTBR0 (not applicable for x86_64)
    pub fn get_val_for_ttbr(&self) -> u64 {
        self.pml4_paddr
    }

    /// Activate this page table
    pub fn activate(&self) {
        unsafe {
            write_cr3(self.pml4_paddr);
        }
    }

    /// Map a virtual page to a physical page
    ///
    /// # Arguments
    /// * `vaddr` - Virtual address to map
    /// * `paddr` - Physical address to map to
    /// * `flags` - Page table entry flags
    pub fn map_page(&mut self, vaddr: u64, paddr: u64, flags: u64) {
        let pml4 = unsafe { &mut *(self.pml4_paddr as *mut PageTable) };

        let pml4_index = ((vaddr >> 39) & 0x1FF) as usize;
        let pdpt_index = ((vaddr >> 30) & 0x1FF) as usize;
        let pd_index = ((vaddr >> 21) & 0x1FF) as usize;
        let pt_index = ((vaddr >> 12) & 0x1FF) as usize;

        // Walk down the page table hierarchy, creating entries as needed
        // This is a simplified implementation

        // For now, just set the entry if the page table exists
        if let Some(pml4_entry) = pml4.get_entry_mut(pml4_index) {
            // ... would continue walking the hierarchy
            let _ = (pdpt_index, pd_index, pt_index, paddr, flags);
            let _ = pml4_entry;
        }
    }
}

/// Get the current root page table from CR3
pub fn get_current_page_table() -> u64 {
    unsafe { read_cr3() }
}

/// Invalidate a TLB entry
pub fn invalidate_tlb(vaddr: u64) {
    unsafe {
        asm!("invlpg [{}]", in(reg) vaddr as usize, options(nostack));
    }
}

/// Flush the entire TLB
pub fn flush_tlb() {
    unsafe {
        let cr3 = read_cr3();
        write_cr3(cr3);
    }
}
