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
#[cfg(feature = "sync-debug")]
use core::panic::Location;
#[cfg(feature = "sync-debug")]
use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize};
use core::sync::atomic::{AtomicU32, Ordering};

use crate::arch::try_get_cpuid;
use crate::environment::MAX_NUM_CPUS;

static PREEMPT_COUNT: [AtomicU32; MAX_NUM_CPUS] = [const { AtomicU32::new(0) }; MAX_NUM_CPUS];

/// Origin of a preemption guard, used by the optional sync diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum PreemptSourceKind {
    Explicit = 1,
    IrqGuard = 2,
    SpinLock = 3,
    IrqSpinLock = 4,
    RwSpinLockRead = 5,
    RwSpinLockWrite = 6,
    IrqRwSpinLockRead = 7,
    IrqRwSpinLockWrite = 8,
}

#[cfg(feature = "sync-debug")]
impl PreemptSourceKind {
    fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            1 => Some(Self::Explicit),
            2 => Some(Self::IrqGuard),
            3 => Some(Self::SpinLock),
            4 => Some(Self::IrqSpinLock),
            5 => Some(Self::RwSpinLockRead),
            6 => Some(Self::RwSpinLockWrite),
            7 => Some(Self::IrqRwSpinLockRead),
            8 => Some(Self::IrqRwSpinLockWrite),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Explicit => "PreemptGuard",
            Self::IrqGuard => "IrqGuard",
            Self::SpinLock => "SpinLock",
            Self::IrqSpinLock => "IrqSpinLock",
            Self::RwSpinLockRead => "RwSpinLock(read)",
            Self::RwSpinLockWrite => "RwSpinLock(write)",
            Self::IrqRwSpinLockRead => "IrqRwSpinLock(read)",
            Self::IrqRwSpinLockWrite => "IrqRwSpinLock(write)",
        }
    }
}

#[cfg(feature = "sync-debug")]
const PREEMPT_DEBUG_SLOT_COUNT: usize = 32;
#[cfg(feature = "sync-debug")]
const DEBUG_SLOT_EMPTY: u8 = 0;
#[cfg(feature = "sync-debug")]
const DEBUG_SLOT_WRITING: u8 = 1;
#[cfg(feature = "sync-debug")]
const DEBUG_SLOT_ACTIVE: u8 = 2;

#[cfg(feature = "sync-debug")]
struct PreemptDebugSlot {
    state: AtomicU8,
    source: AtomicU8,
    lock_address: AtomicUsize,
    location: AtomicPtr<Location<'static>>,
}

#[cfg(feature = "sync-debug")]
impl PreemptDebugSlot {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(DEBUG_SLOT_EMPTY),
            source: AtomicU8::new(0),
            lock_address: AtomicUsize::new(0),
            location: AtomicPtr::new(core::ptr::null_mut()),
        }
    }
}

#[cfg(feature = "sync-debug")]
static PREEMPT_DEBUG_SLOTS: [[PreemptDebugSlot; PREEMPT_DEBUG_SLOT_COUNT]; MAX_NUM_CPUS] =
    [const { [const { PreemptDebugSlot::new() }; PREEMPT_DEBUG_SLOT_COUNT] }; MAX_NUM_CPUS];
#[cfg(feature = "sync-debug")]
static PREEMPT_DEBUG_UNTRACKED: [AtomicU32; MAX_NUM_CPUS] =
    [const { AtomicU32::new(0) }; MAX_NUM_CPUS];

#[cfg(feature = "sync-debug")]
fn register_preempt_source(
    cpu: usize,
    source: PreemptSourceKind,
    lock_address: usize,
    location: &'static Location<'static>,
) -> Option<u8> {
    for (index, slot) in PREEMPT_DEBUG_SLOTS[cpu].iter().enumerate() {
        if slot
            .state
            .compare_exchange(
                DEBUG_SLOT_EMPTY,
                DEBUG_SLOT_WRITING,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_err()
        {
            continue;
        }

        slot.source.store(source as u8, Ordering::Relaxed);
        slot.lock_address.store(lock_address, Ordering::Relaxed);
        slot.location.store(
            location as *const Location<'static> as *mut Location<'static>,
            Ordering::Relaxed,
        );
        slot.state.store(DEBUG_SLOT_ACTIVE, Ordering::Release);
        return Some(index as u8);
    }

    PREEMPT_DEBUG_UNTRACKED[cpu].fetch_add(1, Ordering::Relaxed);
    None
}

#[cfg(feature = "sync-debug")]
fn unregister_preempt_source(cpu: usize, slot_index: Option<u8>) {
    let Some(slot_index) = slot_index else {
        let previous = PREEMPT_DEBUG_UNTRACKED[cpu].fetch_sub(1, Ordering::Relaxed);
        debug_assert!(previous > 0);
        return;
    };

    let slot = &PREEMPT_DEBUG_SLOTS[cpu][slot_index as usize];
    let previous = slot.state.swap(DEBUG_SLOT_EMPTY, Ordering::AcqRel);
    debug_assert_eq!(previous, DEBUG_SLOT_ACTIVE);
}

#[inline]
fn current_cpu() -> Option<usize> {
    try_get_cpuid()
}

#[inline]
fn increment_preempt_count(cpu: usize) {
    let previous = PREEMPT_COUNT[cpu]
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            count.checked_add(1)
        })
        .expect("preempt_count overflow");
    debug_assert!(previous < u32::MAX);
}

#[inline]
fn decrement_preempt_count(cpu: usize) {
    let previous = PREEMPT_COUNT[cpu]
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            count.checked_sub(1)
        })
        .expect("preempt_count underflow");
    debug_assert!(previous > 0);
}

/// Return the current CPU's preemption counter.
///
/// # Returns
///
/// The current preempt count for the executing CPU, or `0` if the per-CPU
/// substrate has not been published yet (early boot, before
/// `sscratch`/`TPIDR_EL1` is programmed).
#[inline]
pub fn preempt_count() -> u32 {
    match current_cpu() {
        Some(cpu) => PREEMPT_COUNT[cpu].load(Ordering::Relaxed),
        None => 0,
    }
}

/// Return whether the current CPU may be preempted.
///
/// # Returns
///
/// `true` when the preempt count is zero. Also `true` while the per-CPU
/// substrate is uninitialized, since lock operations are no-ops in that
/// window.
#[inline]
pub fn preemptible() -> bool {
    preempt_count() == 0
}

/// Increment the preempt count on the current CPU.
///
/// Must be paired with [`preempt_enable`]. Prefer [`PreemptGuard::new`] for
/// RAII safety. No-op when the per-CPU substrate has not been published,
/// so callers on an uninitialized CPU behave as plain busy-wait spinlocks.
#[inline]
pub fn preempt_disable() {
    if let Some(cpu) = current_cpu() {
        increment_preempt_count(cpu);
    }
}

/// Decrement the preempt count on the current CPU.
///
/// # Panics
///
/// Panics on underflow to catch unbalanced
/// `preempt_disable`/`preempt_enable` use. No-op when the per-CPU substrate
/// has not been published.
#[inline]
pub fn preempt_enable() {
    if let Some(cpu) = current_cpu() {
        decrement_preempt_count(cpu);
    }
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
    cpu: Option<usize>,
    #[cfg(feature = "sync-debug")]
    debug_slot: Option<u8>,
    _not_send: PhantomData<*mut ()>,
}

impl PreemptGuard {
    /// Disable preemption on the current CPU and return a guard that
    /// re-enables it on drop.
    ///
    /// # Returns
    ///
    /// A guard bound to the current CPU. If the per-CPU substrate has not
    /// been published yet, the guard remains unarmed and requires that state
    /// to stay uninitialized until it is dropped.
    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn new() -> Self {
        Self::new_with_source(PreemptSourceKind::Explicit, 0)
    }

    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub(crate) fn new_with_source(source: PreemptSourceKind, lock_address: usize) -> Self {
        let cpu = current_cpu();
        if let Some(cpu) = cpu {
            increment_preempt_count(cpu);
        }
        #[cfg(feature = "sync-debug")]
        let caller = Location::caller();
        #[cfg(feature = "sync-debug")]
        let debug_slot =
            cpu.and_then(|cpu| register_preempt_source(cpu, source, lock_address, caller));
        #[cfg(not(feature = "sync-debug"))]
        let _ = (source, lock_address);
        Self {
            cpu,
            #[cfg(feature = "sync-debug")]
            debug_slot,
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
        match self.cpu {
            Some(cpu) => {
                assert_eq!(
                    current_cpu(),
                    Some(cpu),
                    "PreemptGuard dropped on a different CPU"
                );
                #[cfg(feature = "sync-debug")]
                unregister_preempt_source(cpu, self.debug_slot.take());
                decrement_preempt_count(cpu);
            }
            None => {
                assert!(
                    current_cpu().is_none(),
                    "unarmed PreemptGuard crossed into initialized per-CPU state"
                );
            }
        }
    }
}

/// Print every live preemption guard on the current CPU.
///
/// With `sync-debug` disabled this compiles to a no-op. The diagnostic path
/// does not allocate and tolerates guards being dropped out of acquisition
/// order.
#[inline]
pub fn dump_active_preempt_guards() {
    #[cfg(feature = "sync-debug")]
    {
        let Some(cpu) = current_cpu() else {
            crate::early_println!("[sync-debug] per-CPU state is not initialized");
            return;
        };
        let count = PREEMPT_COUNT[cpu].load(Ordering::Relaxed);
        let untracked = PREEMPT_DEBUG_UNTRACKED[cpu].load(Ordering::Relaxed);
        crate::early_println!(
            "[sync-debug] cpu={} preempt_count={} active guard(s):",
            cpu,
            count
        );

        let mut tracked = 0u32;
        for (index, slot) in PREEMPT_DEBUG_SLOTS[cpu].iter().enumerate() {
            if slot.state.load(Ordering::Acquire) != DEBUG_SLOT_ACTIVE {
                continue;
            }
            tracked = tracked.saturating_add(1);
            let source = PreemptSourceKind::from_raw(slot.source.load(Ordering::Relaxed));
            let lock_address = slot.lock_address.load(Ordering::Relaxed);
            let location_ptr = slot.location.load(Ordering::Relaxed);
            let Some(source) = source else {
                crate::early_println!(
                    "[sync-debug]   slot={} kind=<invalid> lock={:#x}",
                    index,
                    lock_address
                );
                continue;
            };
            if location_ptr.is_null() {
                crate::early_println!(
                    "[sync-debug]   slot={} kind={} lock={:#x} at <unknown>",
                    index,
                    source.name(),
                    lock_address
                );
                continue;
            }

            // SAFETY: `Location::caller()` returns a reference with static
            // lifetime. A slot remains active until its owning guard drops.
            let location = unsafe { &*location_ptr };
            crate::early_println!(
                "[sync-debug]   slot={} kind={} lock={:#x} at {}:{}:{}",
                index,
                source.name(),
                lock_address,
                location.file(),
                location.line(),
                location.column()
            );
        }

        if untracked != 0 || count != tracked.saturating_add(untracked) {
            crate::early_println!(
                "[sync-debug]   tracked={} untracked={} count_delta={}",
                tracked,
                untracked,
                count as i64 - tracked.saturating_add(untracked) as i64
            );
        }
    }
}

#[cfg(test)]
impl PreemptGuard {
    /// Test-only helper to reset the current CPU's preempt count.
    ///
    /// Callers must ensure no other `PreemptGuard` is live on this CPU.
    pub(crate) fn reset_count_for_test() {
        if let Some(cpu) = current_cpu() {
            #[cfg(feature = "sync-debug")]
            {
                for slot in &PREEMPT_DEBUG_SLOTS[cpu] {
                    assert_eq!(
                        slot.state.load(Ordering::Relaxed),
                        DEBUG_SLOT_EMPTY,
                        "cannot reset live preemption diagnostic state"
                    );
                }
                assert_eq!(
                    PREEMPT_DEBUG_UNTRACKED[cpu].load(Ordering::Relaxed),
                    0,
                    "cannot reset untracked live preemption guards"
                );
            }
            PREEMPT_COUNT[cpu].store(0, Ordering::Relaxed);
        }
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
    fn test_non_lifo_guards_decrement_their_acquisition_cpu() {
        PreemptGuard::reset_count_for_test();

        let first = PreemptGuard::new();
        let second = PreemptGuard::new();
        assert_eq!(preempt_count(), 2);

        drop(first);
        assert_eq!(preempt_count(), 1);

        drop(second);
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
