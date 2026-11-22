//! Per-address-space resident mappings for a hosted kernel.
//!
//! These tables are Scarlet metadata. Worker address-space updates are
//! delivered by the VM transaction, outside the page-table lock.

use alloc::{boxed::Box, collections::BTreeMap, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicU16, Ordering};

use crate::{
    environment::PAGE_SIZE,
    sync::{IrqSpinLock, IrqSpinLockGuard, Once},
    vm::vmem::{MemoryAttribute, VirtualMemoryMap},
};

pub mod mmu {
    pub use super::PageTable;
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ResidentPage {
    pub paddr: u64,
    pub permissions: usize,
    pub memory_attribute: MemoryAttribute,
}

#[derive(Debug, Default)]
pub struct PageTable {
    pages: BTreeMap<usize, ResidentPage>,
}

struct Slot {
    live: AtomicBool,
    table: IrqSpinLock<Option<PageTable>>,
}

const ASID_COUNT: usize = u16::MAX as usize + 1;
static ADDRESS_SPACES: Once<Box<[Slot]>> = Once::new();
static NEXT_ASID: AtomicU16 = AtomicU16::new(1);

fn slots() -> &'static [Slot] {
    ADDRESS_SPACES.call_once(|| {
        (0..ASID_COUNT)
            .map(|_| Slot {
                live: AtomicBool::new(false),
                table: IrqSpinLock::new(None),
            })
            .collect()
    })
}

pub fn alloc_virtual_address_space() -> u16 {
    let slots = slots();
    let start = NEXT_ASID.fetch_add(1, Ordering::Relaxed);
    for offset in 0..ASID_COUNT {
        let asid = start.wrapping_add(offset as u16);
        if asid == 0 {
            continue;
        }
        let slot = &slots[asid as usize];
        if slot.live.load(Ordering::Acquire) {
            continue;
        }
        let mut table = slot.table.lock();
        if table.is_none() {
            *table = Some(PageTable::default());
            slot.live.store(true, Ordering::Release);
            return asid;
        }
    }
    panic!("No available hosted address-space ID");
}

pub fn is_asid_used(asid: u16) -> bool {
    ADDRESS_SPACES
        .get()
        .is_some_and(|slots| slots[asid as usize].live.load(Ordering::Acquire))
}

pub fn free_virtual_address_space(asid: u16) {
    let slot = &slots()[asid as usize];
    let retired = {
        let mut table = slot.table.lock();
        let retired = table.take().expect("Hosted address space is already free");
        slot.live.store(false, Ordering::Release);
        retired
    };
    drop(retired);
}

pub struct RootPageTableGuard {
    table: IrqSpinLockGuard<'static, Option<PageTable>>,
}

pub fn get_root_pagetable(asid: u16) -> Option<RootPageTableGuard> {
    let table = ADDRESS_SPACES.get()?.get(asid as usize)?.table.lock();
    table.as_ref()?;
    Some(RootPageTableGuard { table })
}

impl RootPageTableGuard {
    fn pages(&mut self) -> &mut BTreeMap<usize, ResidentPage> {
        &mut self
            .table
            .as_mut()
            .expect("Hosted table is locked in place")
            .pages
    }

    pub(crate) fn map_memory_area(
        &mut self,
        map: VirtualMemoryMap,
        accessed: bool,
        dirty: bool,
    ) -> Result<(), &'static str> {
        if !map.vmarea.start.is_multiple_of(PAGE_SIZE)
            || !map.pmarea.start.is_multiple_of(PAGE_SIZE as u64)
            || map.vmarea.end % PAGE_SIZE != PAGE_SIZE - 1
            || map.pmarea.byte_len() != Some(map.vmarea.size() as u64)
        {
            return Err("Unaligned hosted mapping, or virtual and physical extents differ");
        }
        for address in (map.vmarea.start..=map.vmarea.end).step_by(PAGE_SIZE) {
            let paddr = map.pmarea.start + (address - map.vmarea.start) as u64;
            self.map(
                address,
                paddr,
                map.permissions,
                map.memory_attribute,
                accessed,
                dirty,
            );
        }
        Ok(())
    }

    pub(crate) fn map(
        &mut self,
        vaddr: usize,
        paddr: u64,
        permissions: usize,
        memory_attribute: MemoryAttribute,
        _accessed: bool,
        _dirty: bool,
    ) {
        let address = vaddr & !(PAGE_SIZE - 1);
        self.pages().insert(
            address,
            ResidentPage {
                paddr: paddr & !(PAGE_SIZE as u64 - 1),
                permissions,
                memory_attribute,
            },
        );
    }

    pub(crate) fn resident_page(&mut self, vaddr: usize) -> Option<ResidentPage> {
        self.pages().get(&(vaddr & !(PAGE_SIZE - 1))).copied()
    }

    pub(crate) fn translate(&mut self, vaddr: usize) -> Option<u64> {
        let offset = vaddr & (PAGE_SIZE - 1);
        let page = self.pages().get(&(vaddr - offset))?;
        Some(page.paddr + offset as u64)
    }

    pub(crate) fn resident_pages(&mut self) -> Vec<(usize, ResidentPage)> {
        self.pages()
            .iter()
            .map(|(&vaddr, &page)| (vaddr, page))
            .collect()
    }

    pub(crate) fn unmap_range(&mut self, start: usize, end: usize) {
        if start <= end {
            self.pages()
                .extract_if((start & !(PAGE_SIZE - 1))..=end, |_, _| true)
                .for_each(drop);
        }
    }

    pub(crate) fn unmap_all(&mut self) {
        self.pages().clear();
    }

    pub(crate) fn unmap_all_no_flush(&mut self) {
        self.unmap_all();
    }
}

pub fn setup_trampoline_for_user(_: &crate::vm::manager::VirtualMemoryManager) {}

pub fn flush_all_tlb() {
    // The software table is visible under its lock. Worker TLB changes are
    // part of the acknowledged host mapping operation.
}
