//! Android's read-only seqlock status page follows native policy generations.
use crate::{
    ipc::SharedMemory,
    object::capability::{MemoryMappingInfo, MemoryMappingOps},
    shield::{Policy, PolicyObserver},
    sync::IrqSpinLock,
};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering, fence};

pub(super) struct PolicyStatus {
    memory: Arc<SharedMemory>,
    generation: IrqSpinLock<u64>,
}
impl PolicyStatus {
    pub fn new(policy: &Policy) -> Result<Arc<Self>, &'static str> {
        let status = Arc::new(Self {
            memory: Arc::new(SharedMemory::new(4096, 1)?),
            generation: IrqSpinLock::new(0),
        });
        let observer: Arc<dyn PolicyObserver> = status.clone();
        policy.observe(&observer)?;
        Ok(status)
    }
    pub fn values(&self) -> [u32; 5] {
        let generation = *self.generation.lock() as u32;
        [
            1,
            generation.wrapping_sub(1).wrapping_mul(2),
            1,
            generation,
            1,
        ]
    }
    pub fn mapping(&self, offset: usize, length: usize) -> Result<MemoryMappingInfo, &'static str> {
        self.memory.get_mapping_info(offset, length)
    }
}
impl PolicyObserver for PolicyStatus {
    fn policy_changed(&self, next: u64) {
        let mut generation = self.generation.lock();
        if next <= *generation {
            return;
        }
        let info = self
            .memory
            .get_mapping_info(0, 4096)
            .expect("owned policy status backing");
        let pointer = crate::vm::addr::phys_to_virt(info.paddr) as *const AtomicU32;
        let sequence = (next as u32).wrapping_sub(1).wrapping_mul(2);
        // SAFETY: this page is retained by self, aligned, and writable only by
        // this serialized observer. Readers map it read-only and use the Linux
        // status seqlock protocol. The counter wraps exactly as a Linux u32.
        unsafe {
            (*pointer.add(1)).store(sequence.wrapping_sub(1), Ordering::Relaxed);
            fence(Ordering::Release);
            (*pointer).store(1, Ordering::Relaxed);
            (*pointer.add(2)).store(1, Ordering::Relaxed);
            (*pointer.add(3)).store(next as u32, Ordering::Relaxed);
            (*pointer.add(4)).store(1, Ordering::Relaxed);
            fence(Ordering::Release);
            (*pointer.add(1)).store(sequence, Ordering::Release);
        }
        *generation = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shield::{Rule, update::PolicyUpdate};
    use alloc::vec;
    #[test_case]
    fn live_status_mapping_tracks_publication_and_never_regresses_on_late_notification() {
        let policy = Policy::new(
            vec![
                (1, "u:r:manager:s0".into()),
                (2, "u:object_r:service:s0".into()),
            ],
            vec![],
        )
        .unwrap();
        let status = PolicyStatus::new(&policy).unwrap();
        let mapping = status.mapping(0, 4096).unwrap();
        let words = crate::vm::addr::phys_to_virt(mapping.paddr) as *const AtomicU32;
        let read = || unsafe {
            core::array::from_fn::<u32, 5, _>(|index| (*words.add(index)).load(Ordering::Acquire))
        };
        assert_eq!(read(), [1, 0, 1, 1, 1]);
        policy
            .extend(
                1,
                PolicyUpdate {
                    rules: vec![Rule {
                        source: 1,
                        target: 2,
                        class: 1,
                        permissions: 2,
                    }],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(read(), [1, 2, 1, 2, 1]);
        assert_eq!(status.values(), read());
        status.policy_changed(1);
        assert_eq!(read(), [1, 2, 1, 2, 1]);
        assert!(
            policy
                .extend(
                    2,
                    PolicyUpdate {
                        rules: vec![Rule {
                            source: 1,
                            target: 999,
                            class: 1,
                            permissions: 1
                        }],
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert_eq!(read(), [1, 2, 1, 2, 1]);
    }
}
