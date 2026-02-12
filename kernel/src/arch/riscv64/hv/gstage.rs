//! Sv48x4 G-stage page table for RISC-V H-extension
//!
//! The G-stage page table translates Guest Physical Addresses (GPA) to
//! Host Physical Addresses (HPA). Sv48x4 uses a 16KiB-aligned root
//! (4 consecutive 4KiB pages) with 2048 entries at the top level,
//! giving a 50-bit guest physical address space.

use crate::environment::PAGE_SIZE;
use crate::mem::page::{allocate_raw_pages, free_raw_pages, Page};

use super::csr;

const ENTRIES_PER_PAGE: usize = 512;
const ROOT_PAGES: usize = 4;
const ROOT_ENTRIES: usize = ENTRIES_PER_PAGE * ROOT_PAGES;
const MAX_PAGING_LEVEL: usize = 3;

// PTE flag bits (same as Sv48)
const PTE_V: u64 = 1 << 0;
const PTE_R: u64 = 1 << 1;
const PTE_W: u64 = 1 << 2;
const PTE_X: u64 = 1 << 3;
const PTE_U: u64 = 1 << 4;
const PTE_A: u64 = 1 << 6;
const PTE_D: u64 = 1 << 7;

#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct GStagePageTableEntry {
    entry: u64,
}

impl GStagePageTableEntry {
    const fn zero() -> Self {
        Self { entry: 0 }
    }

    fn is_valid(&self) -> bool {
        self.entry & PTE_V != 0
    }

    fn is_leaf(&self) -> bool {
        if !self.is_valid() {
            return false;
        }
        let r = (self.entry >> 1) & 1;
        let x = (self.entry >> 3) & 1;
        r == 1 || x == 1
    }

    fn get_ppn(&self) -> usize {
        ((self.entry >> 10) & 0x3ffffffffff) as usize
    }

    fn set(&mut self, ppn: usize, flags: u64) {
        let ppn_bits = ((ppn as u64) & 0x3ffffffffff) << 10;
        self.entry = ppn_bits | (flags & 0x3ff);
    }

    fn clear(&mut self) {
        self.entry = 0;
    }
}

pub struct GStagePageTable {
    root: *mut GStagePageTableEntry,
    root_pages_ptr: *mut Page,
    inner_pages: spin::Mutex<alloc::vec::Vec<*mut Page>>,
}

// SAFETY: The page table is accessed with proper synchronization
// at the VmObject level (Mutex<VmState>).
unsafe impl Send for GStagePageTable {}
unsafe impl Sync for GStagePageTable {}

impl GStagePageTable {
    pub fn new() -> Result<Self, &'static str> {
        let root_pages_ptr = allocate_raw_pages(ROOT_PAGES);
        if root_pages_ptr.is_null() {
            return Err("Failed to allocate G-stage root page table");
        }

        let root_addr = root_pages_ptr as usize;
        if root_addr % (PAGE_SIZE * ROOT_PAGES) != 0 {
            // The allocator returns page-aligned memory; with 4 consecutive pages
            // from Box<[Page]>, alignment is at least PAGE_SIZE. For Sv48x4 the root
            // needs 16KiB alignment. allocate_raw_pages returns zeroed pages that are
            // contiguous, so we just need to verify alignment.
            free_raw_pages(root_pages_ptr, ROOT_PAGES);
            return Err("G-stage root page table not 16KiB aligned");
        }

        Ok(Self {
            root: root_addr as *mut GStagePageTableEntry,
            root_pages_ptr,
            inner_pages: spin::Mutex::new(alloc::vec::Vec::new()),
        })
    }

    /// Compute hgatp value for this page table.
    ///
    /// Mode 9 = Sv48x4, vmid in bits 58:44, root PPN in bits 43:0.
    pub fn hgatp_value(&self, vmid: u16) -> u64 {
        let mode: u64 = 9;
        let ppn = (self.root as u64) >> 12;
        (mode << 60) | ((vmid as u64) << 44) | ppn
    }

    pub fn map_page(&mut self, gpa: u64, hpa: u64, readonly: bool) -> Result<(), &'static str> {
        let gpa_aligned = gpa & !((PAGE_SIZE as u64) - 1);
        let hpa_aligned = hpa & !((PAGE_SIZE as u64) - 1);

        let pte = self.walk(gpa_aligned, true)?;

        let mut flags = PTE_V | PTE_R | PTE_A | PTE_D | PTE_U;
        if !readonly {
            flags |= PTE_W;
        }
        flags |= PTE_X;

        let ppn = (hpa_aligned >> 12) as usize;
        pte.set(ppn, flags);

        Ok(())
    }

    pub fn unmap_page(&mut self, gpa: u64) -> Result<(), &'static str> {
        let gpa_aligned = gpa & !((PAGE_SIZE as u64) - 1);

        match self.walk(gpa_aligned, false) {
            Ok(pte) => {
                if pte.is_valid() {
                    pte.clear();
                }
                Ok(())
            }
            Err(_) => Ok(()),
        }
    }

    /// Walk the 4-level page table for Sv48x4.
    ///
    /// Level 3 uses 11 bits (VPN\[3\] = gpa\[49:39\]) indexing into 2048 root entries.
    /// Levels 2..0 use 9 bits each, same as standard Sv48.
    fn walk(&mut self, gpa: u64, alloc: bool) -> Result<&mut GStagePageTableEntry, &'static str> {
        let mut table = self.root;
        let mut num_entries = ROOT_ENTRIES;

        for level in (1..=MAX_PAGING_LEVEL).rev() {
            let shift = 12 + 9 * level;
            let mask = if level == MAX_PAGING_LEVEL {
                // Level 3: 11 bits for Sv48x4
                (ROOT_ENTRIES - 1) as u64
            } else {
                0x1ff
            };
            let idx = ((gpa >> shift) & mask) as usize;
            if idx >= num_entries {
                return Err("GPA out of range for Sv48x4");
            }

            let pte = unsafe { &mut *table.add(idx) };

            if pte.is_valid() {
                if pte.is_leaf() {
                    return Err("Unexpected huge page in G-stage walk");
                }
                table = (pte.get_ppn() << 12) as *mut GStagePageTableEntry;
            } else {
                if !alloc {
                    return Err("Unmapped GPA");
                }
                let new_page = allocate_raw_pages(1);
                if new_page.is_null() {
                    return Err("Failed to allocate G-stage page table page");
                }
                self.inner_pages.lock().push(new_page);
                let new_addr = new_page as usize;
                pte.set(new_addr >> 12, PTE_V);
                table = new_addr as *mut GStagePageTableEntry;
            }

            num_entries = ENTRIES_PER_PAGE;
        }

        // Level 0
        let idx = ((gpa >> 12) & 0x1ff) as usize;
        unsafe { Ok(&mut *table.add(idx)) }
    }

    pub fn flush_tlb(&self) {
        csr::hfence_gvma_all();
    }
}

impl Drop for GStagePageTable {
    fn drop(&mut self) {
        let inner = self.inner_pages.lock();
        for &page_ptr in inner.iter() {
            free_raw_pages(page_ptr, 1);
        }
        free_raw_pages(self.root_pages_ptr, ROOT_PAGES);
    }
}
