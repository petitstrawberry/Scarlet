//! Cumulative measurements that retain 64 bits on every supported target.
//!
//! Counters saturate instead of wrapping. A snapshot describes only the count;
//! it is not an acquire operation for publishing other objects. Native 64-bit
//! atomics avoid locks. The protected implementation uses an IRQ-safe lock and
//! is unsuitable for emergency diagnostics or lock-instrumentation internals.

#[cfg(target_has_atomic = "64")]
use core::sync::atomic::{AtomicU64, Ordering};

/// A concurrently updated cumulative measurement, saturated at `u64::MAX`.
pub(crate) struct SaturatingCounter {
    #[cfg(target_has_atomic = "64")]
    value: AtomicU64,
    #[cfg(not(target_has_atomic = "64"))]
    value: ProtectedCounter,
}

impl SaturatingCounter {
    pub const fn new(value: u64) -> Self {
        Self {
            #[cfg(target_has_atomic = "64")]
            value: AtomicU64::new(value),
            #[cfg(not(target_has_atomic = "64"))]
            value: ProtectedCounter::new(value),
        }
    }

    /// Add a measurement and return the total at this operation's update.
    pub fn add(&self, amount: u64) -> u64 {
        #[cfg(target_has_atomic = "64")]
        {
            self.value
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    Some(current.saturating_add(amount))
                })
                .expect("counter update always produces a value")
                .saturating_add(amount)
        }
        #[cfg(not(target_has_atomic = "64"))]
        self.value.add(amount)
    }

    pub fn snapshot(&self) -> u64 {
        #[cfg(target_has_atomic = "64")]
        {
            self.value.load(Ordering::Relaxed)
        }
        #[cfg(not(target_has_atomic = "64"))]
        self.value.snapshot()
    }
}

impl core::fmt::Debug for SaturatingCounter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("SaturatingCounter")
            .field(&self.snapshot())
            .finish()
    }
}

#[cfg(any(test, not(target_has_atomic = "64")))]
struct ProtectedCounter {
    value: crate::sync::IrqSpinLock<u64>,
}

#[cfg(any(test, not(target_has_atomic = "64")))]
impl ProtectedCounter {
    const fn new(value: u64) -> Self {
        Self {
            value: crate::sync::IrqSpinLock::new(value),
        }
    }

    fn add(&self, amount: u64) -> u64 {
        let mut value = self.value.lock();
        *value = value.saturating_add(amount);
        *value
    }

    fn snapshot(&self) -> u64 {
        *self.value.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_boundaries(add: impl Fn(u64) -> u64, snapshot: impl Fn() -> u64) {
        assert_eq!(add(3), 0x1_0000_0001);
        assert_eq!(snapshot(), 0x1_0000_0001);
        assert_eq!(add(0), 0x1_0000_0001);
        assert_eq!(add(u64::MAX), u64::MAX);
        assert_eq!(add(1), u64::MAX);
        assert_eq!(snapshot(), u64::MAX);
    }

    #[test_case]
    fn cumulative_measurements_cross_pointer_width_and_saturate() {
        let selected = SaturatingCounter::new(u32::MAX as u64 - 1);
        check_boundaries(|amount| selected.add(amount), || selected.snapshot());
        let protected = ProtectedCounter::new(u32::MAX as u64 - 1);
        check_boundaries(|amount| protected.add(amount), || protected.snapshot());
    }
}
