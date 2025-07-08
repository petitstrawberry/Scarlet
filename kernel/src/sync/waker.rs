//! Waker - Synchronization primitive for task waiting and waking
//!
//! This module provides the `Waker` struct, which manages asynchronous task waiting
//! and waking mechanisms. It allows tasks to block on specific events and be woken
//! up when those events occur, such as I/O completion or interrupt handling.

extern crate alloc;

use alloc::collections::VecDeque;
use spin::Mutex;
use core::fmt;
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
            
            // Add task to wait queue first
            {
                let mut queue = self.wait_queue.lock();
                queue.push_back(task_id);
            }
            
            // Set task state to blocked
            task.set_state(TaskState::Blocked(self.block_type));
            
            // Yield CPU to scheduler
            let cpu = crate::arch::get_cpu();
            get_scheduler().schedule(cpu);
        }
    }

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
        let task_id = {
            let mut queue = self.wait_queue.lock();
            queue.pop_front()
        };

        if let Some(task_id) = task_id {
            // Use the scheduler's wake_task method to move from blocked to ready queue
            get_scheduler().wake_task(task_id)
        } else {
            false
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
        let task_ids = {
            let mut queue = self.wait_queue.lock();
            let ids: VecDeque<usize> = queue.drain(..).collect();
            ids
        };

        let mut woken_count = 0;
        for task_id in task_ids {
            // Use the scheduler's wake_task method to move from blocked to ready queue
            if get_scheduler().wake_task(task_id) {
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
}

impl fmt::Debug for Waker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let waiting_tasks = self.wait_queue.lock();
        f.debug_struct("Waker")
            .field("name", &self.name)
            .field("block_type", &self.block_type)
            .field("waiting_count", &waiting_tasks.len())
            .field("waiting_task_ids", &*waiting_tasks)
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
