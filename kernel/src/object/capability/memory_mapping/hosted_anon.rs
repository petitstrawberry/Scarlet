//! Shared anonymous backing uses native-sized groups. Group placement follows
//! the first mapping's guest offset, not owner-relative page zero. Aliases with
//! different offsets remain valid but may require checked worker publication.
use super::*;

struct Resident {
    pages: BTreeMap<usize, u64>,
    groups: BTreeMap<usize, ContiguousPages>,
    bias: Option<usize>,
    generation: u64,
}

impl Resident {
    fn changed(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("Anonymous publication generation exhausted");
    }
}

pub struct AnonymousPageOwner {
    resident: IrqRwSpinLock<Resident>,
}

impl AnonymousPageOwner {
    pub fn new() -> Self {
        Self {
            resident: IrqRwSpinLock::new(Resident {
                pages: BTreeMap::new(),
                groups: BTreeMap::new(),
                bias: None,
                generation: 0,
            }),
        }
    }
}

impl MemoryMappingOps for AnonymousPageOwner {
    fn supports_private_relocation(&self) -> bool {
        true
    }
    fn supports_permission_changes(&self) -> bool {
        true
    }
    fn supports_mmap(&self) -> bool {
        false
    }
    fn mmap_owner_name(&self) -> String {
        String::from("anonymous")
    }
    fn get_mapping_info(
        &self,
        _: usize,
        _: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("AnonymousPageOwner does not support get_mapping_info")
    }
    fn publication_generation(&self) -> Option<u64> {
        Some(self.resident.read().generation)
    }
    fn is_page_resident(&self, page: usize) -> Option<bool> {
        Some(self.resident.read().pages.contains_key(&page))
    }
    fn stable_physical_extent(&self, start: usize, count: usize) -> Option<u64> {
        if count == 0 {
            return None;
        }
        let end = start.checked_add(count)?;
        let resident = self.resident.read();
        let first = *resident.pages.get(&start)?;
        for index in start..end {
            if resident.pages.get(&index).copied()
                != Some(first + ((index - start) * PAGE_SIZE) as u64)
            {
                return None;
            }
        }
        Some(first)
    }
    fn visit_resident_extents(
        &self,
        start: usize,
        count: usize,
        visit: &mut dyn FnMut(usize, usize, u64),
    ) {
        let resident = self.resident.read();
        let mut span: Option<(usize, usize, u64)> = None;
        for (&index, &pa) in resident.pages.range(start..start.saturating_add(count)) {
            if let Some((base, length, first)) = span.as_mut()
                && index == *base + *length
                && pa == *first + (*length * PAGE_SIZE) as u64
            {
                *length += 1;
            } else {
                if let Some((base, length, first)) = span {
                    visit(base, length, first);
                }
                span = Some((index, 1, pa));
            }
        }
        if let Some((base, length, first)) = span {
            visit(base, length, first);
        }
    }
    fn resolve_fault(
        &self,
        _: &AccessKind,
        page: usize,
        vm_start: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        let mut resident = self.resident.write();
        if let Some(&pa) = resident.pages.get(&page) {
            return Ok(ResolveFaultResult {
                paddr_page_base: pa,
                is_tail: false,
            });
        }
        let n = crate::vm::execution_page_size() / PAGE_SIZE;
        let bias = *resident.bias.get_or_insert(vm_start / PAGE_SIZE % n);
        let absolute = page.checked_add(bias).ok_or(ResolveFaultError::Invalid)?;
        let aligned = absolute / n * n;
        let base = aligned.saturating_sub(bias);
        let count = if aligned == 0 { n - bias } else { n };
        if let Some(group) = resident.groups.get(&base) {
            // Re-faulting a released subpage must not expose its previous data.
            let pa = group.as_paddr() + ((page - base) * PAGE_SIZE) as u64;
            unsafe {
                core::ptr::write_bytes(phys_to_virt(pa) as *mut u8, 0, PAGE_SIZE);
            }
            resident.pages.insert(page, pa);
        } else {
            let address = base
                .checked_add(bias)
                .and_then(|p| p.checked_mul(PAGE_SIZE))
                .ok_or(ResolveFaultError::Invalid)?;
            let group = ContiguousPages::new_for_user_range(count, address)
                .ok_or(ResolveFaultError::NoMemory)?;
            for i in 0..count {
                resident
                    .pages
                    .insert(base + i, group.as_paddr() + (i * PAGE_SIZE) as u64);
            }
            resident.groups.insert(base, group);
        }
        resident.changed();
        Ok(ResolveFaultResult {
            paddr_page_base: resident.pages[&page],
            is_tail: false,
        })
    }
    fn release_pages(&self, start: usize, count: usize) {
        let mut resident = self.resident.write();
        let end = start.saturating_add(count);
        let old_len = resident.pages.len();
        resident
            .pages
            .retain(|&index, _| index < start || index >= end);
        if resident.pages.len() != old_len {
            resident.changed();
            let Resident { pages, groups, .. } = &mut *resident;
            groups.retain(|&base, group| pages.range(base..base + group.len()).next().is_some());
        }
    }
    fn fork_clone(&self) -> Option<Arc<dyn MemoryMappingOps>> {
        fork_clone_owner(self)
    }
}

pub fn fork_clone_owner(owner: &AnonymousPageOwner) -> Option<Arc<dyn MemoryMappingOps>> {
    let source = owner.resident.read();
    let mut cloned = Resident {
        pages: BTreeMap::new(),
        groups: BTreeMap::new(),
        bias: source.bias,
        generation: 0,
    };
    for (&base, group) in &source.groups {
        let address = base
            .checked_add(source.bias.unwrap_or(0))?
            .checked_mul(PAGE_SIZE)?;
        let copy = ContiguousPages::new_for_user_range(group.len(), address)?;
        for (&index, &pa) in source.pages.range(base..base + group.len()) {
            let destination = copy.as_paddr() + ((index - base) * PAGE_SIZE) as u64;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    phys_to_virt(pa) as *const u8,
                    phys_to_virt(destination) as *mut u8,
                    PAGE_SIZE,
                );
            }
            cloned.pages.insert(index, destination);
        }
        cloned.groups.insert(base, copy);
    }
    Some(Arc::new(AnonymousPageOwner {
        resident: IrqRwSpinLock::new(cloned),
    }))
}
