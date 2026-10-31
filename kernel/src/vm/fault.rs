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
