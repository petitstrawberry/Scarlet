//! Owned scheduler measurements and requests, with native wide publication.

#[cfg(not(target_has_atomic = "64"))]
use crate::sync::IrqSpinLock;
use crate::sync::snapshot::{SnapshotCell, SnapshotValue};
#[cfg(target_has_atomic = "64")]
use core::sync::atomic::{AtomicU64, Ordering};

/// Scalar placement observations. Each accessor samples its own measurement;
/// callers must not interpret separate reads as one migration transaction.
pub(super) struct PlacementHistory {
    #[cfg(target_has_atomic = "64")]
    last_ns: AtomicU64,
    #[cfg(target_has_atomic = "64")]
    count: AtomicU64,
    #[cfg(target_has_atomic = "64")]
    low_since_ns: AtomicU64,
    #[cfg(not(target_has_atomic = "64"))]
    state: IrqSpinLock<PlacementState>,
}

#[cfg(not(target_has_atomic = "64"))]
struct PlacementState {
    last_ns: u64,
    count: u64,
    low_since_ns: u64,
}

impl PlacementHistory {
    pub const fn new() -> Self {
        Self {
            #[cfg(target_has_atomic = "64")]
            last_ns: AtomicU64::new(0),
            #[cfg(target_has_atomic = "64")]
            count: AtomicU64::new(0),
            #[cfg(target_has_atomic = "64")]
            low_since_ns: AtomicU64::new(0),
            #[cfg(not(target_has_atomic = "64"))]
            state: IrqSpinLock::new(PlacementState {
                last_ns: 0,
                count: 0,
                low_since_ns: 0,
            }),
        }
    }

    pub fn last_migration_ns(&self) -> u64 {
        #[cfg(target_has_atomic = "64")]
        {
            self.last_ns.load(Ordering::Relaxed)
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            self.state.lock().last_ns
        }
    }

    pub fn migration_count(&self) -> u64 {
        #[cfg(target_has_atomic = "64")]
        {
            self.count.load(Ordering::Relaxed)
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            self.state.lock().count
        }
    }

    pub fn migrated(&self, now_ns: u64) {
        #[cfg(target_has_atomic = "64")]
        {
            self.last_ns.store(now_ns, Ordering::Relaxed);
            let _ = self
                .count
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    Some(count.saturating_add(1))
                });
            self.low_since_ns.store(0, Ordering::Relaxed);
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            let mut state = self.state.lock();
            state.last_ns = now_ns;
            state.count = state.count.saturating_add(1);
            state.low_since_ns = 0;
        }
    }

    pub fn low_since_ns(&self) -> u64 {
        #[cfg(target_has_atomic = "64")]
        {
            self.low_since_ns.load(Ordering::Relaxed)
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            self.state.lock().low_since_ns
        }
    }

    pub fn observe_low_utilization(&self, now_ns: u64) -> u64 {
        let observed = now_ns.max(1);
        #[cfg(target_has_atomic = "64")]
        {
            match self.low_since_ns.compare_exchange(
                0,
                observed,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => observed,
                Err(since) => since,
            }
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            let mut state = self.state.lock();
            if state.low_since_ns == 0 {
                state.low_since_ns = observed;
            }
            state.low_since_ns
        }
    }

    pub fn clear_low_utilization(&self) {
        #[cfg(target_has_atomic = "64")]
        self.low_since_ns.store(0, Ordering::Relaxed);
        #[cfg(not(target_has_atomic = "64"))]
        {
            self.state.lock().low_since_ns = 0;
        }
    }
}

/// A wall-time quantum, retaining its unit and range on every target.
pub(super) struct SchedulingQuantum {
    #[cfg(target_has_atomic = "64")]
    ns: AtomicU64,
    #[cfg(not(target_has_atomic = "64"))]
    ns: IrqSpinLock<u64>,
}

impl SchedulingQuantum {
    pub const fn new(ns: u64) -> Self {
        Self {
            #[cfg(target_has_atomic = "64")]
            ns: AtomicU64::new(ns),
            #[cfg(not(target_has_atomic = "64"))]
            ns: IrqSpinLock::new(ns),
        }
    }
    pub fn duration_ns(&self) -> u64 {
        #[cfg(target_has_atomic = "64")]
        {
            self.ns.load(Ordering::Relaxed)
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            *self.ns.lock()
        }
    }
    pub fn set_duration_ns(&self, ns: u64) {
        #[cfg(target_has_atomic = "64")]
        self.ns.store(ns, Ordering::Relaxed);
        #[cfg(not(target_has_atomic = "64"))]
        {
            *self.ns.lock() = ns;
        }
    }
}

#[derive(Clone, Copy)]
struct ExecutionStart(Option<u64>);

impl SnapshotValue<2> for ExecutionStart {
    fn encode(self) -> [u64; 2] {
        [self.0.unwrap_or(0), u64::from(self.0.is_some())]
    }
    fn decode(words: [u64; 2]) -> Self {
        Self((words[1] != 0).then_some(words[0]))
    }
}

/// Advances the accounting frontier once, including concurrent callers.
pub(crate) struct ExecutionClock(SnapshotCell<ExecutionStart, 2>);

impl ExecutionClock {
    pub fn new() -> Self {
        Self(SnapshotCell::new(ExecutionStart(None)))
    }
    pub fn start(&self, now_ns: u64) {
        self.0.update(|state| state.0 = Some(now_ns));
    }
    pub fn stop(&self) {
        self.0.update(|state| state.0 = None);
    }
    pub fn started_at(&self) -> Option<u64> {
        self.0.read().0
    }
    pub fn advance(&self, now_ns: u64) -> Option<u64> {
        self.0.update(|state| {
            let previous = state.0?;
            state.0 = Some(previous.max(now_ns));
            Some(now_ns.saturating_sub(previous))
        })
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct FairRequestValue {
    pub vruntime: u64,
    pub deadline: u64,
    pub slice_ns: u64,
}

impl SnapshotValue<3> for FairRequestValue {
    fn encode(self) -> [u64; 3] {
        [self.vruntime, self.deadline, self.slice_ns]
    }
    fn decode(words: [u64; 3]) -> Self {
        Self {
            vruntime: words[0],
            deadline: words[1],
            slice_ns: words[2],
        }
    }
}

/// Related fair-request values share one publication. Queue insertion/removal
/// still requires the owning fair-queue lock; request updates never acquire it.
pub(crate) struct FairRequest(SnapshotCell<FairRequestValue, 3>);

impl FairRequest {
    pub fn new() -> Self {
        Self(SnapshotCell::new(FairRequestValue::default()))
    }
    pub fn snapshot(&self) -> FairRequestValue {
        self.0.read()
    }
    pub fn update<R>(&self, update: impl FnOnce(&mut FairRequestValue) -> R) -> R {
        self.0.update(update)
    }
    pub fn reset_request(&self) {
        self.update(|state| {
            state.deadline = 0;
            state.slice_ns = 0;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn scheduler_observations_preserve_wide_time_and_low_utilization_windows() {
        let history = PlacementHistory::new();
        let now = 0x1_0000_0000;
        assert_eq!(history.observe_low_utilization(now), now);
        assert_eq!(history.observe_low_utilization(now + 10), now);
        history.migrated(now + 20);
        assert_eq!(history.last_migration_ns(), now + 20);
        assert_eq!(history.migration_count(), 1);
        assert_eq!(history.low_since_ns(), 0);
        let quantum = SchedulingQuantum::new(now);
        assert_eq!(quantum.duration_ns(), now);
        quantum.set_duration_ns(now + 1);
        assert_eq!(quantum.duration_ns(), now + 1);
    }

    #[test_case]
    fn execution_frontier_charges_zero_start_and_does_not_rewind() {
        let clock = ExecutionClock::new();
        assert_eq!(clock.advance(10), None);
        clock.start(0);
        assert_eq!(clock.advance(10), Some(10));
        assert_eq!(clock.advance(9), Some(0));
        assert_eq!(clock.advance(11), Some(1));
        clock.stop();
        assert_eq!(clock.advance(u64::MAX), None);
    }
}
