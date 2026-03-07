//! ARMv8.0-A MMU implementation with 4KB granule size
//!
//! This module implements the ARMv8.0-A Memory Management Unit (MMU) with:
//! - 4KB page granule
//! - 4-level page table translation (L0-L3)
//! - 48-bit virtual address space
//!
//! Strategy follows RISC-V sv48 implementation for consistency.

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
    pub const fn new() -> Self {
        PageTableEntry { entry: 0 }
    }

    pub fn get_ppn(&self) -> usize {
        ((self.entry >> 12) & 0xfffffffff) as usize
    }

    pub fn is_valid(&self) -> bool {
        self.entry & 1 == 1
    }

    /// Check if this is a table descriptor (not a block/page leaf)
    /// For L0-L2: bits[1:0] = 0b11 means table, 0b01 means block
    /// For L3: always page descriptor (bits[1:0] = 0b11)
    pub fn is_table(&self) -> bool {
        self.is_valid() && (self.entry & 0x3) == 0x3
    }

    pub fn validate(&mut self) {
        self.entry |= 1;
    }

    pub fn invalidate(&mut self) {
        self.entry &= !1;
    }

    pub fn set_ppn(&mut self, ppn: usize) -> &mut Self {
        let ppn_mask = 0xfffffffff;
        self.entry &= !(ppn_mask << 12);
        self.entry |= ((ppn as u64) & ppn_mask) << 12;
        self
    }

    pub fn clear_all(&mut self) -> &mut Self {
        self.entry = 0;
        self
    }

    /// Set as table descriptor (L0-L2): bits[1:0] = 0b11
    pub fn set_table(&mut self) -> &mut Self {
        self.entry |= 0x3;
        self
    }

    /// Set as page descriptor (L3): bits[1:0] = 0b11, AF=1
    pub fn set_page(&mut self) -> &mut Self {
        self.entry |= 0x3;
        self.entry |= 1 << 10; // AF (Access Flag)
        self
    }

    pub fn set_ap(&mut self, ap: u8) -> &mut Self {
        self.entry &= !(0x3 << 6);
        self.entry |= ((ap as u64) & 0x3) << 6;
        self
    }

    pub fn executable(&mut self) -> &mut Self {
        self.entry &= !(1 << 54); // Clear UXN
        self.entry &= !(1 << 53); // Clear PXN
        self
    }

    pub fn set_memory_attr(&mut self, attr_index: u8) -> &mut Self {
        self.entry &= !(0x7 << 2);
        self.entry |= ((attr_index as u64) & 0x7) << 2;
        self
    }

    pub fn set_shareability(&mut self, sh: u8) -> &mut Self {
        self.entry &= !(0x3 << 8);
        self.entry |= ((sh as u64) & 0x3) << 8;
        self
    }

    pub fn set_non_global(&mut self) -> &mut Self {
        self.entry |= 1 << 11;
        self
    }

    pub fn set_global(&mut self) -> &mut Self {
        self.entry &= !(1 << 11);
        self
    }

    // Test helper methods
    pub fn get_flags(&self) -> u64 {
        self.entry & 0xfff
    }

    pub fn is_leaf(&self) -> bool {
        self.is_valid() && (self.entry & 0x2) == 0
    }

    pub fn set_flags(&mut self, flags: u64) -> &mut Self {
        self.entry |= flags & 0xfff;
        self
    }

    pub fn clear_flags(&mut self) -> &mut Self {
        self.entry &= !0xfff;
        self
    }

    pub fn readable(&mut self) -> &mut Self {
        self
    }

    pub fn writable(&mut self) -> &mut Self {
        self
    }

    pub fn accessible_from_user(&mut self) -> &mut Self {
        self
    }

    pub fn set_valid(&mut self, valid: bool) {
        if valid {
            self.validate();
        } else {
            self.invalidate();
        }
    }

    pub fn set_readable(&mut self, _readable: bool) {}
    pub fn is_readable(&self) -> bool {
        (self.entry >> 7) & 1 == 0
    }
    pub fn set_writable(&mut self, _writable: bool) {}
    pub fn is_writable(&self) -> bool {
        (self.entry >> 7) & 1 == 0
    }
    pub fn set_executable(&mut self, executable: bool) {
        if executable {
            self.executable();
        } else {
            self.entry |= (1 << 54) | (1 << 53);
        }
    }
    pub fn is_executable(&self) -> bool {
        (self.entry >> 54) & 1 == 0 && (self.entry >> 53) & 1 == 0
    }
    pub fn set_user_accessible(&mut self, accessible: bool) {
        if accessible {
            self.entry |= 1 << 6;
        } else {
            self.entry &= !(1 << 6);
        }
    }
    pub fn is_user_accessible(&self) -> bool {
        (self.entry >> 6) & 1 == 1
    }
    pub fn set_memory_type_device(&mut self) {
        self.set_memory_attr(0);
    }
    pub fn is_device_memory(&self) -> bool {
        (self.entry >> 2) & 0x7 == 0
    }
    pub fn set_memory_type_normal_cacheable(&mut self) {
        self.set_memory_attr(1);
    }
    pub fn is_normal_cacheable_memory(&self) -> bool {
        (self.entry >> 2) & 0x7 == 1
    }
    pub fn set_outer_shareable(&mut self) {
        self.set_shareability(0b10);
    }
    pub fn is_outer_shareable(&self) -> bool {
        (self.entry >> 8) & 0x3 == 0b10
    }
    pub fn set_inner_shareable(&mut self) {
        self.set_shareability(0b11);
    }
    pub fn is_inner_shareable(&self) -> bool {
        (self.entry >> 8) & 0x3 == 0b11
    }
}

/// Shareability attributes
#[repr(u8)]
pub enum Shareability {
    NonShareable = 0b00,
    OuterShareable = 0b10,
    InnerShareable = 0b11,
}

/// Memory attribute indices for MAIR_EL1
#[repr(u8)]
pub enum MemoryAttribute {
    Device = 0,
    Normal = 1,
    NonCacheable = 2,
}

/// Page table structure aligned to 4KB
#[repr(align(4096))]
#[derive(Debug)]
pub struct PageTable {
    pub entries: [PageTableEntry; 512],
}

impl PageTable {
    #[inline(always)]
    fn is_canonical_48(vaddr: usize) -> bool {
        // For 48-bit VA, bits[63:48] must be all 0 when bit47=0, or all 1 when bit47=1.
        let sign = (vaddr >> 47) & 1;
        if sign == 0 {
            (vaddr >> 48) == 0
        } else {
            (vaddr >> 48) == 0xffff
        }
    }

    pub fn new() -> Self {
        PageTable {
            entries: [PageTableEntry::new(); 512],
        }
    }

    /// Switch to this page table (like RISC-V's switch())
    pub fn switch(&self, asid: u16) {
        let ttbr_val = self.get_val_for_ttbr(asid);

        // Remember kernel TTBR for trampoline
        crate::arch::aarch64::get_cpu().set_kernel_ttbr0(ttbr_val);

        unsafe {
            // Update TTBR0 (user translation base) only.
            // TTBR1 is managed separately and is expected to stay fixed to the kernel table.
            asm!(
                "msr ttbr0_el1, {ttbr}",
                "isb",
                "tlbi vmalle1is",
                "dsb ish",
                "isb",
                ttbr = in(reg) ttbr_val,
            );

            // Enable MMU if not already enabled
            let mut sctlr: u64;
            asm!("mrs {}, sctlr_el1", out(reg) sctlr);
            if sctlr & 1 == 0 {
                init_mmu_registers();
            }
        }
    }

    /// Switch TTBR1 only (for kernel high-VA)
    pub fn switch_ttbr1(&self, asid: u16) {
        let ttbr_val = self.get_val_for_ttbr(asid);
        unsafe {
            asm!(
                "msr ttbr1_el1, {ttbr}",
                "isb",
                "tlbi vmalle1is",
                "dsb ish",
                "isb",
                ttbr = in(reg) ttbr_val,
            );
        }
    }

    /// Get TTBR value (like RISC-V's get_val_for_satp())
    pub fn get_val_for_ttbr(&self, asid: u16) -> u64 {
        let baddr = (self as *const _ as u64) & 0xffffffffffff;
        let asid_val = (asid as u64) << 48;
        baddr | asid_val
    }

    /// Map a memory area (like RISC-V's map_memory_area())
    pub fn map_memory_area(
        &mut self,
        asid: u16,
        mmap: VirtualMemoryMap,
        accessed: bool,
        dirty: bool,
    ) -> Result<(), &'static str> {
        if mmap.vmarea.start % PAGE_SIZE != 0
            || mmap.pmarea.start % PAGE_SIZE != 0
            || mmap.vmarea.size() % PAGE_SIZE != 0
            || mmap.pmarea.size() % PAGE_SIZE != 0
        {
            return Err("Address is not aligned to PAGE_SIZE");
        }

        let mut vaddr = mmap.vmarea.start;
        let mut paddr = mmap.pmarea.start;
        // Avoid overflow when mapping regions near usize::MAX.
        while vaddr <= mmap.vmarea.end.saturating_sub(PAGE_SIZE - 1) {
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

    /// Map a single page (like RISC-V's map())
    pub fn map(
        &mut self,
        asid: u16,
        vaddr: usize,
        paddr: usize,
        permissions: usize,
        _accessed: bool,
        _dirty: bool,
    ) {
        if !Self::is_canonical_48(vaddr) {
            panic!(
                "Virtual address {:#x} is not canonical for 48-bit VA",
                vaddr
            );
        }

        let vaddr = vaddr & !0xfff;
        let paddr = paddr & !0xfff;

        let pte = match self.walk(vaddr, true, asid) {
            Some(pte) => pte,
            None => panic!("map: walk() couldn't allocate page-table page"),
        };

        pte.clear_all();

        // Set as L3 page descriptor
        pte.set_page();
        pte.set_ppn(paddr >> 12);

        // Determine memory type
        let is_user = VirtualMemoryPermission::User.contained_in(permissions);
        let is_device = !VirtualMemoryPermission::Execute.contained_in(permissions)
            && !is_user
            && !VirtualMemoryPermission::Read.contained_in(permissions);

        if is_device {
            pte.set_memory_attr(MemoryAttribute::Device as u8);
            pte.set_shareability(Shareability::OuterShareable as u8);
        } else {
            pte.set_memory_attr(MemoryAttribute::Normal as u8);
            pte.set_shareability(Shareability::InnerShareable as u8);
        }

        // AP[7:6] encoding
        let is_write = VirtualMemoryPermission::Write.contained_in(permissions);
        let ap = match (is_user, is_write) {
            (false, true) => 0b00,
            (true, true) => 0b01,
            (false, false) => 0b10,
            (true, false) => 0b11,
        };
        pte.set_ap(ap);

        // nG bit for user pages (ASID-tagged)
        if is_user {
            pte.set_non_global();
        } else {
            pte.set_global();
        }

        // Execute permission
        if VirtualMemoryPermission::Execute.contained_in(permissions) {
            pte.executable();
        }

        // Ensure the updated PTE is visible to the hardware table walker.
        crate::arch::aarch64::clean_dcache_to_poc_range(
            (pte as *const PageTableEntry) as usize,
            core::mem::size_of::<PageTableEntry>(),
        );

        // TLB invalidate (like RISC-V's sfence.vma)
        unsafe { asm!("dsb ish", "tlbi vmalle1is", "dsb ish", "isb") };
    }

    /// Walk page table hierarchy (like RISC-V's walk())
    ///
    /// AArch64 4KB granule, 4-level:
    /// - L0: bits 47:39 (9 bits)
    /// - L1: bits 38:30 (9 bits)
    /// - L2: bits 29:21 (9 bits)
    /// - L3: bits 20:12 (9 bits)
    pub fn walk(&mut self, vaddr: usize, alloc: bool, asid: u16) -> Option<&mut PageTableEntry> {
        if !Self::is_canonical_48(vaddr) {
            return None;
        }

        let mut pagetable = self as *mut PageTable;

        unsafe {
            // Walk L0, L1, L2 (intermediate levels)
            for level in 0..MAX_PAGING_LEVEL {
                let shift = 12 + 9 * (3 - level);
                let index = (vaddr >> shift) & 0x1ff;
                let pte = &mut (*pagetable).entries[index];

                if pte.is_valid() {
                    // Must be a table descriptor at intermediate levels
                    if !pte.is_table() {
                        return None; // Block entry not supported
                    }
                    pagetable = (pte.get_ppn() << 12) as *mut PageTable;
                } else {
                    if !alloc {
                        return None;
                    }
                    // Allocate new page table
                    let new_table = new_raw_pagetable(asid);
                    if new_table.is_null() {
                        return None;
                    }
                    pte.clear_all();
                    pte.set_ppn(new_table as usize >> 12);
                    pte.set_table();

                    // Ensure the parent table PTE update is visible to the walker.
                    crate::arch::aarch64::clean_dcache_to_poc_range(
                        (pte as *const PageTableEntry) as usize,
                        core::mem::size_of::<PageTableEntry>(),
                    );

                    pagetable = new_table;
                }
            }

            // Return L3 entry
            let index = (vaddr >> 12) & 0x1ff;
            Some(&mut (*pagetable).entries[index])
        }
    }

    /// Translate a virtual address to a physical address by walking the page table.
    ///
    /// # Arguments
    ///
    /// * `vaddr` - The virtual address to translate
    ///
    /// # Returns
    ///
    /// The physical address if the mapping exists, or `None` if unmapped.
    pub fn translate(&mut self, vaddr: usize) -> Option<usize> {
        let pte = self.walk(vaddr, false, 0)?;
        if pte.is_valid() {
            let ppn = pte.get_ppn();
            let page_offset = vaddr & 0xfff;
            Some((ppn << 12) | page_offset)
        } else {
            None
        }
    }

    /// Unmap a single page (like RISC-V's unmap())
    pub fn unmap(&mut self, _asid: u16, vaddr: usize) {
        if !Self::is_canonical_48(vaddr) {
            panic!(
                "Virtual address {:#x} is not canonical for 48-bit VA",
                vaddr
            );
        }

        let vaddr = vaddr & !0xfff;

        if let Some(pte) = self.walk(vaddr, false, 0) {
            if pte.is_valid() {
                pte.clear_all();
                crate::arch::aarch64::clean_dcache_to_poc_range(
                    (pte as *const PageTableEntry) as usize,
                    core::mem::size_of::<PageTableEntry>(),
                );
                unsafe { asm!("dsb ish", "tlbi vmalle1is", "dsb ish", "isb") };
            }
        }
    }

    /// Unmap all entries (like RISC-V's unmap_all())
    pub fn unmap_all(&mut self) {
        for entry in &mut self.entries {
            entry.clear_all();
        }
        unsafe { asm!("dsb ish", "tlbi vmalle1is", "dsb ish", "isb") };
    }
}

/// Initialize MMU registers
pub fn init_mmu_registers() {
    unsafe {
        // MAIR_EL1: memory attribute configuration
        // Index 0: Device-nGnRnE (0x00)
        // Index 1: Normal, Write-Back (0xFF)
        // Index 2: Normal, Non-Cacheable (0x44)
        let mair_val: u64 = 0x44ff00;
        asm!("msr mair_el1, {}", in(reg) mair_val);

        // TCR_EL1: Translation Control Register
        // T0SZ = 16 (48-bit VA for TTBR0)
        // T1SZ = 16 (48-bit VA for TTBR1)
        // TG0 = 0b00 (4KB granule for TTBR0)
        // TG1 = 0b10 (4KB granule for TTBR1)
        // SH0/SH1 = 0b11 (Inner Shareable)
        // ORGN0/ORGN1 = 0b01 (Write-Back)
        // IRGN0/IRGN1 = 0b01 (Write-Back)
        // IPS = 0b001 (36-bit PA, supports up to 64GB) - bit[34:32]
        let tcr_val: u64 = 0x1_B510_3510;
        asm!("msr tcr_el1, {}", in(reg) tcr_val);

        // SCTLR_EL1: System Control Register
        let mut sctlr: u64;
        asm!("mrs {}, sctlr_el1", out(reg) sctlr);
        sctlr |= 1; // M: MMU enable
        sctlr |= 1 << 2; // C: Data cache enable
        sctlr |= 1 << 12; // I: Instruction cache enable
        asm!("msr sctlr_el1, {}", in(reg) sctlr);
        asm!("dsb sy", "isb");
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
    fn test_page_table_ttbr_value() {
        let page_table = PageTable::new();
        let asid = 42u16;
        let ttbr_val = page_table.get_val_for_ttbr(asid);
        let expected_asid = ((ttbr_val >> 48) & 0xffff) as u16;
        assert_eq!(expected_asid, asid);
    }
}
