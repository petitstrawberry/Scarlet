//! Native busy-wait reader-writer lock primitives.
//!
//! `RwSpinLock<T>` allows multiple readers or a single writer, with
//! `preempt_count` integration. `IrqRwSpinLock<T>` additionally masks
//! interrupts while held.
//!
//! The implementation is reader-preference: a steady stream of readers can
//! starve writers and is sufficient until the scheduler grows fairness
//! support.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sync::irq_guard::IrqGuard;
use crate::sync::preempt::{PreemptGuard, PreemptSourceKind, note_spin_contention};

const WRITER_BIT: usize = 1 << (usize::BITS - 1);
const READER_MASK: usize = !WRITER_BIT;

/// Busy-wait reader-writer lock that disables preemption while held.
///
/// This lock does not mask local interrupts. Use [`IrqRwSpinLock`] for data
/// shared with interrupt or trap-entry context on the same CPU.
pub struct RwSpinLock<T> {
    state: AtomicUsize,
    data: UnsafeCell<T>,
}

// SAFETY: `RwSpinLock` permits shared reads only when `T: Sync` and permits
// exclusive writes only when `T: Send`; its atomic state serializes writers.
unsafe impl<T: Send + Sync> Send for RwSpinLock<T> {}
// SAFETY: `RwSpinLock` permits shared reads only when `T: Sync` and permits
// exclusive writes only when `T: Send`; its atomic state serializes writers.
unsafe impl<T: Send + Sync> Sync for RwSpinLock<T> {}

impl<T> core::fmt::Debug for RwSpinLock<T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RwSpinLock")
            .field("state", &self.state.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl<T> core::fmt::Debug for RwSpinLockReadGuard<'_, T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T> core::fmt::Display for RwSpinLockReadGuard<'_, T>
where
    T: core::fmt::Display,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&**self, f)
    }
}

impl<T> core::fmt::Debug for RwSpinLockWriteGuard<'_, T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T> RwSpinLock<T> {
    /// Create a new `RwSpinLock` initialized to `value`.
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicUsize::new(0),
            data: UnsafeCell::new(value),
        }
    }

    /// Acquire a shared read lock, spinning until granted.
    ///
    /// Preemption is disabled for the duration of the returned guard. Local
    /// interrupts remain unchanged.
    ///
    /// # Returns
    ///
    /// An [`RwSpinLockReadGuard`] granting `&` access to the protected data.
    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn read(&self) -> RwSpinLockReadGuard<'_, T> {
        let preempt = PreemptGuard::new_with_source(
            PreemptSourceKind::RwSpinLockRead,
            self as *const Self as usize,
        );
        self.acquire_read();
        RwSpinLockReadGuard {
            lock: self,
            _preempt: preempt,
            _not_send: PhantomData,
        }
    }

    /// Attempt to acquire a read lock without spinning.
    ///
    /// # Returns
    ///
    /// `Some(guard)` if acquired, `None` if a writer holds the lock or the
    /// reader count is saturated. Existing readers do not prevent success.
    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn try_read(&self) -> Option<RwSpinLockReadGuard<'_, T>> {
        let preempt = PreemptGuard::new_with_source(
            PreemptSourceKind::RwSpinLockRead,
            self as *const Self as usize,
        );
        if self.try_acquire_read() {
            Some(RwSpinLockReadGuard {
                lock: self,
                _preempt: preempt,
                _not_send: PhantomData,
            })
        } else {
            drop(preempt);
            None
        }
    }

    /// Acquire an exclusive write lock, spinning until granted.
    ///
    /// Preemption is disabled for the duration of the returned guard. Local
    /// interrupts remain unchanged.
    ///
    /// # Returns
    ///
    /// An [`RwSpinLockWriteGuard`] granting `&mut` access to the protected
    /// data.
    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn write(&self) -> RwSpinLockWriteGuard<'_, T> {
        let preempt = PreemptGuard::new_with_source(
            PreemptSourceKind::RwSpinLockWrite,
            self as *const Self as usize,
        );
        self.acquire_write();
        RwSpinLockWriteGuard {
            lock: self,
            _preempt: preempt,
            _not_send: PhantomData,
        }
    }

    /// Attempt to acquire a write lock without spinning.
    ///
    /// # Returns
    ///
    /// `Some(guard)` if acquired, `None` if any reader or writer holds the
    /// lock.
    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn try_write(&self) -> Option<RwSpinLockWriteGuard<'_, T>> {
        let preempt = PreemptGuard::new_with_source(
            PreemptSourceKind::RwSpinLockWrite,
            self as *const Self as usize,
        );
        if self
            .state
            .compare_exchange(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(RwSpinLockWriteGuard {
                lock: self,
                _preempt: preempt,
                _not_send: PhantomData,
            })
        } else {
            drop(preempt);
            None
        }
    }

    /// Return the number of writers currently holding the lock (0 or 1).
    ///
    /// # Returns
    ///
    /// `1` while a writer holds the lock, `0` otherwise.
    #[inline]
    pub fn writer_count(&self) -> usize {
        if self.state.load(Ordering::Relaxed) & WRITER_BIT != 0 {
            1
        } else {
            0
        }
    }

    /// Return the number of readers currently holding the lock.
    ///
    /// # Returns
    ///
    /// Current reader count.
    #[inline]
    pub fn reader_count(&self) -> usize {
        self.state.load(Ordering::Relaxed) & READER_MASK
    }

    #[inline]
    fn acquire_read(&self) {
        loop {
            let state = self.state.load(Ordering::Relaxed);
            if state & WRITER_BIT == 0 && state != READER_MASK {
                if self
                    .state
                    .compare_exchange_weak(state, state + 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return;
                }
            }
            note_spin_contention();
        }
    }

    #[inline]
    fn try_acquire_read(&self) -> bool {
        let mut state = self.state.load(Ordering::Relaxed);
        loop {
            if state & WRITER_BIT != 0 || state == READER_MASK {
                return false;
            }
            match self.state.compare_exchange_weak(
                state,
                state + 1,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(observed) => state = observed,
            }
        }
    }

    #[inline]
    fn acquire_write(&self) {
        loop {
            if self
                .state
                .compare_exchange_weak(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
            note_spin_contention();
        }
    }

    #[inline]
    fn release_read(&self) {
        self.state.fetch_sub(1, Ordering::Release);
    }

    #[inline]
    fn release_write(&self) {
        self.state.fetch_and(!WRITER_BIT, Ordering::Release);
    }
}

/// RAII guard granting shared read access to an [`RwSpinLock`]'s data.
pub struct RwSpinLockReadGuard<'a, T> {
    lock: &'a RwSpinLock<T>,
    _preempt: PreemptGuard,
    _not_send: PhantomData<*mut ()>,
}

impl<T> core::ops::Deref for RwSpinLockReadGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: The guard holds a read lock; writers cannot mutate the
        // data concurrently.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> Drop for RwSpinLockReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.release_read();
    }
}

/// RAII guard granting exclusive write access to an [`RwSpinLock`]'s data.
pub struct RwSpinLockWriteGuard<'a, T> {
    lock: &'a RwSpinLock<T>,
    _preempt: PreemptGuard,
    _not_send: PhantomData<*mut ()>,
}

impl<T> core::ops::Deref for RwSpinLockWriteGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: The guard holds the write lock.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> core::ops::DerefMut for RwSpinLockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: Same justification as `Deref`.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for RwSpinLockWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.release_write();
    }
}

/// Reader-writer lock that also masks interrupts while held.
pub struct IrqRwSpinLock<T> {
    state: AtomicUsize,
    data: UnsafeCell<T>,
}

// SAFETY: `IrqRwSpinLock` permits shared reads only when `T: Sync` and
// exclusive writes only when `T: Send`; atomic state serializes writers and
// local IRQ masking prevents same-CPU interrupt reentrancy.
unsafe impl<T: Send + Sync> Send for IrqRwSpinLock<T> {}
// SAFETY: `IrqRwSpinLock` permits shared reads only when `T: Sync` and
// exclusive writes only when `T: Send`; atomic state serializes writers and
// local IRQ masking prevents same-CPU interrupt reentrancy.
unsafe impl<T: Send + Sync> Sync for IrqRwSpinLock<T> {}

impl<T> core::fmt::Debug for IrqRwSpinLock<T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IrqRwSpinLock")
            .field("state", &self.state.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl<T> core::fmt::Debug for IrqRwSpinLockReadGuard<'_, T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T> core::fmt::Display for IrqRwSpinLockReadGuard<'_, T>
where
    T: core::fmt::Display,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&**self, f)
    }
}

impl<T> core::fmt::Debug for IrqRwSpinLockWriteGuard<'_, T>
where
    T: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T> IrqRwSpinLock<T> {
    /// Create a new `IrqRwSpinLock` initialized to `value`.
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicUsize::new(0),
            data: UnsafeCell::new(value),
        }
    }

    /// Acquire a shared read lock, spinning until granted.
    ///
    /// Interrupts are masked on the current CPU and preemption is disabled
    /// for the duration of the returned guard.
    ///
    /// # Returns
    ///
    /// An [`IrqRwSpinLockReadGuard`] granting `&` access to the protected
    /// data.
    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn read(&self) -> IrqRwSpinLockReadGuard<'_, T> {
        let irq_guard = IrqGuard::new_with_source(
            PreemptSourceKind::IrqRwSpinLockRead,
            self as *const Self as usize,
        );
        self.acquire_read();
        IrqRwSpinLockReadGuard {
            lock: self,
            irq_guard: Some(irq_guard),
            _not_send: PhantomData,
        }
    }

    /// Attempt to acquire a read lock without spinning.
    ///
    /// # Returns
    ///
    /// `Some(guard)` if acquired, `None` if a writer holds the lock or the
    /// reader count is saturated. Existing readers do not prevent success.
    /// On failure this drops the shared IRQ/preemption token before returning.
    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn try_read(&self) -> Option<IrqRwSpinLockReadGuard<'_, T>> {
        let irq_guard = IrqGuard::new_with_source(
            PreemptSourceKind::IrqRwSpinLockRead,
            self as *const Self as usize,
        );
        if self.try_acquire_read() {
            Some(IrqRwSpinLockReadGuard {
                lock: self,
                irq_guard: Some(irq_guard),
                _not_send: PhantomData,
            })
        } else {
            drop(irq_guard);
            None
        }
    }

    /// Acquire an exclusive write lock, spinning until granted.
    ///
    /// Interrupts are masked on the current CPU and preemption is disabled
    /// for the duration of the returned guard.
    ///
    /// # Returns
    ///
    /// An [`IrqRwSpinLockWriteGuard`] granting `&mut` access to the protected
    /// data.
    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn write(&self) -> IrqRwSpinLockWriteGuard<'_, T> {
        let irq_guard = IrqGuard::new_with_source(
            PreemptSourceKind::IrqRwSpinLockWrite,
            self as *const Self as usize,
        );
        self.acquire_write();
        IrqRwSpinLockWriteGuard {
            lock: self,
            irq_guard: Some(irq_guard),
            _not_send: PhantomData,
        }
    }

    /// Attempt to acquire a write lock without spinning.
    ///
    /// # Returns
    ///
    /// `Some(guard)` if acquired, `None` if any reader or writer holds the
    /// lock. On failure this drops the shared IRQ/preemption token before
    /// returning.
    #[inline]
    #[cfg_attr(feature = "sync-debug", track_caller)]
    pub fn try_write(&self) -> Option<IrqRwSpinLockWriteGuard<'_, T>> {
        let irq_guard = IrqGuard::new_with_source(
            PreemptSourceKind::IrqRwSpinLockWrite,
            self as *const Self as usize,
        );
        if self
            .state
            .compare_exchange(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(IrqRwSpinLockWriteGuard {
                lock: self,
                irq_guard: Some(irq_guard),
                _not_send: PhantomData,
            })
        } else {
            drop(irq_guard);
            None
        }
    }

    /// Return the number of writers currently holding the lock (0 or 1).
    ///
    /// # Returns
    ///
    /// `1` while a writer holds the lock, `0` otherwise.
    #[inline]
    pub fn writer_count(&self) -> usize {
        if self.state.load(Ordering::Relaxed) & WRITER_BIT != 0 {
            1
        } else {
            0
        }
    }

    /// Return the number of readers currently holding the lock.
    ///
    /// # Returns
    ///
    /// Current reader count.
    #[inline]
    pub fn reader_count(&self) -> usize {
        self.state.load(Ordering::Relaxed) & READER_MASK
    }

    #[inline]
    fn acquire_read(&self) {
        loop {
            let state = self.state.load(Ordering::Relaxed);
            if state & WRITER_BIT == 0 && state != READER_MASK {
                if self
                    .state
                    .compare_exchange_weak(state, state + 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return;
                }
            }
            note_spin_contention();
        }
    }

    #[inline]
    fn try_acquire_read(&self) -> bool {
        let mut state = self.state.load(Ordering::Relaxed);
        loop {
            if state & WRITER_BIT != 0 || state == READER_MASK {
                return false;
            }
            match self.state.compare_exchange_weak(
                state,
                state + 1,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(observed) => state = observed,
            }
        }
    }

    #[inline]
    fn acquire_write(&self) {
        loop {
            if self
                .state
                .compare_exchange_weak(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
            note_spin_contention();
        }
    }

    #[inline]
    fn release_read(&self) {
        self.state.fetch_sub(1, Ordering::Release);
    }

    #[inline]
    fn release_write(&self) {
        self.state.fetch_and(!WRITER_BIT, Ordering::Release);
    }
}

/// RAII guard granting shared read access to an [`IrqRwSpinLock`]'s data.
///
/// Dropping the guard releases the reader state before dropping its shared
/// IRQ/preemption token.
pub struct IrqRwSpinLockReadGuard<'a, T> {
    lock: &'a IrqRwSpinLock<T>,
    irq_guard: Option<IrqGuard>,
    _not_send: PhantomData<*mut ()>,
}

impl<T> core::ops::Deref for IrqRwSpinLockReadGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: The guard holds a read lock.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> Drop for IrqRwSpinLockReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.release_read();
        drop(self.irq_guard.take());
    }
}

/// RAII guard granting exclusive write access to an [`IrqRwSpinLock`]'s data.
///
/// Dropping the guard releases the writer state before dropping its shared
/// IRQ/preemption token.
pub struct IrqRwSpinLockWriteGuard<'a, T> {
    lock: &'a IrqRwSpinLock<T>,
    irq_guard: Option<IrqGuard>,
    _not_send: PhantomData<*mut ()>,
}

impl<T> core::ops::Deref for IrqRwSpinLockWriteGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: The guard holds the write lock.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> core::ops::DerefMut for IrqRwSpinLockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: Same justification as `Deref`.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for IrqRwSpinLockWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.release_write();
        drop(self.irq_guard.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::irq_guard::IrqGuard;
    use crate::sync::preempt::{PreemptGuard, preempt_count};

    #[test_case]
    fn test_rw_spin_lock_multiple_readers() {
        let lock = RwSpinLock::new(10u32);
        {
            let reader_one = lock.read();
            let reader_two = lock.read();
            assert_eq!(preempt_count(), 2);
            assert_eq!(*reader_one, 10);
            assert_eq!(*reader_two, 10);
        }
        assert_eq!(preempt_count(), 0);
    }

    #[test_case]
    fn test_rw_spin_lock_write_exclusive() {
        let lock = RwSpinLock::new(0u32);
        {
            let mut writer = lock.write();
            assert_eq!(preempt_count(), 1);
            *writer = 5;
        }
        assert_eq!(*lock.read(), 5);
    }

    #[test_case]
    fn test_irq_rw_spin_lock_try_read_allows_existing_readers() {
        let lock = IrqRwSpinLock::new(7u32);
        let reader_one = lock.try_read().unwrap();
        let reader_two = lock.try_read().unwrap();
        assert_eq!(lock.reader_count(), 2);
        assert_eq!(lock.writer_count(), 0);
        assert_eq!(*reader_one, 7);
        assert_eq!(*reader_two, 7);
        drop(reader_two);
        drop(reader_one);

        let writer = lock.try_write().unwrap();
        assert_eq!(lock.reader_count(), 0);
        assert_eq!(lock.writer_count(), 1);
        assert!(lock.try_read().is_none());
        drop(writer);
    }

    #[test_case]
    fn test_irq_rw_spin_locks_release_non_lifo() {
        IrqGuard::reset_for_test();
        PreemptGuard::reset_count_for_test();

        let first_lock = IrqRwSpinLock::new(());
        let second_lock = IrqRwSpinLock::new(());
        let first = first_lock.read();
        let second = second_lock.write();
        assert_eq!(IrqGuard::depth_for_test(), 2);
        assert_eq!(preempt_count(), 2);

        drop(first);
        assert_eq!(first_lock.reader_count(), 0);
        assert_eq!(IrqGuard::depth_for_test(), 1);
        assert_eq!(preempt_count(), 1);

        drop(second);
        assert_eq!(IrqGuard::depth_for_test(), 0);
        assert_eq!(preempt_count(), 0);
    }
}
