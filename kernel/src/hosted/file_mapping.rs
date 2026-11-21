//! Demand-backed private file views for Darwin's native page granule.
use crate::{
    environment::PAGE_SIZE,
    fs::FileType,
    mem::page::ContiguousPages,
    object::capability::{
        FileObject, MemoryMappingInfo, MemoryMappingOps,
        memory_mapping::{
            AccessKind, AccessOp, MappingError, MappingRequest, PreparedMapping, ResolveFaultError,
            ResolveFaultResult,
        },
    },
    sync::IrqRwSpinLock,
    task::elf_loader::{ElfHeader, hosted::visit_instructions},
    vm::execution_page_size,
};
use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use core::ops::Range;
use core::sync::atomic::{AtomicUsize, Ordering};

pub(crate) fn prepare(
    file: &Arc<dyn FileObject>,
    request: MappingRequest,
    mapping: &mut PreparedMapping,
) -> Result<(), MappingError> {
    if !request.private_demand || mapping.owner.is_some() || mapping.info.is_some() {
        return Ok(());
    }
    if !file
        .metadata()
        .is_ok_and(|meta| matches!(meta.file_type, FileType::RegularFile))
    {
        return Ok(());
    }
    let code = crate::vm::vmem::VirtualMemoryPermission::Execute
        .contained_in(mapping.maximum_permissions)
        .then(|| read_header(file.as_ref()))
        .flatten()
        .filter(|h| h.e_machine == 183 && h.ei_class == 2 && h.ei_data == 1 && h.e_shnum != 0)
        .and_then(|h| {
            let mut ranges = Vec::new();
            let start = request.offset as u64;
            let end = start.checked_add(mapping.length as u64)?;
            visit_instructions(&h, file.as_ref(), |_, offset, size| {
                let lo = offset.max(start);
                let hi = offset.saturating_add(size as u64).min(end);
                if lo < hi {
                    ranges.push((lo - start) as usize..(hi - start) as usize);
                }
                Ok(())
            })
            .ok()?;
            Some(Arc::from(ranges))
        });
    mapping.owner = Some(Arc::new(PrivateFile {
        file: file.clone(),
        offset: request.offset,
        length: mapping.length,
        phase: AtomicUsize::new(usize::MAX),
        groups: IrqRwSpinLock::new(BTreeMap::new()),
        code,
    }));
    Ok(())
}

fn read_header(file: &dyn FileObject) -> Option<ElfHeader> {
    let mut header = [0u8; 64];
    let mut read = 0;
    while read < header.len() {
        let count = file.read_at(read as u64, &mut header[read..]).ok()?;
        if count == 0 || count > header.len() - read {
            return None;
        }
        read += count;
    }
    ElfHeader::parse(&header).ok()
}

struct PrivateFile {
    file: Arc<dyn FileObject>,
    offset: usize,
    length: usize,
    // Set on the first fault. Fork/relocation may share this owner: never
    // regroup already published immutable pages when a later VA differs.
    phase: AtomicUsize,
    groups: IrqRwSpinLock<BTreeMap<usize, ContiguousPages>>,
    code: Option<Arc<[Range<usize>]>>,
}

impl PrivateFile {
    fn group_layout(page: usize, phase: usize, n: usize) -> (usize, usize) {
        let aligned = (page + phase) / n * n;
        (aligned.saturating_sub(phase), phase.saturating_sub(aligned))
    }
}

impl MemoryMappingOps for PrivateFile {
    fn publication_generation(&self) -> Option<u64> {
        // Groups are only inserted, never replaced or removed. Permissions and
        // instruction intervals are immutable for this owner, even after fork.
        Some(self.groups.read().len() as u64)
    }
    fn supports_permission_changes(&self) -> bool {
        true
    }
    fn supports_private_relocation(&self) -> bool {
        true
    }
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("Private file pages are demand backed")
    }
    fn instruction_ranges(&self) -> Option<Arc<[Range<usize>]>> {
        self.code.clone()
    }

    fn resolve_fault(
        &self,
        _: &AccessKind,
        page: usize,
        vm_start: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        let granule = execution_page_size();
        let n = granule / PAGE_SIZE;
        page.checked_mul(PAGE_SIZE)
            .filter(|&v| v < self.length)
            .ok_or(ResolveFaultError::Unmapped)?;
        // mmap's original VA, not file offset or relative page zero, fixes the
        // physical phase. Otherwise every interior host page of a 4K-offset
        // VMA straddles allocations and must trap on every read.
        let requested_phase = vm_start % granule / PAGE_SIZE;
        let phase = match self.phase.compare_exchange(
            usize::MAX,
            requested_phase,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => requested_phase,
            Err(phase) => phase,
        };
        let (base, prefix) = Self::group_layout(page, phase, n);
        let delta = (prefix + page - base) * PAGE_SIZE;
        let result = |pa| ResolveFaultResult {
            paddr_page_base: pa + delta as u64,
            is_tail: false,
        };
        if let Some(group) = self.groups.read().get(&base) {
            return Ok(result(group.as_paddr()));
        }
        let pos = self
            .offset
            .checked_add(base * PAGE_SIZE)
            .ok_or(ResolveFaultError::Invalid)?;
        let file_size = self
            .file
            .metadata()
            .map_err(|_| ResolveFaultError::Invalid)?
            .size;
        if pos >= file_size {
            return Err(ResolveFaultError::Unmapped);
        }
        let pages = ContiguousPages::new_aligned(n, granule).ok_or(ResolveFaultError::NoMemory)?;
        let limit = (granule - prefix * PAGE_SIZE)
            .min(file_size - pos)
            .min(self.length - base * PAGE_SIZE);
        // The allocation is fresh and zero filled, including a short final page.
        let dest = unsafe {
            core::slice::from_raw_parts_mut(
                pages.as_ptr().cast::<u8>().add(prefix * PAGE_SIZE),
                limit,
            )
        };
        let mut read = 0;
        while read < limit {
            let bytes = self
                .file
                .read_at((pos + read) as u64, &mut dest[read..])
                .map_err(|_| ResolveFaultError::Invalid)?;
            if bytes == 0 || bytes > limit - read {
                return Err(ResolveFaultError::Invalid);
            }
            read += bytes;
        }
        let mut groups = self.groups.write();
        Ok(result(groups.entry(base).or_insert(pages).as_paddr()))
    }

    fn stable_physical_extent(&self, page: usize, count: usize) -> Option<u64> {
        let n = execution_page_size() / PAGE_SIZE;
        let phase = self.phase.load(Ordering::Acquire);
        if phase == usize::MAX
            || count == 0
            || page.checked_add(count)?.checked_mul(PAGE_SIZE)? > self.length
        {
            return None;
        }
        let (base, prefix) = Self::group_layout(page, phase, n);
        if count > n - prefix - (page - base) {
            return None;
        }
        self.groups
            .read()
            .get(&base)
            .map(|p| p.as_paddr() + ((prefix + page - base) * PAGE_SIZE) as u64)
    }

    fn visit_resident_extents(
        &self,
        start: usize,
        count: usize,
        visit: &mut dyn FnMut(usize, usize, u64),
    ) {
        let n = execution_page_size() / PAGE_SIZE;
        let phase = self.phase.load(Ordering::Acquire);
        if phase == usize::MAX || count == 0 {
            return;
        }
        let end = start.saturating_add(count).min(self.length / PAGE_SIZE);
        if start >= end {
            return;
        }
        let (first, _) = Self::group_layout(start, phase, n);
        for (&base, group) in self.groups.read().range(first..end) {
            let (_, prefix) = Self::group_layout(base, phase, n);
            let lo = start.max(base);
            let hi = end.min(base + n - prefix);
            visit(
                lo,
                hi - lo,
                group.as_paddr() + ((prefix + lo - base) * PAGE_SIZE) as u64,
            );
        }
    }
    fn private_fault_requires_copy(&self, access: &AccessKind) -> bool {
        access.op == AccessOp::Store
    }
    fn fault_page_permissions(&self, access: &AccessKind, permissions: usize) -> usize {
        if access.op == AccessOp::Store {
            permissions
        } else {
            permissions & !2
        }
    }
}
