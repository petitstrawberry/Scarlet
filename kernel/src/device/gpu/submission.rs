//! Owned in-flight GPU work and bounded kernel admission.
//!
//! An accepted request must outlive every handle that submitted or observed it.
//! Drivers return rejected requests to the generic queue, or retain accepted
//! requests until retirement. Accidentally dropping an unsettled request fails
//! its observation and quarantines backing instead of freeing live DMA memory.

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicUsize, Ordering};

use super::execution::{GpuAttachedBuffer, GpuAttachedImage};
use super::{GpuBackendContext, GpuBackendQueue, GpuCompletionFailure, GpuCompletionSignal};

/// Generic per-queue ceiling, additionally limited by backend capacity.
pub(super) const MAX_PENDING_SUBMISSIONS: u32 = 32;

#[derive(Default)]
pub(super) struct SubmissionSlots {
    pending: Arc<AtomicUsize>,
}

impl SubmissionSlots {
    pub(super) fn reserve(&self, limit: u32) -> Option<SubmissionPermit> {
        self.pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                (pending < limit.min(MAX_PENDING_SUBMISSIONS) as usize).then_some(pending + 1)
            })
            .ok()?;
        Some(SubmissionPermit(Arc::clone(&self.pending)))
    }
}

pub(super) struct SubmissionPermit(Arc<AtomicUsize>);

impl Drop for SubmissionPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Attachment snapshots and owners retained independently of observer handles.
pub(super) struct SubmissionResources {
    pub(super) _images: Vec<GpuAttachedImage>,
    pub(super) _buffers: Vec<GpuAttachedBuffer>,
    pub(super) _context: Arc<dyn GpuBackendContext>,
    pub(super) _queue: Arc<dyn GpuBackendQueue>,
}

struct RetainedSubmission {
    commands: Vec<u8>,
    _resources: SubmissionResources,
    _permit: SubmissionPermit,
}

/// Owned commands, resource authority, backing pins, and completion producer.
///
/// Async backends must return this value with a side-effect-free rejection or
/// keep it in their independently driven in-flight queue. The generic layer
/// prevents detach from racing validation/enqueue; the snapshot keeps backing
/// alive afterwards, but the backend must also preserve GPU mappings/authority.
///
/// Drop is a fail-safe, not cancellation: it reports abandonment and leaks the
/// retained request, including its admission slot. Use [`Self::complete`] or
/// [`Self::retire_failed`] only after hardware quiescence. Resource destruction
/// can enter a backend, so retirement must run outside its transport locks.
pub struct GpuSubmission {
    retained: Option<Box<RetainedSubmission>>,
    signal: Option<GpuCompletionSignal>,
}

impl GpuSubmission {
    pub(super) fn new(
        commands: Vec<u8>,
        resources: SubmissionResources,
        permit: SubmissionPermit,
        signal: GpuCompletionSignal,
    ) -> Self {
        Self {
            retained: Some(Box::new(RetainedSubmission {
                commands,
                _resources: resources,
                _permit: permit,
            })),
            signal: Some(signal),
        }
    }

    /// Borrow the kernel-owned opaque command bytes.
    ///
    /// # Returns
    ///
    /// Validated-length bytes, or an empty queue checkpoint. All dialect parsing
    /// and resource authorization remain the backend's responsibility.
    pub fn commands(&self) -> &[u8] {
        self.retained.as_ref().map_or(&[], |owned| &owned.commands)
    }

    /// Retire successful work, release its retained ownership, and wake observers.
    ///
    /// # Returns
    ///
    /// Nothing. The driver must first establish retirement of all covered GPU
    /// accesses and earlier work on this queue. This is not a display or cache
    /// barrier. Call outside driver locks used by resource destruction.
    pub fn complete(mut self) {
        drop(self.retained.take());
        if let Some(signal) = self.signal.take() {
            signal.complete();
        }
    }

    /// Report failure while retaining commands, backing, and admission capacity.
    ///
    /// # Arguments
    ///
    /// * `reason` - Terminal failure, not a certificate of hardware quiescence.
    ///
    /// # Returns
    ///
    /// Nothing. The driver must keep this request until hardware retirement or
    /// reset, then call [`Self::retire_failed`]. Subsequent reports do not change
    /// the first terminal observation.
    pub fn fail(&mut self, reason: GpuCompletionFailure) {
        if let Some(signal) = self.signal.take() {
            signal.fail(reason);
        }
    }

    /// Release failed work only after the driver has stopped all covered access.
    ///
    /// # Arguments
    ///
    /// * `reason` - Failure to report if no earlier terminal report exists.
    ///
    /// # Returns
    ///
    /// Nothing. Releases the retained resources and capacity without reporting
    /// success. Call outside driver locks used by resource destruction.
    pub fn retire_failed(mut self, reason: GpuCompletionFailure) {
        self.fail(reason);
        drop(self.retained.take());
    }

    /// A returned Busy/Rejected request certifies that no work was accepted.
    pub(super) fn reject(mut self) {
        drop(self.retained.take());
    }
}

impl Drop for GpuSubmission {
    fn drop(&mut self) {
        if let Some(retained) = self.retained.take() {
            // No retirement proof exists. Keep both the DMA backing and slot
            // permanently reserved; a late device access cannot hit freed pages.
            core::mem::forget(retained);
        }
        // Dropping the unique producer reports abandonment unless already failed.
    }
}

impl core::fmt::Debug for GpuSubmission {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("GpuSubmission")
            .field("command_bytes", &self.commands().len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_PENDING_SUBMISSIONS, SubmissionSlots};

    #[test_case]
    fn admission_slots_are_bounded_and_reclaimed_independently_of_observers() {
        let slots = SubmissionSlots::default();
        assert!(slots.reserve(0).is_none());
        let first = slots.reserve(2).expect("first slot");
        let second = slots.reserve(2).expect("second slot");
        assert!(slots.reserve(2).is_none());
        drop(first);
        let third = slots.reserve(2).expect("retired slot can be reused");
        assert!(slots.reserve(2).is_none());
        drop((second, third));

        let held: alloc::vec::Vec<_> = (0..MAX_PENDING_SUBMISSIONS)
            .map(|_| slots.reserve(u32::MAX).expect("bounded slot"))
            .collect();
        assert!(slots.reserve(u32::MAX).is_none());
        drop(held);
        assert!(slots.reserve(1).is_some());
    }
}
