//! Native reader-writer lock primitives.
//!
//! `RwLock<T>` allows multiple readers or a single writer, with
//! `preempt_count` integration. `IrqSafeRwLock<T>` additionally masks
//! interrupts while held.
//!
//! The implementation is reader-preference: a steady stream of readers can
//! starve writers. This matches typical `spin::RwLock` semantics and is
//! sufficient until the scheduler grows fairness support.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::arch::interrupt::{restore_interrupts, save_and_disable_interrupts};
use crate::sync::preempt::PreemptGuard;

const WRITER_BIT: usize = 1 << (usize::BITS - 1);
const READER_MASK: usize = !WRITER_BIT;

pub struct RwLock<T> {
    state: AtomicUsize,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send + Sync> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    /// Create a new `RwLock` initialized to `value`.
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicUsize::new(0),
            data: UnsafeCell::new(value),
        }
    }

    /// Acquire a shared read lock, spinning until granted.
    ///
    /// Preemption is disabled for the duration of the returned guard.
    ///
    /// # Returns
    ///
    /// An [`RwLockReadGuard`] granting `&` access to the protected data.
    #[inline]
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        let preempt = PreemptGuard::new();
        self.acquire_read();
        RwLockReadGuard {
            lock: self,
            _preempt: preempt,
            _not_send: PhantomData,
        }
    }

    /// Acquire an exclusive write lock, spinning until granted.
    ///
    /// Preemption is disabled for the duration of the returned guard.
    ///
    /// # Returns
    ///
    /// An [`RwLockWriteGuard`] granting `&mut` access to the protected data.
    #[inline]
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        let preempt = PreemptGuard::new();
        self.acquire_write();
        RwLockWriteGuard {
            lock: self,
            _preempt: preempt,
            _not_send: PhantomData,
        }
    }

    #[inline]
    fn acquire_read(&self) {
        loop {
            let state = self.state.load(Ordering::Relaxed);
            if state & WRITER_BIT == 0 {
                let new_state = state.wrapping_add(1) & READER_MASK;
                if self
                    .state
                    .compare_exchange_weak(state, new_state, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return;
                }
            }
            core::hint::spin_loop();
        }
    }

    #[inline]
    fn acquire_write(&self) {
        loop {
            let state = self.state.load(Ordering::Relaxed);
            if state == 0
                && self
                    .state
                    .compare_exchange_weak(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                return;
            }
            core::hint::spin_loop();
        }
    }

    #[inline]
    fn release_read(&self) {
        let prev = self.state.fetch_sub(1, Ordering::Release);
        let _ = prev;
    }

    #[inline]
    fn release_write(&self) {
        self.state.fetch_and(!WRITER_BIT, Ordering::Release);
    }
}

/// RAII guard granting shared read access to an [`RwLock`]'s data.
pub struct RwLockReadGuard<'a, T> {
    lock: &'a RwLock<T>,
    _preempt: PreemptGuard,
    _not_send: PhantomData<*mut ()>,
}

impl<T> core::ops::Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: The guard holds a read lock; writers cannot mutate the
        // data concurrently.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> Drop for RwLockReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.release_read();
    }
}

/// RAII guard granting exclusive write access to an [`RwLock`]'s data.
pub struct RwLockWriteGuard<'a, T> {
    lock: &'a RwLock<T>,
    _preempt: PreemptGuard,
    _not_send: PhantomData<*mut ()>,
}

impl<T> core::ops::Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: The guard holds the write lock.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> core::ops::DerefMut for RwLockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: Same justification as `Deref`.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for RwLockWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.release_write();
    }
}

/// Reader-writer lock that also masks interrupts while held.
pub struct IrqSafeRwLock<T> {
    state: AtomicUsize,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send + Sync> Send for IrqSafeRwLock<T> {}
unsafe impl<T: Send + Sync> Sync for IrqSafeRwLock<T> {}

impl<T> IrqSafeRwLock<T> {
    /// Create a new `IrqSafeRwLock` initialized to `value`.
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
    /// An [`IrqSafeRwLockReadGuard`] granting `&` access to the protected
    /// data.
    #[inline]
    pub fn read(&self) -> IrqSafeRwLockReadGuard<'_, T> {
        let saved_irq = save_and_disable_interrupts();
        let preempt = PreemptGuard::new();
        loop {
            let state = self.state.load(Ordering::Relaxed);
            if state & WRITER_BIT == 0 {
                let new_state = state.wrapping_add(1) & READER_MASK;
                if self
                    .state
                    .compare_exchange_weak(state, new_state, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }
            }
            core::hint::spin_loop();
        }
        IrqSafeRwLockReadGuard {
            lock: self,
            saved_irq,
            _preempt: preempt,
            _not_send: PhantomData,
        }
    }

    /// Acquire an exclusive write lock, spinning until granted.
    ///
    /// Interrupts are masked on the current CPU and preemption is disabled
    /// for the duration of the returned guard.
    ///
    /// # Returns
    ///
    /// An [`IrqSafeRwLockWriteGuard`] granting `&mut` access to the
    /// protected data.
    #[inline]
    pub fn write(&self) -> IrqSafeRwLockWriteGuard<'_, T> {
        let saved_irq = save_and_disable_interrupts();
        let preempt = PreemptGuard::new();
        loop {
            let state = self.state.load(Ordering::Relaxed);
            if state == 0
                && self
                    .state
                    .compare_exchange_weak(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                break;
            }
            core::hint::spin_loop();
        }
        IrqSafeRwLockWriteGuard {
            lock: self,
            saved_irq,
            _preempt: preempt,
            _not_send: PhantomData,
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

pub struct IrqSafeRwLockReadGuard<'a, T> {
    lock: &'a IrqSafeRwLock<T>,
    saved_irq: usize,
    _preempt: PreemptGuard,
    _not_send: PhantomData<*mut ()>,
}

impl<T> core::ops::Deref for IrqSafeRwLockReadGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: The guard holds a read lock.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> Drop for IrqSafeRwLockReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.release_read();
        restore_interrupts(self.saved_irq);
    }
}

pub struct IrqSafeRwLockWriteGuard<'a, T> {
    lock: &'a IrqSafeRwLock<T>,
    saved_irq: usize,
    _preempt: PreemptGuard,
    _not_send: PhantomData<*mut ()>,
}

impl<T> core::ops::Deref for IrqSafeRwLockWriteGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: The guard holds the write lock.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> core::ops::DerefMut for IrqSafeRwLockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: Same justification as `Deref`.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for IrqSafeRwLockWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.release_write();
        restore_interrupts(self.saved_irq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_rwlock_multiple_readers() {
        let lock = RwLock::new(10u32);
        {
            let r1 = lock.read();
            let r2 = lock.read();
            assert_eq!(preempt_count(), 2);
            assert_eq!(*r1, 10);
            assert_eq!(*r2, 10);
        }
        assert_eq!(preempt_count(), 0);
    }

    #[test_case]
    fn test_rwlock_write_exclusive() {
        let lock = RwLock::new(0u32);
        {
            let mut w = lock.write();
            assert_eq!(preempt_count(), 1);
            *w = 5;
        }
        assert_eq!(*lock.read(), 5);
    }

    #[test_case]
    fn test_irq_safe_rwlock_basic() {
        let lock = IrqSafeRwLock::new(7u32);
        {
            let r = lock.read();
            assert_eq!(*r, 7);
        }
        {
            let mut w = lock.write();
            *w += 1;
        }
        assert_eq!(*lock.read(), 8);
    }
}
