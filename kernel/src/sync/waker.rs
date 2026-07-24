//! Waker - Synchronization primitive for task waiting and waking
//!
//! This module provides the `Waker` struct, which manages asynchronous task waiting
//! and waking mechanisms. It allows tasks to block on specific events and be woken
//! up when those events occur, such as I/O completion or interrupt handling.

extern crate alloc;

use crate::arch::Trapframe;
use crate::sched::scheduler::{
    get_task_by_id, remove_from_ready_queues, schedule, unmark_blocked, wake_task, wake_task_on,
};
use crate::sync::IrqSpinLock;
use crate::task::{BlockedType, TaskState};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

const DIAGNOSTIC_WAKE_EVENT_WAITERS_ON_SOURCE_CPU: bool = false;

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
    /// Queue of waiting task IDs
    wait_queue: IrqSpinLock<VecDeque<usize>>,
    /// The type of blocking this waker uses (interruptible or uninterruptible)
    block_type: BlockedType,
    /// Human-readable name for debugging purposes
    name: &'static str,
    /// Pending wake count: incremented by wake_one()/wake_all() when the queue
    /// is empty (i.e. the wake arrived before the waiter enqueued itself).
    /// Consumed by wait() so the task does not sleep on an already-fired wake.
    pending_wakes: AtomicUsize,
}

impl Waker {
    fn wake_waiting_task(&self, task_id: usize) -> bool {
        if DIAGNOSTIC_WAKE_EVENT_WAITERS_ON_SOURCE_CPU && self.name.starts_with("event_") {
            wake_task_on(task_id, crate::arch::get_cpu().get_cpuid())
        } else {
            wake_task(task_id)
        }
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
        if self.prepare_wait(task_id) {
            schedule(trapframe);
        }
    }

    /// Block using an owned waker handle without retaining it on a suspended stack.
    ///
    /// # Arguments
    ///
    /// * `task_id` - ID of the current task to block
    /// * `trapframe` - Current task's saved execution state
    pub fn wait_owned(self: Arc<Self>, task_id: usize, trapframe: &mut Trapframe) {
        let should_schedule = self.prepare_wait(task_id);
        drop(self);
        if should_schedule {
            schedule(trapframe);
        }
    }

    fn prepare_wait(&self, task_id: usize) -> bool {
        // Consume a pending wake that arrived before we enqueued ourselves.
        // This closes the lost-wake window on SMP: if wake_one()/wake_all()
        // fired while the queue was empty (between the caller's condition check
        // and this wait() call), we return immediately instead of sleeping.
        if self
            .pending_wakes
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                if n > 0 { Some(n - 1) } else { None }
            })
            .is_ok()
        {
            return false;
        }

        let task = get_task_by_id(task_id)
            .unwrap_or_else(|| panic!("[WAKER] Task ID {} not found in scheduler", task_id));
        let blocked_state = TaskState::Blocked(self.block_type);
        if let Err(actual) = task.state.compare_exchange(
            TaskState::Running,
            blocked_state,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            drop(task);
            match actual {
                TaskState::Zombie | TaskState::Terminated | TaskState::Ready => {
                    return true;
                }
                TaskState::NotInitialized | TaskState::Running | TaskState::Blocked(_) => {
                    panic!(
                        "[WAKER] Task {} cannot enter wait from state {:?}",
                        task_id, actual
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
        let late_wake = {
            let mut queue = self.wait_queue.lock();
            queue.push_back(task_id);

            self.pending_wakes
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    if n > 0 { Some(n - 1) } else { None }
                })
                .is_ok()
        };

        let terminated_while_enqueuing = get_task_by_id(task_id).is_some_and(|task| {
            matches!(task.get_state(), TaskState::Zombie | TaskState::Terminated)
        });
        if terminated_while_enqueuing {
            let _cancelled = self.cancel_prepared_wait(task_id);
            return true;
        }

        if late_wake {
            // If ownership was lost, leave the ready task untouched and let the
            // scheduler observe it rather than consuming a wake on another CPU.
            return !self.cancel_prepared_wait(task_id);
        }

        true
    }

    /// Roll back a prepared wait only while this CPU still owns the task.
    ///
    /// # Returns
    ///
    /// `true` if queue bookkeeping was repaired, or `false` if the task is
    /// missing or is no longer the current locally-owned task.
    fn cancel_prepared_wait(&self, task_id: usize) -> bool {
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
            queue.retain(|&id| id != task_id);
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
        timeout_ticks: Option<u64>,
    ) -> bool {
        if matches!(timeout_ticks, Some(0)) {
            return false;
        }

        if let Some(ticks) = timeout_ticks {
            use crate::timer::{TimerHandler, add_timer, cancel_timer, get_tick};
            use alloc::sync::Arc;
            use core::sync::atomic::{AtomicBool, Ordering};

            struct TimeoutWake {
                task_id: usize,
                timed_out: AtomicBool,
            }

            impl TimerHandler for TimeoutWake {
                fn on_timer_expired(self: Arc<Self>, _context: usize) {
                    self.timed_out.store(true, Ordering::SeqCst);
                    let _ = wake_task(self.task_id);
                }
            }

            let handler: Arc<TimeoutWake> = Arc::new(TimeoutWake {
                task_id,
                timed_out: AtomicBool::new(false),
            });
            let handler_ref: Arc<dyn TimerHandler> = handler.clone();
            let id = add_timer(get_tick().saturating_add(ticks), &handler_ref, 0);

            let should_schedule = self.prepare_wait(task_id);
            if handler.timed_out.load(Ordering::SeqCst) {
                let _cancelled = self.cancel_prepared_wait(task_id);
                cancel_timer(id);
                return false;
            }
            if should_schedule {
                schedule(trapframe);
            }

            cancel_timer(id);

            let timed_out = handler.timed_out.load(Ordering::SeqCst);
            if timed_out {
                let _cancelled = self.cancel_prepared_wait(task_id);
            }

            !timed_out
        } else {
            self.wait(task_id, trapframe);
            true
        }
    }

    /// Block the task until woken, but wait at least `min_wait_ticks` before
    /// returning even if woken early by the selectable waker. After the minimum
    /// wait elapses, continues blocking until woken or `timeout_ticks` expires.
    ///
    /// Returns `true` if woken by an event, `false` if the overall timeout elapsed.
    pub fn wait_with_min_timeout(
        &self,
        task_id: usize,
        trapframe: &mut Trapframe,
        timeout_ticks: Option<u64>,
        min_wait_ticks: u64,
    ) -> bool {
        use crate::timer::{TimerHandler, add_timer, cancel_timer, get_tick};
        use alloc::sync::Arc;
        use core::sync::atomic::{AtomicBool, Ordering};

        struct MinTimeoutWake {
            task_id: usize,
            fired: AtomicBool,
        }

        impl TimerHandler for MinTimeoutWake {
            fn on_timer_expired(self: Arc<Self>, _context: usize) {
                self.fired.store(true, Ordering::SeqCst);
                let _ = wake_task(self.task_id);
            }
        }

        let min_handler: Arc<MinTimeoutWake> = Arc::new(MinTimeoutWake {
            task_id,
            fired: AtomicBool::new(false),
        });
        let min_handler_ref: Arc<dyn TimerHandler> = min_handler.clone();
        let min_timer_id = add_timer(
            get_tick().saturating_add(min_wait_ticks),
            &min_handler_ref,
            0,
        );

        let should_schedule = self.prepare_wait(task_id);
        if min_handler.fired.load(Ordering::SeqCst) {
            let _cancelled = self.cancel_prepared_wait(task_id);
        } else if should_schedule {
            schedule(trapframe);
        }

        while !min_handler.fired.load(Ordering::SeqCst) {
            let should_schedule = self.prepare_wait(task_id);
            if min_handler.fired.load(Ordering::SeqCst) {
                let _cancelled = self.cancel_prepared_wait(task_id);
                break;
            }
            if should_schedule {
                schedule(trapframe);
            }
        }

        cancel_timer(min_timer_id);

        if let Some(ticks) = timeout_ticks {
            if ticks > min_wait_ticks {
                let remaining = ticks - min_wait_ticks;
                self.wait_with_timeout(task_id, trapframe, Some(remaining))
            } else {
                true
            }
        } else {
            self.wait_with_timeout(task_id, trapframe, None)
        }
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
        while let Some(task_id) = {
            let mut queue = self.wait_queue.lock();
            queue.pop_front()
        } {
            let woke = self.wake_waiting_task(task_id);
            if crate::sched::scheduler::DEBUG_SMP_TASK_FLOW {
                crate::println!(
                    "[SMPDBG waker-wake-one] waker={} cpu={} task={} woke={}",
                    self.name,
                    crate::arch::get_cpu().get_cpuid(),
                    task_id,
                    woke,
                );
            }
            if woke {
                return true;
            }
        }

        self.pending_wakes.fetch_add(1, Ordering::SeqCst);
        false
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
        let task_ids = {
            let mut queue = self.wait_queue.lock();
            let ids: VecDeque<usize> = queue.drain(..).collect();
            ids
        };

        if task_ids.is_empty() {
            self.pending_wakes.fetch_add(1, Ordering::SeqCst);
            return 0;
        }

        let mut woken_count = 0;
        for task_id in task_ids {
            let woke = self.wake_waiting_task(task_id);
            if woke {
                woken_count += 1;
            }
            if crate::sched::scheduler::DEBUG_SMP_TASK_FLOW {
                crate::println!(
                    "[SMPDBG waker-wake-all] waker={} cpu={} task={} woke={}",
                    self.name,
                    crate::arch::get_cpu().get_cpuid(),
                    task_id,
                    woke,
                );
            }
        }

        if woken_count == 0 {
            self.pending_wakes.fetch_add(1, Ordering::SeqCst);
        }

        woken_count
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
        self.wait_queue.lock().clone()
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
        self.wait_queue.lock().contains(&task_id)
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
            waiting_task_ids: waiting_tasks.clone(),
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
        f.debug_struct("Waker")
            .field("name", &self.name)
            .field("block_type", &self.block_type)
            .field("waiting_count", &waiting_tasks.len())
            .field("waiting_task_ids", &*waiting_tasks)
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
    /// List of task IDs currently waiting
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
    fn test_wake_empty_queue() {
        let waker = Waker::new_interruptible("empty_test");
        assert_eq!(waker.wake_one(), false);
        assert_eq!(waker.wake_all(), 0);
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
        waker.wait_queue.lock().extend([usize::MAX, task_id]);

        assert!(waker.wake_one());
        assert_eq!(task.get_state(), TaskState::Ready);
        assert_eq!(waker.waiting_count(), 0);
        drop(task);
        reset();
    }

    #[test_case]
    fn test_wake_all_latches_pending_wake_when_all_waiters_are_stale() {
        let waker = Waker::new_interruptible("all-stale");
        waker.wait_queue.lock().extend([usize::MAX, usize::MAX - 1]);

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
        waker.wait_queue.lock().push_back(task_id);

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
