//! Atomic storage for values whose range really is 64 bits.
//!
//! Prefer native word atomics for addresses, indices and bounded masks. Values
//! already protected by a lock belong inside that lock; immutable published
//! values belong in `Once`. Time, cumulative statistics and generation numbers
//! that must retain their 64-bit range can use this module.
//!
//! Selection depends on atomic capabilities, not pointer width. On targets
//! without 64-bit atomics, operations mask local interrupts and take a private
//! 32-bit lock. They are therefore **not lock-free**. Diagnostics observing a
//! potentially stopped CPU must use [`try_load_u64`] instead of waiting.
//! Storage is opaque kernel memory, never an MMIO register, PTE or ABI record.

use core::sync::atomic::Ordering;

#[cfg(target_has_atomic = "64")]
pub use core::sync::atomic::AtomicU64;

#[cfg(any(not(target_has_atomic = "64"), test))]
#[path = "atomic64_fallback.rs"]
mod fallback;
#[cfg(not(target_has_atomic = "64"))]
pub use fallback::AtomicU64;

/// Sample a 64-bit value without waiting for a software-lock owner.
///
/// Returns `None` on contention on targets without native 64-bit atomics.
/// This is a scalar sample; callers must still validate any enclosing snapshot
/// generation. Valid load orderings match `core::sync::atomic::AtomicU64`.
#[inline]
pub fn try_load_u64(value: &AtomicU64, order: Ordering) -> Option<u64> {
    #[cfg(target_has_atomic = "64")]
    {
        Some(value.load(order))
    }
    #[cfg(not(target_has_atomic = "64"))]
    {
        value.try_load(order)
    }
}

/// Attempt a CAS for diagnostic state without waiting for a stopped owner.
///
/// `None` means the software lock is busy. `Some(Err(observed))` means the
/// expected value differed; `Some(Ok(previous))` means the update succeeded.
#[inline]
pub fn try_compare_exchange_u64(
    value: &AtomicU64,
    current: u64,
    new: u64,
    success: Ordering,
    failure: Ordering,
) -> Option<Result<u64, u64>> {
    #[cfg(target_has_atomic = "64")]
    {
        Some(value.compare_exchange(current, new, success, failure))
    }
    #[cfg(not(target_has_atomic = "64"))]
    {
        value.try_compare_exchange(current, new, success, failure)
    }
}
