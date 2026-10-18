//! Sleep before entering a contended GPU control critical section.
//!
//! Synchronous control commands still retain their IRQ guard through DMA
//! retirement. Other task-context callers must not mask IRQs and spin for
//! that entire host wait too. Completion/admission workers use try_lock.

use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sync::{IrqSpinLock, IrqSpinLockGuard, Waker};

pub(super) struct CoreLock<T> {
    inner: IrqSpinLock<T>,
    released: AtomicUsize,
    sleepers: AtomicUsize,
    wake: Waker,
}

impl<T> CoreLock<T> {
    pub(super) fn new(value: T) -> Self {
        Self {
            inner: IrqSpinLock::new(value),
            released: AtomicUsize::new(0),
            sleepers: AtomicUsize::new(0),
            wake: Waker::new_uninterruptible("virtio-gpu-core"),
        }
    }

    #[inline]
    #[track_caller]
    pub(super) fn try_lock(&self) -> Option<CoreGuard<'_, T>> {
        self.inner.try_lock().map(|guard| CoreGuard {
            lock: self,
            guard: Some(guard),
        })
    }

    #[track_caller]
    pub(super) fn lock(&self) -> CoreGuard<'_, T> {
        if let Some(guard) = self.try_lock() {
            return guard;
        }
        // Boot and final cleanup under another IRQ guard cannot schedule.
        // They retain the existing spin-lock semantics.
        if crate::sync::preemptible() {
            if let Some(task) = crate::task::mytask() {
                loop {
                    let released = self.released.load(Ordering::Acquire);
                    self.sleepers.fetch_add(1, Ordering::AcqRel);
                    if let Some(guard) = self.try_lock() {
                        self.sleepers.fetch_sub(1, Ordering::AcqRel);
                        return guard;
                    }
                    // The sequence closes release-before-registration races.
                    // No IRQ guard or ownership of the core crosses schedule.
                    self.wake.wait_with_condition(
                        task.get_id(),
                        task.get_trapframe(),
                        None,
                        0,
                        || self.released.load(Ordering::Acquire) != released,
                    );
                    self.sleepers.fetch_sub(1, Ordering::AcqRel);
                }
            }
        }
        CoreGuard {
            lock: self,
            guard: Some(self.inner.lock()),
        }
    }
}

pub(super) struct CoreGuard<'a, T> {
    lock: &'a CoreLock<T>,
    guard: Option<IrqSpinLockGuard<'a, T>>,
}

impl<T> Deref for CoreGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.guard.as_deref().unwrap()
    }
}

impl<T> DerefMut for CoreGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_deref_mut().unwrap()
    }
}

impl<T> Drop for CoreGuard<'_, T> {
    fn drop(&mut self) {
        // Publish unlock and wake as one non-preemptible transition. A remote
        // task exit must not abandon this stack between those two operations.
        let _transition = crate::sync::IrqGuard::new();
        drop(self.guard.take());
        self.lock.released.fetch_add(1, Ordering::Release);
        if self.lock.sleepers.load(Ordering::Acquire) != 0 {
            self.lock.wake.wake_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn failed_gpu_admission_restores_irq_guard_and_release_is_observable() {
        let lock = CoreLock::new(7);
        let count = crate::sync::preempt_count();
        let mut owner = lock.lock();
        let held_count = crate::sync::preempt_count();
        assert!(held_count > count);
        assert!(lock.try_lock().is_none());
        assert_eq!(crate::sync::preempt_count(), held_count);
        *owner = 9;
        drop(owner);
        assert_eq!(crate::sync::preempt_count(), count);
        assert_eq!(lock.released.load(Ordering::Acquire), 1);
        assert_eq!(*lock.lock(), 9);
    }
}
