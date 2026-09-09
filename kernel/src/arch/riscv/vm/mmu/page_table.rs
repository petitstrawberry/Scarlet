use core::arch::asm;
use core::result::Result;

use crate::arch::riscv::vm::synchronize_tlb;
use crate::arch::vm::new_raw_pagetable;
use crate::environment::PAGE_SIZE;
use crate::vm::addr::{kernel_virt_to_phys, phys_to_virt};
use crate::vm::vmem::MemoryAttribute;
use crate::vm::vmem::VirtualMemoryMap;
use crate::vm::vmem::VirtualMemoryPermission;

use super::{
    ASID_BITS, INDEX_BITS, MAX_PAGING_LEVEL, PPN_BITS, SATP_MODE, SATP_MODE_SHIFT, SATP_PPN_BITS,
    TABLE_ENTRIES, is_canonical,
};

/// Attributes applied to a leaf page-table entry.
#[derive(Clone, Copy)]
struct MapAttrs {
    permissions: usize,
    accessed: bool,
    dirty: bool,
}

fn assert_canonical(vaddr: usize) {
    assert!(
        is_canonical(vaddr),
        "Non-canonical virtual address: {vaddr:#x}"
    );
}

/// Returns the page size represented by a page-table level.
///
/// Level 0 is 4 KiB. Each further level contributes INDEX_BITS address bits:
/// Sv32 has a 4 MiB leaf, while Sv48 also has 2 MiB, 1 GiB and 512 GiB leaves.
fn page_size_for_level(level: usize) -> usize {
    1usize << (12 + INDEX_BITS * level)
}

/// Chooses the largest page-table level usable for a mapping chunk.
///
/// The selected level must fit in the remaining size and both virtual and
/// physical addresses must be aligned to that level's page size.
fn best_page_level(vaddr: usize, paddr: u64, size: usize) -> usize {
    for level in (1..=MAX_PAGING_LEVEL).rev() {
        let page_size = page_size_for_level(level);
        if size >= page_size
            && vaddr.is_multiple_of(page_size)
            && paddr.is_multiple_of(page_size as u64)
        {
            return level;
        }
    }
    0
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct PageTableEntry {
    pub entry: usize,
}

impl PageTableEntry {
    pub const fn new() -> Self {
        PageTableEntry { entry: 0 }
    }

    pub fn get_ppn(&self) -> u64 {
        ((self.entry >> 10) as u64) & ((1u64 << PPN_BITS) - 1)
    }

    pub fn get_flags(&self) -> u64 {
        (self.entry & 0x3ff) as u64
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
        let mask = (1u64 << (INDEX_BITS * level)) - 1;
        self.get_ppn() & mask == 0
    }

    pub fn validate(&mut self) {
        self.entry |= 1;
    }

    pub fn invalidate(&mut self) {
        self.entry &= !1;
    }

    pub fn set_ppn(&mut self, ppn: u64) -> &mut Self {
        let ppn_mask = (1u64 << PPN_BITS) - 1;
        assert!(
            ppn <= ppn_mask,
            "physical page number exceeds the selected paging mode"
        );

        self.entry &= !((ppn_mask as usize) << 10); // Clear the PPN bits in the entry
        self.entry |= (ppn as usize) << 10; // Set the new PPN bits
        self
    }

    pub fn set_flags(&mut self, flags: u64) -> &mut Self {
        let mask = 0x3ff;
        self.entry |= (flags & mask) as usize;
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
    pub entries: [PageTableEntry; TABLE_ENTRIES],
}

impl PageTable {
    /// Create a new page table with all entries initialized to zero
    pub(in crate::arch::riscv::vm) fn new() -> Self {
        PageTable {
            entries: [PageTableEntry::new(); TABLE_ENTRIES],
        }
    }

    pub(in crate::arch::riscv::vm) fn switch(&self, asid: u16) {
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
    /// The PPN, ASID and MODE fields follow the selected paging geometry.
    pub(in crate::arch::riscv::vm) fn get_val_for_satp(&self, asid: u16) -> usize {
        let asid = asid as usize;
        assert!(asid < (1usize << ASID_BITS));
        let ppn = kernel_virt_to_phys(self as *const _ as usize) >> 12;
        assert!(ppn < (1u64 << SATP_PPN_BITS));
        SATP_MODE << SATP_MODE_SHIFT | asid << SATP_PPN_BITS | ppn as usize
    }

    pub(in crate::arch::riscv::vm) fn map_memory_area(
        &mut self,
        asid: u16,
        mmap: VirtualMemoryMap,
        accessed: bool,
        dirty: bool,
    ) -> Result<(), &'static str> {
        // Check if the address and size is aligned to PAGE_SIZE
        if !mmap.vmarea.start.is_multiple_of(PAGE_SIZE)
            || !mmap.pmarea.start.is_multiple_of(PAGE_SIZE as u64)
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
        let mut changed = false;
        while vaddr <= mmap.vmarea.end {
            // MemoryArea uses an inclusive end. Overflow here means the range
            // would require more than usize::MAX bytes and cannot be mapped.
            let Some(remaining) = mmap
                .vmarea
                .end
                .checked_sub(vaddr)
                .and_then(|remaining| remaining.checked_add(1))
            else {
                if changed {
                    synchronize_tlb(asid);
                }
                return Err("Address range overflow");
            };
            let mut level = best_page_level(vaddr, paddr, remaining);
            loop {
                if self
                    .try_map_at_level(asid, vaddr, paddr, attrs, level)
                    .is_ok()
                {
                    changed = true;
                    break;
                }
                if level == 0 {
                    if changed {
                        synchronize_tlb(asid);
                    }
                    return Err("Failed to map memory area");
                }
                level -= 1;
            }

            let page_size = page_size_for_level(level);
            match vaddr.checked_add(page_size) {
                Some(addr) => vaddr = addr,
                None => break,
            }
            match paddr.checked_add(page_size as u64) {
                Some(addr) => paddr = addr,
                None => break,
            }
        }

        if changed {
            synchronize_tlb(asid);
        }

        Ok(())
    }

    /// Validates an existing range after a direct-map attribute change.
    ///
    /// This implementation does not encode Scarlet's memory attributes in stage-1 leaves, so
    /// the live mapping only needs to remain physically consistent and have its
    /// translations synchronized after the metadata transition.
    pub(in crate::arch::riscv::vm) fn retag_memory_area(
        &mut self,
        asid: u16,
        mmap: VirtualMemoryMap,
    ) -> Result<(), &'static str> {
        if mmap.vmarea.start % PAGE_SIZE != 0
            || mmap.pmarea.start % PAGE_SIZE as u64 != 0
            || mmap.vmarea.size() % PAGE_SIZE != 0
            || mmap.pmarea.size() % PAGE_SIZE != 0
            || mmap.vmarea.size() != mmap.pmarea.size()
        {
            return Err("retag memory area is not page-aligned");
        }

        let mut vaddr = mmap.vmarea.start;
        let mut paddr = mmap.pmarea.start;
        while vaddr <= mmap.vmarea.end {
            let (_, level) = self
                .walk_leaf(vaddr)
                .ok_or("retag memory area has no existing leaf mapping")?;
            if self.translate(vaddr) != Some(paddr) {
                return Err("retag memory area does not match the existing physical mapping");
            }

            let leaf_size = page_size_for_level(level);
            let leaf_start = vaddr & !(leaf_size - 1);
            let leaf_end = leaf_start
                .checked_add(leaf_size - 1)
                .ok_or("retag leaf range overflows")?;
            if leaf_end >= mmap.vmarea.end {
                break;
            }
            let step = leaf_end
                .checked_sub(vaddr)
                .and_then(|size| size.checked_add(1))
                .ok_or("retag range step overflows")?;
            vaddr = leaf_end
                .checked_add(1)
                .ok_or("retag virtual range overflows")?;
            paddr = paddr
                .checked_add(step as u64)
                .ok_or("retag physical range overflows")?;
        }

        synchronize_tlb(asid);
        Ok(())
    }

    /// Maps a single 4 KiB page in the root page table.
    ///
    /// # Arguments
    ///
    /// * `asid` - Address-space identifier for the page-table hierarchy.
    /// * `vaddr` - Virtual address to map.
    /// * `paddr` - Physical address to map.
    /// * `permissions` - Requested virtual-memory permissions.
    /// * `_memory_attribute` - Requested cacheability or device attribute; This implementation does not encode it.
    /// * `accessed` - Whether to set the accessed bit.
    /// * `dirty` - Whether to set the dirty bit.
    pub(in crate::arch::riscv::vm) fn map(
        &mut self,
        asid: u16,
        vaddr: usize,
        paddr: u64,
        permissions: usize,
        _memory_attribute: MemoryAttribute,
        accessed: bool,
        dirty: bool,
    ) {
        // Check if the virtual address is properly canonicalized for the paging mode
        assert_canonical(vaddr);

        let vaddr = vaddr & !(PAGE_SIZE - 1); // Page align
        let paddr = paddr & !(PAGE_SIZE as u64 - 1);

        let attrs = MapAttrs {
            permissions,
            accessed,
            dirty,
        };
        self.try_map_at_level(asid, vaddr, paddr, attrs, 0)
            .expect("map: couldn't install a 4 KiB leaf mapping");
        synchronize_tlb(asid);
    }

    /// Attempts to install a leaf mapping at the specified page-table level.
    ///
    /// The mapping must be aligned to the target level's page size and cannot
    /// replace an existing non-leaf page-table entry.
    fn try_map_at_level(
        &mut self,
        asid: u16,
        vaddr: usize,
        paddr: u64,
        attrs: MapAttrs,
        level: usize,
    ) -> Result<(), &'static str> {
        let page_size = page_size_for_level(level);
        // This also protects direct callers such as map(), not only the
        // map_memory_area() path that preselects a compatible level.
        if !vaddr.is_multiple_of(page_size) || !paddr.is_multiple_of(page_size as u64) {
            return Err("Address is not aligned to page size");
        }

        let pte = self
            .walk_to_level(vaddr, level, true, asid)
            .ok_or("walk failed")?;
        if pte.is_valid() && !pte.is_leaf() {
            return Err("Cannot replace existing page table with a leaf");
        }
        // Allow remapping - just update the existing entry
        let ppn = paddr >> 12;

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
        // sfence.vma deferred to caller for batching.
        Ok(())
    }

    // Find the address of the PTE in page table that corresponds to virtual address vaddr.
    // If alloc == true, create any required page-table pages.
    // Returns None if walk() couldn't allocate a needed page-table page.
    //
    // Geometry selects Sv32 (2 x 10-bit indices) or Sv48 (4 x 9-bit indices).
    pub(in crate::arch::riscv::vm) fn walk(
        &mut self,
        vaddr: usize,
        alloc: bool,
        asid: u16,
    ) -> Option<&mut PageTableEntry> {
        self.walk_to_level(vaddr, 0, alloc, asid)
    }

    /// Walks to the PTE at `target_level` for `vaddr`.
    ///
    /// Intermediate page tables are allocated when `alloc` is true. Existing
    /// leaf entries above `target_level` stop the walk to avoid splitting or
    /// overwriting a huge-page mapping implicitly.
    pub(in crate::arch::riscv::vm) fn walk_to_level(
        &mut self,
        vaddr: usize,
        target_level: usize,
        alloc: bool,
        asid: u16,
    ) -> Option<&mut PageTableEntry> {
        let mut pagetable = self as *mut PageTable;

        // Check if virtual address is within valid canonical range for the paging mode
        if !is_canonical(vaddr) {
            return None;
        }

        unsafe {
            for level in ((target_level + 1)..=MAX_PAGING_LEVEL).rev() {
                let vpn = (vaddr >> (12 + INDEX_BITS * level)) & (TABLE_ENTRIES - 1);
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

            let vpn = (vaddr >> (12 + INDEX_BITS * target_level)) & (TABLE_ENTRIES - 1);
            Some(&mut (*pagetable).entries[vpn])
        }
    }

    /// Finds the leaf PTE that translates `vaddr`.
    ///
    /// The returned level is used to calculate the offset within a huge page.
    fn walk_leaf(&mut self, vaddr: usize) -> Option<(&mut PageTableEntry, usize)> {
        let mut pagetable = self as *mut PageTable;

        if !is_canonical(vaddr) {
            return None;
        }

        unsafe {
            for level in (0..=MAX_PAGING_LEVEL).rev() {
                let vpn = (vaddr >> (12 + INDEX_BITS * level)) & (TABLE_ENTRIES - 1);
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
    pub(in crate::arch::riscv::vm) fn translate(&mut self, vaddr: usize) -> Option<u64> {
        let (pte, level) = self.walk_leaf(vaddr)?;
        let page_offset = vaddr & (page_size_for_level(level) - 1);
        Some((pte.get_ppn() << 12) | page_offset as u64)
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
                child_pte.set_ppn(leaf_ppn + (idx * child_ppn_step) as u64);
            }

            pte.clear_all();
            pte.set_ppn(kernel_virt_to_phys(child_table as usize) >> 12);
            pte.validate();
        }

        Ok(())
    }

    fn unmap(&mut self, vaddr: usize) -> bool {
        // Check if the virtual address is properly canonicalized for the paging mode
        assert_canonical(vaddr);

        let vaddr = vaddr & !(PAGE_SIZE - 1); // Page align

        if let Some((pte, _)) = self.walk_leaf(vaddr) {
            pte.clear_all();
            true
        } else {
            false
        }
    }

    /// Unmap a virtual address range.
    ///
    /// Whole huge-page leaves are cleared directly. If the range only covers
    /// part of a huge-page leaf, the leaf is split into the next lower level so
    /// mappings outside the requested range are preserved.
    pub(in crate::arch::riscv::vm) fn unmap_range(
        &mut self,
        asid: u16,
        vaddr_start: usize,
        vaddr_end: usize,
    ) {
        if vaddr_start > vaddr_end {
            return;
        }

        assert_canonical(vaddr_start);
        assert_canonical(vaddr_end);

        let mut vaddr = vaddr_start & !(PAGE_SIZE - 1);
        let mut changed = false;
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
                changed |= self.unmap(leaf_start);
                match leaf_end.checked_add(1) {
                    Some(next) => vaddr = next,
                    None => break,
                }
            } else if level == 0 {
                changed |= self.unmap(vaddr);
                match vaddr.checked_add(PAGE_SIZE) {
                    Some(next) => vaddr = next,
                    None => break,
                }
            } else {
                self.split_leaf(asid, vaddr, level)
                    .expect("unmap_range: failed to split huge-page leaf");
                changed = true;
            }
        }
        if changed {
            synchronize_tlb(asid);
        }
    }

    pub(in crate::arch::riscv::vm) fn unmap_all(&mut self, asid: u16) {
        self.unmap_all_no_flush();
        synchronize_tlb(asid);
    }

    /// Clear all root-level entries without issuing a TLB shootdown.
    ///
    /// Intended for batched page-table rebuilds (e.g. `exec`). The caller
    /// **must** call [`synchronize_tlb`] or equivalent before the affected
    /// address space becomes visible to any hart.
    pub(in crate::arch::riscv::vm) fn unmap_all_no_flush(&mut self) {
        for i in 0..TABLE_ENTRIES {
            let entry = &mut self.entries[i];
            entry.clear_all();
        }
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
    fn test_pte_preserves_the_complete_physical_page_number() {
        let mut entry = PageTableEntry::new();
        let ppn = (1u64 << PPN_BITS) - 1;
        entry.set_ppn(ppn).readable().accessed();
        entry.validate();
        assert_eq!(entry.get_ppn(), ppn);
        assert!(entry.is_valid() && entry.is_leaf());
        assert_eq!(
            core::mem::size_of::<PageTableEntry>(),
            core::mem::size_of::<usize>()
        );
        assert_eq!(core::mem::size_of::<PageTable>(), PAGE_SIZE);
        // Sv32's 22-bit PPN represents physical addresses wider than XLEN.
        assert_eq!(
            (entry.get_ppn() as u64) << 12,
            ((1u64 << PPN_BITS) - 1) << 12
        );
    }

    #[test_case]
    fn test_map_memory_area_uses_native_huge_page() {
        let asid = alloc_virtual_address_space();
        let mut root =
            crate::arch::vm::get_root_pagetable(asid).expect("root page table not found");
        let page_size = page_size_for_level(1);
        let vaddr = 0x4000_0000;
        let paddr = 0x8000_0000;
        let mmap = VirtualMemoryMap::new(
            crate::vm::vmem::PhysicalMemoryArea::new(paddr, paddr + page_size as u64 - 1),
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
        let vaddr = 0x4000_0000 + huge_page_size;
        let paddr = 0x8000_0000 + huge_page_size as u64;
        let mmap = VirtualMemoryMap::new(
            crate::vm::vmem::PhysicalMemoryArea::new(paddr, paddr + map_size as u64 - 1),
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
        assert!(tail_pte.is_leaf());

        assert_eq!(root.translate(vaddr + 0x1234), Some(paddr + 0x1234));
        assert_eq!(
            root.translate(tail_vaddr + 0x123),
            Some(paddr + huge_page_size as u64 + 0x123)
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
            crate::vm::vmem::PhysicalMemoryArea::new(paddr, paddr + huge_page_size as u64 - 1),
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
            Some(paddr + 2 * PAGE_SIZE as u64)
        );
        assert!(
            root.walk_to_level(vaddr, 0, false)
                .expect("split 4 KiB PTE not found")
                .is_leaf()
        );

        drop(root);
        free_virtual_address_space(asid);
    }

    #[test_case]
    #[cfg(target_pointer_width = "64")]
    fn test_unmap_range_preserves_partial_1g_huge_page() {
        let asid = alloc_virtual_address_space();
        let mut root =
            crate::arch::vm::get_root_pagetable(asid).expect("root page table not found");
        let huge_page_size = page_size_for_level(2);
        let vaddr = 0x8000_0000;
        let paddr = 0x1_0000_0000;
        let mmap = VirtualMemoryMap::new(
            crate::vm::vmem::PhysicalMemoryArea::new(paddr, paddr + huge_page_size as u64 - 1),
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
            Some(paddr + page_size_for_level(1) as u64 + PAGE_SIZE as u64)
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
