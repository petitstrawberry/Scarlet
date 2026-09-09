//! Best-effort diagnostic publication, including from reentrant trap handlers.
//!
//! Nothing here waits for another CPU or calls IRQ/preemption instrumentation.
//! A stopped publisher must not stop its observer. Contended operations may be
//! omitted; these types must not own state needed for kernel correctness.
//! Native 64-bit targets use actual atomic words. Other targets protect every
//! wide access with exclusive, try-only ownership, including reads.

use core::cell::UnsafeCell;
#[cfg(target_has_atomic = "64")]
use core::sync::atomic::AtomicU64;
use core::sync::atomic::{AtomicBool, Ordering};

/// Short, copyable diagnostic state whose contended operations can be skipped.
pub(crate) struct TryDiagnosticState<T: Copy> {
    busy: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: All access to value requires the exclusive atomic ownership token.
// No reference to the payload escapes an operation. Sending T suffices to
// transfer its value between CPUs under that ownership.
unsafe impl<T: Copy + Send> Sync for TryDiagnosticState<T> {}

struct Ownership<'a>(&'a AtomicBool);

impl Drop for Ownership<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl<T: Copy> TryDiagnosticState<T> {
    pub const fn new(value: T) -> Self {
        Self {
            busy: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    /// Try one short update. The function must not wait, format, allocate, or
    /// call kernel services. A reentrant operation on this state returns None.
    pub fn try_update<R>(&self, update: impl FnOnce(&mut T) -> R) -> Option<R> {
        self.busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()?;
        let _ownership = Ownership(&self.busy);
        // SAFETY: Exclusive ownership excludes readers as well as writers;
        // release on every exit happens after the mutable borrow ends.
        Some(update(unsafe { &mut *self.value.get() }))
    }

    pub fn snapshot(&self) -> Option<T> {
        self.try_update(|value| *value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PublishedRecord<const N: usize> {
    pub sequence: u64,
    pub words: [u64; N],
}

/// Coherent numeric record with bounded observation and best-effort publication.
pub(crate) struct DiagnosticRecord<const N: usize> {
    #[cfg(target_has_atomic = "64")]
    inner: NativeRecord<N>,
    #[cfg(not(target_has_atomic = "64"))]
    inner: ProtectedRecord<N>,
}

impl<const N: usize> DiagnosticRecord<N> {
    pub const fn new(words: [u64; N]) -> Self {
        Self {
            #[cfg(target_has_atomic = "64")]
            inner: NativeRecord::new(words),
            #[cfg(not(target_has_atomic = "64"))]
            inner: ProtectedRecord::new(words),
        }
    }

    /// Return false on contention or generation exhaustion. Sequence numbers
    /// never wrap, so a read cannot accept an ABA generation after exhaustion.
    pub fn try_publish(&self, words: [u64; N]) -> bool {
        self.inner.try_publish(words)
    }

    /// None means unavailable, not an empty record or zero-valued payload.
    pub fn snapshot(&self) -> Option<PublishedRecord<N>> {
        self.inner.snapshot()
    }
}

#[cfg(target_has_atomic = "64")]
struct NativeRecord<const N: usize> {
    sequence: AtomicU64,
    words: [AtomicU64; N],
}

#[cfg(target_has_atomic = "64")]
impl<const N: usize> NativeRecord<N> {
    const fn new(words: [u64; N]) -> Self {
        let mut result = Self {
            sequence: AtomicU64::new(0),
            words: [const { AtomicU64::new(0) }; N],
        };
        let mut index = 0;
        while index < N {
            result.words[index] = AtomicU64::new(words[index]);
            index += 1;
        }
        result
    }

    fn try_publish(&self, words: [u64; N]) -> bool {
        let sequence = self.sequence.load(Ordering::SeqCst);
        if sequence & 1 != 0 {
            return false;
        }
        let Some(committed) = sequence.checked_add(2) else {
            return false;
        };
        if self
            .sequence
            .compare_exchange(sequence, sequence + 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        // One writer owns the odd generation, including across nested traps.
        // Every payload access is atomic. SeqCst orders the bracketing sequence
        // reads with all payload accesses, so an accepted record is coherent.
        for (destination, value) in self.words.iter().zip(words) {
            destination.store(value, Ordering::SeqCst);
        }
        self.sequence.store(committed, Ordering::SeqCst);
        true
    }

    fn snapshot(&self) -> Option<PublishedRecord<N>> {
        self.snapshot_with_probe(|| {})
    }

    fn snapshot_with_probe(&self, mut after_payload: impl FnMut()) -> Option<PublishedRecord<N>> {
        for _ in 0..4 {
            let sequence = self.sequence.load(Ordering::SeqCst);
            if sequence & 1 != 0 {
                continue;
            }
            let words = core::array::from_fn(|index| self.words[index].load(Ordering::SeqCst));
            after_payload();
            if self.sequence.load(Ordering::SeqCst) == sequence {
                return Some(PublishedRecord { sequence, words });
            }
        }
        None
    }
}

#[cfg(any(test, not(target_has_atomic = "64")))]
struct ProtectedRecord<const N: usize>(TryDiagnosticState<PublishedRecord<N>>);

#[cfg(any(test, not(target_has_atomic = "64")))]
impl<const N: usize> ProtectedRecord<N> {
    const fn new(words: [u64; N]) -> Self {
        Self(TryDiagnosticState::new(PublishedRecord {
            sequence: 0,
            words,
        }))
    }

    fn try_publish(&self, words: [u64; N]) -> bool {
        self.0
            .try_update(|record| {
                let Some(sequence) = record.sequence.checked_add(2) else {
                    return false;
                };
                *record = PublishedRecord { sequence, words };
                true
            })
            .unwrap_or(false)
    }

    fn snapshot(&self) -> Option<PublishedRecord<N>> {
        self.0.snapshot()
    }
}

/// A liveness indicator. On protected targets, contended ticks may be skipped.
/// Never use this for accounting, reference counts, or timer delivery itself.
pub(crate) struct DiagnosticCounter {
    #[cfg(target_has_atomic = "64")]
    value: AtomicU64,
    #[cfg(not(target_has_atomic = "64"))]
    value: TryDiagnosticState<u64>,
}

impl DiagnosticCounter {
    pub const fn new() -> Self {
        Self {
            #[cfg(target_has_atomic = "64")]
            value: AtomicU64::new(0),
            #[cfg(not(target_has_atomic = "64"))]
            value: TryDiagnosticState::new(0),
        }
    }

    pub fn tick(&self) -> Option<u64> {
        #[cfg(target_has_atomic = "64")]
        {
            Some(self.value.fetch_add(1, Ordering::Relaxed).wrapping_add(1))
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            self.value.try_update(|count| {
                *count = count.wrapping_add(1);
                *count
            })
        }
    }

    pub fn reset(&self) {
        #[cfg(target_has_atomic = "64")]
        self.value.store(0, Ordering::Relaxed);
        #[cfg(not(target_has_atomic = "64"))]
        let _ = self.value.try_update(|count| *count = 0);
    }

    pub fn snapshot(&self) -> Option<u64> {
        #[cfg(target_has_atomic = "64")]
        {
            Some(self.value.load(Ordering::Relaxed))
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            self.value.snapshot()
        }
    }
}

/// Rate-limited permission to emit a diagnostic. Contention denies permission.
pub(crate) struct ReportInterval {
    #[cfg(target_has_atomic = "64")]
    last_ns: AtomicU64,
    #[cfg(not(target_has_atomic = "64"))]
    last_ns: TryDiagnosticState<u64>,
}

impl ReportInterval {
    pub const fn new() -> Self {
        Self {
            #[cfg(target_has_atomic = "64")]
            last_ns: AtomicU64::new(0),
            #[cfg(not(target_has_atomic = "64"))]
            last_ns: TryDiagnosticState::new(0),
        }
    }

    pub fn try_claim(&self, now_ns: u64, interval_ns: u64) -> bool {
        fn eligible(last: u64, now: u64, interval: u64) -> bool {
            last == 0 || now.saturating_sub(last) >= interval
        }
        #[cfg(target_has_atomic = "64")]
        {
            let last = self.last_ns.load(Ordering::Relaxed);
            eligible(last, now_ns, interval_ns)
                && self
                    .last_ns
                    .compare_exchange(last, now_ns.max(1), Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
        }
        #[cfg(not(target_has_atomic = "64"))]
        {
            self.last_ns
                .try_update(|last| {
                    if !eligible(*last, now_ns, interval_ns) {
                        return false;
                    }
                    *last = now_ns.max(1);
                    true
                })
                .unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn protected_diagnostics_do_not_wait_for_stopped_or_reentrant_owners() {
        let record = ProtectedRecord::new([0, 0]);
        assert!(record.try_publish([0x1_0000_0001, u64::MAX]));
        record
            .0
            .try_update(|_| {
                assert_eq!(record.snapshot(), None);
                assert!(!record.try_publish([1, 2]));
            })
            .unwrap();
        assert_eq!(record.snapshot().unwrap().words, [0x1_0000_0001, u64::MAX]);
        assert!(record.try_publish([3, 4]));
        assert_eq!(record.snapshot().unwrap().words, [3, 4]);
    }

    #[cfg(target_has_atomic = "64")]
    #[test_case]
    fn native_diagnostics_retry_changed_generations_and_bound_stopped_writers() {
        let record = NativeRecord::new([0, 0]);
        assert!(record.try_publish([1, 2]));
        let mut changed = false;
        let snapshot = record
            .snapshot_with_probe(|| {
                if !changed {
                    changed = true;
                    assert!(record.try_publish([3, 4]));
                }
            })
            .unwrap();
        assert_eq!(snapshot.words, [3, 4]);
        record.sequence.store(5, Ordering::SeqCst);
        assert_eq!(record.snapshot(), None);
        assert!(!record.try_publish([5, 6]));
    }

    #[test_case]
    fn diagnostic_generations_do_not_reuse_exhausted_versions() {
        let protected = ProtectedRecord::new([7]);
        protected
            .0
            .try_update(|record| record.sequence = u64::MAX - 1)
            .unwrap();
        assert!(!protected.try_publish([8]));
        assert_eq!(protected.snapshot().unwrap().words, [7]);
        #[cfg(target_has_atomic = "64")]
        {
            let native = NativeRecord::new([7]);
            native.sequence.store(u64::MAX - 1, Ordering::SeqCst);
            assert!(!native.try_publish([8]));
            assert_eq!(native.snapshot().unwrap().words, [7]);
        }
    }

    #[test_case]
    fn diagnostic_rate_limits_preserve_wide_timestamps() {
        let limit = ReportInterval::new();
        let now = 0x1_0000_0000;
        assert!(limit.try_claim(now, 1_000_000_000));
        assert!(!limit.try_claim(now, 1_000_000_000));
        assert!(!limit.try_claim(now - 1, 1_000_000_000));
        assert!(limit.try_claim(now + 1_000_000_000, 1_000_000_000));
    }
}
