//! Native mutual-exclusion primitives.
//!
//! `Mutex<T>` provides mutual exclusion with `preempt_count` integration.
//! While a [`MutexGuard`] is alive, the current CPU's preempt count is
//! non-zero, preventing involuntary task switches on top of the existing
//! IRQ-mask discipline.
//!
//! `IrqSafeMutex<T>` additionally saves and restores interrupt state on the
//! current CPU, for data that may be touched from interrupt context.
//!
//! Both locks spin via [`core::hint::spin_loop`] while contended. No
//! fairness, backoff, or priority inheritance is provided; the
//! implementation matches `spin::Mutex` semantics so existing call sites can
//! migrate by changing the import.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::interrupt::{restore_interrupts, save_and_disable_interrupts};
use crate::sync::preempt::PreemptGuard;

pub struct Mutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: `Mutex` provides mutual exclusion across CPUs. `T: Send` is
// sufficient because data only crosses CPU boundaries while exclusively
// held.
unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// Create a new mutex initialized to `value`.
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    /// Acquire the lock, spinning until exclusive access is granted.
    ///
    /// Preemption is disabled for the duration of the returned guard.
    ///
    /// # Returns
    ///
    /// A [`MutexGuard`] granting `&mut` access to the protected data.
    #[inline]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        let preempt = PreemptGuard::new();
        self.acquire();
        MutexGuard {
            mutex: self,
            _preempt: preempt,
            _not_send: PhantomData,
        }
    }

    /// Attempt to acquire the lock without spinning.
    ///
    /// # Returns
    ///
    /// `Some(guard)` if the lock was free, `None` if it was contended.
    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        let preempt = PreemptGuard::new();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(MutexGuard {
                mutex: self,
                _preempt: preempt,
                _not_send: PhantomData,
            })
        } else {
            drop(preempt);
            None
        }
    }

    #[inline]
    fn acquire(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }

    #[inline]
    fn release(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

impl<T> core::fmt::Debug for Mutex<T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Mutex")
            .field("locked", &self.locked.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl<T> core::fmt::Debug for MutexGuard<'_, T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T> core::fmt::Display for MutexGuard<'_, T>
where
    T: core::fmt::Display,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&**self, f)
    }
}

impl<T> core::fmt::Debug for IrqSafeMutexGuard<'_, T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T> core::fmt::Display for IrqSafeMutexGuard<'_, T>
where
    T: core::fmt::Display,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&**self, f)
    }
}

impl<T> Mutex<T> {
    ///
    /// This is intended for restricted scheduler paths that rely on an
    /// external exclusion mechanism (such as the `running_cpu` ownership
    /// token) instead of the lock itself.
    ///
    /// # Safety
    ///
    /// The caller is responsible for synchronization; the lock is not
    /// acquired. Dereferencing the returned pointer without external
    /// exclusion is undefined behavior.
    #[inline]
    pub fn as_mut_ptr(&self) -> *mut T {
        self.data.get()
    }

    #[inline]
    fn lock_inner(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }

    #[inline]
    fn try_lock_inner(&self) -> bool {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    #[inline]
    fn unlock_inner(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

// SAFETY: `Mutex<()>` is a correct busy-wait mutex: `lock_inner` acquires,
// `try_lock_inner` observes success or failure, `unlock_inner` releases.
// Preempt count is bumped on lock and restored on unlock to keep the
// critical section non-preemptible on the local CPU.
unsafe impl lock_api::RawMutex for Mutex<()> {
    type GuardMarker = lock_api::GuardSend;

    const INIT: Mutex<()> = Mutex::new(());

    #[inline]
    fn lock(&self) {
        crate::sync::preempt::preempt_disable();
        self.lock_inner();
    }

    #[inline]
    fn try_lock(&self) -> bool {
        crate::sync::preempt::preempt_disable();
        if self.try_lock_inner() {
            true
        } else {
            crate::sync::preempt::preempt_enable();
            false
        }
    }

    #[inline]
    unsafe fn unlock(&self) {
        self.unlock_inner();
        crate::sync::preempt::preempt_enable();
    }
}

/// RAII guard granting exclusive access to a [`Mutex`]'s data.
///
/// Dropping the guard releases the lock and re-enables preemption.
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
    _preempt: PreemptGuard,
    _not_send: PhantomData<*mut ()>,
}

impl<T> core::ops::Deref for MutexGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: The guard holds the lock, so no other CPU or reentrant
        // caller can observe the data concurrently.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> core::ops::DerefMut for MutexGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: Same justification as `Deref`.
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.mutex.release();
        // `_preempt` restores the preempt count via its own Drop.
    }
}

/// Mutual-exclusion lock that also masks interrupts while held.
///
/// Use for data that is read or written from both normal task context and
/// interrupt/trap-entry context on the same CPU. [`Mutex`] is sufficient
/// for data shared only across CPUs.
pub struct IrqSafeMutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: Same justification as `Mutex`.
unsafe impl<T: Send> Send for IrqSafeMutex<T> {}
unsafe impl<T: Send> Sync for IrqSafeMutex<T> {}

impl<T> IrqSafeMutex<T> {
    /// Create a new `IrqSafeMutex` initialized to `value`.
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    /// Acquire the lock, spinning until exclusive access is granted.
    ///
    /// Interrupts are masked on the current CPU and preemption is disabled
    /// for the duration of the returned guard.
    ///
    /// # Returns
    ///
    /// An [`IrqSafeMutexGuard`] granting `&mut` access to the protected
    /// data.
    #[inline]
    pub fn lock(&self) -> IrqSafeMutexGuard<'_, T> {
        let saved_irq = save_and_disable_interrupts();
        let preempt = PreemptGuard::new();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        IrqSafeMutexGuard {
            mutex: self,
            saved_irq,
            _preempt: preempt,
            _not_send: PhantomData,
        }
    }

    /// Attempt to acquire the lock without spinning.
    ///
    /// # Returns
    ///
    /// `Some(guard)` if the lock was free, `None` if it was contended.
    #[inline]
    pub fn try_lock(&self) -> Option<IrqSafeMutexGuard<'_, T>> {
        let saved_irq = save_and_disable_interrupts();
        let preempt = PreemptGuard::new();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(IrqSafeMutexGuard {
                mutex: self,
                saved_irq,
                _preempt: preempt,
                _not_send: PhantomData,
            })
        } else {
            drop(preempt);
            restore_interrupts(saved_irq);
            None
        }
    }

    #[inline]
    fn release(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

impl<T> core::fmt::Debug for IrqSafeMutex<T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IrqSafeMutex")
            .field("locked", &self.locked.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// RAII guard granting exclusive access to an [`IrqSafeMutex`]'s data.
///
/// Dropping the guard releases the lock, restores the previous interrupt
/// state, and re-enables preemption.
pub struct IrqSafeMutexGuard<'a, T> {
    mutex: &'a IrqSafeMutex<T>,
    saved_irq: usize,
    _preempt: PreemptGuard,
    _not_send: PhantomData<*mut ()>,
}

impl<T> core::ops::Deref for IrqSafeMutexGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: The guard holds the lock.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> core::ops::DerefMut for IrqSafeMutexGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: Same justification as `Deref`.
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for IrqSafeMutexGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.mutex.release();
        restore_interrupts(self.saved_irq);
        // `_preempt` restores the preempt count via its own Drop.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::preempt::preempt_count;

    #[test_case]
    fn test_mutex_lock_unlock() {
        let m = Mutex::new(42u32);
        assert_eq!(preempt_count(), 0);
        {
            let mut g = m.lock();
            assert_eq!(preempt_count(), 1);
            assert_eq!(*g, 42);
            *g = 7;
        }
        assert_eq!(preempt_count(), 0);
        assert_eq!(*m.lock(), 7);
    }

    #[test_case]
    fn test_mutex_try_lock_contended() {
        let m = Mutex::new(());
        let g = m.lock();
        assert!(m.try_lock().is_none());
        drop(g);
        assert!(m.try_lock().is_some());
    }

    #[test_case]
    fn test_irq_safe_mutex_restores_preempt() {
        let m = IrqSafeMutex::new(99u32);
        assert_eq!(preempt_count(), 0);
        {
            let mut g = m.lock();
            assert_eq!(preempt_count(), 1);
            *g += 1;
        }
        assert_eq!(preempt_count(), 0);
        assert_eq!(*m.lock(), 100);
    }
}
