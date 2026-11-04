//! ABI-neutral failures of a userspace memory access.
use crate::object::capability::memory_mapping::{AccessKind, AccessOp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserMemoryFaultReason {
    Unmapped,
    Protection,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use crate::object::capability::{MemoryMappingInfo, MemoryMappingOps};
    use crate::vm::{manager::VirtualMemoryManager, vmem::{MemoryArea, PhysicalMemoryArea, VirtualMemoryMap}};
    struct FailingBacking;
    impl MemoryMappingOps for FailingBacking {
        fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> { Err("backing unavailable") }
        fn supports_mmap(&self) -> bool { true }
    }
    #[test_case]
    fn user_fault_classification_preserves_internal_backing_errors() {
        let vm=VirtualMemoryManager::new();
        let access=AccessKind {op:AccessOp::Load,vaddr:0x20000000,size:None};
        assert!(matches!(vm.resolve_user_memory_fault(access),Err(UserFaultResolutionError::User(UserMemoryFault {reason:UserMemoryFaultReason::Unmapped,..}))));
        vm.add_memory_map(VirtualMemoryMap::new(PhysicalMemoryArea::new(0,0),
            MemoryArea::new(access.vaddr,access.vaddr+4095),9,false,Some(Arc::new(FailingBacking)))).unwrap();
        for op in [AccessOp::Store,AccessOp::Instruction] {
            assert!(matches!(vm.resolve_user_memory_fault(AccessKind {op,..access}),Err(UserFaultResolutionError::User(UserMemoryFault {reason:UserMemoryFaultReason::Protection,..}))));
        }
        assert!(matches!(vm.resolve_user_memory_fault(access),Err(UserFaultResolutionError::Internal(_))));
    }

    struct AllocationShortage {
        attempts: core::sync::atomic::AtomicUsize,
        always_fails: bool,
        page: crate::mem::page::ContiguousPages,
    }
    impl MemoryMappingOps for AllocationShortage {
        fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
            Err("fault-backed test mapping")
        }
        fn supports_mmap(&self) -> bool { true }
        fn resolve_fault(&self, _: &AccessKind, _: usize, _: usize)
            -> Result<crate::object::capability::memory_mapping::ResolveFaultResult,
                crate::object::capability::memory_mapping::ResolveFaultError> {
            use crate::object::capability::memory_mapping::{ResolveFaultError, ResolveFaultResult};
            let attempt = self.attempts.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if self.always_fails || attempt == 0 { return Err(ResolveFaultError::NoMemory); }
            Ok(ResolveFaultResult { paddr_page_base: self.page.as_paddr(), is_tail: false })
        }
    }

    #[test_case]
    fn memory_shortage_retries_once_and_keeps_exhaustion_an_internal_fault() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        for always_fails in [false, true] {
            let vm = VirtualMemoryManager::new();
            vm.set_asid(crate::arch::vm::alloc_virtual_address_space());
            let owner = Arc::new(AllocationShortage {
                attempts: AtomicUsize::new(0), always_fails,
                page: crate::mem::page::ContiguousPages::new(1).unwrap(),
            });
            let access = AccessKind { op: AccessOp::Load, vaddr: 0x30000000, size: None };
            vm.add_memory_map(VirtualMemoryMap::new(PhysicalMemoryArea::new(0, 0),
                MemoryArea::new(access.vaddr, access.vaddr + 4095), 9, true, Some(owner.clone()))).unwrap();
            let result = vm.resolve_user_memory_fault(access);
            if always_fails {
                assert!(matches!(result, Err(UserFaultResolutionError::Internal(_))));
            } else {
                assert!(result.is_ok(), "{result:?}");
                assert_eq!(vm.get_root_page_table().unwrap().translate(access.vaddr), Some(owner.page.as_paddr()));
            }
            assert_eq!(owner.attempts.load(Ordering::Relaxed), 2);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct UserMemoryFault {
    pub access: AccessKind,
    pub reason: UserMemoryFaultReason,
}

#[derive(Clone, Copy, Debug)]
pub enum UserFaultResolutionError {
    User(UserMemoryFault),
    Internal(&'static str),
}

impl super::manager::VirtualMemoryManager {
    /// Resolve demand paging first, distinguishing user access violations from
    /// allocation/backing failures. Internal failures must not become catchable
    /// address errors merely because an allocation failed.
    pub fn resolve_user_memory_fault(&self, access: AccessKind) -> Result<(), UserFaultResolutionError> {
        let violation = || {
            self.search_memory_map(access.vaddr).and_then(|map| {
                let required = 8 | match access.op {
                    AccessOp::Load => 1,
                    AccessOp::Store => 2,
                    AccessOp::Instruction => 4,
                };
                (map.permissions & required != required).then_some(UserMemoryFault {
                    access, reason: UserMemoryFaultReason::Protection,
                })
            })
        };
        if let Some(fault) = violation() { return Err(UserFaultResolutionError::User(fault)); }
        match self.lazy_map_page_with(access) {
            Ok(()) => Ok(()),
            Err(reason) => {
                if let Some(fault) = violation() { return Err(UserFaultResolutionError::User(fault)); }
                if self.search_memory_map(access.vaddr).is_none() {
                    Err(UserFaultResolutionError::User(UserMemoryFault {
                        access, reason: UserMemoryFaultReason::Unmapped,
                    }))
                } else {
                    Err(UserFaultResolutionError::Internal(reason))
                }
            }
        }
    }
}
