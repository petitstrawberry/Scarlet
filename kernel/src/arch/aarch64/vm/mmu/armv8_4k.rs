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
use crate::vm::addr::{phys_to_virt, virt_to_phys};
use crate::vm::vmem::MemoryAttribute;
use crate::vm::vmem::VirtualMemoryMap;
use crate::vm::vmem::VirtualMemoryPermission;

const MAIR_DEVICE_NGNRNE: u64 = 0x00;
const MAIR_NORMAL_WRITE_BACK: u64 = 0xff;
const MAIR_NORMAL_NON_CACHEABLE: u64 = 0x44;
const MAIR_DEVICE_GRE: u64 = 0x0c;
const SCARLET_MAIR_EL1: u64 = MAIR_NORMAL_WRITE_BACK
    | (MAIR_NORMAL_WRITE_BACK << 8)
    | (MAIR_NORMAL_NON_CACHEABLE << 16)
    | (MAIR_DEVICE_GRE << 24)
    | (MAIR_DEVICE_NGNRNE << 32);
// TCR_EL1.IPS=0b010 selects a 40-bit PA range; QEMU virt can place PCI ECAM above 36 bits.
const SCARLET_TCR_EL1: u64 = 0x2_B510_3510;
const SCTLR_EL1_ENABLE_MASK: u64 = 1 | (1 << 2) | (1 << 12);
/// Bits we always clear in SCTLR_EL1 when (re)enabling the MMU.
///
/// - bit 1 (A): Strict alignment check for all data accesses. We must keep
///   this DISABLED so that unaligned loads/stores to Normal memory (e.g.
///   SIMD `ldr q0, [x8]` from a non-16B-aligned address) do not raise
///   alignment faults (DFSC=0x21). Limine may leave this set, so we clear it
///   explicitly. SP-alignment checks (SA/SA0, bits 3/4) are intentionally
///   left untouched.
const SCTLR_EL1_DISABLE_MASK: u64 = 1 << 1;
const DEBUG_DEVICE_FAULT_VA: usize = 0xffff_0008_3afd_b000;

/// Maximum paging levels for AArch64 4KB granule (4 levels: 0-3)
const MAX_PAGING_LEVEL: usize = 3;
const MAX_BLOCK_LEVEL: usize = 2;

/// Attributes carried while installing a mapping.
///
/// Keeping the attributes bundled makes the level-specific mapping path explicit.
#[derive(Clone, Copy)]
struct MapAttrs {
    permissions: usize,
    memory_attribute: MemoryAttribute,
}

/// Returns the page size represented by a logical page-table level.
///
/// Level 0 is a 4 KiB page, level 1 is a 2 MiB block, and level 2 is a
/// 1 GiB block for the AArch64 4 KiB granule translation regime.
fn page_size_for_level(level: usize) -> usize {
    1usize << (12 + 9 * level)
}

/// Chooses the largest supported page level for a mapping chunk.
///
/// A block level is selected only when the remaining size is large enough and
/// both virtual and physical addresses are aligned to that level's size.
fn best_page_level(vaddr: usize, paddr: usize, size: usize) -> usize {
    for level in (1..=MAX_BLOCK_LEVEL).rev() {
        let page_size = page_size_for_level(level);
        if size >= page_size && vaddr.is_multiple_of(page_size) && paddr.is_multiple_of(page_size) {
            return level;
        }
    }
    0
}

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

    pub fn set_entry(&mut self, entry: u64) -> &mut Self {
        self.entry = entry;
        self
    }

    // Test helper methods
    pub fn get_flags(&self) -> u64 {
        self.entry & 0xfff
    }

    pub fn is_leaf(&self) -> bool {
        self.is_valid() && (self.entry & 0x3) == 0x1
    }

    /// Returns whether this PTE is a leaf descriptor for `level`.
    ///
    /// AArch64 uses page descriptors (`0b11`) at level 0 and block descriptors
    /// (`0b01`) at levels 1 and 2.
    fn is_leaf_for_level(&self, level: usize) -> bool {
        if level == 0 {
            self.is_valid() && (self.entry & 0x3) == 0x3
        } else {
            self.is_leaf()
        }
    }

    /// Returns whether this PTE's output address is aligned for `level`.
    ///
    /// Huge-page block descriptors must have zero lower output-address fields
    /// for all lower page-table levels.
    pub fn is_aligned_for_level(&self, level: usize) -> bool {
        let mask = (1usize << (9 * level)) - 1;
        self.get_ppn() & mask == 0
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
        self.set_memory_attr(4);
    }
    pub fn is_device_memory(&self) -> bool {
        (self.entry >> 2) & 0x7 == 4
    }
    pub fn set_memory_type_normal_cacheable(&mut self) {
        self.set_memory_attr(0);
    }
    pub fn is_normal_cacheable_memory(&self) -> bool {
        (self.entry >> 2) & 0x7 == 0
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

#[inline(always)]
pub(in crate::arch::aarch64::vm) fn invalidate_stage1_translations_inner_shareable() {
    // SAFETY: The barriers and broadcast TLBI form the architected completion
    // sequence for publishing stage-1 translation-table changes to all PEs in
    // the inner-shareable domain.
    unsafe {
        asm!(
            "dsb ishst",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            options(nostack),
        );
    }
}

#[inline(always)]
fn publish_page_table_entry(pte: &PageTableEntry) {
    crate::arch::aarch64::clean_dcache_to_poc_range(
        (pte as *const PageTableEntry) as usize,
        core::mem::size_of::<PageTableEntry>(),
    );
}

#[inline(always)]
fn replacement_requires_break_before_make(old_entry: u64, new_entry: u64) -> bool {
    old_entry & 1 != 0 && old_entry != new_entry
}

fn break_before_make(pte: &mut PageTableEntry) {
    pte.clear_all();
    publish_page_table_entry(pte);
    invalidate_stage1_translations_inner_shareable();
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
        let upper = vaddr >> 48;
        upper == 0 || upper == 0xffff
    }

    pub(in crate::arch::aarch64::vm) fn new() -> Self {
        PageTable {
            entries: [PageTableEntry::new(); 512],
        }
    }

    /// Switch to this page table (like RISC-V's switch())
    pub(in crate::arch::aarch64::vm) fn switch(&self, asid: u16) {
        let ttbr_val = self.get_val_for_ttbr(asid);

        unsafe {
            let mut sctlr: u64;
            asm!("mrs {sctlr}, sctlr_el1", sctlr = out(reg) sctlr, options(nostack));
            if sctlr & 1 == 0 {
                asm!(
                    "msr mair_el1, {mair}",
                    "msr tcr_el1, {tcr}",
                    "isb",
                    "msr ttbr0_el1, {ttbr}",
                    "isb",
                    "tlbi vmalle1",
                    "dsb nsh",
                    "isb",
                    "mrs {tmp}, sctlr_el1",
                    "bic {tmp}, {tmp}, {sctlr_clear}",
                    "orr {tmp}, {tmp}, {sctlr_flags}",
                    "msr sctlr_el1, {tmp}",
                    "dsb sy",
                    "isb",
                    mair = in(reg) SCARLET_MAIR_EL1,
                    tcr = in(reg) SCARLET_TCR_EL1,
                    ttbr = in(reg) ttbr_val,
                    sctlr_flags = in(reg) SCTLR_EL1_ENABLE_MASK,
                    sctlr_clear = in(reg) SCTLR_EL1_DISABLE_MASK,
                    tmp = lateout(reg) _,
                    options(nostack),
                );
            } else {
                asm!(
                    "msr ttbr0_el1, {ttbr}",
                    "isb",
                    "tlbi vmalle1",
                    "dsb nsh",
                    "isb",
                    ttbr = in(reg) ttbr_val,
                    options(nostack),
                );
            }
        }
    }

    /// Switch TTBR1 only (for kernel high-VA)
    pub(in crate::arch::aarch64::vm) fn switch_ttbr1(&self, asid: u16) {
        let ttbr_val = self.get_val_for_ttbr(asid);
        unsafe {
            asm!(
                "msr ttbr1_el1, {ttbr}",
                "isb",
                "tlbi vmalle1",
                "dsb nsh",
                "isb",
                ttbr = in(reg) ttbr_val,
                options(nostack),
            );
        }
    }

    pub fn switch_for_boot(&self, asid: u16) {
        let ttbr_val = self.get_val_for_ttbr(asid);
        unsafe {
            asm!(
                "msr mair_el1, {mair}",
                "msr tcr_el1, {tcr}",
                "isb",
                "dsb ishst",
                "msr ttbr1_el1, {ttbr}",
                "msr ttbr0_el1, {ttbr}",
                "isb",
                "tlbi vmalle1",
                "dsb nsh",
                "ic iallu",
                "dsb sy",
                "isb",
                "mrs {tmp}, sctlr_el1",
                "bic {tmp}, {tmp}, {sctlr_clear}",
                "orr {tmp}, {tmp}, {sctlr_flags}",
                "msr sctlr_el1, {tmp}",
                "isb",
                mair = in(reg) SCARLET_MAIR_EL1,
                tcr = in(reg) SCARLET_TCR_EL1,
                ttbr = in(reg) ttbr_val,
                sctlr_flags = in(reg) SCTLR_EL1_ENABLE_MASK,
                sctlr_clear = in(reg) SCTLR_EL1_DISABLE_MASK,
                tmp = lateout(reg) _,
                options(nostack),
            );
        }
    }

    /// Get TTBR value (like RISC-V's get_val_for_satp())
    pub(in crate::arch::aarch64::vm) fn get_val_for_ttbr(&self, asid: u16) -> u64 {
        let baddr = (virt_to_phys(self as *const _ as usize) as u64) & 0xffffffffffff;
        let asid_val = (asid as u64) << 48;
        baddr | asid_val
    }

    /// Map a memory area (like RISC-V's map_memory_area())
    pub(in crate::arch::aarch64::vm) fn map_memory_area(
        &mut self,
        asid: u16,
        mmap: VirtualMemoryMap,
        _accessed: bool,
        _dirty: bool,
    ) -> Result<(), &'static str> {
        if mmap.vmarea.start % PAGE_SIZE != 0
            || mmap.pmarea.start % PAGE_SIZE != 0
            || mmap.vmarea.size() % PAGE_SIZE != 0
            || mmap.pmarea.size() % PAGE_SIZE != 0
        {
            return Err("Address is not aligned to PAGE_SIZE");
        }

        let attrs = MapAttrs {
            permissions: mmap.permissions,
            memory_attribute: mmap.memory_attribute,
        };
        let mut vaddr = mmap.vmarea.start;
        let mut paddr = mmap.pmarea.start;
        while vaddr <= mmap.vmarea.end {
            let remaining = mmap
                .vmarea
                .end
                .checked_sub(vaddr)
                .and_then(|remaining| remaining.checked_add(1))
                .ok_or("Address range overflow")?;
            let mut level = best_page_level(vaddr, paddr, remaining);
            while self
                .try_map_at_level(asid, vaddr, paddr, attrs, level)
                .is_err()
            {
                if level == 0 {
                    return Err("Failed to map memory area");
                }
                level -= 1;
            }

            let page_size = page_size_for_level(level);
            match vaddr.checked_add(page_size) {
                Some(addr) => vaddr = addr,
                None => break,
            }
            match paddr.checked_add(page_size) {
                Some(addr) => paddr = addr,
                None => break,
            }
        }

        // Publish new mappings to all PEs that may run this address space.
        invalidate_stage1_translations_inner_shareable();

        Ok(())
    }

    /// Map a single page (like RISC-V's map())
    pub(in crate::arch::aarch64::vm) fn map(
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

        self.try_map_at_level(
            asid,
            vaddr,
            paddr,
            MapAttrs {
                permissions,
                memory_attribute: MemoryAttribute::Normal,
            },
            0,
        )
        .expect("map: couldn't install a 4 KiB leaf mapping");
        crate::breadcrumb::drop(crate::breadcrumb::PT_LEAF_DONE, vaddr as u64, paddr as u64);

        // Publish new mappings to all PEs that may run this address space.
        crate::breadcrumb::drop(crate::breadcrumb::PT_TLBI_BEGIN, vaddr as u64, asid as u64);
        invalidate_stage1_translations_inner_shareable();
        crate::breadcrumb::drop(crate::breadcrumb::PT_TLBI_DONE, vaddr as u64, asid as u64);
    }

    /// Attempts to install a leaf mapping at the specified logical level.
    ///
    /// The addresses must be aligned to the selected level's page size. Existing
    /// lower-level page tables are not replaced by block leaves implicitly.
    fn try_map_at_level(
        &mut self,
        asid: u16,
        vaddr: usize,
        paddr: usize,
        attrs: MapAttrs,
        level: usize,
    ) -> Result<(), &'static str> {
        let page_size = page_size_for_level(level);
        if level > MAX_BLOCK_LEVEL
            || !vaddr.is_multiple_of(page_size)
            || !paddr.is_multiple_of(page_size)
        {
            return Err("Address is not aligned to page size");
        }

        let pte = self
            .walk_to_level(vaddr, level, true, asid)
            .ok_or("walk failed")?;
        if pte.is_valid() && level > 0 && pte.is_table() {
            return Err("Cannot replace existing page table with a leaf");
        }

        let entry = Self::make_leaf_entry(
            vaddr,
            paddr,
            attrs.permissions,
            level,
            attrs.memory_attribute,
        );
        if pte.entry == entry {
            return Ok(());
        }
        if replacement_requires_break_before_make(pte.entry, entry) {
            break_before_make(pte);
        }
        pte.set_entry(entry);

        // Ensure the updated PTE is visible to the hardware table walker.
        publish_page_table_entry(pte);

        // TLB invalidation is deferred to the caller for batching.
        Ok(())
    }

    /// Builds an AArch64 page or block descriptor for a leaf mapping.
    ///
    /// Level 0 uses a page descriptor (`0b11`); levels 1 and 2 use block
    /// descriptors (`0b01`). Permission, memory type, shareability, and execute
    /// attributes are encoded from the mapping request.
    pub(crate) fn make_leaf_entry(
        vaddr: usize,
        paddr: usize,
        permissions: usize,
        level: usize,
        memory_attribute: MemoryAttribute,
    ) -> u64 {
        let is_user = VirtualMemoryPermission::User.contained_in(permissions);
        let memory_attr = match memory_attribute {
            // AttrIndx 0 stays Limine-compatible Normal WB while Limine's
            // inherited TTBRs remain active during the boot-table handoff.
            MemoryAttribute::Normal => 0,
            MemoryAttribute::NonCacheable => 2,
            MemoryAttribute::DeviceBurstable => 3,
            MemoryAttribute::Device => 4,
        };
        let shareability = match memory_attribute {
            MemoryAttribute::Device | MemoryAttribute::DeviceBurstable => {
                Shareability::OuterShareable as u64
            }
            MemoryAttribute::Normal | MemoryAttribute::NonCacheable => {
                Shareability::InnerShareable as u64
            }
        };

        let mut entry = 0u64;
        entry |= if level == 0 { 0x3 } else { 0x1 };
        entry |= 1 << 10;
        entry |= ((paddr >> 12) as u64 & 0xfffffffff) << 12;
        entry |= memory_attr << 2;
        entry |= shareability << 8;

        // AP[7:6] encoding
        let is_write = VirtualMemoryPermission::Write.contained_in(permissions);
        let ap = match (is_user, is_write) {
            (false, true) => 0b00,
            (true, true) => 0b01,
            (false, false) => 0b10,
            (true, false) => 0b11,
        };
        entry |= (ap as u64) << 6;

        // nG bit for user pages (ASID-tagged)
        if is_user {
            entry |= 1 << 11;
        }

        // Execute permission
        if !VirtualMemoryPermission::Execute.contained_in(permissions) {
            entry |= (1 << 54) | (1 << 53);
        }

        #[cfg(any(debug_assertions, test))]
        if vaddr == DEBUG_DEVICE_FAULT_VA {
            crate::early_println!(
                "[vm-map] target va={:#x} paddr={:#x} perms={:#x} is_user={} is_device={} entry={:#x}",
                vaddr,
                paddr,
                permissions,
                is_user,
                memory_attribute == MemoryAttribute::Device,
                entry,
            );
        }

        entry
    }

    /// Walk page table hierarchy (like RISC-V's walk())
    ///
    /// AArch64 4KB granule, 4-level:
    /// - L0: bits 47:39 (9 bits)
    /// - L1: bits 38:30 (9 bits)
    /// - L2: bits 29:21 (9 bits)
    /// - L3: bits 20:12 (9 bits)
    pub(in crate::arch::aarch64::vm) fn walk(
        &mut self,
        vaddr: usize,
        alloc: bool,
        asid: u16,
    ) -> Option<&mut PageTableEntry> {
        self.walk_to_level(vaddr, 0, alloc, asid)
    }

    /// Walks to the PTE for `vaddr` at `target_level`.
    ///
    /// Intermediate tables are allocated when `alloc` is true. Valid target
    /// levels are 0 through 3, where 0 is the final 4 KiB page level.
    pub(in crate::arch::aarch64::vm) fn walk_to_level(
        &mut self,
        vaddr: usize,
        target_level: usize,
        alloc: bool,
        asid: u16,
    ) -> Option<&mut PageTableEntry> {
        if !Self::is_canonical_48(vaddr) {
            return None;
        }
        if target_level > MAX_PAGING_LEVEL {
            return None;
        }

        let mut pagetable = self as *mut PageTable;

        unsafe {
            for level in ((target_level + 1)..=MAX_PAGING_LEVEL).rev() {
                let index = (vaddr >> (12 + 9 * level)) & 0x1ff;
                let pte = &mut (*pagetable).entries[index];

                // Publish intermediate table descriptors atomically. The
                // shared kernel page table is mutated concurrently (fork,
                // wake, ioremap), so two walkers mapping adjacent VAs share
                // this parent PTE. A non-atomic clear/set sequence lets the
                // loser clobber the winner's child table, orphaning its leaf
                // mappings and corrupting translation for every CPU.
                pagetable = Self::publish_or_descend_intermediate(pte, alloc, asid)?;
            }

            let index = (vaddr >> (12 + 9 * target_level)) & 0x1ff;
            Some(&mut (*pagetable).entries[index])
        }
    }

    /// Atomically publish a child page-table descriptor in an intermediate
    /// PTE, or descend into a table already published by another CPU.
    ///
    /// The shared kernel page table is mutated concurrently by multiple CPUs
    /// (fork, wake, ioremap, trampoline setup). Two walkers targeting adjacent
    /// virtual addresses share the same parent PTE at intermediate levels. A
    /// non-atomic "clear -> set ppn -> set table" sequence lets the loser
    /// overwrite the winner's published child table, orphaning every leaf the
    /// loser installs and corrupting translation for every CPU.
    ///
    /// This performs the invalid -> table-descriptor transition with a
    /// compare-exchange on the full 64-bit descriptor. Losers reuse the
    /// winner's table and continue the walk, so both mappings land in the same
    /// live table.
    ///
    /// On a lost race the locally-allocated table is intentionally leaked: it
    /// is never linked into the live page table, but remains recorded per-ASID
    /// and is reclaimed when the address space is torn down. Leaking one zeroed
    /// page on a rare race is strictly safer than clobbering a peer's table.
    fn publish_or_descend_intermediate(
        pte: &mut PageTableEntry,
        alloc: bool,
        asid: u16,
    ) -> Option<*mut PageTable> {
        use core::sync::atomic::{AtomicU64, Ordering};

        // SAFETY: `PageTableEntry` is `#[repr(align(8))]` wrapping a single
        // `u64`, so the entry pointer is 8-byte aligned and valid for atomic
        // compare-exchange. The caller operates in the page-table walker's
        // unsafe context on live `PageTable` memory.
        let atomic = unsafe { &*(pte as *mut PageTableEntry as *const AtomicU64) };

        let observed = atomic.load(Ordering::SeqCst);
        if observed & 1 != 0 {
            let existing = PageTableEntry { entry: observed };
            // Intermediate descriptors must be table descriptors.
            if !existing.is_table() {
                return None;
            }
            return Some(phys_to_virt(existing.get_ppn() << 12) as *mut PageTable);
        }

        if !alloc {
            return None;
        }

        let new_table = unsafe { new_raw_pagetable(asid) };
        if new_table.is_null() {
            return None;
        }

        let mut desc = PageTableEntry::new();
        desc.set_ppn(virt_to_phys(new_table as usize) >> 12);
        desc.set_table();

        match atomic.compare_exchange(observed, desc.entry, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => {
                publish_page_table_entry(pte);
                Some(new_table)
            }
            Err(actual) => {
                // Lost the publication race. Descend into the winner's table.
                // The freshly-allocated table is leaked (reclaimed at teardown).
                let winner = PageTableEntry { entry: actual };
                if !winner.is_table() {
                    return None;
                }
                Some(phys_to_virt(winner.get_ppn() << 12) as *mut PageTable)
            }
        }
    }

    /// Finds the leaf descriptor that translates `vaddr`.
    ///
    /// Returns the PTE and its logical level so callers can calculate offsets
    /// inside either page or block mappings. Invalid descriptors or misaligned
    /// block outputs are treated as absent mappings.
    fn walk_leaf(&mut self, vaddr: usize) -> Option<(&mut PageTableEntry, usize)> {
        if !Self::is_canonical_48(vaddr) {
            return None;
        }

        let mut pagetable = self as *mut PageTable;

        unsafe {
            for level in (0..=MAX_PAGING_LEVEL).rev() {
                let index = (vaddr >> (12 + 9 * level)) & 0x1ff;
                let pte = &mut (*pagetable).entries[index];
                if !pte.is_valid() {
                    return None;
                }
                if pte.is_leaf_for_level(level) {
                    if !pte.is_aligned_for_level(level) {
                        return None;
                    }
                    return Some((pte, level));
                }
                if level == 0 || !pte.is_table() {
                    return None;
                }
                pagetable = phys_to_virt(pte.get_ppn() << 12) as *mut PageTable;
            }
        }
        None
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
    pub(in crate::arch::aarch64::vm) fn translate(&mut self, vaddr: usize) -> Option<usize> {
        let (pte, level) = self.walk_leaf(vaddr)?;
        let page_offset = vaddr & (page_size_for_level(level) - 1);
        Some((pte.get_ppn() << 12) | page_offset)
    }

    /// Unmap a single page (like RISC-V's unmap())
    fn unmap(&mut self, vaddr: usize) {
        if !Self::is_canonical_48(vaddr) {
            panic!(
                "Virtual address {:#x} is not canonical for 48-bit VA",
                vaddr
            );
        }

        let vaddr = vaddr & !0xfff;

        if let Some((pte, _)) = self.walk_leaf(vaddr) {
            pte.clear_all();
            crate::arch::aarch64::clean_dcache_to_poc_range(
                (pte as *const PageTableEntry) as usize,
                core::mem::size_of::<PageTableEntry>(),
            );
            invalidate_stage1_translations_inner_shareable();
        }
    }

    /// Unmap a virtual address range.
    ///
    /// Whole huge-page leaves are cleared directly. Partial huge-page unmaps
    /// split the leaf into the next lower level so mappings outside the
    /// requested range are preserved.
    pub(in crate::arch::aarch64::vm) fn unmap_range(
        &mut self,
        asid: u16,
        vaddr_start: usize,
        vaddr_end: usize,
    ) {
        if vaddr_start > vaddr_end {
            return;
        }

        let mut vaddr = vaddr_start & !(PAGE_SIZE - 1);
        while vaddr <= vaddr_end {
            let Some((_, level)) = self.walk_leaf(vaddr) else {
                match vaddr.checked_add(PAGE_SIZE) {
                    Some(next) => vaddr = next,
                    None => break,
                }
                continue;
            };

            let leaf_size = page_size_for_level(level);
            let leaf_start = vaddr & !(leaf_size - 1);
            let leaf_end = leaf_start + leaf_size - 1;

            if vaddr_start <= leaf_start && leaf_end <= vaddr_end {
                self.unmap(leaf_start);
                match leaf_end.checked_add(1) {
                    Some(next) => vaddr = next,
                    None => break,
                }
            } else if level == 0 {
                self.unmap(vaddr);
                match vaddr.checked_add(PAGE_SIZE) {
                    Some(next) => vaddr = next,
                    None => break,
                }
            } else {
                self.split_leaf(asid, vaddr, level)
                    .expect("unmap_range: failed to split huge-page leaf");
            }
        }
    }

    /// Splits a huge-page block leaf into next-lower-level entries.
    ///
    /// The child table preserves the original mapping attributes while changing
    /// descriptor type as needed. Allocation and cache maintenance are required
    /// because the hardware page-table walker observes the produced table.
    fn split_leaf(&mut self, asid: u16, vaddr: usize, level: usize) -> Result<(), &'static str> {
        if level == 0 {
            return Err("Cannot split a 4 KiB leaf");
        }

        let (pte, leaf_level) = self.walk_leaf(vaddr).ok_or("No leaf mapping found")?;
        if leaf_level != level {
            return Err("Unexpected leaf level");
        }

        let leaf_entry = pte.entry;
        let leaf_ppn = pte.get_ppn();
        let child_level = level - 1;
        let child_ppn_step = page_size_for_level(child_level) >> 12;

        unsafe {
            let child_table = new_raw_pagetable(asid);
            if child_table.is_null() {
                return Err("Failed to allocate split page table");
            }

            for (idx, child_pte) in (*child_table).entries.iter_mut().enumerate() {
                child_pte.entry = leaf_entry;
                child_pte.entry &= !0x3;
                child_pte.entry |= if child_level == 0 { 0x3 } else { 0x1 };
                child_pte.set_ppn(leaf_ppn + idx * child_ppn_step);
            }
            crate::arch::aarch64::clean_dcache_to_poc_range(
                child_table as usize,
                crate::environment::PAGE_SIZE,
            );

            break_before_make(pte);
            pte.set_ppn(virt_to_phys(child_table as usize) >> 12);
            pte.set_table();
            publish_page_table_entry(pte);
        }

        Ok(())
    }

    /// Unmap all entries (like RISC-V's unmap_all())
    pub(in crate::arch::aarch64::vm) fn unmap_all(&mut self) {
        for entry in &mut self.entries {
            entry.clear_all();
        }
        crate::arch::aarch64::clean_dcache_to_poc_range(
            self as *const PageTable as usize,
            crate::environment::PAGE_SIZE,
        );
        invalidate_stage1_translations_inner_shareable();
    }
}

/// Initialize MMU registers
pub fn init_mmu_registers() {
    unsafe {
        asm!(
            "msr mair_el1, {mair}",
            "msr tcr_el1, {tcr}",
            "isb",
            "mrs {tmp}, sctlr_el1",
            "bic {tmp}, {tmp}, {sctlr_clear}",
            "orr {tmp}, {tmp}, {sctlr_flags}",
            "msr sctlr_el1, {tmp}",
            "dsb sy",
            "isb",
            mair = in(reg) SCARLET_MAIR_EL1,
            tcr = in(reg) SCARLET_TCR_EL1,
            sctlr_flags = in(reg) SCTLR_EL1_ENABLE_MASK,
            sctlr_clear = in(reg) SCTLR_EL1_DISABLE_MASK,
            tmp = lateout(reg) _,
            options(nostack),
        );
    }
}

pub fn sync_el1_translation_registers_if_needed() {
    unsafe {
        let mut sctlr: u64;
        asm!("mrs {sctlr}, sctlr_el1", sctlr = out(reg) sctlr, options(nostack));
        if sctlr & 1 == 0 {
            return;
        }

        let mut mair: u64;
        let mut tcr: u64;
        asm!(
            "mrs {mair}, mair_el1",
            "mrs {tcr}, tcr_el1",
            mair = out(reg) mair,
            tcr = out(reg) tcr,
            options(nostack),
        );

        let alignment_check_enabled = (sctlr & SCTLR_EL1_DISABLE_MASK) != 0;

        let runtime_enabled = sctlr & SCTLR_EL1_ENABLE_MASK == SCTLR_EL1_ENABLE_MASK;

        if mair == SCARLET_MAIR_EL1
            && tcr == SCARLET_TCR_EL1
            && runtime_enabled
            && !alignment_check_enabled
        {
            return;
        }

        asm!(
            "msr mair_el1, {mair}",
            "msr tcr_el1, {tcr}",
            "isb",
            "mrs {tmp}, sctlr_el1",
            "bic {tmp}, {tmp}, {sctlr_clear}",
            "orr {tmp}, {tmp}, {sctlr_flags}",
            "msr sctlr_el1, {tmp}",
            "isb",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            mair = in(reg) SCARLET_MAIR_EL1,
            tcr = in(reg) SCARLET_TCR_EL1,
            sctlr_flags = in(reg) SCTLR_EL1_ENABLE_MASK,
            sctlr_clear = in(reg) SCTLR_EL1_DISABLE_MASK,
            tmp = lateout(reg) _,
            options(nostack),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch::vm::{alloc_virtual_address_space, free_virtual_address_space};
    use crate::vm::vmem::MemoryArea;

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

    #[test_case]
    fn test_kernel_normal_leaf_encoding() {
        let mut page_table = PageTable::new();
        page_table.map(
            1,
            0xffff_ffff_8000_0000,
            0x8000_0000,
            0x01 | 0x02 | 0x04,
            true,
            true,
        );
        let pte = page_table.walk(0xffff_ffff_8000_0000, false, 1).unwrap();
        assert_eq!(pte.entry & 0xfff, 0x703);
    }

    #[test_case]
    fn test_leaf_encoding_uses_declared_memory_attribute() {
        let asid = alloc_virtual_address_space();
        let mut root =
            crate::arch::vm::get_root_pagetable(asid).expect("root page table not found");
        let vaddr = 0x4100_0000;
        let paddr = 0x8100_0000;
        let mmap = VirtualMemoryMap::new(
            MemoryArea::new(paddr, paddr + PAGE_SIZE - 1),
            MemoryArea::new(vaddr, vaddr + PAGE_SIZE - 1),
            VirtualMemoryPermission::Read as usize
                | VirtualMemoryPermission::Write as usize
                | VirtualMemoryPermission::User as usize,
            true,
            None,
        )
        .with_memory_attribute(crate::vm::vmem::MemoryAttribute::NonCacheable);

        root.map_memory_area(mmap, true, true)
            .expect("non-cacheable mapping failed");

        let pte = root
            .walk_to_level(vaddr, 0, false)
            .expect("leaf PTE not found");
        assert_eq!((pte.entry >> 2) & 0b111, 2);

        let burstable_entry = PageTable::make_leaf_entry(
            0x4300_0000,
            0x8300_0000,
            VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
            0,
            MemoryAttribute::DeviceBurstable,
        );
        assert_eq!((burstable_entry >> 2) & 0b111, 3);
        assert_eq!((SCARLET_MAIR_EL1 >> 24) & 0xff, 0x0c);

        drop(root);
        free_virtual_address_space(asid);
    }

    #[test_case]
    fn test_leaf_encoding_does_not_infer_device_from_ioremap_va() {
        let normal_entry = PageTable::make_leaf_entry(
            crate::environment::IOREMAP_START,
            0x8100_0000,
            VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
            0,
            MemoryAttribute::Normal,
        );
        assert_eq!((normal_entry >> 2) & 0b111, 0);

        let device_entry = PageTable::make_leaf_entry(
            0x4200_0000,
            0x8200_0000,
            VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
            0,
            MemoryAttribute::Device,
        );
        assert_eq!((device_entry >> 2) & 0b111, 4);
    }

    #[test_case]
    fn test_mair_attr_index_mappings() {
        assert_eq!(SCARLET_MAIR_EL1 & 0xff, MAIR_NORMAL_WRITE_BACK);
        assert_eq!((SCARLET_MAIR_EL1 >> 8) & 0xff, MAIR_NORMAL_WRITE_BACK);
        assert_eq!((SCARLET_MAIR_EL1 >> 16) & 0xff, MAIR_NORMAL_NON_CACHEABLE);
        assert_eq!((SCARLET_MAIR_EL1 >> 24) & 0xff, MAIR_DEVICE_GRE);
        assert_eq!((SCARLET_MAIR_EL1 >> 32) & 0xff, MAIR_DEVICE_NGNRNE);

        let normal = PageTable::make_leaf_entry(
            0x4100_0000,
            0x8100_0000,
            VirtualMemoryPermission::Read as usize,
            0,
            MemoryAttribute::Normal,
        );
        let non_cacheable = PageTable::make_leaf_entry(
            0x4200_0000,
            0x8200_0000,
            VirtualMemoryPermission::Read as usize,
            0,
            MemoryAttribute::NonCacheable,
        );
        let burstable = PageTable::make_leaf_entry(
            0x4300_0000,
            0x8300_0000,
            VirtualMemoryPermission::Read as usize,
            0,
            MemoryAttribute::DeviceBurstable,
        );
        let device = PageTable::make_leaf_entry(
            0x4400_0000,
            0x8400_0000,
            VirtualMemoryPermission::Read as usize,
            0,
            MemoryAttribute::Device,
        );

        assert_eq!((normal >> 2) & 0b111, 0);
        assert_eq!((non_cacheable >> 2) & 0b111, 2);
        assert_eq!((burstable >> 2) & 0b111, 3);
        assert_eq!((device >> 2) & 0b111, 4);
    }

    #[test_case]
    fn test_valid_translation_change_requires_break_before_make() {
        let old_entry = PageTable::make_leaf_entry(
            0x4100_0000,
            0x8100_0000,
            VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
            0,
            MemoryAttribute::Normal,
        );
        let new_address_entry = PageTable::make_leaf_entry(
            0x4100_0000,
            0x8200_0000,
            VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
            0,
            MemoryAttribute::Normal,
        );
        let new_attribute_entry = PageTable::make_leaf_entry(
            0x4100_0000,
            0x8100_0000,
            VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
            0,
            MemoryAttribute::NonCacheable,
        );

        assert!(!replacement_requires_break_before_make(0, old_entry));
        assert!(!replacement_requires_break_before_make(
            old_entry, old_entry
        ));
        assert!(replacement_requires_break_before_make(
            old_entry,
            new_address_entry
        ));
        assert!(replacement_requires_break_before_make(
            old_entry,
            new_attribute_entry
        ));
    }

    #[test_case]
    fn test_map_memory_area_uses_2m_huge_page() {
        let asid = alloc_virtual_address_space();
        let mut root =
            crate::arch::vm::get_root_pagetable(asid).expect("root page table not found");
        let page_size = page_size_for_level(1);
        let vaddr = 0x4000_0000;
        let paddr = 0x8000_0000;
        let mmap = VirtualMemoryMap::new(
            MemoryArea::new(paddr, paddr + page_size - 1),
            MemoryArea::new(vaddr, vaddr + page_size - 1),
            VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
            false,
            None,
        );

        root.map_memory_area(mmap, true, true)
            .expect("huge-page mapping failed");

        let pte = root
            .walk_to_level(vaddr, 1, false)
            .expect("huge-page PTE not found");
        assert!(pte.is_leaf());
        assert!(pte.is_aligned_for_level(1));
        assert_eq!(root.translate(vaddr + 0x1234), Some(paddr + 0x1234));

        drop(root);
        free_virtual_address_space(asid);
    }

    #[test_case]
    fn test_map_memory_area_uses_huge_page_with_4k_tail() {
        let asid = alloc_virtual_address_space();
        let mut root =
            crate::arch::vm::get_root_pagetable(asid).expect("root page table not found");
        let huge_page_size = page_size_for_level(1);
        let map_size = huge_page_size + PAGE_SIZE;
        let vaddr = 0x4020_0000;
        let paddr = 0x8020_0000;
        let mmap = VirtualMemoryMap::new(
            MemoryArea::new(paddr, paddr + map_size - 1),
            MemoryArea::new(vaddr, vaddr + map_size - 1),
            VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
            false,
            None,
        );

        root.map_memory_area(mmap, true, true)
            .expect("mixed huge-page mapping failed");

        let huge_pte = root
            .walk_to_level(vaddr, 1, false)
            .expect("huge-page PTE not found");
        assert!(huge_pte.is_leaf());
        assert!(huge_pte.is_aligned_for_level(1));

        let tail_vaddr = vaddr + huge_page_size;
        let tail_pte = root
            .walk_to_level(tail_vaddr, 0, false)
            .expect("tail 4 KiB PTE not found");
        assert!(tail_pte.is_leaf_for_level(0));

        assert_eq!(root.translate(vaddr + 0x1234), Some(paddr + 0x1234));
        assert_eq!(
            root.translate(tail_vaddr + 0x123),
            Some(paddr + huge_page_size + 0x123)
        );

        drop(root);
        free_virtual_address_space(asid);
    }

    #[test_case]
    fn test_unmap_range_preserves_partial_huge_page() {
        let asid = alloc_virtual_address_space();
        let mut root =
            crate::arch::vm::get_root_pagetable(asid).expect("root page table not found");
        let huge_page_size = page_size_for_level(1);
        let vaddr = 0x4040_0000;
        let paddr = 0x8040_0000;
        let mmap = VirtualMemoryMap::new(
            MemoryArea::new(paddr, paddr + huge_page_size - 1),
            MemoryArea::new(vaddr, vaddr + huge_page_size - 1),
            VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
            false,
            None,
        );

        root.map_memory_area(mmap, true, true)
            .expect("huge-page mapping failed");
        assert!(
            root.walk_to_level(vaddr, 1, false)
                .expect("huge-page PTE not found")
                .is_leaf()
        );

        root.unmap_range(vaddr + PAGE_SIZE, vaddr + 2 * PAGE_SIZE - 1);

        assert_eq!(root.translate(vaddr), Some(paddr));
        assert_eq!(root.translate(vaddr + PAGE_SIZE), None);
        assert_eq!(
            root.translate(vaddr + 2 * PAGE_SIZE),
            Some(paddr + 2 * PAGE_SIZE)
        );
        assert!(
            root.walk_to_level(vaddr, 0, false)
                .expect("split 4 KiB PTE not found")
                .is_leaf_for_level(0)
        );

        drop(root);
        free_virtual_address_space(asid);
    }

    #[test_case]
    fn test_unmap_range_preserves_partial_1g_huge_page() {
        let asid = alloc_virtual_address_space();
        let mut root =
            crate::arch::vm::get_root_pagetable(asid).expect("root page table not found");
        let huge_page_size = page_size_for_level(2);
        let vaddr = 0x8000_0000;
        let paddr = 0x1_0000_0000;
        let mmap = VirtualMemoryMap::new(
            MemoryArea::new(paddr, paddr + huge_page_size - 1),
            MemoryArea::new(vaddr, vaddr + huge_page_size - 1),
            VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
            false,
            None,
        );

        root.map_memory_area(mmap, true, true)
            .expect("1 GiB huge-page mapping failed");
        assert!(
            root.walk_to_level(vaddr, 2, false)
                .expect("1 GiB huge-page PTE not found")
                .is_leaf()
        );

        let removed_vaddr = vaddr + page_size_for_level(1);
        root.unmap_range(removed_vaddr, removed_vaddr + PAGE_SIZE - 1);

        assert_eq!(root.translate(vaddr), Some(paddr));
        assert_eq!(root.translate(removed_vaddr), None);
        assert_eq!(
            root.translate(removed_vaddr + PAGE_SIZE),
            Some(paddr + page_size_for_level(1) + PAGE_SIZE)
        );
        assert!(
            root.walk_to_level(vaddr, 1, false)
                .expect("split 2 MiB PTE not found")
                .is_leaf()
        );

        drop(root);
        free_virtual_address_space(asid);
    }
}
