//! Physical allocation lifetime and import permission retained by driver VMs.

/// An opaque strong reference to the generic allocation behind an attachment.
/// A backend VM must retain this value whenever its page tables can expose that
/// allocation, including idle VMs and old attachment generations. It does not
/// retain a context or queue, so storing it in a VM cannot create an owner cycle.
#[derive(Clone)]
pub struct GpuBackingPin {
    pub(super) _owner: alloc::sync::Arc<dyn Send + Sync>,
    pub(super) gpu_writable: bool,
}

impl GpuBackingPin {
    /// False for consumer/imported backing even if a backend's resource metadata
    /// advertises a write usage. VM and wire authority must both honor this cap.
    pub fn allows_gpu_write(&self) -> bool {
        self.gpu_writable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicUsize, Ordering};

    struct Backing(Arc<AtomicUsize>);
    impl Drop for Backing {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn old_vm_generation_retains_actual_allocation_until_last_pin() {
        let freed = Arc::new(AtomicUsize::new(0));
        let allocation = Arc::new(Backing(Arc::clone(&freed)));
        let pin = GpuBackingPin {
            _owner: allocation.clone(),
            gpu_writable: true,
        };
        let admitted_generation = pin.clone();
        drop(allocation);
        drop(pin); // context detach / last public resource handle
        assert_eq!(freed.load(Ordering::Relaxed), 0);
        drop(admitted_generation); // verified retirement of the old generation
        assert_eq!(freed.load(Ordering::Relaxed), 1);
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn readonly_import_cap_survives_vm_generation_clones() {
        let pin = GpuBackingPin {
            _owner: Arc::new(()),
            gpu_writable: false,
        };
        assert!(!pin.allows_gpu_write());
        assert!(!pin.clone().allows_gpu_write());
    }
}
