use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use spin::RwLock;

use crate::environment::PAGE_SIZE;
use crate::mem::page::{ContiguousPages, allocate_raw_pages, free_raw_pages};
use crate::vm::addr::{phys_to_virt, virt_to_phys};

use super::{AccessKind, AccessOp, MemoryMappingOps, ResolveFaultError, ResolveFaultResult};

pub struct AnonymousPageOwner {
    pages: RwLock<BTreeMap<usize, usize>>,
}

impl AnonymousPageOwner {
    pub fn new() -> Self {
        Self {
            pages: RwLock::new(BTreeMap::new()),
        }
    }

    fn alloc_page(&self) -> Option<usize> {
        let ptr = allocate_raw_pages(1);
        if ptr.is_null() {
            return None;
        }
        Some(virt_to_phys(ptr as usize))
    }
}

impl Drop for AnonymousPageOwner {
    fn drop(&mut self) {
        let pages = core::mem::take(&mut *self.pages.write());
        for (_, paddr) in pages {
            free_raw_pages(phys_to_virt(paddr) as *mut _, 1);
        }
    }
}

impl MemoryMappingOps for AnonymousPageOwner {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("AnonymousPageOwner does not support get_mapping_info")
    }

    fn supports_mmap(&self) -> bool {
        false
    }

    fn mmap_owner_name(&self) -> String {
        String::from("anonymous")
    }

    fn resolve_fault(
        &self,
        _access: &AccessKind,
        page_idx: usize,
        _vm_start: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        let mut pages = self.pages.write();
        let paddr = if let Some(&existing) = pages.get(&page_idx) {
            existing
        } else {
            let paddr = self.alloc_page().ok_or(ResolveFaultError::Unmapped)?;
            pages.insert(page_idx, paddr);
            paddr
        };
        Ok(ResolveFaultResult {
            paddr_page_base: paddr,
            is_tail: false,
        })
    }

    fn release_pages(&self, start_page_idx: usize, page_count: usize) {
        let mut pages = self.pages.write();
        for idx in start_page_idx..start_page_idx + page_count {
            if let Some(paddr) = pages.remove(&idx) {
                unsafe {
                    free_raw_pages(phys_to_virt(paddr) as *mut _, 1);
                }
            }
        }
    }

    fn fork_clone(&self) -> Option<Arc<dyn MemoryMappingOps>> {
        fork_clone_owner(self)
    }
}

pub fn fork_clone_owner(owner: &AnonymousPageOwner) -> Option<Arc<dyn MemoryMappingOps>> {
    let pages = owner.pages.read();
    let mut new_pages = BTreeMap::new();
    for (&idx, &paddr) in pages.iter() {
        let new_ptr = allocate_raw_pages(1);
        if new_ptr.is_null() {
            for (_, &old_paddr) in new_pages.iter() {
                unsafe {
                    free_raw_pages(phys_to_virt(old_paddr) as *mut _, 1);
                }
            }
            return None;
        }
        let new_paddr = virt_to_phys(new_ptr as usize);
        unsafe {
            core::ptr::copy_nonoverlapping(
                phys_to_virt(paddr) as *const u8,
                new_ptr as *mut u8,
                PAGE_SIZE,
            );
        }
        new_pages.insert(idx, new_paddr);
    }
    drop(pages);

    Some(Arc::new(AnonymousPageOwner {
        pages: RwLock::new(new_pages),
    }))
}

pub struct ForkCowPageOwner {
    base_page_idx: usize,
    pages: RwLock<Option<ContiguousPages>>,
}

impl ForkCowPageOwner {
    pub fn new(base_page_idx: usize, pages: ContiguousPages) -> Self {
        Self {
            base_page_idx,
            pages: RwLock::new(Some(pages)),
        }
    }
}

impl MemoryMappingOps for ForkCowPageOwner {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("ForkCowPageOwner does not support get_mapping_info")
    }

    fn supports_mmap(&self) -> bool {
        false
    }

    fn mmap_owner_name(&self) -> String {
        String::from("fork-cow")
    }

    fn resolve_fault(
        &self,
        _access: &AccessKind,
        page_idx: usize,
        _vm_start: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        let pages = self.pages.read();
        let pages = pages.as_ref().ok_or(ResolveFaultError::Unmapped)?;
        if page_idx < self.base_page_idx {
            return Err(ResolveFaultError::Unmapped);
        }
        let offset = page_idx - self.base_page_idx;
        if offset >= pages.len() {
            return Err(ResolveFaultError::Unmapped);
        }
        Ok(ResolveFaultResult {
            paddr_page_base: pages.as_paddr() + offset * PAGE_SIZE,
            is_tail: false,
        })
    }

    fn fault_page_permissions(&self, access: &AccessKind, default_permissions: usize) -> usize {
        if access.op == AccessOp::Store {
            default_permissions
        } else {
            default_permissions & !0x2
        }
    }

    fn private_fault_requires_copy(&self, access: &AccessKind) -> bool {
        access.op == AccessOp::Store
    }

    fn release_pages(&self, _start_page_idx: usize, _page_count: usize) {
        // A fork COW owner can be referenced by multiple task VM maps. Releasing
        // from one mapping must not drop the backing pages while another mapping
        // can still fault them in. The pages are freed when the final Arc to this
        // owner is dropped.
    }
}
