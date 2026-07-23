//! Per-CPU preemption counter.
//!
//! `preempt_count > 0` marks the current CPU as non-preemptible. While the
//! count is elevated, the scheduler must not switch out the running task.
//! Locks own a `PreemptGuard` to prevent preemption for the duration of
//! their critical sections.
//!
//! # Current Status
//!
//! Scarlet still disables interrupts at trap entry, which already prevents
//! involuntary preemption. `preempt_count` becomes load-bearing once
//! interrupt-enabled trap handlers are introduced; today it documents the
//! "no schedule here" contract that lock holders rely on and gives the
//! scheduler a single predicate to check at safe boundaries.
//!
//! # Ordering
//!
//! Operations use `Relaxed` ordering. The counter is per-CPU and read by
//! the same CPU that writes it; cross-CPU synchronization of the count
//! itself is unnecessary. Reentrancy via interrupt is gated by IRQ state,
//! not by the count value.

use core::marker::PhantomData;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::arch::get_cpu;
use crate::environment::MAX_NUM_CPUS;

static PREEMPT_COUNT: [AtomicU32; MAX_NUM_CPUS] = [const { AtomicU32::new(0) }; MAX_NUM_CPUS];

#[inline]
fn current_cpu() -> usize {
    get_cpu().get_cpuid()
}

/// Return the current CPU's preemption counter.
///
/// A non-zero value means the CPU is inside a non-preemptible section.
///
/// # Returns
///
/// The current preempt count for the executing CPU.
#[inline]
pub fn preempt_count() -> u32 {
    PREEMPT_COUNT[current_cpu()].load(Ordering::Relaxed)
}

/// Return whether the current CPU may be preempted.
///
/// # Returns
///
/// `true` when the preempt count is zero.
#[inline]
pub fn preemptible() -> bool {
    preempt_count() == 0
}

/// Increment the preempt count on the current CPU.
///
/// Must be paired with [`preempt_enable`]. Prefer [`PreemptGuard::new`] for
/// RAII safety.
#[inline]
pub fn preempt_disable() {
    let cpu = current_cpu();
    let prev = PREEMPT_COUNT[cpu].fetch_add(1, Ordering::Relaxed);
    let _ = prev;
}

/// Decrement the preempt count on the current CPU.
///
/// # Panics
///
/// In debug builds, panics on underflow to catch unbalanced
/// `preempt_disable`/`preempt_enable` use.
#[inline]
pub fn preempt_enable() {
    let cpu = current_cpu();
    let prev = PREEMPT_COUNT[cpu].fetch_sub(1, Ordering::Relaxed);
    debug_assert!(prev > 0, "preempt_count underflow on cpu={}", cpu);
    let _ = prev;
}

/// RAII guard that disables preemption while alive.
///
/// Dropping the guard restores the previous preempt state. The guard is
/// `!Send` because preemption state is per-CPU.
///
/// # Examples
///
/// ```
/// let _guard = PreemptGuard::new();
/// // Preemption is disabled for the duration of this scope.
/// ```
pub struct PreemptGuard {
    _not_send: PhantomData<*mut ()>,
}

impl PreemptGuard {
    /// Disable preemption on the current CPU and return a guard that
    /// re-enables it on drop.
    #[inline]
    pub fn new() -> Self {
        preempt_disable();
        Self {
            _not_send: PhantomData,
        }
    }
}

impl Default for PreemptGuard {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PreemptGuard {
    #[inline]
    fn drop(&mut self) {
        preempt_enable();
    }
}

#[cfg(test)]
impl PreemptGuard {
    /// Test-only helper to reset the current CPU's preempt count.
    ///
    /// Callers must ensure no other `PreemptGuard` is live on this CPU.
    pub(crate) fn reset_count_for_test() {
        PREEMPT_COUNT[current_cpu()].store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_preempt_count_starts_zero() {
        PreemptGuard::reset_count_for_test();
        assert_eq!(preempt_count(), 0);
        assert!(preemptible());
    }

    #[test_case]
    fn test_guard_increments_and_decrements() {
        PreemptGuard::reset_count_for_test();
        assert_eq!(preempt_count(), 0);
        {
            let _g = PreemptGuard::new();
            assert_eq!(preempt_count(), 1);
            assert!(!preemptible());
        }
        assert_eq!(preempt_count(), 0);
        assert!(preemptible());
    }

    #[test_case]
    fn test_nested_guards_accumulate() {
        PreemptGuard::reset_count_for_test();
        {
            let _a = PreemptGuard::new();
            assert_eq!(preempt_count(), 1);
            {
                let _b = PreemptGuard::new();
                assert_eq!(preempt_count(), 2);
            }
            assert_eq!(preempt_count(), 1);
        }
        assert_eq!(preempt_count(), 0);
    }

    #[test_case]
    fn test_explicit_disable_enable_balance() {
        PreemptGuard::reset_count_for_test();
        preempt_disable();
        preempt_disable();
        assert_eq!(preempt_count(), 2);
        preempt_enable();
        preempt_enable();
        assert_eq!(preempt_count(), 0);
    }
}
