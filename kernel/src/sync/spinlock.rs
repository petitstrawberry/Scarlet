//! Native busy-wait mutual-exclusion primitives.
//!
//! `SpinLock<T>` provides mutual exclusion with `preempt_count` integration.
//! While a [`SpinLockGuard`] is alive, the current CPU's preempt count is
//! non-zero, preventing involuntary task switches. It does not mask local
//! interrupts.
//!
//! `IrqSpinLock<T>` additionally saves and restores interrupt state on the
//! current CPU, for data that may be touched from interrupt context.
//!
//! Both locks spin via [`core::hint::spin_loop`] while contended. No
//! fairness, backoff, or priority inheritance is provided; the
//! implementation uses standard busy-wait lock semantics.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::sync::irq_guard::IrqGuard;
use crate::sync::preempt::{PreemptGuard, PreemptSourceKind, note_spin_contention};

/// Busy-wait mutual-exclusion lock that disables preemption while held.
///
/// This lock does not mask local interrupts. Use [`IrqSpinLock`] for data
/// shared with interrupt or trap-entry context on the same CPU.
pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: `SpinLock` provides mutual exclusion across CPUs. `T: Send` is
// sufficient because data only crosses CPU boundaries while exclusively
// held.
unsafe impl<T: Send> Send for SpinLock<T> {}
// SAFETY: `SpinLock` provides mutual exclusion across CPUs. Access to `T` is
// serialized by `locked`, and the guard prevents migration while held.
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    /// Create a new spin lock initialized to `value`.
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    /// Acquire the lock, spinning until exclusive access is granted.
    ///
    /// Preemption is disabled for the duration of the returned guard. Local
    /// interrupts remain unchanged.
    ///
    /// # Returns
    ///
    /// A [`SpinLockGuard`] granting `&mut` access to the protected data.
    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        let preempt = PreemptGuard::new_with_source(
            PreemptSourceKind::SpinLock,
            self as *const Self as usize,
        );
        self.acquire();
        SpinLockGuard {
            lock: self,
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
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        let preempt = PreemptGuard::new_with_source(
            PreemptSourceKind::SpinLock,
            self as *const Self as usize,
        );
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SpinLockGuard {
                lock: self,
                _preempt: preempt,
                _not_send: PhantomData,
            })
        } else {
            drop(preempt);
            None
        }
    }

    /// Return a mutable pointer without acquiring this spin lock.
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
    pub(crate) unsafe fn as_mut_ptr(&self) -> *mut T {
        self.data.get()
    }

    #[inline]
    fn acquire(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                note_spin_contention();
            }
        }
    }

    #[inline]
    fn release(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

impl<T> core::fmt::Debug for SpinLock<T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SpinLock")
            .field("locked", &self.locked.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl<T> core::fmt::Debug for SpinLockGuard<'_, T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T> core::fmt::Display for SpinLockGuard<'_, T>
where
    T: core::fmt::Display,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&**self, f)
    }
}

/// RAII guard granting exclusive access to a [`SpinLock`]'s data.
///
/// Dropping the guard releases the lock and re-enables preemption.
pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    _preempt: PreemptGuard,
    _not_send: PhantomData<*mut ()>,
}

impl<T> core::ops::Deref for SpinLockGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: The guard holds the lock, so no other CPU or reentrant
        // caller can observe the data concurrently.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> core::ops::DerefMut for SpinLockGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: Same justification as `Deref`.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.release();
        // `_preempt` restores the preempt count via its own Drop.
    }
}

/// `lock_api` raw mutex backend that masks local interrupts while held.
///
/// This backend is for integrations, such as Talc's global allocator, that
/// require `lock_api::RawMutex`. It is not an ordinary guard API. The shared
/// [`IrqGuard`] token is stored after atomic acquisition because
/// `RawMutex::unlock` has no token parameter. Atomic ownership and
/// [`lock_api::GuardNoSend`] ensure that only the owning CPU accesses that
/// token.
pub struct RawIrqSpinLock {
    locked: AtomicBool,
    irq_guard_handoff: AtomicBool,
    irq_guard: UnsafeCell<Option<IrqGuard>>,
}

// SAFETY: `locked` serializes ownership of `irq_guard`, and
// `irq_guard_handoff` keeps a new owner from accessing the cell while the
// preceding owner removes its token after releasing `locked`.
unsafe impl Send for RawIrqSpinLock {}
// SAFETY: `locked` and `irq_guard_handoff` serialize all access to
// `irq_guard`; callers may share the raw backend between CPUs as required by
// `lock_api::RawMutex`.
unsafe impl Sync for RawIrqSpinLock {}

impl RawIrqSpinLock {
    /// Create an unlocked raw IRQ-masking spin lock.
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            irq_guard_handoff: AtomicBool::new(false),
            irq_guard: UnsafeCell::new(None),
        }
    }

    #[inline]
    fn lock_inner(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                note_spin_contention();
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

    #[inline]
    fn store_irq_guard(&self, irq_guard: IrqGuard) {
        while self.irq_guard_handoff.load(Ordering::Acquire) {
            note_spin_contention();
        }
        // SAFETY: The caller owns `locked`, and the handoff flag is clear only
        // after the preceding owner has removed its token from this cell.
        unsafe {
            debug_assert!((*self.irq_guard.get()).is_none());
            *self.irq_guard.get() = Some(irq_guard);
        }
    }
}

impl Default for RawIrqSpinLock {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: `lock` and `try_lock` acquire an `IrqGuard` before taking `locked`
// and store that token while owning the atomic lock. `unlock` marks the token
// handoff, releases the atomic lock, then removes and drops the token. The
// handoff flag prevents the next atomic owner from racing that `UnsafeCell`
// access, and dropping the token restores nested preemption and IRQ state in
// the correct order.
// `GuardNoSend` ensures unlock remains on the acquiring CPU.
unsafe impl lock_api::RawMutex for RawIrqSpinLock {
    type GuardMarker = lock_api::GuardNoSend;

    const INIT: RawIrqSpinLock = RawIrqSpinLock::new();

    #[inline]
    fn lock(&self) {
        let irq_guard =
            IrqGuard::new_with_source(PreemptSourceKind::IrqSpinLock, self as *const Self as usize);
        self.lock_inner();
        self.store_irq_guard(irq_guard);
    }

    #[inline]
    fn try_lock(&self) -> bool {
        let irq_guard =
            IrqGuard::new_with_source(PreemptSourceKind::IrqSpinLock, self as *const Self as usize);
        if self.try_lock_inner() {
            self.store_irq_guard(irq_guard);
            true
        } else {
            drop(irq_guard);
            false
        }
    }

    #[inline]
    unsafe fn unlock(&self) {
        // SAFETY: `RawMutex::unlock` is only called by a guard that acquired
        // this lock. `GuardNoSend` keeps that guard on the acquiring CPU. The
        // handoff flag prevents a new atomic owner from accessing `irq_guard`
        // until this CPU has removed it after releasing atomic ownership.
        self.irq_guard_handoff.store(true, Ordering::Release);
        self.unlock_inner();
        let irq_guard = unsafe { (*self.irq_guard.get()).take() }
            .expect("RawIrqSpinLock unlocked without an IRQ guard");
        self.irq_guard_handoff.store(false, Ordering::Release);
        drop(irq_guard);
    }
}

/// Mutual-exclusion lock that also masks interrupts while held.
///
/// Use for data that is read or written from both normal task context and
/// interrupt/trap-entry context on the same CPU. [`SpinLock`] is sufficient
/// for data shared only across CPUs.
pub struct IrqSpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: `IrqSpinLock` provides mutual exclusion across CPUs. `T: Send` is
// sufficient because data only crosses CPU boundaries while exclusively
// held.
unsafe impl<T: Send> Send for IrqSpinLock<T> {}
// SAFETY: `IrqSpinLock` serializes access and masks local IRQs while a guard
// is live, preventing same-CPU interrupt reentrancy.
unsafe impl<T: Send> Sync for IrqSpinLock<T> {}

impl<T> IrqSpinLock<T> {
    /// Create a new `IrqSpinLock` initialized to `value`.
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    /// Return a mutable pointer without acquiring this IRQ spin lock.
    ///
    /// This is intended for restricted scheduler paths that rely on an
    /// external exclusion mechanism, such as the `running_cpu` ownership
    /// token, instead of the lock itself.
    ///
    /// # Safety
    ///
    /// The caller is responsible for synchronization; neither the atomic lock
    /// nor local IRQ masking is acquired. Dereferencing the returned pointer
    /// without external exclusion is undefined behavior.
    #[inline]
    pub(crate) unsafe fn as_mut_ptr(&self) -> *mut T {
        self.data.get()
    }

    /// Acquire the lock, spinning until exclusive access is granted.
    ///
    /// Interrupts are masked on the current CPU and preemption is disabled
    /// for the duration of the returned guard.
    ///
    /// # Returns
    ///
    /// An [`IrqSpinLockGuard`] granting `&mut` access to the protected data.
    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn lock(&self) -> IrqSpinLockGuard<'_, T> {
        let irq_guard =
            IrqGuard::new_with_source(PreemptSourceKind::IrqSpinLock, self as *const Self as usize);
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                note_spin_contention();
            }
        }
        irq_guard.mark_acquired();
        IrqSpinLockGuard {
            lock: self,
            irq_guard: Some(irq_guard),
            _not_send: PhantomData,
        }
    }

    /// Attempt to acquire the lock without spinning.
    ///
    /// # Returns
    ///
    /// `Some(guard)` if the lock was free, `None` if it was contended. On
    /// failure this drops the shared IRQ/preemption token before returning.
    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn try_lock(&self) -> Option<IrqSpinLockGuard<'_, T>> {
        let irq_guard =
            IrqGuard::new_with_source(PreemptSourceKind::IrqSpinLock, self as *const Self as usize);
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            irq_guard.mark_acquired();
            Some(IrqSpinLockGuard {
                lock: self,
                irq_guard: Some(irq_guard),
                _not_send: PhantomData,
            })
        } else {
            drop(irq_guard);
            None
        }
    }

    #[inline]
    fn release(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

impl<T> core::fmt::Debug for IrqSpinLock<T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IrqSpinLock")
            .field("locked", &self.locked.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// RAII guard granting exclusive access to an [`IrqSpinLock`]'s data.
///
/// Dropping the guard releases the lock, then drops its shared IRQ/preemption
/// token. The final nested token restores the previous interrupt state.
pub struct IrqSpinLockGuard<'a, T> {
    lock: &'a IrqSpinLock<T>,
    irq_guard: Option<IrqGuard>,
    _not_send: PhantomData<*mut ()>,
}

impl<T> core::fmt::Debug for IrqSpinLockGuard<'_, T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T> core::fmt::Display for IrqSpinLockGuard<'_, T>
where
    T: core::fmt::Display,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&**self, f)
    }
}

impl<T> core::ops::Deref for IrqSpinLockGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: The guard holds the lock.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> core::ops::DerefMut for IrqSpinLockGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: Same justification as `Deref`.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for IrqSpinLockGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.release();
        drop(self.irq_guard.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::irq_guard::IrqGuard;
    use crate::sync::preempt::{PreemptGuard, preempt_count};

    #[test_case]
    fn test_spin_lock_lock_unlock() {
        let lock = SpinLock::new(42u32);
        assert_eq!(preempt_count(), 0);
        {
            let mut guard = lock.lock();
            assert_eq!(preempt_count(), 1);
            assert_eq!(*guard, 42);
            *guard = 7;
        }
        assert_eq!(preempt_count(), 0);
        assert_eq!(*lock.lock(), 7);
    }

    #[test_case]
    fn test_spin_lock_try_lock_contended() {
        let lock = SpinLock::new(());
        let guard = lock.lock();
        assert!(lock.try_lock().is_none());
        drop(guard);
        assert!(lock.try_lock().is_some());
    }

    #[test_case]
    fn test_irq_spin_lock_restores_preempt() {
        let lock = IrqSpinLock::new(99u32);
        assert_eq!(preempt_count(), 0);
        {
            let mut guard = lock.lock();
            assert_eq!(preempt_count(), 1);
            *guard += 1;
            assert!(lock.try_lock().is_none());
            assert_eq!(preempt_count(), 1);
        }
        assert_eq!(preempt_count(), 0);
        assert_eq!(*lock.lock(), 100);
    }

    #[test_case]
    fn test_irq_spin_locks_release_non_lifo() {
        IrqGuard::reset_for_test();
        PreemptGuard::reset_count_for_test();

        let first_lock = IrqSpinLock::new(());
        let second_lock = IrqSpinLock::new(());
        let first = first_lock.lock();
        let second = second_lock.lock();
        assert_eq!(IrqGuard::depth_for_test(), 2);
        assert_eq!(preempt_count(), 2);

        drop(first);
        assert_eq!(IrqGuard::depth_for_test(), 1);
        assert_eq!(preempt_count(), 1);
        assert!(first_lock.try_lock().is_some());

        drop(second);
        assert_eq!(IrqGuard::depth_for_test(), 0);
        assert_eq!(preempt_count(), 0);
    }

    #[test_case]
    fn test_raw_irq_spin_locks_release_non_lifo() {
        IrqGuard::reset_for_test();
        PreemptGuard::reset_count_for_test();

        let first_lock = lock_api::Mutex::<RawIrqSpinLock, ()>::new(());
        let second_lock = lock_api::Mutex::<RawIrqSpinLock, ()>::new(());
        let first = first_lock.lock();
        let second = second_lock.lock();
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
