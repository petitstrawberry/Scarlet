//! Interrupt guard for SMP-safe per-CPU data access.
//!
//! Per-CPU depth and saved-state entries use `Relaxed` ordering because local
//! IRQ masking and the acquired preemption token pin access to one CPU. They
//! are not used for cross-CPU synchronization.

use core::marker::PhantomData;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::arch::try_get_cpuid;
use crate::environment::MAX_NUM_CPUS;
use crate::sync::preempt::PreemptGuard;

static IRQ_DEPTH: [AtomicU32; MAX_NUM_CPUS] = [const { AtomicU32::new(0) }; MAX_NUM_CPUS];
static SAVED_IRQ_STATE: [AtomicUsize; MAX_NUM_CPUS] = [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];

/// RAII token that masks local interrupts and disables preemption.
///
/// Every CPU maintains one shared nesting depth for all live `IrqGuard`s.
/// The outermost guard saves the prior IRQ state, and the final guard restores
/// it. This keeps IRQ masking correct even when guards are dropped out of
/// acquisition order.
///
/// The guard is `!Send` because both IRQ state and preemption state belong to
/// the acquiring CPU.
pub struct IrqGuard {
    cpu: usize,
    preempt: Option<PreemptGuard>,
    _not_send: PhantomData<*mut ()>,
}

impl IrqGuard {
    /// Mask local interrupts and disable preemption on the current CPU.
    ///
    /// # Panics
    ///
    /// Panics when the per-CPU identity has not been published yet, or when
    /// the per-CPU nesting depth would overflow.
    ///
    /// # Returns
    ///
    /// A CPU-bound token that keeps local interrupts masked until the final
    /// nested token on that CPU is dropped.
    #[inline]
    pub fn new() -> Self {
        let cpu = try_get_cpuid().expect("IrqGuard requires an initialized per-CPU identity");
        let saved_state = crate::arch::interrupt::save_and_disable_interrupts();
        let preempt = PreemptGuard::new();
        assert_eq!(
            try_get_cpuid(),
            Some(cpu),
            "IrqGuard crossed CPUs during acquisition"
        );
        let previous =
            match IRQ_DEPTH[cpu].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |depth| {
                depth.checked_add(1)
            }) {
                Ok(previous) => previous,
                Err(_) => {
                    drop(preempt);
                    crate::arch::interrupt::restore_interrupts(saved_state);
                    panic!("IRQ guard nesting overflow on cpu={}", cpu);
                }
            };
        if previous == 0 {
            SAVED_IRQ_STATE[cpu].store(saved_state, Ordering::Relaxed);
        }
        Self {
            cpu,
            preempt: Some(preempt),
            _not_send: PhantomData,
        }
    }
}

impl Drop for IrqGuard {
    fn drop(&mut self) {
        assert_eq!(
            try_get_cpuid(),
            Some(self.cpu),
            "IrqGuard dropped on a different CPU"
        );
        let previous = IRQ_DEPTH[self.cpu]
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |depth| {
                depth.checked_sub(1)
            })
            .expect("IRQ guard nesting underflow");
        debug_assert!(previous > 0);

        drop(self.preempt.take());

        if previous == 1 {
            let saved_state = SAVED_IRQ_STATE[self.cpu].load(Ordering::Relaxed);
            crate::arch::interrupt::restore_interrupts(saved_state);
        }
    }
}

#[cfg(test)]
impl IrqGuard {
    /// Return the current CPU's IRQ-guard nesting depth for tests.
    ///
    /// # Panics
    ///
    /// Panics when the current CPU has not published its per-CPU identity.
    pub(crate) fn depth_for_test() -> u32 {
        let cpu = try_get_cpuid().expect("IRQ guard test requires an initialized CPU");
        IRQ_DEPTH[cpu].load(Ordering::Relaxed)
    }

    /// Reset the current CPU's IRQ-guard test state.
    ///
    /// Callers must ensure no `IrqGuard` is live on the current CPU.
    ///
    /// # Panics
    ///
    /// Panics when the current CPU has not published its per-CPU identity or
    /// when a live guard would be discarded.
    pub(crate) fn reset_for_test() {
        let cpu = try_get_cpuid().expect("IRQ guard test requires an initialized CPU");
        assert_eq!(
            IRQ_DEPTH[cpu].load(Ordering::Relaxed),
            0,
            "cannot reset live IRQ guard state"
        );
        SAVED_IRQ_STATE[cpu].store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::preempt::{PreemptGuard, preempt_count};

    #[test_case]
    fn test_non_lifo_irq_guards_hold_the_outermost_irq_state() {
        IrqGuard::reset_for_test();
        PreemptGuard::reset_count_for_test();

        let first = IrqGuard::new();
        let second = IrqGuard::new();
        assert_eq!(IrqGuard::depth_for_test(), 2);
        assert_eq!(preempt_count(), 2);

        drop(first);
        assert_eq!(IrqGuard::depth_for_test(), 1);
        assert_eq!(preempt_count(), 1);

        drop(second);
        assert_eq!(IrqGuard::depth_for_test(), 0);
        assert_eq!(preempt_count(), 0);
    }
}
