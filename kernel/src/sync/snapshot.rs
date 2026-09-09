//! Coherent snapshots of related numeric state.
//!
//! Writers update a private copy under one IRQ-safe lock and publish it before
//! releasing that lock. Targets with native 64-bit atomics can read the last
//! completed publication without acquiring the writer lock. Other targets copy
//! the protected state. This is a transaction API, not a substitute for core
//! atomic types or their progress guarantees.

use super::IrqSpinLock;
#[cfg(target_has_atomic = "64")]
use core::sync::atomic::{AtomicU64, Ordering};

/// Numeric encoding used only inside a snapshot owner, not as a wire ABI.
/// Decoding a value's encoding must reproduce that value.
pub(crate) trait SnapshotValue<const N: usize>: Copy {
    fn encode(self) -> [u64; N];
    fn decode(words: [u64; N]) -> Self;
}

pub(crate) struct SnapshotCell<T: SnapshotValue<N>, const N: usize> {
    state: IrqSpinLock<T>,
    #[cfg(target_has_atomic = "64")]
    sequence: AtomicU64,
    #[cfg(target_has_atomic = "64")]
    words: [AtomicU64; N],
}

impl<T: SnapshotValue<N>, const N: usize> SnapshotCell<T, N> {
    pub fn new(value: T) -> Self {
        Self {
            state: IrqSpinLock::new(value),
            #[cfg(target_has_atomic = "64")]
            sequence: AtomicU64::new(0),
            #[cfg(target_has_atomic = "64")]
            words: value.encode().map(AtomicU64::new),
        }
    }

    /// Complete one transaction. The function must contain only bounded value
    /// manipulation: no clock reads, allocation, callbacks, or other locks.
    /// It must not recursively access this cell. No borrowed state escapes.
    pub fn update<R>(&self, update: impl FnOnce(&mut T) -> R) -> R {
        let mut state = self.state.lock();
        let mut next = *state;
        let result = update(&mut next);
        #[cfg(target_has_atomic = "64")]
        {
            let words = next.encode();
            let sequence = self.sequence.load(Ordering::Relaxed);
            let committed = sequence
                .checked_add(2)
                .expect("snapshot generation exhausted");
            // The owner lock serializes writers and masks local IRQs. Mark
            // publication busy only for the atomic copies below; readers can
            // still read the previous publication during value computation.
            self.sequence.store(sequence + 1, Ordering::SeqCst);
            for (destination, value) in self.words.iter().zip(words) {
                destination.store(value, Ordering::SeqCst);
            }
            self.sequence.store(committed, Ordering::SeqCst);
        }
        *state = next;
        result
    }

    /// Read a coherent publication. May wait for a competing CPU's update.
    pub fn read(&self) -> T {
        #[cfg(target_has_atomic = "64")]
        {
            loop {
                if let Some(value) = self.try_read() {
                    return value;
                }
                core::hint::spin_loop();
            }
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            *self.state.lock()
        }
    }

    /// Bounded observation for callers that may skip a sample. The protected
    /// path uses the ordinary IRQ lock's try operation, so this API requires
    /// per-CPU initialization and must not be used by lock/clock instrumentation.
    pub fn try_read(&self) -> Option<T> {
        #[cfg(target_has_atomic = "64")]
        {
            self.try_read_with_probe(|| {})
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            self.state.try_lock().map(|state| *state)
        }
    }

    #[cfg(target_has_atomic = "64")]
    fn try_read_with_probe(&self, mut after_payload: impl FnMut()) -> Option<T> {
        // All payload accesses are atomic; speculative ordinary reads would
        // be a data race even if a changed sequence caused their rejection.
        for _ in 0..4 {
            let sequence = self.sequence.load(Ordering::SeqCst);
            if sequence & 1 != 0 {
                continue;
            }
            let words = core::array::from_fn(|index| self.words[index].load(Ordering::SeqCst));
            after_payload();
            if self.sequence.load(Ordering::SeqCst) == sequence {
                return Some(T::decode(words));
            }
        }
        None
    }
}

impl<T: SnapshotValue<N> + core::fmt::Debug, const N: usize> core::fmt::Debug
    for SnapshotCell<T, N>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Release protection before invoking a formatter.
        f.debug_tuple("SnapshotCell")
            .field(&self.try_read())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Interval {
        completed: u64,
        start: u64,
    }

    impl SnapshotValue<2> for Interval {
        fn encode(self) -> [u64; 2] {
            [self.completed, self.start]
        }
        fn decode(words: [u64; 2]) -> Self {
            Self {
                completed: words[0],
                start: words[1],
            }
        }
    }

    #[test_case]
    fn snapshot_publishes_completed_transactions_with_wide_values() {
        let before = Interval {
            completed: 0x1_0000_0000,
            start: 0x2_0000_0000,
        };
        let cell = SnapshotCell::new(before);
        cell.update(|state| {
            state.completed += 7;
            #[cfg(target_has_atomic = "64")]
            assert_eq!(cell.try_read(), Some(before));
            #[cfg(not(target_has_atomic = "64"))]
            assert_eq!(cell.try_read(), None);
            state.start = 0;
        });
        assert_eq!(
            cell.read(),
            Interval {
                completed: before.completed + 7,
                start: 0
            }
        );
    }

    #[cfg(target_has_atomic = "64")]
    #[test_case]
    fn native_snapshot_does_not_mix_committed_time_with_an_old_active_interval() {
        let cell = SnapshotCell::new(Interval {
            completed: 10,
            start: 100,
        });
        let mut switched = false;
        let observed = cell
            .try_read_with_probe(|| {
                if !switched {
                    switched = true;
                    cell.update(|state| {
                        state.completed = 20;
                        state.start = 0;
                    });
                }
            })
            .unwrap();
        assert_eq!(
            observed,
            Interval {
                completed: 20,
                start: 0
            }
        );
    }
}
