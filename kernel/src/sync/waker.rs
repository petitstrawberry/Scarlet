//! Waker - Synchronization primitive for task waiting and waking
//!
//! This module provides the `Waker` struct, which manages asynchronous task waiting
//! and waking mechanisms. It allows tasks to block on specific events and be woken
//! up when those events occur, such as I/O completion or interrupt handling.

extern crate alloc;

use alloc::collections::VecDeque;
use spin::Mutex;
use crate::task::{BlockedType, TaskState, mytask};
use crate::sched::scheduler::get_scheduler;

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
    wait_queue: Mutex<VecDeque<usize>>,
    /// The type of blocking this waker uses (interruptible or uninterruptible)
    block_type: BlockedType,
    /// Human-readable name for debugging purposes
    name: &'static str,
}

impl Waker {
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
            wait_queue: Mutex::new(VecDeque::new()),
            block_type: BlockedType::Interruptible,
            name,
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
            wait_queue: Mutex::new(VecDeque::new()),
            block_type: BlockedType::Uninterruptible,
            name,
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
    /// 2. Sets the task state to `Blocked(self.block_type)`
    /// 3. Adds the task ID to the wait queue
    /// 4. Calls the scheduler to yield CPU to other tasks
    /// 
    /// # Note
    /// 
    /// This function will block until the task is woken up. After waking,
    /// execution continues from this point.
    pub fn wait(&self) {
        if let Some(task) = mytask() {
            let task_id = task.get_id();
            
            // Add task to wait queue
            {
                let mut queue = self.wait_queue.lock();
                queue.push_back(task_id);
            }
            
            // Set task state to blocked
            task.set_state(TaskState::Blocked(self.block_type));
            
            // Yield CPU to scheduler
            // TODO: Call scheduler to switch to another task
            // This will be implemented in Phase 3 when scheduler integration is done
        }
    }

    /// Wake up one waiting task
    /// 
    /// This method removes one task from the wait queue and sets its state
    /// to `Ready`, making it eligible for scheduling again.
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
        let task_id = {
            let mut queue = self.wait_queue.lock();
            queue.pop_front()
        };

        if let Some(task_id) = task_id {
            // Get the task and set it to Ready state
            if let Some(task) = get_scheduler().get_task_by_id(task_id) {
                task.set_state(TaskState::Ready);
                return true;
            }
        }
        
        false
    }

    /// Wake up all waiting tasks
    /// 
    /// This method removes all tasks from the wait queue and sets their
    /// states to `Ready`, making them all eligible for scheduling again.
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

        let mut woken_count = 0;
        for task_id in task_ids {
            if let Some(task) = get_scheduler().get_task_by_id(task_id) {
                task.set_state(TaskState::Ready);
                woken_count += 1;
            }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_waker_creation() {
        let interruptible_waker = Waker::new_interruptible("test_int");
        assert_eq!(interruptible_waker.name(), "test_int");
        assert_eq!(interruptible_waker.block_type(), BlockedType::Interruptible);
        assert_eq!(interruptible_waker.waiting_count(), 0);

        let uninterruptible_waker = Waker::new_uninterruptible("test_unint");
        assert_eq!(uninterruptible_waker.name(), "test_unint");
        assert_eq!(uninterruptible_waker.block_type(), BlockedType::Uninterruptible);
        assert_eq!(uninterruptible_waker.waiting_count(), 0);
    }

    #[test_case]
    fn test_wake_empty_queue() {
        let waker = Waker::new_interruptible("empty_test");
        assert_eq!(waker.wake_one(), false);
        assert_eq!(waker.wake_all(), 0);
    }
}
