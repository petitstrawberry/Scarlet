//! Architecture-independent publication of userspace CPU capabilities.
//!
//! An architecture reports each CPU's safe ELF HWCAP words. The intersection
//! is frozen before the first exec. Late APs wait for publication and may enter
//! the scheduler only when they contain every bit already promised to userspace.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::environment::MAX_NUM_CPUS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuCapabilities {
    pub hwcap: u64,
    pub hwcap2: u64,
}

impl CpuCapabilities {
    fn intersection(self, other: Self) -> Self {
        Self {
            hwcap: self.hwcap & other.hwcap,
            hwcap2: self.hwcap2 & other.hwcap2,
        }
    }

    fn contains(self, required: Self) -> bool {
        self.hwcap & required.hwcap == required.hwcap
            && self.hwcap2 & required.hwcap2 == required.hwcap2
    }
}

pub struct CpuFeatureRegistry {
    hwcap: [AtomicU64; MAX_NUM_CPUS],
    hwcap2: [AtomicU64; MAX_NUM_CPUS],
    reported: AtomicU64,
    published_hwcap: AtomicU64,
    published_hwcap2: AtomicU64,
    published: AtomicBool,
}

impl CpuFeatureRegistry {
    pub const fn new() -> Self {
        Self {
            hwcap: [const { AtomicU64::new(0) }; MAX_NUM_CPUS],
            hwcap2: [const { AtomicU64::new(0) }; MAX_NUM_CPUS],
            reported: AtomicU64::new(0),
            published_hwcap: AtomicU64::new(0),
            published_hwcap2: AtomicU64::new(0),
            published: AtomicBool::new(false),
        }
    }

    pub fn report(&self, cpu_id: usize, capabilities: CpuCapabilities) {
        assert!(cpu_id < MAX_NUM_CPUS);
        assert!(
            !self.published.load(Ordering::Acquire) || self.reported_mask() & (1 << cpu_id) == 0
        );
        self.hwcap[cpu_id].store(capabilities.hwcap, Ordering::Relaxed);
        self.hwcap2[cpu_id].store(capabilities.hwcap2, Ordering::Relaxed);
        self.reported.fetch_or(1 << cpu_id, Ordering::Release);
    }

    pub fn report_secondary_and_wait(&self, cpu_id: usize, capabilities: CpuCapabilities) -> bool {
        self.report(cpu_id, capabilities);
        while !self.published.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }
        capabilities.contains(self.published_capabilities())
    }

    pub fn reported_mask(&self) -> u64 {
        self.reported.load(Ordering::Acquire)
    }

    /// Freeze the intersection of CPUs that completed their probe. The boot
    /// protocol controls how long to wait for missing APs before calling this.
    pub fn publish(&self) -> CpuCapabilities {
        assert!(!self.published.load(Ordering::Relaxed));
        let mask = self.reported_mask();
        assert_ne!(mask, 0, "boot CPU has not reported its capabilities");
        let mut common = CpuCapabilities {
            hwcap: u64::MAX,
            hwcap2: u64::MAX,
        };
        for cpu_id in 0..MAX_NUM_CPUS {
            if mask & (1 << cpu_id) != 0 {
                common = common.intersection(CpuCapabilities {
                    hwcap: self.hwcap[cpu_id].load(Ordering::Relaxed),
                    hwcap2: self.hwcap2[cpu_id].load(Ordering::Relaxed),
                });
            }
        }
        self.published_hwcap.store(common.hwcap, Ordering::Relaxed);
        self.published_hwcap2
            .store(common.hwcap2, Ordering::Relaxed);
        self.published.store(true, Ordering::Release);
        common
    }

    pub fn published_capabilities(&self) -> CpuCapabilities {
        if !self.published.load(Ordering::Acquire) {
            return CpuCapabilities::default();
        }
        CpuCapabilities {
            hwcap: self.published_hwcap.load(Ordering::Relaxed),
            hwcap2: self.published_hwcap2.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_is_frozen_and_late_weak_cpu_is_rejected() {
        let registry = CpuFeatureRegistry::new();
        registry.report(
            0,
            CpuCapabilities {
                hwcap: 0b111,
                hwcap2: 0b11,
            },
        );
        registry.report(
            1,
            CpuCapabilities {
                hwcap: 0b110,
                hwcap2: 0b01,
            },
        );
        assert_eq!(registry.reported_mask(), 0b11);
        assert_eq!(
            registry.publish(),
            CpuCapabilities {
                hwcap: 0b110,
                hwcap2: 0b01
            }
        );
        assert!(!registry.report_secondary_and_wait(
            2,
            CpuCapabilities {
                hwcap: 0b010,
                hwcap2: 0b01
            }
        ));
        assert!(registry.report_secondary_and_wait(
            3,
            CpuCapabilities {
                hwcap: 0b110,
                hwcap2: 0b11
            }
        ));
        assert_eq!(registry.published_capabilities().hwcap, 0b110);
    }
}
