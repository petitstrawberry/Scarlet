//! Read-only receive memory. The VMA retains only the backing, not the Binder
//! file/endpoint, so closing the file can notify pending callers immediately.
use super::*;
use crate::{
    ipc::SharedMemory,
    vm::{
        addr::phys_to_virt,
        vmem::{MemoryArea, PhysicalMemoryArea, VirtualMemoryMap},
    },
};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(super) const SLOT: usize = 64 * 1024;
pub(super) struct Arena {
    backing: SharedMemory,
    paddr: u64,
    size: usize,
    base: AtomicUsize,
    live: AtomicBool,
    slots: IrqSpinLock<Vec<bool>>,
}
impl Arena {
    fn new(size: usize) -> Result<Arc<Self>, usize> {
        let backing = SharedMemory::new(size, 1).map_err(|_| errno::ENOMEM)?;
        let paddr = backing
            .get_mapping_info(0, size)
            .map_err(|_| errno::ENOMEM)?
            .paddr;
        // SAFETY: this newly allocated contiguous backing is owned exclusively
        // here and has not been exposed to a task yet. Zero every mapped byte.
        unsafe {
            core::ptr::write_bytes(phys_to_virt(paddr) as *mut u8, 0, size);
        }
        Ok(Arc::new(Self {
            backing,
            paddr,
            size,
            base: AtomicUsize::new(0),
            live: AtomicBool::new(false),
            slots: IrqSpinLock::new(vec![false; size / SLOT]),
        }))
    }
    pub(super) fn reserve(&self) -> Result<usize, usize> {
        if !self.live.load(Ordering::Acquire) {
            return Err(errno::EINVAL);
        }
        let mut slots = self.slots.lock();
        let index = slots.iter().position(|used| !*used).ok_or(errno::ENOSPC)?;
        slots[index] = true;
        Ok(self.base.load(Ordering::Relaxed) + index * SLOT)
    }
    fn index(&self, address: usize) -> Result<usize, usize> {
        let offset = address
            .checked_sub(self.base.load(Ordering::Relaxed))
            .ok_or(errno::EINVAL)?;
        if offset / SLOT >= self.size / SLOT || !offset.is_multiple_of(SLOT) {
            return Err(errno::EINVAL);
        }
        Ok(offset / SLOT)
    }
    pub(super) fn free(&self, address: usize) -> Result<(), usize> {
        let index = self.index(address)?;
        let mut slots = self.slots.lock();
        if !slots[index] {
            return Err(errno::EINVAL);
        }
        // SAFETY: this live slot belongs to the immutable contiguous backing.
        // Zeroing on release also implements TF_CLEAR_BUF conservatively.
        unsafe {
            core::ptr::write_bytes(
                (phys_to_virt(self.paddr) as *mut u8).add(index * SLOT),
                0,
                SLOT,
            );
        }
        slots[index] = false;
        Ok(())
    }
    pub(super) fn store(&self, address: usize, bytes: &[u8]) -> Result<(), usize> {
        let index = self.index(address)?;
        let slots = self.slots.lock();
        if bytes.len() > SLOT || !slots[index] || !self.live.load(Ordering::Acquire) {
            return Err(errno::EINVAL);
        }
        // SAFETY: immutable backing spans this reserved slot; the source is an
        // owned RPC Vec, disjoint from every userspace receive mapping.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (phys_to_virt(self.paddr) as *mut u8).add(index * SLOT),
                bytes.len(),
            );
        }
        Ok(())
    }
}
impl MemoryMappingOps for Arena {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        self.backing.get_mapping_info(offset, length)
    }
    fn on_mapped(&self, vaddr: usize, _: u64, _: usize, _: usize) {
        self.base.store(vaddr, Ordering::Relaxed);
        self.live.store(true, Ordering::Release);
    }
    fn on_unmapped(&self, _: usize, _: usize) {
        // Any partial unmap invalidates the receive arena. Do not deliver
        // pointers into a range whose ownership is no longer intact.
        self.live.store(false, Ordering::Release);
    }
    fn inherit_on_fork(&self) -> bool {
        false
    }
    fn supports_private_mmap(&self) -> bool {
        false
    }
}

impl BinderFile {
    pub(crate) fn mmap(
        &self,
        task: &Task,
        addr: usize,
        length: usize,
        prot: usize,
        flags: usize,
        offset: usize,
    ) -> usize {
        let result = (|| {
            self.check_owner(task)?;
            // Binder MAP_PRIVATE still maps the driver's shared receive pages.
            // Fixed mappings and remapping an existing arena are not supported.
            if addr != 0
                || offset != 0
                || prot != 1
                || !(SLOT..=4 * 1024 * 1024).contains(&length)
                || !matches!(flags & 3, 1 | 2)
                || flags & !(3 | 0x4000) != 0
            {
                return Err(errno::EINVAL);
            }
            let mut state = self.state.lock();
            if state.arena.is_some() {
                return Err(errno::EBUSY);
            }
            let arena = Arena::new(length)?;
            let area = MemoryArea::new(0, length - 1);
            let map = VirtualMemoryMap::new(
                PhysicalMemoryArea::new(arena.paddr, arena.paddr + length as u64 - 1),
                area,
                9,
                true,
                Some(arena.clone()),
            );
            let base = task
                .vm_manager
                .add_memory_map_anywhere(map)
                .map_err(|_| errno::ENOMEM)?;
            arena.on_mapped(base, arena.paddr, length, 0);
            state.arena = Some(arena);
            Ok(base)
        })();
        result.unwrap_or_else(errno::to_result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test_case]
    fn binder_arena_bounds_zeroing_and_partial_unmap() {
        let arena = Arena::new(SLOT * 2 - 4096).unwrap();
        arena.on_mapped(0x100000, arena.paddr, arena.size, 0);
        assert_eq!(arena.get_mapping_info(0, 4096).unwrap().permissions, 1);
        assert!(!arena.inherit_on_fork());
        assert_eq!(arena.free(0x100000 + SLOT), Err(errno::EINVAL));
        assert_eq!(arena.free(usize::MAX), Err(errno::EINVAL));
        let address = arena.reserve().unwrap();
        assert_eq!(arena.reserve(), Err(errno::ENOSPC));
        arena.store(address, b"secret").unwrap();
        arena.free(address).unwrap();
        assert_eq!(arena.free(address), Err(errno::EINVAL));
        assert_eq!(arena.reserve().unwrap(), address);
        // SAFETY: the arena retains this initialized, contiguous backing.
        let bytes =
            unsafe { core::slice::from_raw_parts(phys_to_virt(arena.paddr) as *const u8, 6) };
        assert_eq!(bytes, &[0; 6]);
        arena.on_unmapped(address, 4096);
        assert_eq!(arena.reserve(), Err(errno::EINVAL));
        assert_eq!(arena.store(address, b"new"), Err(errno::EINVAL));
    }
}
