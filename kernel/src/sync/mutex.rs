//! Native sleepable mutual-exclusion primitive.
//!
//! [`Mutex`] serializes task-context access without keeping preemption
//! disabled while the guard is held. Contended callers are placed on a
//! scheduler wait queue and resume when an owner releases the lock.
//!
//! The internal [`IrqSpinLock`] protects only waiter bookkeeping. In
//! particular, it is always released before entering the scheduler.

extern crate alloc;

use alloc::collections::VecDeque;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::sync::IrqSpinLock;
use crate::task::{AtomicTaskState, BlockedType, TaskState};

#[inline]
fn transition_waiter_to_blocked(state: &AtomicTaskState) -> Result<bool, TaskState> {
    let blocked = TaskState::Blocked(BlockedType::Uninterruptible);
    match state.compare_exchange(
        TaskState::Running,
        blocked,
        Ordering::SeqCst,
        Ordering::SeqCst,
    ) {
        Ok(_) => Ok(true),
        // A remote exit can publish a terminal state while this task still
        // owns the current CPU. Preserve that state and let the caller
        // schedule the task away without adding it to the mutex wait queue.
        Err(TaskState::Zombie | TaskState::Terminated) => Ok(false),
        Err(actual) => Err(actual),
    }
}

/// A task-context mutual-exclusion lock that sleeps while contended.
///
/// Unlike [`crate::sync::SpinLock`] and [`crate::sync::IrqSpinLock`], holding
/// this lock does not alter the current CPU's preemption count. Code protected
/// by a `Mutex` may therefore block or enter the scheduler.
///
/// `lock()` must be called from preemptible task context. `try_lock()` never
/// sleeps and may be used where blocking is not permitted.
pub struct Mutex<T> {
    locked: AtomicBool,
    waiters: IrqSpinLock<VecDeque<usize>>,
    data: UnsafeCell<T>,
}

// SAFETY: `Mutex` provides exclusive access to its data across CPUs. `T: Send`
// is sufficient because protected data can cross CPU boundaries only while
// owned by a single guard.
unsafe impl<T: Send> Send for Mutex<T> {}
// SAFETY: acquisition and release serialize every access to `data`; contended
// tasks sleep without retaining a reference into it until they acquire the
// atomic ownership flag.
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// Create an unlocked mutex containing `value`.
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            waiters: IrqSpinLock::new(VecDeque::new()),
            data: UnsafeCell::new(value),
        }
    }

    /// Acquire the mutex, sleeping if another task owns it.
    ///
    /// # Panics
    ///
    /// Panics when called with preemption disabled, or when contention occurs
    /// outside a schedulable task context.
    #[inline]
    #[track_caller]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        assert!(
            crate::sync::preemptible(),
            "Mutex::lock called while preemption is disabled"
        );

        if self.try_acquire() {
            return MutexGuard::new(self);
        }

        self.lock_contended()
    }

    /// Attempt to acquire the mutex without sleeping.
    ///
    /// Returns `None` immediately if another caller owns the lock.
    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        self.try_acquire().then(|| MutexGuard::new(self))
    }

    /// Return a mutable reference to the protected value without locking.
    ///
    /// The mutable borrow of the mutex guarantees exclusive access.
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }

    /// Consume the mutex and return the protected value.
    #[inline]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }

    #[inline]
    fn try_acquire(&self) -> bool {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    #[cold]
    #[track_caller]
    fn lock_contended(&self) -> MutexGuard<'_, T> {
        let task = crate::task::mytask()
            .expect("contended Mutex::lock requires a current schedulable task");
        let task_id = task.get_id();

        loop {
            {
                // Retrying acquisition while holding the waiter lock closes
                // the unlock/enqueue race: an unlock either observes this
                // waiter, or this retry observes the unlocked mutex.
                let mut waiters = self.waiters.lock();
                if self.try_acquire() {
                    return MutexGuard::new(self);
                }

                match transition_waiter_to_blocked(&task.state) {
                    Ok(true) => {
                        crate::sched::scheduler::mark_blocked(task_id);
                        if !waiters.contains(&task_id) {
                            waiters.push_back(task_id);
                        }
                    }
                    // The schedule below switches a terminal task away through
                    // the scheduler's normal running_cpu handoff.
                    Ok(false) => {}
                    Err(actual) => {
                        // Do not strand the mutex's internal spin lock if an
                        // invariant failure reaches the kernel panic handler.
                        drop(waiters);
                        panic!(
                            "Mutex waiter task {} is not running (state={:?})",
                            task_id, actual
                        );
                    }
                }
            }

            // The waiter spin lock and its preemption guard are gone before
            // the context switch. The task may already have been woken; the
            // scheduler handles that Ready-before-schedule race.
            crate::sched::scheduler::schedule(task.get_trapframe());
        }
    }

    #[inline]
    fn release(&self) {
        self.locked.store(false, Ordering::Release);

        // Skip stale task IDs left by task teardown and wake one live waiter.
        loop {
            let task_id = self.waiters.lock().pop_front();
            let Some(task_id) = task_id else {
                return;
            };
            if crate::sched::scheduler::wake_task(task_id) {
                return;
            }
        }
    }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Mutex")
            .field("locked", &self.locked.load(Ordering::Relaxed))
            .field("waiting_count", &self.waiters.lock().len())
            .finish_non_exhaustive()
    }
}

/// RAII guard granting exclusive access to a [`Mutex`]'s data.
///
/// The guard does not disable preemption and may remain alive across a
/// scheduler context switch. It is not transferable between tasks.
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
    _not_send: PhantomData<*mut ()>,
}

impl<'a, T> MutexGuard<'a, T> {
    #[inline]
    fn new(mutex: &'a Mutex<T>) -> Self {
        Self {
            mutex,
            _not_send: PhantomData,
        }
    }
}

impl<T> core::ops::Deref for MutexGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: this guard exclusively owns the mutex's atomic lock.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> core::ops::DerefMut for MutexGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: same justification as `Deref`.
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.mutex.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::preempt_count;

    #[test_case]
    fn mutex_guard_keeps_preemption_enabled() {
        let mutex = Mutex::new(41u32);
        assert_eq!(preempt_count(), 0);
        {
            let mut guard = mutex.lock();
            assert_eq!(preempt_count(), 0);
            *guard += 1;
        }
        assert_eq!(preempt_count(), 0);
        assert_eq!(*mutex.lock(), 42);
    }

    #[test_case]
    fn mutex_try_lock_reports_contention() {
        let mutex = Mutex::new(());
        let guard = mutex.lock();
        assert!(mutex.try_lock().is_none());
        drop(guard);
        assert!(mutex.try_lock().is_some());
    }

    #[test_case]
    fn mutex_get_mut_and_into_inner() {
        let mut mutex = Mutex::new(7u32);
        *mutex.get_mut() = 9;
        assert_eq!(mutex.into_inner(), 9);
    }

    #[test_case]
    fn mutex_waiter_transition_preserves_terminal_states() {
        for terminal in [TaskState::Zombie, TaskState::Terminated] {
            let state = AtomicTaskState::new(terminal);
            assert_eq!(transition_waiter_to_blocked(&state), Ok(false));
            assert_eq!(state.load(Ordering::SeqCst), terminal);
        }
    }
}
