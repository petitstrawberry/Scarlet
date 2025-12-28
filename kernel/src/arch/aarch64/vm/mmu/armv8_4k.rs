//! ARMv8.0-A MMU implementation with 4KB granule size
//!
//! This module implements the ARMv8.0-A Memory Management Unit (MMU) with:
//! - 4KB page granule
//! - 4-level page table translation
//! - 48-bit virtual address space
//! - Support for EL0 and EL1 page tables

use core::arch::asm;
use core::result::Result;

use crate::arch::vm::new_raw_pagetable;
use crate::environment::PAGE_SIZE;
use crate::vm::vmem::VirtualMemoryMap;
use crate::vm::vmem::VirtualMemoryPermission;

/// Maximum paging levels for AArch64 4KB granule (4 levels: 0-3)
const MAX_PAGING_LEVEL: usize = 3;

/// Page table entry for ARMv8.0-A architecture
#[repr(align(8))]
#[derive(Clone, Copy, Debug)]
pub struct PageTableEntry {
    pub entry: u64,
}

impl PageTableEntry {
    /// Create a new empty page table entry
    pub const fn new() -> Self {
        PageTableEntry { entry: 0 }
    }

    /// Get the physical page number (PPN) from the entry
    /// For AArch64, this extracts bits 47:12 of the output address
    pub fn get_ppn(&self) -> usize {
        ((self.entry >> 12) & 0xfffffffff) as usize // 36 bits for physical address
    }

    /// Get the attribute flags from the entry
    pub fn get_flags(&self) -> u64 {
        self.entry & 0xfff // Lower 12 bits contain attributes
    }

    /// Check if the entry is valid (present)
    /// In AArch64, bit 0 indicates validity
    pub fn is_valid(&self) -> bool {
        self.entry & 1 == 1
    }

    /// Check if this entry is a leaf (block/page entry)
    /// In AArch64:
    /// - For levels 0-2: bit 1 = 0 means block (leaf), bit 1 = 1 means table (non-leaf)
    /// - For level 3: always a page entry (leaf)
    pub fn is_leaf(&self) -> bool {
        if !self.is_valid() {
            return false;
        }
        // For levels 0-2: bit 1 = 0 means block (leaf), bit 1 = 1 means table (non-leaf)
        // We return true if it's a block (bit 1 = 0)
        (self.entry >> 1) & 1 == 0
    }

    /// Mark the entry as valid
    pub fn validate(&mut self) {
        self.entry |= 1;
    }

    /// Mark the entry as invalid
    pub fn invalidate(&mut self) {
        self.entry &= !1;
    }

    /// Set the physical page number
    pub fn set_ppn(&mut self, ppn: usize) -> &mut Self {
        let ppn_mask = 0xfffffffff; // 36 bits for physical address
        let masked_ppn = (ppn as u64) & ppn_mask;

        self.entry &= !(ppn_mask << 12); // Clear existing PPN
        self.entry |= masked_ppn << 12; // Set new PPN
        self
    }

    /// Set attribute flags
    pub fn set_flags(&mut self, flags: u64) -> &mut Self {
        let mask = 0xfff; // Lower 12 bits
        self.entry |= flags & mask;
        self
    }

    /// Clear all flags except PPN
    pub fn clear_flags(&mut self) -> &mut Self {
        // Keep PPN and clear lower 12 bits
        self.entry &= !0xfff;
        self
    }

    /// Clear the entire entry
    pub fn clear_all(&mut self) -> &mut Self {
        self.entry = 0;
        self
    }

    /// Set as a table descriptor (for levels 0-2)
    /// In AArch64: bits [1:0] = 0b11 means table descriptor
    pub fn set_table(&mut self) -> &mut Self {
        self.entry |= 0x3; // Valid (bit 0) + Table (bit 1)
        self
    }

    /// Set as a block descriptor (for levels 1-2) or page descriptor (for level 3)
    /// In AArch64:
    /// - For levels 1-2: bits [1:0] = 0b01 means block descriptor
    /// - For level 3: bits [1:0] = 0b11 means page descriptor
    pub fn set_block_page(&mut self) -> &mut Self {
        // For level 3, we need 0b11 (page descriptor)
        // This function should only be called for level 3 pages
        self.entry |= 0x3; // Valid + Page descriptor
        self
    }

    /// Set readable permission (bit 6 controls read access for EL0)
    pub fn readable(&mut self) -> &mut Self {
        // In AArch64, access permissions are controlled by AP bits [7:6]
        // AP[1] = 0 for read/write, AP[0] controls EL0 access
        self.entry &= !(1 << 7); // Clear AP[1] for read/write access
        self
    }

    /// Set writable permission
    pub fn writable(&mut self) -> &mut Self {
        // AP[1] = 0 allows write access (already set by readable())
        self.readable()
    }

    /// Set executable permission
    pub fn executable(&mut self) -> &mut Self {
        // Clear XN (eXecute Never) bit for EL1 (bit 54) and EL0 (bit 53)
        self.entry &= !(1 << 54); // Clear UXN (EL0 execute)
        self.entry &= !(1 << 53); // Clear PXN (EL1 execute)
        self
    }

    /// Allow access from EL0 (user space)
    pub fn accessible_from_user(&mut self) -> &mut Self {
        self.entry |= 1 << 6; // Set AP[0] for EL0 access
        self
    }

    /// Set memory attributes (using MAIR index)
    pub fn set_memory_attr(&mut self, attr_index: u8) -> &mut Self {
        let attr_bits = (attr_index as u64 & 0x7) << 2; // AttrIndx[2:0] in bits 4:2
        self.entry &= !(0x7 << 2); // Clear existing attributes
        self.entry |= attr_bits;
        self
    }

    /// Set shareability attributes
    pub fn set_shareability(&mut self, sh: Shareability) -> &mut Self {
        let sh_bits = (sh as u64) << 8; // SH[1:0] in bits 9:8
        self.entry &= !(0x3 << 8); // Clear existing shareability
        self.entry |= sh_bits;
        self
    }

    // Test helper methods
    /// Set valid flag
    pub fn set_valid(&mut self, valid: bool) {
        if valid {
            self.entry |= 1;
        } else {
            self.entry &= !1;
        }
    }

    /// Set readable flag (for testing)
    pub fn set_readable(&mut self, readable: bool) {
        if readable {
            self.readable();
        } else {
            self.entry |= 1 << 7; // Set AP[1] to deny read
        }
    }

    /// Check if readable (for testing)
    pub fn is_readable(&self) -> bool {
        (self.entry >> 7) & 1 == 0 // AP[1] = 0 allows read
    }

    /// Set writable flag (for testing)
    pub fn set_writable(&mut self, writable: bool) {
        if writable {
            self.writable();
        } else {
            self.entry |= 1 << 7; // Set AP[1] to make read-only
        }
    }

    /// Check if writable (for testing)
    pub fn is_writable(&self) -> bool {
        (self.entry >> 7) & 1 == 0 // AP[1] = 0 allows write
    }

    /// Set executable flag (for testing)
    pub fn set_executable(&mut self, executable: bool) {
        if executable {
            self.executable();
        } else {
            self.entry |= 1 << 54; // Set UXN (EL0 execute never)
            self.entry |= 1 << 53; // Set PXN (EL1 execute never)
        }
    }

    /// Check if executable (for testing)
    pub fn is_executable(&self) -> bool {
        (self.entry >> 54) & 1 == 0 && (self.entry >> 53) & 1 == 0
    }

    /// Set user accessible flag (for testing)
    pub fn set_user_accessible(&mut self, accessible: bool) {
        if accessible {
            self.accessible_from_user();
        } else {
            self.entry &= !(1 << 6); // Clear AP[0] to deny EL0 access
        }
    }

    /// Check if user accessible (for testing)
    pub fn is_user_accessible(&self) -> bool {
        (self.entry >> 6) & 1 == 1
    }

    /// Set memory type to device (for testing)
    pub fn set_memory_type_device(&mut self) {
        self.set_memory_attr(MemoryAttribute::Device as u8);
    }

    /// Check if device memory (for testing)
    pub fn is_device_memory(&self) -> bool {
        let attr_index = (self.entry >> 2) & 0x7;
        attr_index == MemoryAttribute::Device as u64
    }

    /// Set memory type to normal cacheable (for testing)
    pub fn set_memory_type_normal_cacheable(&mut self) {
        self.set_memory_attr(MemoryAttribute::Normal as u8);
    }

    /// Check if normal cacheable memory (for testing)
    pub fn is_normal_cacheable_memory(&self) -> bool {
        let attr_index = (self.entry >> 2) & 0x7;
        attr_index == MemoryAttribute::Normal as u64
    }

    /// Set outer shareable (for testing)
    pub fn set_outer_shareable(&mut self) {
        self.set_shareability(Shareability::OuterShareable);
    }

    /// Check if outer shareable (for testing)
    pub fn is_outer_shareable(&self) -> bool {
        let sh = (self.entry >> 8) & 0x3;
        sh == Shareability::OuterShareable as u64
    }

    /// Set inner shareable (for testing)  
    pub fn set_inner_shareable(&mut self) {
        self.set_shareability(Shareability::InnerShareable);
    }

    /// Check if inner shareable (for testing)
    pub fn is_inner_shareable(&self) -> bool {
        let sh = (self.entry >> 8) & 0x3;
        sh == Shareability::InnerShareable as u64
    }
}

/// Shareability attributes for AArch64 page table entries
#[repr(u8)]
pub enum Shareability {
    NonShareable = 0b00,
    OuterShareable = 0b10,
    InnerShareable = 0b11,
}

/// Memory attribute indices for MAIR_EL1 register
#[repr(u8)]
pub enum MemoryAttribute {
    Device = 0,       // Device memory
    Normal = 1,       // Normal memory, cacheable
    NonCacheable = 2, // Normal memory, non-cacheable
}

/// Page table structure aligned to 4KB boundary
#[repr(align(4096))]
#[derive(Debug)]
pub struct PageTable {
    pub entries: [PageTableEntry; 512],
}

impl PageTable {
    /// Create a new page table with all entries initialized to zero
    pub fn new() -> Self {
        PageTable {
            entries: [PageTableEntry::new(); 512],
        }
    }

    /// Switch to this page table by updating TTBR0_EL1 and TTBR1_EL1
    ///
    /// On AArch64, TTBR0 is for lower VA (user) and TTBR1 is for upper VA (kernel/trampoline).
    /// We set both to the same page table so that identity-mapped kernel addresses work
    /// regardless of whether the VA is in the lower or upper range.
    pub fn switch(&self, asid: u16) {
        let ttbr_val = self.get_val_for_ttbr(asid);
        unsafe {
            asm!(
                "msr ttbr0_el1, {ttbr}",
                "msr ttbr1_el1, {ttbr}",
                "dsb sy",
                "isb",
                "tlbi vmalle1",
                "dsb sy",
                "isb",
                ttbr = in(reg) ttbr_val,
            );
        }
    }

    /// Switch TTBR1_EL1 to this page table (for kernel high-VA mapping)
    pub fn switch_ttbr1(&self, asid: u16) {
        let ttbr_val = self.get_val_for_ttbr(asid);
        unsafe {
            asm!(
                "msr ttbr1_el1, {ttbr}",
                "dsb sy",
                "isb",
                "tlbi vmalle1",
                "dsb sy",
                "isb",
                ttbr = in(reg) ttbr_val,
            );
        }
    }

    /// Get the value for TTBR0_EL1 register
    pub fn get_val_for_ttbr(&self, asid: u16) -> u64 {
        let baddr = (self as *const _ as u64) & 0xffffffffffff; // 48-bit address
        let asid_val = (asid as u64) << 48; // ASID in bits 63:48
        baddr | asid_val
    }

    /// Map a memory area using this page table
    pub fn map_memory_area(
        &mut self,
        asid: u16,
        mmap: VirtualMemoryMap,
        accessed: bool,
        dirty: bool,
    ) -> Result<(), &'static str> {
        // Check alignment
        if mmap.vmarea.start % PAGE_SIZE != 0
            || mmap.pmarea.start % PAGE_SIZE != 0
            || mmap.vmarea.size() % PAGE_SIZE != 0
            || mmap.pmarea.size() % PAGE_SIZE != 0
        {
            return Err("Address is not aligned to PAGE_SIZE");
        }

        let mut vaddr = mmap.vmarea.start;
        let mut paddr = mmap.pmarea.start;

        while vaddr + (PAGE_SIZE - 1) <= mmap.vmarea.end {
            self.map(asid, vaddr, paddr, mmap.permissions, accessed, dirty);

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

    /// Map a single page
    pub fn map(
        &mut self,
        asid: u16,
        vaddr: usize,
        paddr: usize,
        permissions: usize,
        _accessed: bool, // TODO: Implement accessed flag support for AArch64
        _dirty: bool,    // TODO: Implement dirty flag support for AArch64
    ) {
        // Validate 48-bit virtual address
        if vaddr >= (1 << 48) {
            panic!("Virtual address {:#x} exceeds 48-bit limit", vaddr);
        }

        let vaddr = vaddr & 0xfffffffffffff000; // Page align
        let paddr = paddr & 0xfffffffffffff000;

        let pte = match self.walk(vaddr, true, asid) {
            Some(pte) => pte,
            None => panic!("map: walk() couldn't allocate a needed page-table page"),
        };

        let ppn = (paddr >> 12) & 0xfffffffff; // 36-bit physical address

        // Clear existing entry
        pte.clear_all();

        // Set up page descriptor
        pte.set_block_page();
        pte.set_ppn(ppn);

        // Set memory attributes (normal memory, cacheable)
        pte.set_memory_attr(MemoryAttribute::Normal as u8);
        pte.set_shareability(Shareability::InnerShareable);

        // Set permissions
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
            pte.accessible_from_user();
        }

        // Ensure memory operations complete before TLB invalidation
        unsafe {
            asm!("dsb sy", "tlbi vmalle1", "dsb sy", "isb");
        }
    }

    /// Walk the page table hierarchy to find or create page table entries
    ///
    /// AArch64 4-level page table structure:
    /// - Level 0: bits 47:39 (9 bits) - PGD
    /// - Level 1: bits 38:30 (9 bits) - PUD  
    /// - Level 2: bits 29:21 (9 bits) - PMD
    /// - Level 3: bits 20:12 (9 bits) - PTE
    pub fn walk(&mut self, vaddr: usize, alloc: bool, asid: u16) -> Option<&mut PageTableEntry> {
        // Validate 48-bit address
        if vaddr >= (1 << 48) {
            crate::early_println!("[walk] vaddr {:#x} exceeds 48-bit limit", vaddr);
            return None;
        }

        let mut pagetable = self as *mut PageTable;

        unsafe {
            // Walk through levels 0, 1, 2 (intermediate levels)
            for level in 0..MAX_PAGING_LEVEL {
                let index = (vaddr >> (12 + 9 * (3 - level))) & 0x1ff;
                let pte = &mut (*pagetable).entries[index];

                if pte.is_valid() {
                    if level < 3 && pte.is_leaf() {
                        // Block entry at intermediate level (not supported for now)
                        crate::early_println!(
                            "[walk] Block entry at level {} for vaddr {:#x}, pte={:#x}, index={}",
                            level,
                            vaddr,
                            pte.entry,
                            index
                        );
                        return None;
                    }
                    // Follow the pointer to the next level
                    pagetable = (pte.get_ppn() << 12) as *mut PageTable;
                } else {
                    if !alloc {
                        return None;
                    }
                    // Allocate new page table
                    let new_table = new_raw_pagetable(asid);
                    if new_table.is_null() {
                        crate::early_println!(
                            "[walk] new_raw_pagetable returned null for asid {}",
                            asid
                        );
                        return None;
                    }
                    // Set up table descriptor
                    pte.clear_all();
                    pte.set_ppn(new_table as usize >> 12);
                    pte.set_table();
                    pagetable = new_table;
                }
            }

            // Return the PTE at level 3 (final level)
            let index = (vaddr >> 12) & 0x1ff;
            Some(&mut (*pagetable).entries[index])
        }
    }

    /// Unmap a single page
    pub fn unmap(&mut self, _asid: u16, vaddr: usize) {
        if vaddr >= (1 << 48) {
            panic!("Virtual address {:#x} exceeds 48-bit limit", vaddr);
        }

        let vaddr = vaddr & 0xfffffffffffff000; // Page align

        match self.walk(vaddr, false, 0) {
            Some(pte) => {
                if pte.is_valid() {
                    pte.clear_all();
                    unsafe {
                        asm!("dsb sy", "tlbi vmalle1", "dsb sy", "isb");
                    }
                }
            }
            None => {
                // Mapping doesn't exist, nothing to unmap
            }
        }
    }

    /// Unmap all entries in this page table
    pub fn unmap_all(&mut self) {
        for entry in &mut self.entries {
            entry.clear_all();
        }
        // Flush TLB
        unsafe {
            asm!("dsb sy", "tlbi vmalle1", "dsb sy", "isb");
        }
    }
}

/// Initialize AArch64 MMU registers
pub fn init_mmu_registers() {
    unsafe {
        // Set up MAIR_EL1 (Memory Attribute Indirection Register)
        // Index 0: Device memory (0x00)
        // Index 1: Normal memory, cacheable (0xff)
        // Index 2: Normal memory, non-cacheable (0x44)
        let mair_val: u64 = 0x44ff00;
        asm!("msr mair_el1, {}", in(reg) mair_val);

        // Set up TCR_EL1 (Translation Control Register)
        // We need BOTH TTBR0 (lower VA) and TTBR1 (upper VA) configured.
        //
        // T0SZ (bits 5:0)   = 16 -> 48-bit lower VA space
        // T1SZ (bits 21:16) = 16 -> 48-bit upper VA space
        // TG0 (bits 15:14)  = 0b00 -> 4KB granule for TTBR0
        // TG1 (bits 31:30)  = 0b10 -> 4KB granule for TTBR1
        // SH0 (bits 13:12)  = 0b11 -> Inner Shareable
        // SH1 (bits 29:28)  = 0b11 -> Inner Shareable
        // ORGN0/IRGN0 (bits 11:10, 9:8) = 0b01 -> Write-Back Cacheable
        // ORGN1/IRGN1 (bits 27:26, 25:24) = 0b01 -> Write-Back Cacheable
        // IPS (bits 34:32)  = 0b000 -> 32-bit PA (enough for QEMU virt with 2GB)
        //
        // TCR = 0x80100010 | T0SZ=16 | T1SZ=16<<16 | TG1=0b10<<30 | SH1/ORGN1/IRGN1
        //     = 0x80803510 (simplified for 4KB granule, 48-bit VA both)
        //
        // Detailed breakdown:
        //   T0SZ=16 (0x10), IRGN0=0b01, ORGN0=0b01, SH0=0b11, TG0=0b00 -> low bits
        //   T1SZ=16 (0x10 << 16), IRGN1=0b01, ORGN1=0b01, SH1=0b11, TG1=0b10 -> high bits
        //   IPS=0b000 for 32-bit PA (sufficient)
        //
        // Compute:
        //   lower = T0SZ(16) | IRGN0(1<<8) | ORGN0(1<<10) | SH0(3<<12) = 0x10 | 0x100 | 0x400 | 0x3000 = 0x3510
        //   upper = T1SZ(16<<16) | IRGN1(1<<24) | ORGN1(1<<26) | SH1(3<<28) | TG1(2<<30)
        //         = 0x100000 | 0x1000000 | 0x4000000 | 0x30000000 | 0x80000000 = 0xB5100000
        //   tcr = lower | upper = 0xB5103510
        let tcr_val: u64 = 0xB5103510;
        asm!("msr tcr_el1, {}", in(reg) tcr_val);

        // Enable MMU in SCTLR_EL1
        let mut sctlr: u64;
        asm!("mrs {}, sctlr_el1", out(reg) sctlr);
        sctlr |= 1; // Set M bit to enable MMU
        asm!("msr sctlr_el1, {}", in(reg) sctlr);

        // Memory barriers
        asm!("dsb sy");
        asm!("isb");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_page_table_entry_creation() {
        let pte = PageTableEntry::new();
        assert_eq!(pte.entry, 0);
        assert!(!pte.is_valid());
    }

    #[test_case]
    fn test_page_table_entry_validation() {
        let mut pte = PageTableEntry::new();
        assert!(!pte.is_valid());

        pte.validate();
        assert!(pte.is_valid());

        pte.invalidate();
        assert!(!pte.is_valid());
    }

    #[test_case]
    fn test_page_table_entry_ppn() {
        let mut pte = PageTableEntry::new();
        let test_ppn = 0x12345;

        pte.set_ppn(test_ppn);
        assert_eq!(pte.get_ppn(), test_ppn);
    }

    #[test_case]
    fn test_page_table_entry_permissions() {
        let mut pte = PageTableEntry::new();

        pte.readable();
        pte.writable();
        pte.executable();
        pte.accessible_from_user();

        // Check that the entry has been modified
        assert_ne!(pte.entry, 0);
    }

    #[test_case]
    fn test_page_table_ttbr_value() {
        let page_table = PageTable {
            entries: [PageTableEntry::new(); 512],
        };
        let asid = 42u16;

        let ttbr_val = page_table.get_val_for_ttbr(asid);
        let expected_asid = ((ttbr_val >> 48) & 0xffff) as u16;

        assert_eq!(expected_asid, asid);
    }
}
