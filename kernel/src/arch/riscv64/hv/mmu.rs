use crate::arch::vm::mmu::PageTable;
use crate::arch::vm::{
    alloc_virtual_address_space, free_virtual_address_space, get_root_pagetable,
    get_root_pagetable_ptr,
};
use crate::vm::vmem::VirtualMemoryPermission;

use super::csr;

pub struct GuestRoot {
    asid: u16,
}

impl GuestRoot {
    pub fn new() -> Result<Self, &'static str> {
        let asid = alloc_virtual_address_space();
        Ok(Self { asid })
    }

    pub fn map_page(&self, gpa: u64, hpa: u64, readonly: bool) -> Result<(), &'static str> {
        let root = get_root_pagetable(self.asid).ok_or("Guest root page table not found")?;
        let mut permissions = VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Execute as usize
            | VirtualMemoryPermission::User as usize;
        if !readonly {
            permissions |= VirtualMemoryPermission::Write as usize;
        }
        root.map(
            self.asid,
            gpa as usize,
            hpa as usize,
            permissions,
            true,
            true,
        );
        Ok(())
    }

    pub fn unmap_page(&self, gpa: u64) -> Result<(), &'static str> {
        let root = get_root_pagetable(self.asid).ok_or("Guest root page table not found")?;
        root.unmap(self.asid, gpa as usize);
        Ok(())
    }

    pub fn root_token(&self, vmid: u16) -> Result<u64, &'static str> {
        let root_ptr: *mut PageTable =
            get_root_pagetable_ptr(self.asid).ok_or("Guest root page table not found")?;
        let ppn = (root_ptr as u64) >> 12;
        let mode: u64 = 9;
        Ok((mode << 60) | ((vmid as u64) << 44) | ppn)
    }

    pub fn flush_tlb(&self) {
        csr::hfence_gvma_all();
    }
}

impl Drop for GuestRoot {
    fn drop(&mut self) {
        free_virtual_address_space(self.asid);
    }
}
