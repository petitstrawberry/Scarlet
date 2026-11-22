//! Hosted shared-memory objects with stable logical frames and kernel aliases.

use super::{SharedMemoryBacking, SharedMemoryObject};
use crate::{
    environment::PAGE_SIZE,
    hosted::{SHARED_OBJECT_LIMIT, SharedBacking},
    object::capability::memory_mapping::{
        AccessKind, MappingError, MappingRequest, MemoryMappingInfo, MemoryMappingOps,
        PreparedMapping, ResolveFaultError, ResolveFaultResult,
    },
    sync::{IrqRwSpinLock, Mutex},
};
use alloc::{format, string::String, sync::Arc};

pub struct SharedMemory {
    inner: Arc<Inner>,
}

struct Inner {
    base: u64,
    permissions: usize,
    backing: Option<Arc<SharedBacking>>,
    limit: usize,
    state: IrqRwSpinLock<State>,
    resize: Mutex<()>,
}

struct State {
    publication_generation: u64,
    size: usize,
    valid: bool,
    pins: usize,
}

impl SharedMemory {
    pub fn new(size: usize, permissions: usize) -> Result<Self, &'static str> {
        let size = size
            .checked_next_multiple_of(PAGE_SIZE)
            .filter(|&size| size != 0)
            .ok_or("Invalid hosted shared memory size")?;
        let backing = SharedBacking::new(size)?;
        Ok(Self::with_backing(
            backing.paddr(),
            size,
            permissions,
            SHARED_OBJECT_LIMIT,
            Some(backing),
        ))
    }

    /// Wrap frames owned by another kernel object.
    ///
    /// # Safety
    /// The caller must keep the entire range valid until the last handle and
    /// mapping owner has been released.
    pub unsafe fn from_paddr(paddr: u64, size: usize, permissions: usize) -> Self {
        Self::with_backing(paddr, size, permissions, size, None)
    }

    fn with_backing(
        base: u64,
        size: usize,
        permissions: usize,
        limit: usize,
        backing: Option<Arc<SharedBacking>>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                base,
                permissions,
                backing,
                limit,
                state: IrqRwSpinLock::new(State {
                    publication_generation: 0,
                    size,
                    valid: true,
                    pins: 0,
                }),
                resize: Mutex::new(()),
            }),
        }
    }

    pub fn invalidate(&self) {
        let mut state = self.inner.state.write();
        state.publication_generation = state
            .publication_generation
            .checked_add(1)
            .expect("Shared publication generation exhausted");
        state.valid = false;
    }
}

impl Inner {
    fn range_address(
        &self,
        state: &State,
        offset: usize,
        length: usize,
    ) -> Result<u64, &'static str> {
        if !state.valid
            || length == 0
            || offset
                .checked_add(length)
                .is_none_or(|end| end > state.size)
        {
            return Err("Invalid hosted shared memory range");
        }
        self.base
            .checked_add(offset as u64)
            .ok_or("Shared memory address overflow")
    }

    fn info(
        &self,
        state: &State,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        Ok(MemoryMappingInfo::new(
            self.range_address(state, offset, length)?,
            self.permissions,
            true,
        ))
    }

    fn visit_extents(
        &self,
        mapping_offset: usize,
        start: usize,
        count: usize,
        visit: &mut dyn FnMut(usize, usize, u64),
    ) {
        let size = {
            let state = self.state.read();
            if !state.valid {
                return;
            }
            state.size
        };
        let available = size.saturating_sub(mapping_offset) / PAGE_SIZE;
        let end = start.saturating_add(count).min(available);
        let mut page = start;
        while page < end {
            let offset = mapping_offset + page * PAGE_SIZE;
            let remaining = (end - page) * PAGE_SIZE;
            let length = match &self.backing {
                Some(backing) => backing.extent_length(offset, remaining).unwrap_or(0),
                None => remaining,
            };
            let pages = length / PAGE_SIZE;
            if pages == 0 {
                break;
            }
            visit(page, pages, self.base + offset as u64);
            page += pages;
        }
    }

    fn resolve(
        &self,
        offset: usize,
        page_idx: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        let offset = page_idx
            .checked_mul(PAGE_SIZE)
            .and_then(|page| offset.checked_add(page))
            .ok_or(ResolveFaultError::Invalid)?;
        let state = self.state.read();
        if !state.valid {
            return Err(ResolveFaultError::Invalid);
        }
        let paddr = self
            .range_address(&state, offset, PAGE_SIZE)
            .map_err(|_| ResolveFaultError::Unmapped)?;
        Ok(ResolveFaultResult {
            paddr_page_base: paddr,
            is_tail: false,
        })
    }
}

impl SharedMemoryObject for SharedMemory {
    fn size(&self) -> usize {
        self.inner.state.read().size
    }

    fn resize(&self, size: usize) -> Result<(), &'static str> {
        let size = size
            .checked_next_multiple_of(PAGE_SIZE)
            .filter(|&size| size <= self.inner.limit)
            .ok_or("Hosted shared memory limit exceeded")?;
        let _resize = self.inner.resize.lock();
        {
            let state = self.inner.state.read();
            if !state.valid {
                return Err("Shared memory is invalid");
            }
            if state.size == size {
                return Ok(());
            }
            if state.pins != 0 {
                return Err("Shared memory cannot resize while imported");
            }
        }
        if let Some(backing) = &self.inner.backing {
            backing.grow(size)?;
        }
        let mut state = self.inner.state.write();
        if !state.valid || state.pins != 0 {
            // New host capacity can exist without admitting new guest bytes.
            return Err("Shared memory was invalidated or pinned during resize");
        }
        state.publication_generation = state
            .publication_generation
            .checked_add(1)
            .expect("Shared publication generation exhausted");
        state.size = size;
        Ok(())
    }

    fn id(&self) -> String {
        format!("shmem_{:#x}", self.inner.base)
    }

    fn is_valid(&self) -> bool {
        self.inner.state.read().valid
    }

    fn pin_range(&self, offset: usize, length: usize) -> Result<SharedMemoryBacking, &'static str> {
        let mut state = self.inner.state.write();
        self.inner.range_address(&state, offset, length)?;
        state.pins = state
            .pins
            .checked_add(1)
            .ok_or("Shared memory pin count overflow")?;
        Ok(SharedMemoryBacking {
            paddr: self.inner.base,
            size: state.size,
        })
    }

    fn unpin_range(&self) {
        let mut state = self.inner.state.write();
        state.pins = state.pins.saturating_sub(1);
    }
}

impl MemoryMappingOps for SharedMemory {
    fn publication_generation(&self) -> Option<u64> {
        // Shared by all address spaces and offset views. Capacity growth alone
        // grants no bytes: publication changes only at the size/valid commit.
        Some(self.inner.state.read().publication_generation)
    }

    fn prepare_mapping(&self, request: MappingRequest) -> Result<PreparedMapping, MappingError> {
        let state = self.inner.state.read();
        if !state.valid {
            return Err(MappingError::Unsupported);
        }
        if !request.offset.is_multiple_of(PAGE_SIZE) {
            return Err(MappingError::InvalidRange("Unaligned shared memory offset"));
        }
        let length = if request.allow_shorter && !request.private_demand {
            request
                .length
                .min(state.size.saturating_sub(request.offset) / PAGE_SIZE * PAGE_SIZE)
        } else {
            request.length
        };
        let info = if request.private_demand {
            None
        } else {
            Some(
                self.inner
                    .info(&state, request.offset, length)
                    .map_err(MappingError::InvalidRange)?,
            )
        };
        drop(state);
        Ok(PreparedMapping {
            length,
            info,
            owner: Some(Arc::new(Mapping {
                inner: self.inner.clone(),
                offset: request.offset,
                private: !request.is_shared,
            })),
            notification: None,
            maximum_permissions: self.new_mapping_permissions(request.is_shared),
        })
    }

    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        self.inner.info(&self.inner.state.read(), offset, length)
    }

    fn supports_mmap(&self) -> bool {
        self.is_valid()
    }

    fn can_extend_vma_on_fault(&self) -> bool {
        true
    }

    fn resolve_fault(
        &self,
        _: &AccessKind,
        page_idx: usize,
        _: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        self.inner.resolve(0, page_idx)
    }

    fn visit_resident_extents(
        &self,
        start: usize,
        count: usize,
        visit: &mut dyn FnMut(usize, usize, u64),
    ) {
        self.inner.visit_extents(0, start, count, visit);
    }

    fn mmap_owner_name(&self) -> String {
        self.id()
    }
}

struct Mapping {
    inner: Arc<Inner>,
    offset: usize,
    private: bool,
}

impl MemoryMappingOps for Mapping {
    fn publication_generation(&self) -> Option<u64> {
        // Shared by all address spaces and offset views. Capacity growth alone
        // grants no bytes: publication changes only at the size/valid commit.
        Some(self.inner.state.read().publication_generation)
    }

    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        let offset = self
            .offset
            .checked_add(offset)
            .ok_or("Shared memory offset overflow")?;
        self.inner.info(&self.inner.state.read(), offset, length)
    }

    fn can_extend_vma_on_fault(&self) -> bool {
        true
    }

    fn visit_resident_extents(
        &self,
        start: usize,
        count: usize,
        visit: &mut dyn FnMut(usize, usize, u64),
    ) {
        self.inner.visit_extents(self.offset, start, count, visit);
    }

    fn stable_physical_extent(&self, page: usize, count: usize) -> Option<u64> {
        let offset = self.offset.checked_add(page.checked_mul(PAGE_SIZE)?)?;
        let length = count.checked_mul(PAGE_SIZE)?;
        self.inner
            .range_address(&self.inner.state.read(), offset, length)
            .ok()
    }

    fn private_fault_requires_copy(&self, access: &AccessKind) -> bool {
        self.private && access.op == crate::object::capability::memory_mapping::AccessOp::Store
    }

    fn fault_page_permissions(&self, access: &AccessKind, permissions: usize) -> usize {
        use crate::object::capability::memory_mapping::AccessOp;
        use crate::vm::vmem::VirtualMemoryPermission;
        if self.private && access.op != AccessOp::Store {
            permissions & !(VirtualMemoryPermission::Write as usize)
        } else {
            permissions
        }
    }

    fn resolve_fault(
        &self,
        _: &AccessKind,
        page_idx: usize,
        _: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        self.inner.resolve(self.offset, page_idx)
    }
}
