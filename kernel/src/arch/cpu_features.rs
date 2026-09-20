//! Architecture-independent publication of userspace CPU capabilities.
//!
//! An architecture reports each CPU's safe ELF HWCAP words. The intersection
//! is frozen before the first exec. Late APs wait for publication and may enter
//! the scheduler only when they contain every bit already promised to userspace.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

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
    reports: [UnsafeCell<CpuCapabilities>; MAX_NUM_CPUS],
    reported: [AtomicBool; MAX_NUM_CPUS],
    published_capabilities: UnsafeCell<CpuCapabilities>,
    published: AtomicBool,
}

// Each CPU writes only its own report before publishing the corresponding
// reported flag. The BSP reads a report only after an Acquire load of that
// flag. It writes the final value once, before publishing it with Release.
unsafe impl Sync for CpuFeatureRegistry {}

impl CpuFeatureRegistry {
    pub const fn new() -> Self {
        Self {
            reports: [const {
                UnsafeCell::new(CpuCapabilities {
                    hwcap: 0,
                    hwcap2: 0,
                })
            }; MAX_NUM_CPUS],
            reported: [const { AtomicBool::new(false) }; MAX_NUM_CPUS],
            published_capabilities: UnsafeCell::new(CpuCapabilities {
                hwcap: 0,
                hwcap2: 0,
            }),
            published: AtomicBool::new(false),
        }
    }

    pub fn report(&self, cpu_id: usize, capabilities: CpuCapabilities) {
        assert!(cpu_id < MAX_NUM_CPUS);
        assert!(!self.reported[cpu_id].load(Ordering::Acquire));
        // SAFETY: Each logical CPU has exactly one reporter. Readers wait for
        // this slot's Release store to `reported` before accessing the value.
        unsafe { *self.reports[cpu_id].get() = capabilities };
        self.reported[cpu_id].store(true, Ordering::Release);
    }

    pub fn report_secondary_and_wait(&self, cpu_id: usize, capabilities: CpuCapabilities) -> bool {
        self.report(cpu_id, capabilities);
        while !self.published.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }
        capabilities.contains(self.published_capabilities())
    }

    pub fn reported_mask(&self) -> u64 {
        let mut mask = 0;
        for cpu_id in 0..MAX_NUM_CPUS {
            if self.reported[cpu_id].load(Ordering::Acquire) {
                mask |= 1 << cpu_id;
            }
        }
        mask
    }

    /// Freeze the intersection of CPUs that completed their probe. The boot
    /// protocol controls how long to wait for missing APs before calling this.
    pub fn publish(&self) -> CpuCapabilities {
        assert!(!self.published.load(Ordering::Relaxed));
        let mut common = CpuCapabilities {
            hwcap: u64::MAX,
            hwcap2: u64::MAX,
        };
        let mut reports = 0;
        for cpu_id in 0..MAX_NUM_CPUS {
            if self.reported[cpu_id].load(Ordering::Acquire) {
                // SAFETY: The Acquire load observes this CPU's completed
                // report. That slot will not be written a second time.
                common = common.intersection(unsafe { *self.reports[cpu_id].get() });
                reports += 1;
            }
        }
        assert_ne!(reports, 0, "boot CPU has not reported its capabilities");
        // SAFETY: Only the BSP publishes, exactly once. Readers wait for the
        // Release store to `published` before accessing the final value.
        unsafe { *self.published_capabilities.get() = common };
        self.published.store(true, Ordering::Release);
        common
    }

    pub fn published_capabilities(&self) -> CpuCapabilities {
        if !self.published.load(Ordering::Acquire) {
            return CpuCapabilities::default();
        }
        // SAFETY: The Acquire load above observes the BSP's completed write.
        unsafe { *self.published_capabilities.get() }
    }
}

// The RV32 toolchain does not package libtest; run this registry test on the
// 64-bit kernel targets while still compiling the registry for RV32.
#[cfg(all(test, target_pointer_width = "64"))]
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
