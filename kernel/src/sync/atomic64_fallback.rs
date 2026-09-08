//! Software implementation used when the target has word CAS but no u64 atomics.
//!
//! The architecture's raw interrupt save/restore functions must mask every
//! interrupt class that can access this storage, act as compiler memory
//! barriers, and neither allocate, schedule nor enter the synchronization
//! diagnostics. Non-maskable handlers must not access these values.

#[cfg(not(target_has_atomic = "32"))]
compile_error!(
    "software AtomicU64 requires 32-bit CAS; no-CAS targets need a separate synchronization backend"
);

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::arch::interrupt::{restore_interrupts, save_and_disable_interrupts};

/// An IRQ-safe, SMP-safe 64-bit scalar backed by a private word lock.
///
/// Operations provide at least the requested ordering (the fallback uses
/// SeqCst throughout). This type is not lock-free and has no native-atomic
/// layout guarantee. Do not cast a pointer to a `u64` into a pointer to this type.
pub struct AtomicU64 {
    locked: AtomicU32,
    value: UnsafeCell<u64>,
}

// SAFETY: Every shared access to value holds locked. Local IRQ masking prevents
// a same-CPU interrupt from waiting for the interrupted owner; word CAS excludes
// other CPUs. No reference into value is exposed while shared.
unsafe impl Sync for AtomicU64 {}

struct Guard<'a> {
    locked: &'a AtomicU32,
    saved_irq_state: usize,
    _not_send: PhantomData<*mut ()>,
}

impl Drop for Guard<'_> {
    #[inline]
    fn drop(&mut self) {
        // Release ownership before an interrupt can reenter on this CPU.
        self.locked.store(0, Ordering::SeqCst);
        restore_interrupts(self.saved_irq_state);
    }
}

impl AtomicU64 {
    pub const fn new(value: u64) -> Self {
        Self {
            locked: AtomicU32::new(0),
            value: UnsafeCell::new(value),
        }
    }

    #[inline]
    fn lock(&self) -> Guard<'_> {
        let saved_irq_state = save_and_disable_interrupts();
        while self
            .locked
            .compare_exchange_weak(0, 1, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) != 0 {
                // Do not use IrqGuard, SpinLock or note_spin_contention here:
                // their diagnostics themselves use 64-bit storage.
                core::hint::spin_loop();
            }
        }
        Guard {
            locked: &self.locked,
            saved_irq_state,
            _not_send: PhantomData,
        }
    }

    #[inline]
    fn with_value<R>(&self, f: impl FnOnce(&mut u64) -> R) -> R {
        let _guard = self.lock();
        // SAFETY: The guard excludes other CPUs and local IRQ reentrancy.
        f(unsafe { &mut *self.value.get() })
    }

    #[inline]
    pub fn load(&self, order: Ordering) -> u64 {
        assert_load_order(order);
        self.with_value(|value| *value)
    }

    /// Make one acquisition attempt, returning `None` if the value is busy.
    #[inline]
    pub fn try_load(&self, order: Ordering) -> Option<u64> {
        assert_load_order(order);
        let _guard = self.try_lock()?;
        // SAFETY: The guard holds the word lock and masks local interrupts.
        Some(unsafe { *self.value.get() })
    }

    #[inline]
    fn try_lock(&self) -> Option<Guard<'_>> {
        let saved_irq_state = save_and_disable_interrupts();
        if self
            .locked
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            restore_interrupts(saved_irq_state);
            return None;
        }
        Some(Guard {
            locked: &self.locked,
            saved_irq_state,
            _not_send: PhantomData,
        })
    }

    #[inline]
    pub fn store(&self, value: u64, order: Ordering) {
        assert_store_order(order);
        self.with_value(|slot| *slot = value);
    }

    #[inline]
    pub fn swap(&self, value: u64, order: Ordering) -> u64 {
        assert_rmw_order(order);
        self.with_value(|slot| core::mem::replace(slot, value))
    }

    #[inline]
    pub fn compare_exchange(
        &self,
        current: u64,
        new: u64,
        success: Ordering,
        failure: Ordering,
    ) -> Result<u64, u64> {
        assert_rmw_order(success);
        assert_load_order(failure);
        self.with_value(|slot| {
            let previous = *slot;
            if previous == current {
                *slot = new;
                Ok(previous)
            } else {
                Err(previous)
            }
        })
    }

    #[inline]
    pub fn compare_exchange_weak(
        &self,
        current: u64,
        new: u64,
        success: Ordering,
        failure: Ordering,
    ) -> Result<u64, u64> {
        self.compare_exchange(current, new, success, failure)
    }

    /// Attempt a CAS without waiting for an existing software-lock owner.
    pub fn try_compare_exchange(
        &self,
        current: u64,
        new: u64,
        success: Ordering,
        failure: Ordering,
    ) -> Option<Result<u64, u64>> {
        assert_rmw_order(success);
        assert_load_order(failure);
        let _guard = self.try_lock()?;
        // SAFETY: The guard excludes all competing accesses.
        let slot = unsafe { &mut *self.value.get() };
        let previous = *slot;
        Some(if previous == current {
            *slot = new;
            Ok(previous)
        } else {
            Err(previous)
        })
    }

    #[inline]
    pub fn fetch_add(&self, value: u64, order: Ordering) -> u64 {
        self.update(order, |old| old.wrapping_add(value))
    }

    #[inline]
    pub fn fetch_sub(&self, value: u64, order: Ordering) -> u64 {
        self.update(order, |old| old.wrapping_sub(value))
    }

    #[inline]
    pub fn fetch_or(&self, value: u64, order: Ordering) -> u64 {
        self.update(order, |old| old | value)
    }

    #[inline]
    pub fn fetch_and(&self, value: u64, order: Ordering) -> u64 {
        self.update(order, |old| old & value)
    }

    #[inline]
    fn update(&self, order: Ordering, f: impl FnOnce(u64) -> u64) -> u64 {
        assert_rmw_order(order);
        self.with_value(|slot| {
            let previous = *slot;
            *slot = f(previous);
            previous
        })
    }

    pub fn get_mut(&mut self) -> &mut u64 {
        self.value.get_mut()
    }

    pub fn into_inner(self) -> u64 {
        self.value.into_inner()
    }
}

impl Default for AtomicU64 {
    fn default() -> Self {
        Self::new(0)
    }
}

impl core::fmt::Debug for AtomicU64 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.try_load(Ordering::Relaxed) {
            Some(value) => value.fmt(f),
            None => f.write_str("<busy>"),
        }
    }
}

#[inline]
fn assert_load_order(order: Ordering) {
    match order {
        Ordering::Relaxed | Ordering::Acquire | Ordering::SeqCst => {}
        _ => panic!("invalid atomic load ordering"),
    }
}

#[inline]
fn assert_store_order(order: Ordering) {
    match order {
        Ordering::Relaxed | Ordering::Release | Ordering::SeqCst => {}
        _ => panic!("invalid atomic store ordering"),
    }
}

#[inline]
fn assert_rmw_order(order: Ordering) {
    match order {
        Ordering::Relaxed
        | Ordering::Acquire
        | Ordering::Release
        | Ordering::AcqRel
        | Ordering::SeqCst => {}
        _ => panic!("invalid atomic read-modify-write ordering"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn software_u64_preserves_high_bits_and_wrapping_arithmetic() {
        let value = AtomicU64::new(0xffff_ffff);
        assert_eq!(value.fetch_add(1, Ordering::Relaxed), 0xffff_ffff);
        assert_eq!(value.load(Ordering::Acquire), 0x1_0000_0000);
        assert_eq!(
            value.compare_exchange(0, 9, Ordering::Release, Ordering::Relaxed),
            Err(0x1_0000_0000)
        );
        assert_eq!(
            value.compare_exchange_weak(
                0x1_0000_0000,
                u64::MAX,
                Ordering::AcqRel,
                Ordering::Acquire
            ),
            Ok(0x1_0000_0000)
        );
        assert_eq!(value.fetch_add(1, Ordering::SeqCst), u64::MAX);
        assert_eq!(value.fetch_sub(1, Ordering::SeqCst), 0);
        assert_eq!(
            value.fetch_and(0xff00_0000_0000_00ff, Ordering::Relaxed),
            u64::MAX
        );
        assert_eq!(
            value.fetch_or(0xff00, Ordering::Relaxed),
            0xff00_0000_0000_00ff
        );
        assert_eq!(value.swap(42, Ordering::SeqCst), 0xff00_0000_0000_ffff);
        assert_eq!(value.try_load(Ordering::Relaxed), Some(42));
        let mut value = value;
        *value.get_mut() = u64::MAX;
        assert_eq!(value.into_inner(), u64::MAX);
    }

    #[test_case]
    fn software_u64_respects_an_existing_irq_mask() {
        let saved = save_and_disable_interrupts();
        let value = AtomicU64::new(7);
        value.fetch_add(1, Ordering::Relaxed);
        assert_eq!(value.try_load(Ordering::Acquire), Some(8));
        assert!(!crate::arch::interrupt::are_interrupts_enabled());
        restore_interrupts(saved);
    }

    #[test_case]
    fn software_u64_diagnostics_do_not_wait_for_a_held_lock() {
        let value = AtomicU64::new(0xdead_beef_1234_5678);
        let guard = value.lock();
        assert_eq!(value.try_load(Ordering::Acquire), None);
        assert_eq!(
            value.try_compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire),
            None
        );
        assert_eq!(alloc::format!("{value:?}"), "<busy>");
        assert!(!crate::arch::interrupt::are_interrupts_enabled());
        drop(guard);
        assert_eq!(
            value.try_load(Ordering::Acquire),
            Some(0xdead_beef_1234_5678)
        );
        assert_eq!(
            value.try_compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed),
            Some(Err(0xdead_beef_1234_5678))
        );
        assert_eq!(
            value.try_compare_exchange(
                0xdead_beef_1234_5678,
                2,
                Ordering::Release,
                Ordering::Relaxed
            ),
            Some(Ok(0xdead_beef_1234_5678))
        );
    }
}
