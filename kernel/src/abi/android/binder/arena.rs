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

const ALIGNMENT: usize = 8;
pub(super) struct Arena {
    backing: SharedMemory,
    paddr: u64,
    size: usize,
    base: AtomicUsize,
    live: AtomicBool,
    allocations: IrqSpinLock<BTreeMap<usize, usize>>,
}
impl Arena {
    fn new(size: usize) -> Result<Arc<Self>, usize> {
        let backing = SharedMemory::new(size, 1).map_err(|_| errno::ENOMEM)?;
        let paddr = backing
            .get_mapping_info(0, size)
            .map_err(|_| errno::ENOMEM)?
            .paddr;
        // SAFETY: fresh contiguous backing has not been exposed to userspace.
        unsafe {
            core::ptr::write_bytes(phys_to_virt(paddr) as *mut u8, 0, size);
        }
        Ok(Arc::new(Self {
            backing,
            paddr,
            size,
            base: AtomicUsize::new(0),
            live: AtomicBool::new(false),
            allocations: IrqSpinLock::new(BTreeMap::new()),
        }))
    }
    fn aligned_size(size: usize) -> Result<usize, usize> {
        size.max(1)
            .checked_add(ALIGNMENT - 1)
            .map(|size| size & !(ALIGNMENT - 1))
            .ok_or(errno::E2BIG)
    }
    fn find_space(&self, allocations: &BTreeMap<usize, usize>, size: usize) -> Option<usize> {
        let mut offset = 0;
        for (&start, &length) in allocations {
            if start.checked_sub(offset)? >= size {
                return Some(offset);
            }
            offset = start.checked_add(length)?;
        }
        (self.size.checked_sub(offset)? >= size).then_some(offset)
    }
    pub(super) fn reserve(&self, size: usize) -> Result<usize, usize> {
        if !self.live.load(Ordering::Acquire) {
            return Err(errno::EINVAL);
        }
        let size = Self::aligned_size(size)?;
        if size > self.size {
            return Err(errno::E2BIG);
        }
        let mut allocations = self.allocations.lock();
        let offset = self.find_space(&allocations, size).ok_or(errno::ENOSPC)?;
        allocations.insert(offset, size);
        Ok(self.base.load(Ordering::Relaxed) + offset)
    }
    pub(super) fn has_space(&self, size: usize) -> bool {
        if !self.live.load(Ordering::Acquire) {
            return true;
        } // let delivery report invalidation
        let Ok(size) = Self::aligned_size(size) else {
            return true;
        };
        size > self.size || self.find_space(&self.allocations.lock(), size).is_some()
    }
    fn offset(&self, address: usize) -> Result<usize, usize> {
        address
            .checked_sub(self.base.load(Ordering::Relaxed))
            .ok_or(errno::EINVAL)
    }
    pub(super) fn free(&self, address: usize) -> Result<(), usize> {
        let offset = self.offset(address)?;
        let mut allocations = self.allocations.lock();
        let size = *allocations.get(&offset).ok_or(errno::EINVAL)?;
        // SAFETY: the allocation lies within the retained contiguous backing.
        // Zero before publishing the range as free (also implements TF_CLEAR_BUF).
        unsafe {
            core::ptr::write_bytes((phys_to_virt(self.paddr) as *mut u8).add(offset), 0, size);
        }
        allocations.remove(&offset);
        Ok(())
    }
    pub(super) fn store(&self, address: usize, bytes: &[u8]) -> Result<(), usize> {
        let offset = self.offset(address)?;
        let allocations = self.allocations.lock();
        let size = *allocations.get(&offset).ok_or(errno::EINVAL)?;
        if bytes.len() > size || !self.live.load(Ordering::Acquire) {
            return Err(errno::EINVAL);
        }
        // SAFETY: source is owned RPC bytes, disjoint from the reserved backing.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (phys_to_virt(self.paddr) as *mut u8).add(offset),
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
                || !(4096..=4 * 1024 * 1024).contains(&length)
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
    fn binder_arena_variable_allocations_reuse_zeroing_and_unmap() {
        let arena = Arena::new(4096).unwrap();
        arena.on_mapped(0x100000, arena.paddr, arena.size, 0);
        assert!(!arena.inherit_on_fork());
        let mut addresses = Vec::new();
        for _ in 0..32 {
            addresses.push(arena.reserve(17).unwrap());
        }
        assert_eq!(addresses[1] - addresses[0], 24);
        assert_eq!(arena.free(addresses[0] + 1), Err(errno::EINVAL));
        arena.store(addresses[0], b"secret").unwrap();
        assert_eq!(arena.store(addresses[0], &[1; 25]), Err(errno::EINVAL));
        arena.free(addresses[0]).unwrap();
        assert_eq!(arena.free(addresses[0]), Err(errno::EINVAL));
        assert_eq!(arena.reserve(24).unwrap(), addresses[0]);
        let bytes =
            unsafe { core::slice::from_raw_parts(phys_to_virt(arena.paddr) as *const u8, 24) };
        assert_eq!(bytes, &[0; 24]);
        let tail = arena.reserve(4096 - 32 * 24).unwrap();
        assert!(!arena.has_space(8));
        assert_eq!(arena.reserve(8), Err(errno::ENOSPC));
        arena.free(addresses[1]).unwrap();
        arena.free(addresses[2]).unwrap();
        assert!(arena.has_space(48));
        assert_eq!(arena.reserve(48).unwrap(), addresses[1]);
        assert_eq!(arena.reserve(usize::MAX), Err(errno::E2BIG));
        arena.on_unmapped(tail, 4096);
        assert_eq!(arena.reserve(8), Err(errno::EINVAL));
        assert_eq!(arena.store(addresses[0], b"new"), Err(errno::EINVAL));
    }
}
