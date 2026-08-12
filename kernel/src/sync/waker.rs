//! Waker - Synchronization primitive for task waiting and waking
//!
//! This module provides the `Waker` struct, which manages asynchronous task waiting
//! and waking mechanisms. It allows tasks to block on specific events and be woken
//! up when those events occur, such as I/O completion or interrupt handling.

extern crate alloc;

use crate::arch::Trapframe;
use crate::sched::scheduler::{
    current_task_id, get_task_by_id, remove_from_ready_queues, schedule, unmark_blocked, wake_task,
    wake_task_on,
};
use crate::sync::IrqSpinLock;
use crate::task::{BlockedType, TaskState};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::fmt;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

const DIAGNOSTIC_WAKE_EVENT_WAITERS_ON_SOURCE_CPU: bool = false;

const WAIT_OUTCOME_PENDING: u8 = 0;
const WAIT_OUTCOME_EVENT_WAKE_IN_PROGRESS: u8 = 1;
const WAIT_OUTCOME_TIMEOUT_WAKE_IN_PROGRESS: u8 = 2;
const WAIT_OUTCOME_EVENT_WAKE_COMPLETE: u8 = 3;
const WAIT_OUTCOME_TIMEOUT_WAKE_COMPLETE: u8 = 4;
const WAIT_OUTCOME_INTERRUPTED: u8 = 5;

fn remaining_before_deadline(deadline_ns: u64, now_ns: u64) -> Option<u64> {
    (deadline_ns > now_ns).then(|| deadline_ns - now_ns)
}

/// One-shot arbitration shared by a waiter, event source, and timeout callback.
///
/// The winner is recorded before either side wakes the task. This prevents a
/// task that was made ready by an event from being reclassified as timed out
/// merely because scheduler load delayed it until after the timer callback.
struct WaitOutcome {
    state: AtomicU8,
}

impl WaitOutcome {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(WAIT_OUTCOME_PENDING),
        }
    }

    fn is_pending(&self) -> bool {
        self.state.load(Ordering::Acquire) == WAIT_OUTCOME_PENDING
    }

    fn try_begin_event_wake(&self) -> bool {
        self.state
            .compare_exchange(
                WAIT_OUTCOME_PENDING,
                WAIT_OUTCOME_EVENT_WAKE_IN_PROGRESS,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Claim a readiness notification that was latched before queueing.
    ///
    /// No producer still owes a `wake_task()` call in this path, so the event
    /// is published directly as complete.
    fn claim_coalesced_event(&self) -> bool {
        self.state
            .compare_exchange(
                WAIT_OUTCOME_PENDING,
                WAIT_OUTCOME_EVENT_WAKE_COMPLETE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn try_timeout(&self) -> bool {
        self.state
            .compare_exchange(
                WAIT_OUTCOME_PENDING,
                WAIT_OUTCOME_TIMEOUT_WAKE_IN_PROGRESS,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Publish that the timeout callback can no longer wake this task.
    ///
    /// The callback must call this only after its `wake_task()` attempt. A task
    /// can otherwise be resumed by an unrelated interruptible wake while the
    /// timeout callback is still running, enter a later wait, and have that
    /// later wait consumed by the old callback.
    fn complete_timeout_wake(&self) {
        debug_assert_eq!(
            self.state.load(Ordering::Acquire),
            WAIT_OUTCOME_TIMEOUT_WAKE_IN_PROGRESS
        );
        self.state
            .store(WAIT_OUTCOME_TIMEOUT_WAKE_COMPLETE, Ordering::Release);
    }

    /// Publish that an event producer can no longer wake this task.
    fn complete_event_wake(&self) {
        debug_assert_eq!(
            self.state.load(Ordering::Acquire),
            WAIT_OUTCOME_EVENT_WAKE_IN_PROGRESS
        );
        self.state
            .store(WAIT_OUTCOME_EVENT_WAKE_COMPLETE, Ordering::Release);
    }

    /// Settle a resumed wait and wait out any producer's final wake attempt.
    ///
    /// `true` means the wait ended for a reason other than its timeout. A
    /// direct interruptible wake is deliberately reported like an event, which
    /// preserves the previous API while cancelling this exact registration.
    fn finish_after_resume(&self) -> bool {
        loop {
            match self.state.load(Ordering::Acquire) {
                WAIT_OUTCOME_PENDING => {
                    let _ = self.state.compare_exchange(
                        WAIT_OUTCOME_PENDING,
                        WAIT_OUTCOME_INTERRUPTED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                }
                WAIT_OUTCOME_EVENT_WAKE_IN_PROGRESS | WAIT_OUTCOME_TIMEOUT_WAKE_IN_PROGRESS => {
                    core::hint::spin_loop();
                }
                WAIT_OUTCOME_EVENT_WAKE_COMPLETE | WAIT_OUTCOME_INTERRUPTED => return true,
                WAIT_OUTCOME_TIMEOUT_WAKE_COMPLETE => return false,
                _ => unreachable!("invalid wait outcome state"),
            }
        }
    }
}

#[derive(Clone)]
struct WaitQueueEntry {
    task_id: usize,
    outcome: Arc<WaitOutcome>,
}

impl WaitQueueEntry {
    fn new(task_id: usize, outcome: Arc<WaitOutcome>) -> Self {
        Self { task_id, outcome }
    }

    /// Claim this registration for an event wake.
    ///
    /// An already-settled registration is stale and must be skipped so an
    /// older wait cannot consume a newer readiness notification.
    fn try_claim_event(&self) -> bool {
        self.outcome.try_begin_event_wake()
    }

    fn complete_event_wake(&self) {
        self.outcome.complete_event_wake();
    }

    fn matches_registration(&self, task_id: usize, outcome: &Arc<WaitOutcome>) -> bool {
        self.task_id == task_id && Arc::ptr_eq(&self.outcome, outcome)
    }
}

/// A synchronization primitive that manages waiting and waking of tasks
///
/// The `Waker` struct provides a mechanism for tasks to wait for specific events
/// and be woken up when those events occur. It maintains a queue of waiting task IDs
/// and provides methods to block the current task or wake up waiting tasks.
///
/// # Examples
///
/// ```
/// // Create a new interruptible waker for UART receive events
/// static UART_RX_WAKER: Waker = Waker::new_interruptible("uart_rx");
///
/// // In a blocking read function
/// UART_RX_WAKER.wait();
///
/// // In an interrupt handler
/// UART_RX_WAKER.wake_one();
/// ```
pub struct Waker {
    /// Queue of task waits with one-shot registration arbitration.
    wait_queue: IrqSpinLock<VecDeque<WaitQueueEntry>>,
    /// The type of blocking this waker uses (interruptible or uninterruptible)
    block_type: BlockedType,
    /// Human-readable name for debugging purposes
    name: &'static str,
    /// Pre-wait notification latch. A wake on an empty queue records one
    /// pending notification so the next waiter cannot miss the condition change.
    /// Repeated wakes are coalesced: resource multiplicity lives in the
    /// protected producer state, not in this synchronization primitive.
    pending_wakes: AtomicUsize,
}

impl Waker {
    fn wake_waiting_task(&self, task_id: usize) -> bool {
        #[cfg(feature = "sync-debug")]
        crate::breadcrumb::drop(crate::breadcrumb::WAKER_WAKE, task_id as u64, 0);
        let woke = if DIAGNOSTIC_WAKE_EVENT_WAITERS_ON_SOURCE_CPU && self.name.starts_with("event_")
        {
            wake_task_on(task_id, crate::arch::get_cpu().get_cpuid())
        } else {
            wake_task(task_id)
        };
        woke
    }

    fn remove_wait_registration(&self, task_id: usize, outcome: &Arc<WaitOutcome>) -> bool {
        let mut queue = self.wait_queue.lock();
        let Some(position) = queue
            .iter()
            .position(|entry| entry.matches_registration(task_id, outcome))
        else {
            return false;
        };
        queue.remove(position);
        true
    }

    /// Create a new interruptible waker
    ///
    /// Interruptible wakers allow waiting tasks to be interrupted by signals
    /// or other asynchronous events. This is suitable for user I/O operations
    /// where cancellation might be needed.
    ///
    /// # Arguments
    ///
    /// * `name` - A human-readable name for debugging purposes
    ///
    /// # Examples
    ///
    /// ```
    /// static KEYBOARD_WAKER: Waker = Waker::new_interruptible("keyboard");
    /// ```
    pub const fn new_interruptible(name: &'static str) -> Self {
        Self {
            wait_queue: IrqSpinLock::new(VecDeque::new()),
            block_type: BlockedType::Interruptible,
            name,
            pending_wakes: AtomicUsize::new(0),
        }
    }

    /// Create a new uninterruptible waker
    ///
    /// Uninterruptible wakers ensure that waiting tasks cannot be interrupted
    /// and will wait until the event occurs. This is suitable for critical
    /// operations like disk I/O where data integrity is important.
    ///
    /// # Arguments
    ///
    /// * `name` - A human-readable name for debugging purposes
    ///
    /// # Examples
    ///
    /// ```
    /// static DISK_IO_WAKER: Waker = Waker::new_uninterruptible("disk_io");
    /// ```
    pub const fn new_uninterruptible(name: &'static str) -> Self {
        Self {
            wait_queue: IrqSpinLock::new(VecDeque::new()),
            block_type: BlockedType::Uninterruptible,
            name,
            pending_wakes: AtomicUsize::new(0),
        }
    }

    /// Block the current task and add it to the wait queue
    ///
    /// This method puts the current task into a blocked state and adds its ID
    /// to the wait queue. The task will remain blocked until another part of
    /// the system calls `wake_one()` or `wake_all()` on this waker.
    ///
    /// # Behavior
    ///
    /// 1. Gets the current task ID
    /// 2. Sets the task state to `Blocked(self.block_type)` FIRST
    /// 3. Adds the task ID to the wait queue
    /// 4. Calls the scheduler to yield CPU to other tasks
    /// 5. Returns when the task is woken up and rescheduled
    ///
    /// # Note
    ///
    /// This function returns when the task is woken up by another part of the system.
    /// The calling code can then continue execution, typically to re-check the
    /// condition that caused the wait.
    ///
    /// # Critical Section
    ///
    /// To prevent race conditions between wait() and wake_one()/wake_all():
    /// 1. Set task state to Blocked BEFORE adding to queue
    /// 2. This ensures wake_task() can safely operate even if called immediately
    pub fn wait(&self, task_id: usize, trapframe: &mut Trapframe) {
        let outcome = Arc::new(WaitOutcome::new());
        let mut should_schedule = self.prepare_wait_registration(task_id, outcome.clone());
        if !outcome.is_pending() && self.cancel_prepared_wait_registration(task_id, Some(&outcome))
        {
            should_schedule = false;
        }
        if should_schedule {
            schedule(trapframe);
        }
        let _ = outcome.finish_after_resume();
        let _ = self.remove_wait_registration(task_id, &outcome);
    }

    /// Block using an owned waker handle without retaining it on a suspended stack.
    ///
    /// # Arguments
    ///
    /// * `task_id` - ID of the current task to block
    /// * `trapframe` - Current task's saved execution state
    pub fn wait_owned(self: Arc<Self>, task_id: usize, trapframe: &mut Trapframe) {
        let outcome = Arc::new(WaitOutcome::new());
        let mut should_schedule = self.prepare_wait_registration(task_id, outcome.clone());
        if !outcome.is_pending() && self.cancel_prepared_wait_registration(task_id, Some(&outcome))
        {
            should_schedule = false;
        }
        let weak_waker = Arc::downgrade(&self);
        drop(self);
        if should_schedule {
            schedule(trapframe);
        }
        let _ = outcome.finish_after_resume();
        if let Some(waker) = weak_waker.upgrade() {
            let _ = waker.remove_wait_registration(task_id, &outcome);
        }
    }

    /// Block until woken or a timeout while dropping the owned waker before scheduling.
    ///
    /// Dynamic wait queues use this variant so forced task teardown cannot
    /// strand an `Arc` on a suspended kernel stack. The timer retains the
    /// waker and removes only this task from its queue when the timeout wins.
    ///
    /// # Arguments
    ///
    /// * `task_id` - ID of the current task to block.
    /// * `trapframe` - Current task's saved execution state.
    /// * `timeout_ns` - Relative timeout in nanoseconds.
    ///
    /// # Returns
    ///
    /// `true` when an event wake won, or `false` when the timeout won.
    pub fn wait_with_timeout_owned(
        self: Arc<Self>,
        task_id: usize,
        trapframe: &mut Trapframe,
        timeout_ns: u64,
    ) -> bool {
        if timeout_ns == 0 {
            return false;
        }

        use crate::timer::{TimerHandle, TimerHandler, add_timer, get_time_ns};

        struct OwnedTimeoutWake {
            task_id: usize,
            outcome: Arc<WaitOutcome>,
            timer_handle: crate::sync::IrqSpinLock<Option<TimerHandle>>,
            waker: Arc<Waker>,
        }

        impl TimerHandler for OwnedTimeoutWake {
            fn on_timer_expired(self: Arc<Self>, _context: usize) {
                let timeout_won = self.outcome.try_timeout();
                #[cfg(feature = "sync-debug")]
                crate::breadcrumb::drop(
                    crate::breadcrumb::WAKER_TIMEOUT,
                    self.task_id as u64,
                    u64::from(timeout_won),
                );
                if let Some(task) = crate::sched::scheduler::get_task_by_id(self.task_id)
                    && let Some(timer_handle) = self.timer_handle.lock().take()
                {
                    task.finish_software_timer(timer_handle);
                }
                if timeout_won {
                    let _ = self
                        .waker
                        .remove_wait_registration(self.task_id, &self.outcome);
                    let _ = wake_task(self.task_id);
                    self.outcome.complete_timeout_wake();
                }
            }
        }

        let Some(task) = crate::sched::scheduler::get_task_by_id(task_id) else {
            return false;
        };
        let outcome = Arc::new(WaitOutcome::new());
        let handler = Arc::new(OwnedTimeoutWake {
            task_id,
            outcome: outcome.clone(),
            timer_handle: crate::sync::IrqSpinLock::new(None),
            waker: self.clone(),
        });
        let handler_ref: Arc<dyn TimerHandler> = handler.clone();
        let timer_handle = add_timer(
            get_time_ns().saturating_add(timeout_ns),
            crate::timer::TimerPrecision::Normal,
            &handler_ref,
            0,
        );
        *handler.timer_handle.lock() = Some(timer_handle);
        task.register_software_timer(timer_handle, handler_ref.clone());
        drop(handler_ref);
        drop(handler);
        drop(task);

        if !outcome.is_pending() {
            if let Some(task) = crate::sched::scheduler::get_task_by_id(task_id) {
                let _ = task.finish_software_timer(timer_handle);
            }
            return outcome.finish_after_resume();
        }

        let mut should_schedule = self.prepare_wait_registration(task_id, outcome.clone());
        if !outcome.is_pending() && self.cancel_prepared_wait_registration(task_id, Some(&outcome))
        {
            should_schedule = false;
        }

        drop(self);
        if should_schedule {
            schedule(trapframe);
        }

        let event_won = outcome.finish_after_resume();
        if let Some(task) = crate::sched::scheduler::get_task_by_id(task_id) {
            let _ = task.finish_software_timer(timer_handle);
        }
        event_won
    }

    #[cfg(test)]
    fn prepare_wait(&self, task_id: usize) -> bool {
        self.prepare_wait_registration(task_id, Arc::new(WaitOutcome::new()))
    }

    /// Consume one coalesced readiness notification without enqueueing.
    ///
    /// This is used after a mandatory minimum sleep. It avoids arming and then
    /// immediately cancelling another timeout timer when IPC activity already
    /// left a readiness notification during that sleep.
    fn consume_pending_wake(&self) -> bool {
        let _queue = self.wait_queue.lock();
        self.pending_wakes.swap(0, Ordering::SeqCst) != 0
    }

    fn prepare_wait_registration(&self, task_id: usize, outcome: Arc<WaitOutcome>) -> bool {
        #[cfg(feature = "sync-debug")]
        crate::breadcrumb::drop(
            crate::breadcrumb::WAKER_PREPARE,
            task_id as u64,
            self.pending_wakes.load(Ordering::Relaxed) as u64,
        );
        // Consume a pending wake that arrived before we enqueued ourselves.
        // This closes the lost-wake window on SMP: if wake_one()/wake_all()
        // fired while the queue was empty (between the caller's condition check
        // and this wait() call), we return immediately instead of sleeping.
        // Serialize the pending-wake check with queue insertion and with the
        // producer's empty-queue path.  Checking the atomic by itself leaves
        // a window where wake_one() observes an empty queue, the waiter then
        // enqueues, and the producer sets the pending-wake latch afterwards;
        // that latch belongs to no waiter and can corrupt a later wait.
        let bypass_wait = {
            let _queue = self.wait_queue.lock();
            if !outcome.is_pending() {
                true
            } else if self.pending_wakes.swap(0, Ordering::SeqCst) != 0 {
                let claimed = outcome.claim_coalesced_event();
                if !claimed {
                    // A timeout won concurrently. Preserve this readiness
                    // notification for the caller's next level-state check.
                    self.pending_wakes.store(1, Ordering::SeqCst);
                }
                true
            } else {
                false
            }
        };
        if bypass_wait {
            return false;
        }

        let task = get_task_by_id(task_id)
            .unwrap_or_else(|| panic!("[WAKER] Task ID {} not found in scheduler", task_id));
        let blocked_state = TaskState::Blocked(self.block_type);
        let local_cpu = crate::arch::get_cpu().get_cpuid();
        let mut state = task.state.load(Ordering::SeqCst);
        loop {
            match state {
                TaskState::Running => match task.state.compare_exchange(
                    TaskState::Running,
                    blocked_state,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => break,
                    Err(actual) => state = actual,
                },
                TaskState::Ready
                    if current_task_id(local_cpu) == Some(task_id)
                        && task.running_cpu.load(Ordering::SeqCst) == local_cpu =>
                {
                    // A task that is physically executing on this CPU cannot
                    // also be a runnable queue candidate. This can occur when
                    // a wake wins immediately before a previous schedule and
                    // the caller reaches another wait before scheduler state
                    // normalization. Remove any stale queue entry before
                    // blocking so wait loops cannot degrade into a userspace
                    // CPU spin.
                    remove_from_ready_queues(task_id);
                    match task.state.compare_exchange(
                        TaskState::Ready,
                        blocked_state,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => break,
                        Err(actual) => state = actual,
                    }
                }
                TaskState::Zombie | TaskState::Terminated | TaskState::Ready => {
                    drop(task);
                    return true;
                }
                TaskState::NotInitialized | TaskState::Blocked(_) => {
                    drop(task);
                    panic!(
                        "[WAKER] Task {} cannot enter wait from state {:?}",
                        task_id, state
                    );
                }
            }
        }
        drop(task);

        crate::sched::scheduler::mark_blocked(task_id);

        // Enqueue and re-check pending_wakes under the same lock acquisition.
        // This closes the SMP lost-wake window: wake_one()/wake_all() that fires
        // after the initial pending_wakes check but before queue insertion will
        // increment pending_wakes on an empty queue.  Re-checking here catches
        // that case so the waiter never sleeps on an already-fired wake.
        let bypass_wait = {
            let mut queue = self.wait_queue.lock();
            if !outcome.is_pending() {
                true
            } else if self.pending_wakes.swap(0, Ordering::SeqCst) != 0 {
                let claimed = outcome.claim_coalesced_event();
                if !claimed {
                    self.pending_wakes.store(1, Ordering::SeqCst);
                }
                true
            } else {
                queue.push_back(WaitQueueEntry::new(task_id, outcome.clone()));
                false
            }
        };

        let terminated_while_enqueuing = get_task_by_id(task_id).is_some_and(|task| {
            matches!(task.get_state(), TaskState::Zombie | TaskState::Terminated)
        });
        if terminated_while_enqueuing {
            let _cancelled = self.cancel_prepared_wait_registration(task_id, Some(&outcome));
            return true;
        }

        let registration_settled = !outcome.is_pending();
        if bypass_wait || registration_settled {
            // If ownership was lost, leave the ready task untouched and let the
            // scheduler observe it rather than consuming a wake on another CPU.
            return !self.cancel_prepared_wait_registration(task_id, Some(&outcome));
        }

        true
    }

    /// Roll back a prepared wait only while this CPU still owns the task.
    ///
    /// # Returns
    ///
    /// `true` if queue bookkeeping was repaired, or `false` if the task is
    /// missing or is no longer the current locally-owned task.
    #[cfg(test)]
    fn cancel_prepared_wait(&self, task_id: usize) -> bool {
        self.cancel_prepared_wait_registration(task_id, None)
    }

    fn cancel_prepared_wait_registration(
        &self,
        task_id: usize,
        outcome: Option<&Arc<WaitOutcome>>,
    ) -> bool {
        let local_cpu = crate::arch::get_cpu().get_cpuid();
        let Some(task) = get_task_by_id(task_id) else {
            return false;
        };
        if crate::sched::scheduler::current_task_id(local_cpu) != Some(task_id)
            || task.running_cpu.load(Ordering::SeqCst) != local_cpu
        {
            if crate::sched::scheduler::DEBUG_SMP_TASK_FLOW {
                crate::println!(
                    "[SMPDBG waker-cancel-rejected] waker={} cpu={} task={} running_cpu={}",
                    self.name,
                    local_cpu,
                    task_id,
                    task.running_cpu.load(Ordering::SeqCst),
                );
            }
            return false;
        }

        {
            let mut queue = self.wait_queue.lock();
            queue.retain(|entry| match outcome {
                Some(outcome) => !entry.matches_registration(task_id, outcome),
                None => entry.task_id != task_id,
            });
        }
        unmark_blocked(task_id);
        remove_from_ready_queues(task_id);

        let blocked_state = TaskState::Blocked(self.block_type);
        let mut state = task.state.load(Ordering::SeqCst);
        loop {
            match state {
                current_state if current_state == blocked_state => {
                    match task.state.compare_exchange(
                        blocked_state,
                        TaskState::Running,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => return true,
                        Err(actual) => state = actual,
                    }
                }
                TaskState::Ready => {
                    // A timeout can wake this current task after preparation but
                    // before schedule. Its ready-queue entry was removed above,
                    // so make the still-executing task running again.
                    match task.state.compare_exchange(
                        TaskState::Ready,
                        TaskState::Running,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => return true,
                        Err(actual) => state = actual,
                    }
                }
                TaskState::NotInitialized
                | TaskState::Running
                | TaskState::Blocked(_)
                | TaskState::Zombie
                | TaskState::Terminated => return true,
            }
        }
    }

    /// Block the task until woken or the timeout elapses.
    ///
    /// Returns true if woken by event, false if timeout elapsed.
    pub fn wait_with_timeout(
        &self,
        task_id: usize,
        trapframe: &mut Trapframe,
        timeout_ns: Option<u64>,
    ) -> bool {
        self.wait_with_timeout_precision(
            task_id,
            trapframe,
            timeout_ns,
            crate::timer::TimerPrecision::Normal,
        )
    }

    /// Block the task until woken or the timeout elapses with explicit timer precision.
    ///
    /// # Arguments
    ///
    /// * `task_id` - Task to block and wake.
    /// * `trapframe` - Current task trapframe.
    /// * `timeout_ns` - Optional relative timeout in nanoseconds.
    /// * `precision` - Permitted timer delivery range.
    ///
    /// # Returns
    ///
    /// `true` if woken by an event, `false` if the timeout elapsed.
    pub fn wait_with_timeout_precision(
        &self,
        task_id: usize,
        trapframe: &mut Trapframe,
        timeout_ns: Option<u64>,
        precision: crate::timer::TimerPrecision,
    ) -> bool {
        if matches!(timeout_ns, Some(0)) {
            return false;
        }

        if let Some(duration_ns) = timeout_ns {
            use crate::timer::{TimerHandle, TimerHandler, add_timer, get_time_ns};
            use alloc::sync::Arc;

            struct TimeoutWake {
                task_id: usize,
                outcome: Arc<WaitOutcome>,
                timer_handle: crate::sync::IrqSpinLock<Option<TimerHandle>>,
            }

            impl TimerHandler for TimeoutWake {
                fn on_timer_expired(self: Arc<Self>, _context: usize) {
                    let timeout_won = self.outcome.try_timeout();
                    #[cfg(feature = "sync-debug")]
                    crate::breadcrumb::drop(
                        crate::breadcrumb::WAKER_TIMEOUT,
                        self.task_id as u64,
                        u64::from(timeout_won),
                    );
                    if let Some(task) = crate::sched::scheduler::get_task_by_id(self.task_id) {
                        if let Some(timer_handle) = self.timer_handle.lock().take() {
                            task.finish_software_timer(timer_handle);
                        }
                    }
                    if timeout_won {
                        let _ = wake_task(self.task_id);
                        self.outcome.complete_timeout_wake();
                    }
                }
            }

            let outcome = Arc::new(WaitOutcome::new());
            let timer_handle = {
                let Some(task) = crate::sched::scheduler::get_task_by_id(task_id) else {
                    return false;
                };
                let handler = Arc::new(TimeoutWake {
                    task_id,
                    outcome: outcome.clone(),
                    timer_handle: crate::sync::IrqSpinLock::new(None),
                });
                let handler_ref: Arc<dyn TimerHandler> = handler.clone();
                let timer_handle = add_timer(
                    get_time_ns().saturating_add(duration_ns),
                    precision,
                    &handler_ref,
                    0,
                );
                *handler.timer_handle.lock() = Some(timer_handle);
                task.register_software_timer(timer_handle, handler_ref.clone());
                drop(handler_ref);
                drop(handler);
                drop(task);
                timer_handle
            };

            if !outcome.is_pending() {
                if let Some(task) = crate::sched::scheduler::get_task_by_id(task_id) {
                    let _ = task.finish_software_timer(timer_handle);
                }
                return outcome.finish_after_resume();
            }

            let mut should_schedule = self.prepare_wait_registration(task_id, outcome.clone());
            if !outcome.is_pending()
                && self.cancel_prepared_wait_registration(task_id, Some(&outcome))
            {
                should_schedule = false;
            }
            if should_schedule {
                schedule(trapframe);
            }

            let event_won = outcome.finish_after_resume();
            let _ = self.remove_wait_registration(task_id, &outcome);
            if let Some(task) = crate::sched::scheduler::get_task_by_id(task_id) {
                let _ = task.finish_software_timer(timer_handle);
            }
            event_won
        } else {
            self.wait(task_id, trapframe);
            true
        }
    }

    /// Block the task until woken, but wait at least `min_wait_ns` before
    /// returning even if woken early by the selectable waker. After the minimum
    /// wait elapses, continues blocking until woken or `timeout_ns` expires. A
    /// finite timeout is measured from function entry and clips the minimum
    /// phase if a direct caller supplies a shorter overall timeout.
    ///
    /// # Arguments
    ///
    /// * `task_id` - Task to block and wake.
    /// * `trapframe` - Current task trapframe.
    /// * `timeout_ns` - Optional relative overall timeout in nanoseconds.
    /// * `min_wait_ns` - Minimum initial blocking interval in nanoseconds.
    ///
    /// # Returns
    ///
    /// `true` if woken by an event, or `false` if the overall timeout elapsed.
    pub fn wait_with_min_timeout(
        &self,
        task_id: usize,
        trapframe: &mut Trapframe,
        timeout_ns: Option<u64>,
        min_wait_ns: u64,
    ) -> bool {
        use crate::timer::{TimerPrecision, get_time_ns};

        if min_wait_ns == 0 {
            return self.wait_with_timeout(task_id, trapframe, timeout_ns);
        }

        let started_ns = get_time_ns();
        let deadline_ns = timeout_ns.map(|duration| started_ns.saturating_add(duration));
        let minimum_duration_ns = timeout_ns
            .map(|duration| min_wait_ns.min(duration))
            .unwrap_or(min_wait_ns);

        if minimum_duration_ns > 0 {
            let Some(task) = get_task_by_id(task_id) else {
                return false;
            };
            // Do not register on the selectable's event Waker during the
            // minimum phase. Readiness notifications remain in its coalesced
            // pending latch, while this dedicated sleep cannot be driven into
            // a wake/re-wait loop by IPC traffic.
            task.sleep_with_precision(trapframe, minimum_duration_ns, TimerPrecision::Normal);
            drop(task);
        }

        if self.consume_pending_wake() {
            return true;
        }

        let remaining_timeout = match deadline_ns {
            Some(deadline_ns) => {
                let Some(remaining_ns) = remaining_before_deadline(deadline_ns, get_time_ns())
                else {
                    return false;
                };
                Some(remaining_ns)
            }
            None => None,
        };
        self.wait_with_timeout(task_id, trapframe, remaining_timeout)
    }

    // /// Block any task (not limited to the current task) and add it to the wait queue
    // ///
    // /// This method is intended for blocking tasks other than the current one.
    // /// It sets the specified task's state to Blocked and adds it to the wait queue.
    // /// No scheduler switch or CPU state saving is performed.
    // ///
    // /// # Arguments
    // /// * `task_id` - The ID of the task to be blocked
    // pub fn block(&self, task_id: usize) {
    //     {
    //         let mut queue = self.wait_queue.lock();
    //         queue.push_back(task_id);
    //     }

    //     if let Some(task) = get_scheduler().get_task_by_id(task_id) {
    //         // Set task state to blocked
    //         task.set_state(TaskState::Blocked(self.block_type));
    //     } else {
    //         panic!("[WAKER] Task ID {} not found in scheduler", task_id);
    //     }

    //     // Yield CPU to scheduler - this will return when the task is woken up
    //     get_scheduler().schedule(cpu);

    //     // When we reach here, the task has been woken up and rescheduled
    //     // crate::println!("[WAKER] Task {} woken up from waker '{}'", task_id, self.name);
    // }

    /// Wake up one waiting task
    ///
    /// This method removes one task from the wait queue and moves it from
    /// the blocked queue to the ready queue, making it eligible for scheduling again.
    ///
    /// # Returns
    ///
    /// * `true` if a task was woken up
    /// * `false` if the wait queue was empty
    ///
    /// # Examples
    ///
    /// ```
    /// // In an interrupt handler
    /// if UART_RX_WAKER.wake_one() {
    ///     // A task was woken up
    /// }
    /// ```
    pub fn wake_one(&self) -> bool {
        loop {
            let entry = {
                let mut queue = self.wait_queue.lock();
                loop {
                    match queue.pop_front() {
                        Some(entry) if entry.try_claim_event() => break entry,
                        Some(_) => {
                            // A timeout or another wake already settled this
                            // registration. Continue to the next live waiter.
                        }
                        None => {
                            // Keep this update under the queue lock so a waiter
                            // cannot enqueue between the empty check and the
                            // pending-wake latch publication.
                            self.pending_wakes.store(1, Ordering::SeqCst);
                            return false;
                        }
                    }
                }
            };
            let task_id = entry.task_id;
            let woke = self.wake_waiting_task(task_id);
            let delivered = woke
                || get_task_by_id(task_id).is_some_and(|task| {
                    matches!(task.get_state(), TaskState::Ready | TaskState::Running)
                });
            // Sample delivery before releasing the completion barrier. Once
            // complete is visible, the resumed task may immediately enter a
            // different wait and change its state back to Blocked.
            entry.complete_event_wake();
            if crate::sched::scheduler::DEBUG_SMP_TASK_FLOW {
                crate::println!(
                    "[SMPDBG waker-wake-one] waker={} cpu={} task={} woke={} delivered={}",
                    self.name,
                    crate::arch::get_cpu().get_cpuid(),
                    task_id,
                    woke,
                    delivered,
                );
            }
            if delivered {
                return true;
            }
        }
    }

    /// Wake up all waiting tasks
    ///
    /// This method removes all tasks from the wait queue and moves them from
    /// the blocked queue to the ready queue, making them all eligible for scheduling again.
    ///
    /// # Returns
    ///
    /// The number of tasks that were woken up
    ///
    /// # Examples
    ///
    /// ```
    /// // Wake all tasks waiting for a broadcast event
    /// let woken_count = BROADCAST_WAKER.wake_all();
    /// println!("Woke up {} tasks", woken_count);
    /// ```
    pub fn wake_all(&self) -> usize {
        let mut woken_count = 0;

        loop {
            let entries = {
                let mut queue = self.wait_queue.lock();
                let entries: VecDeque<WaitQueueEntry> = queue
                    .drain(..)
                    .filter_map(|entry| entry.try_claim_event().then_some(entry))
                    .collect();
                if entries.is_empty() {
                    if woken_count == 0 {
                        // A condition notification is a latch, not a
                        // semaphore permit. Repeated empty-queue wakes
                        // must not make future waits spin through history.
                        self.pending_wakes.store(1, Ordering::SeqCst);
                    }
                    return woken_count;
                }
                entries
            };

            for entry in entries {
                let task_id = entry.task_id;
                let woke = self.wake_waiting_task(task_id);
                let delivered = woke
                    || get_task_by_id(task_id).is_some_and(|task| {
                        matches!(task.get_state(), TaskState::Ready | TaskState::Running)
                    });
                entry.complete_event_wake();
                if delivered {
                    woken_count += 1;
                }
                if crate::sched::scheduler::DEBUG_SMP_TASK_FLOW {
                    crate::println!(
                        "[SMPDBG waker-wake-all] waker={} cpu={} task={} woke={} delivered={}",
                        self.name,
                        crate::arch::get_cpu().get_cpuid(),
                        task_id,
                        woke,
                        delivered,
                    );
                }
            }

            if woken_count > 0 {
                return woken_count;
            }

            // Every drained entry was stale. Recheck atomically with
            // queue insertion before publishing the pending latch.
        }
    }

    /// Get the blocking type of this waker
    ///
    /// # Returns
    ///
    /// The `BlockedType` (either `Interruptible` or `Uninterruptible`)
    pub fn block_type(&self) -> BlockedType {
        self.block_type
    }

    /// Get the number of tasks currently waiting
    ///
    /// # Returns
    ///
    /// The number of tasks in the wait queue
    pub fn waiting_count(&self) -> usize {
        self.wait_queue.lock().len()
    }

    /// Get the name of this waker
    ///
    /// # Returns
    ///
    /// The human-readable name for debugging purposes
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Get a list of task IDs currently waiting in the queue
    ///
    /// This method returns a snapshot of all task IDs currently waiting
    /// in this waker's queue. Useful for debugging and monitoring.
    ///
    /// # Returns
    ///
    /// A vector containing all waiting task IDs
    ///
    /// # Examples
    ///
    /// ```
    /// let waiting_tasks = waker.get_waiting_task_ids();
    /// println!("Tasks waiting: {:?}", waiting_tasks);
    /// ```
    pub fn get_waiting_task_ids(&self) -> VecDeque<usize> {
        self.wait_queue
            .lock()
            .iter()
            .map(|entry| entry.task_id)
            .collect()
    }

    /// Check if a specific task is waiting in this waker
    ///
    /// # Arguments
    ///
    /// * `task_id` - The ID of the task to check
    ///
    /// # Returns
    ///
    /// `true` if the task is waiting in this waker, `false` otherwise
    pub fn is_task_waiting(&self, task_id: usize) -> bool {
        self.wait_queue
            .lock()
            .iter()
            .any(|entry| entry.task_id == task_id)
    }

    /// Remove every queued occurrence of one task without waking it.
    ///
    /// This is used by task teardown after the task can no longer resume. It
    /// must not be used as a normal cancellation mechanism because it does not
    /// repair the task's scheduler state.
    ///
    /// # Arguments
    ///
    /// * `task_id` - Task being permanently removed.
    ///
    /// # Returns
    ///
    /// `true` when at least one queue entry was removed.
    pub(crate) fn remove_terminated_task(&self, task_id: usize) -> bool {
        let mut queue = self.wait_queue.lock();
        let previous_len = queue.len();
        queue.retain(|entry| entry.task_id != task_id);
        queue.len() != previous_len
    }

    /// Get detailed statistics about this waker
    ///
    /// This method provides detailed information about the current state
    /// of the waker, including all waiting tasks and their metadata.
    ///
    /// # Returns
    ///
    /// A `WakerStats` struct containing comprehensive state information
    ///
    /// # Examples
    ///
    /// ```
    /// let stats = uart_waker.get_stats();
    /// // Use Debug trait to print the stats
    /// ```
    pub fn get_stats(&self) -> WakerStats {
        let waiting_tasks = self.wait_queue.lock();
        WakerStats {
            name: self.name,
            block_type: self.block_type,
            waiting_count: waiting_tasks.len(),
            waiting_task_ids: waiting_tasks.iter().map(|entry| entry.task_id).collect(),
        }
    }

    /// Print debug information about this waker
    ///
    /// Outputs detailed information about the waker's current state
    /// including name, blocking type, waiting task count, and task IDs.
    /// Useful for debugging and monitoring system state.
    ///
    /// # Examples
    ///
    /// ```
    /// waker.debug_print();
    /// // Output:
    /// // [Waker DEBUG] uart_rx: Interruptible, 3 waiting tasks: [42, 137, 89]
    /// ```
    /// Check if the waker has any waiting tasks
    ///
    /// # Returns
    ///
    /// `true` if there are no waiting tasks, `false` otherwise
    pub fn is_empty(&self) -> bool {
        self.wait_queue.lock().is_empty()
    }

    /// Clear all waiting tasks without waking them
    ///
    /// This is a dangerous operation that should only be used in
    /// exceptional circumstances like system cleanup or error recovery.
    /// The tasks will remain in blocked state and need to be handled
    /// separately.
    ///
    /// # Returns
    ///
    /// The number of tasks that were removed from the queue
    ///
    /// # Safety
    ///
    /// This operation can leave tasks in a permanently blocked state.
    /// Use with extreme caution.
    pub fn clear_queue(&self) -> usize {
        let mut queue = self.wait_queue.lock();
        let count = queue.len();
        queue.clear();
        count
    }

    #[cfg(test)]
    pub(crate) fn pending_wake_count_for_test(&self) -> usize {
        self.pending_wakes.load(Ordering::SeqCst)
    }
}

impl fmt::Debug for Waker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let waiting_tasks = self.wait_queue.lock();
        let waiting_task_ids: VecDeque<usize> =
            waiting_tasks.iter().map(|entry| entry.task_id).collect();
        f.debug_struct("Waker")
            .field("name", &self.name)
            .field("block_type", &self.block_type)
            .field("waiting_count", &waiting_tasks.len())
            .field("waiting_task_ids", &waiting_task_ids)
            .field("pending_wakes", &self.pending_wakes.load(Ordering::Relaxed))
            .finish()
    }
}

/// Statistics and state information for a Waker
///
/// This struct provides a comprehensive view of a waker's current state,
/// useful for debugging, monitoring, and system analysis.
#[derive(Debug, Clone)]
pub struct WakerStats {
    /// Human-readable name of the waker
    pub name: &'static str,
    /// The blocking type (Interruptible or Uninterruptible)
    pub block_type: BlockedType,
    /// Number of tasks currently waiting
    pub waiting_count: usize,
    /// List of task IDs currently waiting in the queue
    pub waiting_task_ids: VecDeque<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sched::scheduler::{
        add_task, get_task_by_id, has_ready_tasks, mark_blocked, register_online_cpu, reset,
        set_current_task_for_test,
    };
    use crate::task::{Task, TaskType};
    use alloc::string::ToString;

    fn pending_entry(task_id: usize) -> WaitQueueEntry {
        WaitQueueEntry::new(task_id, Arc::new(WaitOutcome::new()))
    }

    #[test_case]
    fn test_waker_creation() {
        let interruptible_waker = Waker::new_interruptible("test_int");
        assert_eq!(interruptible_waker.name(), "test_int");
        assert_eq!(interruptible_waker.block_type(), BlockedType::Interruptible);
        assert_eq!(interruptible_waker.waiting_count(), 0);

        let uninterruptible_waker = Waker::new_uninterruptible("test_unint");
        assert_eq!(uninterruptible_waker.name(), "test_unint");
        assert_eq!(
            uninterruptible_waker.block_type(),
            BlockedType::Uninterruptible
        );
        assert_eq!(uninterruptible_waker.waiting_count(), 0);
    }

    #[test_case]
    fn wait_outcome_records_only_the_first_winner() {
        let event_first = WaitOutcome::new();
        assert!(event_first.try_begin_event_wake());
        assert!(!event_first.try_timeout());
        event_first.complete_event_wake();
        assert!(event_first.finish_after_resume());

        let timeout_first = WaitOutcome::new();
        assert!(timeout_first.try_timeout());
        assert!(!timeout_first.try_begin_event_wake());
        timeout_first.complete_timeout_wake();
        assert!(!timeout_first.finish_after_resume());
        assert!(!timeout_first.try_begin_event_wake());

        let interrupted = WaitOutcome::new();
        assert!(interrupted.finish_after_resume());
        assert!(!interrupted.try_begin_event_wake());

        let coalesced = WaitOutcome::new();
        assert!(coalesced.claim_coalesced_event());
        assert!(coalesced.finish_after_resume());
    }

    #[test_case]
    fn remaining_timeout_uses_the_original_absolute_deadline() {
        assert_eq!(remaining_before_deadline(1_000, 400), Some(600));
        assert_eq!(remaining_before_deadline(1_000, 1_000), None);
        assert_eq!(remaining_before_deadline(1_000, 1_200), None);
    }

    #[test_case]
    fn test_wake_empty_queue() {
        let waker = Waker::new_interruptible("empty_test");
        assert_eq!(waker.wake_one(), false);
        assert_eq!(waker.wake_all(), 0);
    }

    #[test_case]
    fn test_empty_queue_wakes_coalesce_into_one_latch() {
        let waker = Waker::new_interruptible("coalesced-pending");

        assert!(!waker.wake_one());
        assert!(!waker.wake_one());
        assert_eq!(waker.wake_all(), 0);
        assert_eq!(
            waker.pending_wake_count_for_test(),
            1,
            "past readiness notifications must not accumulate"
        );
    }

    #[test_case]
    fn test_wake_one_skips_stale_waiter_before_live_waiter() {
        reset();
        register_online_cpu(crate::arch::get_cpu().get_cpuid());
        let waker = Waker::new_interruptible("stale-before-live");
        let task_id = add_task(Task::new("live-waiter".to_string(), 1, TaskType::Kernel), 0);
        let task = get_task_by_id(task_id).expect("live waiter must be registered");
        task.set_state(TaskState::Blocked(BlockedType::Interruptible));
        mark_blocked(task_id);
        waker
            .wait_queue
            .lock()
            .extend([pending_entry(usize::MAX), pending_entry(task_id)]);

        assert!(waker.wake_one());
        assert_eq!(task.get_state(), TaskState::Ready);
        assert_eq!(waker.waiting_count(), 0);
        drop(task);
        reset();
    }

    #[test_case]
    fn test_wake_one_skips_settled_registrations_before_live_waiter() {
        reset();
        register_online_cpu(crate::arch::get_cpu().get_cpuid());
        let waker = Waker::new_interruptible("timed-out-before-live");
        let task_id = add_task(Task::new("live-waiter".to_string(), 1, TaskType::Kernel), 0);
        let task = get_task_by_id(task_id).expect("live waiter must be registered");
        task.set_state(TaskState::Blocked(BlockedType::Interruptible));
        mark_blocked(task_id);

        let timed_out = Arc::new(WaitOutcome::new());
        assert!(timed_out.try_timeout());
        let interrupted = Arc::new(WaitOutcome::new());
        assert!(interrupted.finish_after_resume());
        waker.wait_queue.lock().extend([
            WaitQueueEntry::new(usize::MAX, timed_out),
            WaitQueueEntry::new(usize::MAX - 1, interrupted),
            pending_entry(task_id),
        ]);

        assert!(waker.wake_one());
        assert_eq!(task.get_state(), TaskState::Ready);
        assert_eq!(waker.waiting_count(), 0);
        drop(task);
        reset();
    }

    #[test_case]
    fn test_timeout_cleanup_removes_only_its_wait_registration() {
        let waker = Waker::new_interruptible("registration-identity");
        let old = Arc::new(WaitOutcome::new());
        let current = Arc::new(WaitOutcome::new());
        waker.wait_queue.lock().extend([
            WaitQueueEntry::new(7, old.clone()),
            WaitQueueEntry::new(7, current.clone()),
        ]);

        assert!(waker.remove_wait_registration(7, &old));
        let queue = waker.wait_queue.lock();
        assert_eq!(queue.len(), 1);
        assert!(queue[0].matches_registration(7, &current));
    }

    #[test_case]
    fn test_wake_all_latches_pending_wake_when_all_waiters_are_stale() {
        let waker = Waker::new_interruptible("all-stale");
        waker
            .wait_queue
            .lock()
            .extend([pending_entry(usize::MAX), pending_entry(usize::MAX - 1)]);

        assert_eq!(waker.wake_all(), 0);
        assert_eq!(waker.pending_wake_count_for_test(), 1);
    }

    #[test_case]
    fn test_timeout_already_fired_cancels_prepared_wait() {
        reset();
        let local_cpu = crate::arch::get_cpu().get_cpuid();
        register_online_cpu(local_cpu);
        let waker = Waker::new_interruptible("timeout-cancel");
        let task_id = add_task(
            Task::new("timeout-waiter".to_string(), 1, TaskType::Kernel),
            local_cpu,
        );
        let task = get_task_by_id(task_id).expect("timeout waiter must be registered");
        task.set_state(TaskState::Running);
        task.running_cpu.store(local_cpu, Ordering::SeqCst);
        set_current_task_for_test(local_cpu, Some(task_id));
        remove_from_ready_queues(task_id);

        assert!(waker.prepare_wait(task_id));
        assert_eq!(
            task.get_state(),
            TaskState::Blocked(BlockedType::Interruptible)
        );
        assert!(wake_task(task_id));
        assert_eq!(task.get_state(), TaskState::Ready);
        assert!(has_ready_tasks(local_cpu));

        assert!(waker.cancel_prepared_wait(task_id));
        assert_eq!(task.get_state(), TaskState::Running);
        assert_eq!(waker.waiting_count(), 0);
        assert!(!has_ready_tasks(local_cpu));
        assert!(!wake_task(task_id));
        set_current_task_for_test(local_cpu, None);
        task.running_cpu.store(usize::MAX, Ordering::SeqCst);
        drop(task);
        reset();
    }

    #[test_case]
    fn test_cancel_prepared_wait_rolls_back_locally_owned_blocked_task() {
        reset();
        let local_cpu = crate::arch::get_cpu().get_cpuid();
        register_online_cpu(local_cpu);
        let waker = Waker::new_interruptible("local-cancel");
        let task_id = add_task(
            Task::new("local-waiter".to_string(), 1, TaskType::Kernel),
            local_cpu,
        );
        let task = get_task_by_id(task_id).expect("local waiter must be registered");
        task.set_state(TaskState::Running);
        task.running_cpu.store(local_cpu, Ordering::SeqCst);
        set_current_task_for_test(local_cpu, Some(task_id));
        remove_from_ready_queues(task_id);

        assert!(waker.prepare_wait(task_id));
        assert!(waker.cancel_prepared_wait(task_id));
        assert_eq!(task.get_state(), TaskState::Running);
        assert_eq!(waker.waiting_count(), 0);

        set_current_task_for_test(local_cpu, None);
        task.running_cpu.store(usize::MAX, Ordering::SeqCst);
        drop(task);
        reset();
    }

    #[test_case]
    fn test_prepare_wait_repairs_locally_owned_ready_task() {
        reset();
        let local_cpu = crate::arch::get_cpu().get_cpuid();
        register_online_cpu(local_cpu);
        let waker = Waker::new_interruptible("ready-current");
        let task = Task::new("ready-current-waiter".to_string(), 1, TaskType::Kernel);
        task.init();
        let task_id = add_task(task, local_cpu);
        let task = get_task_by_id(task_id).expect("ready current task must be registered");
        task.running_cpu.store(local_cpu, Ordering::SeqCst);
        set_current_task_for_test(local_cpu, Some(task_id));

        assert_eq!(task.get_state(), TaskState::Ready);
        assert!(has_ready_tasks(local_cpu));
        assert!(waker.prepare_wait(task_id));
        assert_eq!(
            task.get_state(),
            TaskState::Blocked(BlockedType::Interruptible)
        );
        assert!(!has_ready_tasks(local_cpu));
        assert_eq!(waker.waiting_count(), 1);

        assert!(waker.cancel_prepared_wait(task_id));
        set_current_task_for_test(local_cpu, None);
        task.running_cpu.store(usize::MAX, Ordering::SeqCst);
        drop(task);
        reset();
    }

    #[test_case]
    fn test_cancel_prepared_wait_leaves_foreign_task_untouched() {
        reset();
        let waker = Waker::new_interruptible("foreign-cancel");
        let task_id = add_task(
            Task::new("foreign-waiter".to_string(), 1, TaskType::Kernel),
            0,
        );
        let task = get_task_by_id(task_id).expect("foreign waiter must be registered");
        task.set_state(TaskState::Blocked(BlockedType::Interruptible));
        mark_blocked(task_id);
        waker.wait_queue.lock().push_back(pending_entry(task_id));

        assert!(!waker.cancel_prepared_wait(task_id));
        assert_eq!(
            task.get_state(),
            TaskState::Blocked(BlockedType::Interruptible)
        );
        assert_eq!(waker.waiting_count(), 1);

        drop(task);
        reset();
    }

    #[test_case]
    fn test_debug_functionality() {
        let waker = Waker::new_interruptible("debug_test");

        // Test empty waker
        assert!(waker.is_empty());
        assert_eq!(waker.waiting_count(), 0);
        assert_eq!(waker.get_waiting_task_ids().len(), 0);
        assert!(!waker.is_task_waiting(42));

        // Test stats
        let stats = waker.get_stats();
        assert_eq!(stats.name, "debug_test");
        assert_eq!(stats.block_type, BlockedType::Interruptible);
        assert_eq!(stats.waiting_count, 0);
        assert!(stats.waiting_task_ids.is_empty());
    }

    #[test_case]
    fn test_debug_trait() {
        let waker = Waker::new_uninterruptible("debug_trait_test");

        // Verify Debug trait implementation exists and works
        let debug_string = alloc::format!("{:?}", waker);
        assert!(debug_string.contains("debug_trait_test"));
        assert!(debug_string.contains("Uninterruptible"));
        assert!(debug_string.contains("waiting_count: 0"));
    }

    #[test_case]
    fn test_clear_queue() {
        let waker = Waker::new_interruptible("clear_test");

        // Test clearing empty queue
        assert_eq!(waker.clear_queue(), 0);
        assert!(waker.is_empty());
    }

    #[test_case]
    fn test_waker_stats_debug() {
        let waker = Waker::new_interruptible("stats_test");
        let stats = waker.get_stats();

        // Test WakerStats Debug implementation
        let debug_string = alloc::format!("{:?}", stats);
        assert!(debug_string.contains("stats_test"));
        assert!(debug_string.contains("Interruptible"));
    }
}
