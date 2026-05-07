use core::arch::asm;
use core::result::Result;

use crate::arch::vm::new_raw_pagetable;
use crate::environment::PAGE_SIZE;
use crate::vm::addr::{kernel_virt_to_phys, phys_to_virt};
use crate::vm::vmem::VirtualMemoryMap;
use crate::vm::vmem::VirtualMemoryPermission;

const MAX_PAGING_LEVEL: usize = 3;

/// Attributes applied to a leaf page-table entry.
#[derive(Clone, Copy)]
struct MapAttrs {
    permissions: usize,
    accessed: bool,
    dirty: bool,
}

/// Returns whether a virtual address is canonical for Sv48.
///
/// Sv48 requires bits 63:48 to be copies of bit 47: all zero for the lower
/// canonical range and all one for the upper canonical range.
fn is_canonical_sv48(vaddr: usize) -> bool {
    let canonical_check = (vaddr >> 47) & 1;
    let upper_bits = (vaddr >> 48) & 0xffff;
    (canonical_check == 1 && upper_bits == 0xffff) || (canonical_check == 0 && upper_bits == 0)
}

fn assert_canonical_sv48(vaddr: usize) {
    if !is_canonical_sv48(vaddr) {
        panic!("Non-canonical virtual address: {:#x}", vaddr);
    }
}

/// Returns the page size represented by a page-table level.
///
/// Level 0 is 4 KiB, level 1 is 2 MiB, level 2 is 1 GiB, and level 3 is
/// 512 GiB.
fn page_size_for_level(level: usize) -> usize {
    1usize << (12 + 9 * level)
}

/// Chooses the largest page-table level usable for a mapping chunk.
///
/// The selected level must fit in the remaining size and both virtual and
/// physical addresses must be aligned to that level's page size.
fn best_page_level(vaddr: usize, paddr: usize, size: usize) -> usize {
    for level in (1..=MAX_PAGING_LEVEL).rev() {
        let page_size = page_size_for_level(level);
        if size >= page_size && vaddr.is_multiple_of(page_size) && paddr.is_multiple_of(page_size) {
            return level;
        }
    }
    0
}

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

    /// Returns whether this PTE's PPN satisfies leaf alignment for a level.
    ///
    /// Huge-page leaves must have zero lower PPN fields for all lower page-table
    /// levels.
    pub fn is_aligned_for_level(&self, level: usize) -> bool {
        let mask = (1usize << (9 * level)) - 1;
        self.get_ppn() & mask == 0
    }

    pub fn validate(&mut self) {
        self.entry |= 1;
    }

    pub fn invalidate(&mut self) {
        self.entry &= !1;
    }

    pub fn set_ppn(&mut self, ppn: usize) -> &mut Self {
        let ppn_mask = 0x3ffffffffff; // Mask for the PPN bits
        let masked_ppn = (ppn as u64) & ppn_mask; // Mask the PPN to fit in the entry

        self.entry &= !(ppn_mask << 10); // Clear the PPN bits in the entry
        self.entry |= masked_ppn << 10; // Set the new PPN bits
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

    pub fn accessed(&mut self) -> &mut Self {
        self.entry |= 0x40;
        self
    }

    pub fn dirty(&mut self) -> &mut Self {
        self.entry |= 0x80;
        self
    }
}

impl Default for PageTableEntry {
    fn default() -> Self {
        Self::new()
    }
}

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

    pub fn switch(&self, asid: u16) {
        let satp = self.get_val_for_satp(asid);
        unsafe {
            asm!(
                "
                csrw satp, {0}
                sfence.vma zero, zero
                ",

                in(reg) satp,
            );
        }
    }

    /// Switch page table for boot-time initialization.
    /// On RISC-V, this just calls switch() (no TTBR1 equivalent).
    pub fn switch_for_boot(&self, asid: u16) {
        self.switch(asid);
    }

    /// Get the value for the satp register.
    ///
    /// # Note
    ///
    /// Only for RISC-V (Sv48).
    pub fn get_val_for_satp(&self, asid: u16) -> u64 {
        let asid = asid as usize;
        let mode = 9;
        let ppn = kernel_virt_to_phys(self as *const _ as usize) >> 12;
        (mode << 60 | asid << 44 | ppn) as u64
    }

    pub fn map_memory_area(
        &mut self,
        asid: u16,
        mmap: VirtualMemoryMap,
        accessed: bool,
        dirty: bool,
    ) -> Result<(), &'static str> {
        // Check if the address and size is aligned to PAGE_SIZE
        if !mmap.vmarea.start.is_multiple_of(PAGE_SIZE)
            || !mmap.pmarea.start.is_multiple_of(PAGE_SIZE)
            || !mmap.vmarea.size().is_multiple_of(PAGE_SIZE)
            || !mmap.pmarea.size().is_multiple_of(PAGE_SIZE)
        {
            return Err("Address is not aligned to PAGE_SIZE");
        }

        let attrs = MapAttrs {
            permissions: mmap.permissions,
            accessed,
            dirty,
        };
        let mut vaddr = mmap.vmarea.start;
        let mut paddr = mmap.pmarea.start;
        while vaddr <= mmap.vmarea.end {
            // MemoryArea uses an inclusive end. Overflow here means the range
            // would require more than usize::MAX bytes and cannot be mapped.
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

        Ok(())
    }

    /* Only for root page table */
    pub fn map(
        &mut self,
        asid: u16,
        vaddr: usize,
        paddr: usize,
        permissions: usize,
        accessed: bool,
        dirty: bool,
    ) {
        // Check if the virtual address is properly canonicalized for Sv48
        assert_canonical_sv48(vaddr);

        let vaddr = vaddr & 0xffff_ffff_ffff_f000; // Page align
        let paddr = paddr & 0xffff_ffff_ffff_f000;

        let attrs = MapAttrs {
            permissions,
            accessed,
            dirty,
        };
        self.try_map_at_level(asid, vaddr, paddr, attrs, 0)
            .expect("map: couldn't install a 4 KiB leaf mapping");
    }

    /// Attempts to install a leaf mapping at the specified page-table level.
    ///
    /// The mapping must be aligned to the target level's page size and cannot
    /// replace an existing non-leaf page-table entry.
    fn try_map_at_level(
        &mut self,
        asid: u16,
        vaddr: usize,
        paddr: usize,
        attrs: MapAttrs,
        level: usize,
    ) -> Result<(), &'static str> {
        let page_size = page_size_for_level(level);
        // This also protects direct callers such as map(), not only the
        // map_memory_area() path that preselects a compatible level.
        if !vaddr.is_multiple_of(page_size) || !paddr.is_multiple_of(page_size) {
            return Err("Address is not aligned to page size");
        }

        let pte = self
            .walk_to_level(vaddr, level, true, asid)
            .ok_or("walk failed")?;
        if pte.is_valid() && !pte.is_leaf() {
            return Err("Cannot replace existing page table with a leaf");
        }
        // Allow remapping - just update the existing entry
        let ppn = (paddr >> 12) & 0xfffffffffff;

        // Clear existing flags before setting new ones
        pte.clear_all();

        if VirtualMemoryPermission::Read.contained_in(attrs.permissions) {
            pte.readable();
        }
        if VirtualMemoryPermission::Write.contained_in(attrs.permissions) {
            // RISC-V: W=1 requires R=1 (reserved encoding otherwise).
            // Ensure readable so the leaf PTE is well-formed.
            pte.readable();
            pte.writable();
        }
        if VirtualMemoryPermission::Execute.contained_in(attrs.permissions) {
            pte.executable();
        }
        if VirtualMemoryPermission::User.contained_in(attrs.permissions) {
            pte.accesible_from_user();
        }
        if attrs.accessed {
            pte.accessed();
        }
        if attrs.dirty {
            pte.dirty();
        }

        pte.set_ppn(ppn);
        pte.validate();
        unsafe { asm!("sfence.vma zero,zero") };
        Ok(())
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
        self.walk_to_level(vaddr, 0, alloc, asid)
    }

    /// Walks to the PTE at `target_level` for `vaddr`.
    ///
    /// Intermediate page tables are allocated when `alloc` is true. Existing
    /// leaf entries above `target_level` stop the walk to avoid splitting or
    /// overwriting a huge-page mapping implicitly.
    fn walk_to_level(
        &mut self,
        vaddr: usize,
        target_level: usize,
        alloc: bool,
        asid: u16,
    ) -> Option<&mut PageTableEntry> {
        let mut pagetable = self as *mut PageTable;

        // Check if virtual address is within valid canonical range for Sv48
        if !is_canonical_sv48(vaddr) {
            return None;
        }

        unsafe {
            for level in ((target_level + 1)..=MAX_PAGING_LEVEL).rev() {
                let vpn = (vaddr >> (12 + 9 * level)) & 0x1ff;
                let pte = &mut (*pagetable).entries[vpn];

                if pte.is_valid() {
                    if pte.is_leaf() {
                        return None;
                    }
                    // If not a leaf, it's a pointer to the next level table.
                    pagetable = phys_to_virt(pte.get_ppn() << 12) as *mut PageTable;
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
                    pte.set_ppn(kernel_virt_to_phys(new_table as usize) >> 12);
                    pte.validate();
                    pagetable = new_table;
                }
            }

            let vpn = (vaddr >> (12 + 9 * target_level)) & 0x1ff;
            Some(&mut (*pagetable).entries[vpn])
        }
    }

    /// Finds the leaf PTE that translates `vaddr`.
    ///
    /// The returned level is used to calculate the offset within a huge page.
    fn walk_leaf(&mut self, vaddr: usize) -> Option<(&mut PageTableEntry, usize)> {
        let mut pagetable = self as *mut PageTable;

        if !is_canonical_sv48(vaddr) {
            return None;
        }

        unsafe {
            for level in (0..=MAX_PAGING_LEVEL).rev() {
                let vpn = (vaddr >> (12 + 9 * level)) & 0x1ff;
                let pte = &mut (*pagetable).entries[vpn];
                if !pte.is_valid() {
                    return None;
                }
                if pte.is_leaf() {
                    if !pte.is_aligned_for_level(level) {
                        return None;
                    }
                    return Some((pte, level));
                }
                if level == 0 {
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
    pub fn translate(&mut self, vaddr: usize) -> Option<usize> {
        let (pte, level) = self.walk_leaf(vaddr)?;
        let page_offset = vaddr & (page_size_for_level(level) - 1);
        Some((pte.get_ppn() << 12) | page_offset)
    }

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
                // Preserve the parent's flags, including A/D bits, because the
                // child entries represent the same already-established mapping.
                child_pte.entry = leaf_entry;
                child_pte.set_ppn(leaf_ppn + idx * child_ppn_step);
            }

            pte.clear_all();
            pte.set_ppn(kernel_virt_to_phys(child_table as usize) >> 12);
            pte.validate();
            asm!("sfence.vma zero,zero");
        }

        Ok(())
    }

    fn unmap(&mut self, vaddr: usize) {
        // Check if the virtual address is properly canonicalized for Sv48
        assert_canonical_sv48(vaddr);

        let vaddr = vaddr & 0xffff_ffff_ffff_f000; // Page align

        if let Some((pte, _)) = self.walk_leaf(vaddr) {
            pte.clear_all();
            unsafe { asm!("sfence.vma zero,zero") };
        }
    }

    /// Unmap a virtual address range.
    ///
    /// Whole huge-page leaves are cleared directly. If the range only covers
    /// part of a huge-page leaf, the leaf is split into the next lower level so
    /// mappings outside the requested range are preserved.
    pub fn unmap_range(&mut self, asid: u16, vaddr_start: usize, vaddr_end: usize) {
        if vaddr_start > vaddr_end {
            return;
        }

        assert_canonical_sv48(vaddr_start);
        assert_canonical_sv48(vaddr_end);

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

    pub fn unmap_all(&mut self) {
        for i in 0..512 {
            let entry = &mut self.entries[i];
            entry.clear_all();
        }
        // Ensure the TLB flush instruction is not optimized away.
        unsafe { asm!("sfence.vma zero,zero") };
    }
}

impl Default for PageTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch::vm::{alloc_virtual_address_space, free_virtual_address_space};
    use crate::vm::vmem::MemoryArea;

    #[test_case]
    fn test_map_memory_area_uses_2m_huge_page() {
        let asid = alloc_virtual_address_space();
        let root = crate::arch::vm::get_root_pagetable(asid).expect("root page table not found");
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

        root.map_memory_area(asid, mmap, true, true)
            .expect("huge-page mapping failed");

        let pte = root
            .walk_to_level(vaddr, 1, false, asid)
            .expect("huge-page PTE not found");
        assert!(pte.is_leaf());
        assert!(pte.is_aligned_for_level(1));
        assert_eq!(root.translate(vaddr + 0x1234), Some(paddr + 0x1234));

        free_virtual_address_space(asid);
    }

    #[test_case]
    fn test_map_memory_area_uses_huge_page_with_4k_tail() {
        let asid = alloc_virtual_address_space();
        let root = crate::arch::vm::get_root_pagetable(asid).expect("root page table not found");
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

        root.map_memory_area(asid, mmap, true, true)
            .expect("mixed huge-page mapping failed");

        let huge_pte = root
            .walk_to_level(vaddr, 1, false, asid)
            .expect("huge-page PTE not found");
        assert!(huge_pte.is_leaf());
        assert!(huge_pte.is_aligned_for_level(1));

        let tail_vaddr = vaddr + huge_page_size;
        let tail_pte = root
            .walk_to_level(tail_vaddr, 0, false, asid)
            .expect("tail 4 KiB PTE not found");
        assert!(tail_pte.is_leaf());

        assert_eq!(root.translate(vaddr + 0x1234), Some(paddr + 0x1234));
        assert_eq!(
            root.translate(tail_vaddr + 0x123),
            Some(paddr + huge_page_size + 0x123)
        );

        free_virtual_address_space(asid);
    }

    #[test_case]
    fn test_unmap_range_preserves_partial_huge_page() {
        let asid = alloc_virtual_address_space();
        let root = crate::arch::vm::get_root_pagetable(asid).expect("root page table not found");
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

        root.map_memory_area(asid, mmap, true, true)
            .expect("huge-page mapping failed");
        assert!(
            root.walk_to_level(vaddr, 1, false, asid)
                .expect("huge-page PTE not found")
                .is_leaf()
        );

        root.unmap_range(asid, vaddr + PAGE_SIZE, vaddr + 2 * PAGE_SIZE - 1);

        assert_eq!(root.translate(vaddr), Some(paddr));
        assert_eq!(root.translate(vaddr + PAGE_SIZE), None);
        assert_eq!(
            root.translate(vaddr + 2 * PAGE_SIZE),
            Some(paddr + 2 * PAGE_SIZE)
        );
        assert!(
            root.walk_to_level(vaddr, 0, false, asid)
                .expect("split 4 KiB PTE not found")
                .is_leaf()
        );

        free_virtual_address_space(asid);
    }

    #[test_case]
    fn test_unmap_range_preserves_partial_1g_huge_page() {
        let asid = alloc_virtual_address_space();
        let root = crate::arch::vm::get_root_pagetable(asid).expect("root page table not found");
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

        root.map_memory_area(asid, mmap, true, true)
            .expect("1 GiB huge-page mapping failed");
        assert!(
            root.walk_to_level(vaddr, 2, false, asid)
                .expect("1 GiB huge-page PTE not found")
                .is_leaf()
        );

        let removed_vaddr = vaddr + page_size_for_level(1);
        root.unmap_range(asid, removed_vaddr, removed_vaddr + PAGE_SIZE - 1);

        assert_eq!(root.translate(vaddr), Some(paddr));
        assert_eq!(root.translate(removed_vaddr), None);
        assert_eq!(
            root.translate(removed_vaddr + PAGE_SIZE),
            Some(paddr + page_size_for_level(1) + PAGE_SIZE)
        );
        assert!(
            root.walk_to_level(vaddr, 1, false, asid)
                .expect("split 2 MiB PTE not found")
                .is_leaf()
        );

        free_virtual_address_space(asid);
    }
}
