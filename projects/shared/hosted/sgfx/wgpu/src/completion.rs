//! WGPU completion receipts, progress driving, and bounded tracking ownership.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

use sgfx_core::backend::{Completion, CompletionStatus};

use super::{Arc, Device, Error, Result, raw};

// This bounds receipts awaiting callback retirement, not all work submitted
// through raw WGPU handles or the retained untracked execution interface.
const TRACKED_SUBMISSION_LIMIT: usize = 64;

#[derive(Default)]
pub(super) struct Tracker {
    pub(super) lost: AtomicBool,
    pending: AtomicUsize,
}

impl Tracker {
    pub(super) fn reserve(self: &Arc<Self>) -> Option<PendingSlot> {
        self.pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                (pending < TRACKED_SUBMISSION_LIMIT).then_some(pending + 1)
            })
            .ok()?;
        Some(PendingSlot(Arc::clone(self)))
    }
}

pub(super) struct PendingSlot(Arc<Tracker>);

impl Drop for PendingSlot {
    fn drop(&mut self) {
        self.0.pending.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Owned completion receipt independent of the executor, uploads and table.
///
/// Dropping this object does not cancel GPU work. WGPU retains submitted
/// resources, and callback retirement owns its tracking slot independently.
/// Use [`Completion`] to observe or wait without reaching through raw WGPU.
#[derive(Clone)]
pub struct Submission {
    device: Device,
    index: raw::SubmissionIndex,
    observation: Arc<AtomicUsize>,
}

impl fmt::Debug for Submission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Submission")
            .field("observation", &self.observation.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl Submission {
    pub(super) fn new(
        device: Device,
        index: raw::SubmissionIndex,
        marker: raw::Buffer,
        slot: PendingSlot,
    ) -> Self {
        let observation = Arc::new(AtomicUsize::new(0));
        let callback_observation = Arc::clone(&observation);
        // A private marker is last written by this submission. Mapping it
        // observes that exact submission, unlike on_submitted_work_done alone,
        // which can attach to a later concurrent raw-queue submit.
        #[cfg(target_arch = "wasm32")]
        let retained_marker = marker.clone();
        marker
            .slice(..)
            .map_async(raw::MapMode::Read, move |result| {
                callback_observation.store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
                #[cfg(target_arch = "wasm32")]
                drop(retained_marker);
            });
        device.raw_queue().on_submitted_work_done(move || {
            // Retain the marker independently of public receipts so dropping
            // one cannot cancel mapping and falsely return tracking capacity.
            // WGPU runs earlier registered map callbacks before this callback.
            #[cfg(not(target_arch = "wasm32"))]
            drop(marker);
            drop(slot);
        });
        Self {
            device,
            index,
            observation,
        }
    }

    fn status(&self) -> Result<CompletionStatus> {
        // WGPU may drain work callbacks before its device-lost callback.
        // Inspect loss only after the progress call has dispatched both.
        if self.device.tracker.lost.load(Ordering::Acquire) {
            Err(Error::DeviceLost)
        } else {
            match self.observation.load(Ordering::Acquire) {
                0 => Ok(CompletionStatus::Pending),
                1 => Ok(CompletionStatus::Complete),
                _ => Err(Error::CompletionObservation),
            }
        }
    }
}

impl Completion for Submission {
    type Error = Error;

    fn poll(&self) -> Result<CompletionStatus> {
        let _ = self.device.raw_device().poll(raw::Maintain::Poll);
        self.status()
    }

    fn wait(&self, timeout: Option<Duration>) -> Result<CompletionStatus> {
        let start = Instant::now();
        if self.poll()? == CompletionStatus::Complete {
            return Ok(CompletionStatus::Complete);
        }
        if cfg!(target_arch = "wasm32") && timeout != Some(Duration::ZERO) {
            return Err(Error::Unsupported(super::UnsupportedFeature::BlockingWait));
        }
        let Some(timeout) = timeout else {
            let _ = self
                .device
                .raw_device()
                .poll(raw::Maintain::WaitForSubmissionIndex(self.index.clone()));
            return self.status();
        };
        loop {
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return Ok(CompletionStatus::Pending);
            }
            // WGPU 24 has no deadline parameter for its blocking poll. Drive
            // nonblocking progress with bounded sleeps instead of busy-spinning
            // or turning a finite wait into an unbounded driver wait.
            std::thread::sleep(remaining.min(Duration::from_millis(1)));
            let status = self.poll()?;
            if status == CompletionStatus::Complete {
                return Ok(status);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Arc, Ordering, TRACKED_SUBMISSION_LIMIT, Tracker};

    #[test]
    fn bounded_tracking_releases_capacity_through_its_owner() {
        let tracker = Arc::new(Tracker::default());
        let mut slots: Vec<_> = (0..TRACKED_SUBMISSION_LIMIT)
            .map(|_| tracker.reserve().expect("reserve slot"))
            .collect();
        assert!(tracker.reserve().is_none());
        drop(slots.pop());
        let recovered = tracker.reserve().expect("reclaimed slot");
        assert!(tracker.reserve().is_none());
        drop(slots);
        drop(recovered);
        assert_eq!(tracker.pending.load(Ordering::Acquire), 0);
    }
}
