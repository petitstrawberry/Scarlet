//! Scheduler module
//!
//! The scheduler module is responsible for scheduling tasks on the CPU.
//! Currently, the scheduler is a simple round-robin scheduler with separate
//! queues for different task states to improve efficiency:
//!
//! - `ready_queue`: Tasks that are ready to run
//! - `blocked_queue`: Tasks waiting for I/O or other events
//! - `zombie_queue`: Finished tasks waiting to be cleaned up
//!
//! This separation avoids unnecessary iteration over blocked/zombie tasks
//! during normal scheduling operations.
//!
//! # TaskPool Safety
//!
//! The global `TaskPool` stores tasks in a fixed-size array indexed by task_id.
//! This design avoids HashMap-related issues and provides stable memory locations:
//!
//! - **Fixed Array**: `tasks[task_id]` ensures stable addresses (no reallocation)
//! - **Direct Indexing**: task_id == index for O(1) access without hash lookup
//! - **ID Recycling**: Free list reuses task IDs to avoid exhaustion
//!
//! The pool provides `get_task()` and `get_task_mut()` which return `&'static`
//! references using raw pointers. This is **unsafe but practical** because:
//!
//! 1. Tasks are stored at fixed addresses (task_id == index)
//! 2. The scheduler never removes running tasks
//! 3. Single-core execution prevents concurrent access
//! 4. Context switches never invalidate the current task's reference
//!
//! **IMPORTANT**: Never access `TaskPool::tasks` directly. Always use the
//! provided methods which document and enforce safety invariants.

extern crate alloc;

use core::panic;
use core::sync::OnceLock;

use alloc::{boxed::Box, collections::vec_deque::VecDeque, string::ToString, vec::Vec};

use crate::print;
use crate::println;
use crate::{
    arch::{
        Arch, Trapframe, get_cpu, get_user_trap_handler, instruction::idle,
        interrupt::enable_external_interrupts, set_next_mode, set_trapvector,
        trap::user::arch_switch_to_user_space,
    },
    environment::NUM_OF_CPUS,
    task::{TaskState, new_kernel_task, wake_parent_waiters, wake_task_waiters},
    timer::get_kernel_timer,
    vm::get_trampoline_trap_vector,
};

use crate::task::Task;

/// Task pool that stores tasks in fixed positions
/// With each Task being 824 bytes, 1024 tasks consume approximately 824 KiB of memory,
/// which is very reasonable for general-purpose systems.
/// TODO: Refactor Task struct to use fine-grained Mutex on individual fields
///       (e.g., state: Mutex<TaskState>, time_slice: Mutex<usize>) and change
///       TaskPool to use Arc<Task> for safe sharing across threads/contexts.
///       This would also eliminate the fixed-size limitation.
const MAX_TASKS: usize = 1024;

/// Global task pool storing all tasks
/// Using spin::Once with Box-ed tasks array to avoid large stack usage.
static TASK_POOL: spin::Once<TaskPool> = spin::Once::new();

/// Get the global task pool (lazy initialization on first call)
pub fn get_task_pool() -> &'static TaskPool {
    TASK_POOL.call_once(|| TaskPool::new())
}

/// Global task pool storing all tasks in a Box-ed fixed-size array
///
/// # Safety
///
/// This struct provides unsafe access to tasks through `get_task()` and `get_task_mut()`
/// which return `&'static` references without holding locks. This is safe because:
///
/// 1. **Stable Box Memory**: Tasks are stored in `Box<[Option<Task>; MAX_TASKS]>`.
///    Box guarantees the underlying array pointer **never moves** after allocation,
///    making `&'static` references safe in practice.
///
/// 2. **Direct Indexing**: `task_id == index` provides O(1) access without HashMap
///    overhead. No rehashing or reallocation can occur.
///
/// 3. **Scheduler Control**: The scheduler controls all task removal and ensures
///    that the currently running task is never removed during context switches.
///
/// 4. **Single-Core Execution**: Current implementation is single-core, preventing
///    concurrent access during context switches.
///
/// **IMPORTANT**: Do NOT directly access the `tasks` array. Always use:
/// - `TaskPool::get_task()` for immutable references
/// - `TaskPool::get_task_mut()` for mutable references
/// - `Scheduler::get_task_by_id()` which is the preferred public API
///
/// Direct array access could violate safety assumptions and cause undefined behavior.
///
/// # Memory Layout
///
/// The tasks array is allocated directly on heap via Vec→Box conversion:
/// - No intermediate stack allocation (824KB never touches stack)
/// - Box<[T]> provides stable pointer for &'static references
/// - Array size is fixed at compile time (MAX_TASKS = 1024)
pub struct TaskPool {
    // Box-ed fixed-length array allocated on heap
    // Pointer is stable for the lifetime of the program
    //
    // ⚠️ DO NOT ACCESS DIRECTLY - Use get_task() or get_task_mut() methods
    tasks: spin::Mutex<Box<[Option<Task>; MAX_TASKS]>>,

    // Free list of recyclable task IDs
    // IDs are added here when tasks are removed
    free_ids: spin::Mutex<VecDeque<usize>>,

    // Next ID to allocate when free list is empty
    // Atomic is sufficient for lock-free allocation
    next_id: core::sync::atomic::AtomicUsize,
}

impl TaskPool {
    fn new() -> Self {
        crate::early_println!("[SCHED] TaskPool::new() starting...");

        // Allocate uninitialized Box array directly on heap
        // No stack usage, no Vec overhead
        let mut tasks: Box<[core::mem::MaybeUninit<Option<Task>>; MAX_TASKS]> =
            unsafe { Box::new_uninit().assume_init() };

        // Initialize all elements to None
        for i in 0..MAX_TASKS {
            tasks[i].write(None);
        }

        // Convert to initialized Box<[Option<Task>]>
        // SAFETY: All elements have been initialized with None
        let tasks: Box<[Option<Task>; MAX_TASKS]> = unsafe { core::mem::transmute(tasks) };

        crate::early_println!("[SCHED] TaskPool created (heap allocation, stable pointers)");

        TaskPool {
            tasks: spin::Mutex::new(tasks),
            free_ids: spin::Mutex::new(VecDeque::new()),
            next_id: core::sync::atomic::AtomicUsize::new(1), // Start from 1, ID 0 is invalid
        }
    }

    /// Allocate a new task ID
    /// Tries to reuse freed IDs first, then allocates new ones sequentially
    /// Uses atomic operations for lock-free allocation
    pub fn allocate_id(&self) -> Option<usize> {
        // Try to reuse freed IDs first
        {
            let mut free_ids = self.free_ids.lock();
            if let Some(id) = free_ids.pop_front() {
                return Some(id);
            }
        }

        // Allocate new ID using atomic fetch_add
        let id = self
            .next_id
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if id >= MAX_TASKS {
            // Rollback on overflow
            self.next_id
                .fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
            None
        } else {
            Some(id)
        }
    }

    /// Add a task to the pool
    /// Allocates an ID, sets it on the task, and returns the ID
    fn add_task(&self, mut task: Task) -> Result<usize, &'static str> {
        // Allocate ID for this task
        let task_id = self.allocate_id().ok_or("Task pool exhausted")?;

        // Add to the pool at the allocated index BEFORE registering namespace mapping
        if task_id >= MAX_TASKS {
            return Err("Task ID out of bounds");
        }

        let mut tasks = self.tasks.lock();

        if tasks[task_id].is_some() {
            return Err("Task ID slot already occupied");
        }

        // Allocate namespace ID AFTER checking slot availability
        let namespace_id = task.get_namespace().allocate_task_id_for(task_id);

        // Set IDs on the task
        task.set_id(task_id);
        task.set_namespace_id(namespace_id);

        tasks[task_id] = Some(task);
        Ok(task_id)
    }

    /// Get a task reference by ID
    /// Returns a static reference using raw pointer for lifetime extension
    ///
    /// # Safety
    ///
    /// This function is safe to use under the following conditions:
    /// - The task must not be removed while the returned reference is in use
    /// - In context switching scenarios, the currently running task is never removed
    /// - Single-core execution ensures no concurrent removal during context switch
    ///
    /// The returned reference points to a fixed location in the TaskPool's
    /// Box-ed array (task_id == index), so the address is **stable**:
    /// - Box guarantees the underlying array never moves
    /// - Unlike Vec or HashMap, no reallocation can occur
    /// - The pointer remains valid for the lifetime of the program
    ///
    /// **Important**: Do NOT directly access `TaskPool::tasks` array.
    /// Always use this method or `get_task_mut()` to ensure proper safety.
    pub fn get_task(task_id: usize) -> Option<&'static Task> {
        if task_id >= MAX_TASKS {
            return None;
        }

        let pool = get_task_pool();
        let tasks = pool.tasks.lock();

        // SAFETY: The Box<[T]> ensures the array pointer is stable.
        // Once a Box is allocated, its underlying pointer never changes.
        // Combined with scheduler guarantees (no removal of running task),
        // this provides a de-facto &'static reference.
        tasks[task_id].as_ref().map(|task| {
            let ptr = task as *const Task;
            unsafe { &*ptr }
        })
    }

    /// Get a mutable task reference by ID
    /// Returns a static mutable reference using raw pointer for lifetime extension
    ///
    /// # Safety
    ///
    /// This function is safe to use under the following conditions:
    /// - The task must not be removed while the returned reference is in use
    /// - In context switching scenarios, the currently running task is never removed
    /// - Single-core execution ensures no concurrent access during context switch
    ///
    /// The returned reference points to a fixed location in the TaskPool's
    /// Box-ed array (task_id == index), so the address is **stable**:
    /// - Box guarantees the underlying array never moves
    /// - Unlike Vec or HashMap, no reallocation can occur
    /// - The pointer remains valid for the lifetime of the program
    ///
    /// **Important**: Do NOT directly access `TaskPool::tasks` array.
    /// Always use this method or `get_task()` to ensure proper safety.
    ///
    /// # Note
    /// This is technically UB in Rust (returning &'static mut without holding lock),
    /// but safe in practice because:
    /// - Box<[T]> provides stable memory location (pointer never changes)
    /// - The scheduler ensures exclusive access during context switches
    /// - Single-core execution prevents concurrent mutable access
    /// - Currently running task is never removed
    pub fn get_task_mut(task_id: usize) -> Option<&'static mut Task> {
        if task_id >= MAX_TASKS {
            return None;
        }

        let pool = get_task_pool();
        let mut tasks = pool.tasks.lock();

        // SAFETY: The Box<[T]> ensures the array pointer is stable.
        // Once a Box is allocated, its underlying pointer never changes.
        // Combined with scheduler guarantees (no removal of running task),
        // this provides a de-facto &'static mut reference.
        tasks[task_id].as_mut().map(|task| {
            let ptr = task as *mut Task;
            unsafe { &mut *ptr }
        })
    }

    /// Remove a task from the pool
    ///
    /// # Safety
    ///
    /// **CRITICAL**: This method invalidates all `&'static` references returned by
    /// `get_task()` and `get_task_mut()` for this task_id. The scheduler must ensure:
    ///
    /// 1. The task being removed is NOT currently running on any CPU
    /// 2. No context switch is in progress for this task
    /// 3. No references to this task are held elsewhere
    ///
    /// The scheduler enforces this by:
    /// - Only removing tasks from zombie_queue (already exited)
    /// - Never removing the currently running task
    /// - Ensuring the task is not in ready/blocked queues before removal
    fn remove_task(&self, task_id: usize) -> Option<Task> {
        if task_id >= MAX_TASKS {
            return None;
        }

        let mut tasks = self.tasks.lock();
        let task = tasks[task_id].take()?;

        // Add ID to free list for reuse
        let mut free_ids = self.free_ids.lock();
        free_ids.push_back(task_id);

        Some(task)
    }

    #[allow(dead_code)]
    fn contains_task(&self, task_id: usize) -> bool {
        if task_id >= MAX_TASKS {
            return false;
        }

        let tasks = self.tasks.lock();
        tasks[task_id].is_some()
    }

    /// Reset the task pool to initial state (test-only)
    ///
    /// Clears all tasks, resets ID allocation, and clears free list.
    /// This should ONLY be called in tests to clean up state between test cases.
    ///
    /// # Safety
    ///
    /// This function INVALIDATES all existing `&'static` references to tasks.
    /// It must ONLY be called when:
    /// - No tasks are currently running
    /// - No task references are held elsewhere
    /// - Called from test code only
    #[cfg(test)]
    pub fn reset(&self) {
        // Clear all task slots
        let mut tasks = self.tasks.lock();
        for i in 0..MAX_TASKS {
            tasks[i] = None;
        }
        drop(tasks);

        // Reset ID allocation to start from 1
        self.next_id.store(1, core::sync::atomic::Ordering::Relaxed);

        // Clear free list
        self.free_ids.lock().clear();
    }
}

static SCHEDULER: OnceLock<Scheduler> = OnceLock::new();

pub fn get_scheduler() -> &'static Scheduler {
    SCHEDULER.get_or_init(|| Scheduler::new())
}

pub struct Scheduler {
    /// Queue for ready-to-run task IDs (per-CPU, lock-protected)
    ready_queue: [RwLock<VecDeque<usize>>; NUM_OF_CPUS],
    /// Queue for blocked task IDs (waiting for I/O, etc.)
    blocked_queue: [RwLock<VecDeque<usize>>; NUM_OF_CPUS],
    /// Queue for zombie task IDs (finished but not yet cleaned up)
    zombie_queue: [RwLock<VecDeque<usize>>; NUM_OF_CPUS],
    current_task_id: [RwLock<Option<usize>>; NUM_OF_CPUS],
}

impl Scheduler {
    pub fn new() -> Self {
        // Initialize arrays with const blocks
        let mut scheduler = Scheduler {
            ready_queue: [const { RwLock::new(VecDeque::new()) }; NUM_OF_CPUS],
            blocked_queue: [const { RwLock::new(VecDeque::new()) }; NUM_OF_CPUS],
            zombie_queue: [const { RwLock::new(VecDeque::new()) }; NUM_OF_CPUS],
            current_task_id: [const { RwLock::new(None) }; NUM_OF_CPUS],
        };

        // Initialize each queue element explicitly
        for i in 0..NUM_OF_CPUS {
            scheduler.ready_queue[i] = RwLock::new(VecDeque::new());
            scheduler.blocked_queue[i] = RwLock::new(VecDeque::new());
            scheduler.zombie_queue[i] = RwLock::new(VecDeque::new());
            scheduler.current_task_id[i] = RwLock::new(None);
        }

        scheduler
    }

    pub fn add_task(&self, task: Task, cpu_id: usize) -> usize {
        // Add task to the global task pool and get the allocated ID
        let task_id = match get_task_pool().add_task(task) {
            Ok(id) => id,
            Err(e) => panic!("Failed to add task: {}", e),
        };
        // Add task state info to ready queue
        self.ready_queue[cpu_id].write().push_back(task_id);
        task_id
    }

    /// Determines the next task to run and returns current and next task IDs
    ///
    /// This method performs the core scheduling algorithm and task state management
    /// without performing actual context switches or hardware setup.
    ///
    /// # Arguments
    /// * `cpu` - The CPU architecture state (for CPU ID)
    ///
    /// # Returns
    /// * `(old_task_id, new_task_id)` - Tuple of old and new task IDs
    fn run(&self, cpu: &Arch) -> (Option<usize>, Option<usize>) {
        let cpu_id = cpu.get_cpuid();

        // Get current task ID and release lock immediately
        let old_current_task_id = *self.current_task_id[cpu_id].read();

        // IMPORTANT: If there's a current running task, re-queue it BEFORE scheduling
        // This ensures it's available as a fallback if no other tasks are ready
        if let Some(current_id) = old_current_task_id {
            // Check if current task is still in ready_queue (it shouldn't be if it's running)
            let already_in_queue = self.ready_queue[cpu_id].read().iter().any(|&id| id == current_id);

            if !already_in_queue {
                // Current task is not in ready_queue (it's running), add it back
                // Only add if the task is in a valid state to be scheduled
                if let Some(task) = self.get_task_by_id(current_id) {
                    match task.state {
                        TaskState::Ready | TaskState::Running => {
                            self.ready_queue[cpu_id].write().push_back(current_id);
                        }
                        _ => {
                            // Task is in Zombie, Terminated, Blocked, or NotInitialized state
                            // Don't re-queue it
                        }
                    }
                }
            }
        }

        // Try to get a task from local queue first
        let mut task_id = self.ready_queue[cpu_id].write().pop_front();

        // If no local tasks, try to steal from other CPUs (work stealing)
        if task_id.is_none() {
            task_id = self.steal_task(cpu_id);
        }

        // Process the selected task
        loop {
            match task_id {
                Some(task_id) => {
                    let t = self
                        .get_task_by_id(task_id)
                        .expect("Task must exist in task pool");

                    match t.state {
                        TaskState::NotInitialized => {
                            panic!("Task must be initialized before scheduling");
                        }
                        TaskState::Zombie => {
                            let task_id = t.get_id();
                            let parent_id = t.get_parent_id();
                            self.zombie_queue[cpu_id].write().push_back(task_id);
                            *self.current_task_id[cpu_id].write() = None;
                            // Wake up any processes waiting for this specific task
                            wake_task_waiters(task_id);
                            // Also wake up parent process for waitpid(-1)
                            if let Some(parent_id) = parent_id {
                                wake_parent_waiters(parent_id);
                            }
                            // Try to get next task
                            task_id = self.ready_queue[cpu_id].write().pop_front();
                            if task_id.is_none() {
                                task_id = self.steal_task(cpu_id);
                            }
                            continue;
                        }
                        TaskState::Terminated => {
                            get_task_pool().remove_task(task_id);
                            // Try to get next task
                            task_id = self.ready_queue[cpu_id].write().pop_front();
                            if task_id.is_none() {
                                task_id = self.steal_task(cpu_id);
                            }
                            continue;
                        }
                        TaskState::Blocked(_) => {
                            // Reset current_task_id since this task is no longer current
                            let mut current = self.current_task_id[cpu_id].write();
                            if *current == Some(task_id) {
                                *current = None;
                            }
                            drop(current);

                            // Put blocked task to blocked queue without running it
                            self.blocked_queue[cpu_id].write().push_back(task_id);

                            // Try to get next task
                            task_id = self.ready_queue[cpu_id].write().pop_front();
                            if task_id.is_none() {
                                task_id = self.steal_task(cpu_id);
                            }
                            continue;
                        }
                        TaskState::Ready | TaskState::Running => {
                            t.state = TaskState::Running;
                            // Task is ready to run
                            t.time_slice = 1; // Reset time slice on dispatch
                            let next_task_id = t.get_id();

                            // Update current task and add back to ready queue
                            *self.current_task_id[cpu_id].write() = Some(next_task_id);
                            self.ready_queue[cpu_id].write().push_back(task_id);

                            return (old_current_task_id, Some(next_task_id));
                        }
                    }
                }
                None => {
                    // No tasks found anywhere
                    return (old_current_task_id, old_current_task_id);
                }
            }
        }
    }

    /// Called every timer tick. Decrements the current task's time_slice.
    /// If time_slice reaches 0, triggers a reschedule.
    pub fn on_tick(&self, cpu_id: usize, trapframe: &mut Trapframe) {
        // crate::early_println!("[SCHED] CPU{}: on_tick called", cpu_id);
        if let Some(task_id) = self.get_current_task_id(cpu_id) {
            if let Some(task) = TaskPool::get_task_mut(task_id) {
                if task.time_slice > 0 {
                    task.time_slice -= 1;
                }
                if task.time_slice == 0 {
                    // crate::early_println!(
                    //     "[SCHED] CPU{}: Time slice expired for Task {}",
                    //     cpu_id,
                    //     task_id
                    // );
                    // Time slice expired, trigger reschedule
                    self.schedule(trapframe);
                }
            }
        } else {
            self.schedule(trapframe);
        }
    }

    /// Schedule tasks on the CPU with kernel context switching
    ///
    /// This function performs cooperative scheduling by switching between task
    /// kernel contexts. It returns to the caller, allowing the trap handler
    /// to handle user space return.
    ///
    /// CRITICAL: All locks MUST be released before calling kernel_context_switch
    /// to prevent deadlock during context switches.
    ///
    /// # Arguments
    /// * `trapframe` - The trapframe to use for scheduling
    pub fn schedule(&self, trapframe: &mut Trapframe) {
        let cpu = get_cpu();
        let cpu_id = cpu.get_cpuid();

        // Step 1: Run scheduling algorithm to get current and next task IDs
        // Locks are acquired and released inside run()
        let (current_task_id, next_task_id) = self.run(cpu);

        // Debug output for monitoring scheduler behavior
        // if let Some(current_id) = current_task_id {
        //     if let Some(next_id) = next_task_id {
        //         if current_id != next_id {
        //             crate::println!("[SCHED] CPU{}: Task {} -> Task {}", cpu_id, current_id, next_id);
        //         }
        //     } else {
        //         crate::println!("[SCHED] CPU{}: Task {} -> idle", cpu_id, current_id);
        //     }
        // } else if let Some(next_id) = next_task_id {
        //     crate::println!("[SCHED] CPU{}: idle -> Task {}", cpu_id, next_id);
        // }

        // Step 2: Check if a context switch is needed
        // All locks from run() are already released at this point
        if next_task_id.is_some() && current_task_id != next_task_id {
            let next_task_id = next_task_id.expect("Next task ID should be valid");

            // Store current task's user state to VCPU
            if let Some(current_task_id) = current_task_id {
                let current_task = self.get_task_by_id(current_task_id).unwrap();
                current_task.vcpu.store(trapframe);

                // Perform kernel context switch
                // CRITICAL: No locks held at this point
                self.kernel_context_switch(cpu_id, current_task_id, next_task_id);
                // NOTE: After this point, the current task will not execute until it is scheduled again

                // Restore trapframe of same task
                let current_task = self.get_task_by_id(current_task_id).unwrap();
                Self::setup_task_execution(get_cpu(), current_task);
            } else {
                // No current task (e.g., first scheduling), just switch to next task
                let next_task = self.get_task_by_id(next_task_id).unwrap();
                // crate::println!("[SCHED] Setting up task {} for execution", next_task_id);
                Self::setup_task_execution(get_cpu(), next_task);
                arch_switch_to_user_space(next_task.get_trapframe()); // Force switch to user space
            }
        }

        // Step 3: Setup task execution and process events (after context switch)
        if let Some(current_task) = self.get_current_task(cpu_id) {
            // Process pending events before dispatching task
            let _ = current_task.process_pending_events();
        }
        // Schedule returns - trap handler will call arch_switch_to_user_space()
    }

    /// Start the scheduler and return the first runnable task ID (if any).
    ///
    /// This function intentionally avoids performing the initial user-mode transition.
    /// The very first switch is architecture-specific and should be performed by
    /// `crate::arch::first_switch_to_user()` from the boot path.
    pub fn start_scheduler(&self) -> Option<usize> {
        let cpu = get_cpu();
        let cpu_id = cpu.get_cpuid();
        let timer = get_kernel_timer();
        timer.stop(cpu_id);

        // Program the periodic timer, but do not force/require the first switch via IRQ.
        timer.set_interval_us(cpu_id, crate::timer::TICK_INTERVAL_US);
        timer.start(cpu_id);

        let (_current_task_id, next_task_id) = self.run(cpu);
        next_task_id
    }

    pub fn get_current_task(&self, cpu_id: usize) -> Option<&mut Task> {
        let task_id = *self.current_task_id[cpu_id].read();
        match task_id {
            Some(id) => TaskPool::get_task_mut(id),
            None => None,
        }
    }

    pub fn get_current_task_id(&self, cpu_id: usize) -> Option<usize> {
        *self.current_task_id[cpu_id].read()
    }

    /// Attempt to steal a task from another CPU's ready queue
    ///
    /// This implements work stealing for load balancing across CPUs.
    /// Searches other CPUs' queues in round-robin order and takes half
    /// of their tasks if found.
    ///
    /// # Arguments
    /// * `cpu_id` - The CPU ID that is trying to steal work
    ///
    /// # Returns
    /// * Some(task_id) if a task was stolen, None otherwise
    fn steal_task(&self, cpu_id: usize) -> Option<usize> {
        for i in 1..NUM_OF_CPUS {
            let target_cpu = (cpu_id + i) % NUM_OF_CPUS;

            // Try to steal from target CPU
            let mut target_queue = self.ready_queue[target_cpu].write();

            if target_queue.len() > 1 {
                // Steal half the tasks (rounded down) to balance load
                let steal_count = target_queue.len() / 2;

                // Take the first task
                let stolen_task = target_queue.pop_front();

                // Move additional tasks to our queue
                if steal_count > 1 {
                    drop(target_queue); // Release lock before acquiring our queue lock
                    let mut our_queue = self.ready_queue[cpu_id].write();

                    // Re-acquire target queue lock to transfer remaining tasks
                    let mut target_queue = self.ready_queue[target_cpu].write();

                    for _ in 1..steal_count {
                        if let Some(task) = target_queue.pop_front() {
                            our_queue.push_back(task);
                        }
                    }
                }

                return stolen_task;
            }
        }

        None
    }

    /// Returns a mutable reference to the task with the specified ID, if found.
    ///
    /// This method searches the TaskPool to find the task with the specified ID.
    /// This is needed for Waker integration.
    ///
    /// # Arguments
    /// * `task_id` - The ID of the task to search for.
    ///
    /// # Returns
    /// A mutable reference to the task if found, or None otherwise.
    pub fn get_task_by_id(&self, task_id: usize) -> Option<&mut Task> {
        TaskPool::get_task_mut(task_id)
    }

    /// Move a task from blocked queue to ready queue when it's woken up
    ///
    /// This method is called by Waker when a blocked task needs to be woken up.
    ///
    /// # Arguments
    /// * `task_id` - The ID of the task to move to ready queue
    ///
    /// # Returns
    /// true if the task was found and moved, false otherwise
    pub fn wake_task(&self, task_id: usize) -> bool {
        // Search for the task in blocked queues
        for cpu_id in 0..NUM_OF_CPUS {
            let mut blocked_queue = self.blocked_queue[cpu_id].write();

            if let Some(pos) = blocked_queue.iter().position(|&id| id == task_id) {
                // Remove from blocked queue
                blocked_queue.remove(pos);
                drop(blocked_queue);

                // Get task from TaskPool and set state to Running
                if let Some(task) = TaskPool::get_task_mut(task_id) {
                    task.state = TaskState::Running;
                    // Memory barrier to ensure state change is visible
                    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
                    // Move to ready queue
                    self.ready_queue[cpu_id].write().push_back(task_id);
                    return true;
                }
            }
        }

        // Not found in blocked queues. This can happen if a wake occurs between
        // a task marking itself Blocked and the scheduler moving it to the
        // blocked_queue. In that case, ensure the task state is set back to
        // Running so that the scheduler does not park it.
        if let Some(task) = TaskPool::get_task_mut(task_id) {
            if let TaskState::Blocked(_) = task.state {
                task.state = TaskState::Running;
                // Memory barrier to ensure state change is visible
                core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

                // Add to the least loaded CPU's queue
                let mut min_len = usize::MAX;
                let mut target_cpu = 0;

                for cpu_id in 0..NUM_OF_CPUS {
                    let len = self.ready_queue[cpu_id].read().len();
                    if len < min_len {
                        min_len = len;
                        target_cpu = cpu_id;
                    }
                }

                // Only add if not already in ready_queue to avoid duplicates
                let mut ready_queue = self.ready_queue[target_cpu].write();
                if !ready_queue.contains(&task_id) {
                    ready_queue.push_back(task_id);
                }
                return true;
            }
        }

        false
    }

    /// Clean up a zombie task after it has been waited on
    ///
    /// This removes the task from zombie_queue and task_pool, freeing all resources.
    /// Should only be called from Task::wait() after confirming the task is a zombie.
    ///
    /// # Arguments
    /// * `task_id` - The ID of the zombie task to clean up
    pub fn cleanup_zombie_task(&self, task_id: usize) {
        // Remove from zombie queue
        for cpu_id in 0..NUM_OF_CPUS {
            let mut zombie_queue = self.zombie_queue[cpu_id].write();

            if let Some(pos) = zombie_queue.iter().position(|&id| id == task_id) {
                zombie_queue.remove(pos);
                crate::println!("[Scheduler] Removed task {} from zombie_queue", task_id);
                break;
            }
        }

        // Remove from task pool (this frees all task resources)
        if let Some(_task) = get_task_pool().remove_task(task_id) {
            crate::println!("[Scheduler] Cleaned up zombie task {}", task_id);
        }
    }

    /// Get IDs of all tasks across ready, blocked, and zombie queues
    ///
    /// This helper is used by subsystems (e.g., event broadcast) that need
    /// to target every task in the system without holding a mutable
    /// reference to the scheduler during delivery.
    pub fn get_all_task_ids(&self) -> alloc::vec::Vec<usize> {
        let mut ids = alloc::vec::Vec::new();

        // Ready tasks
        for q in &self.ready_queue {
            let queue = q.read();
            for t in queue.iter() {
                ids.push(*t);
            }
        }

        // Blocked tasks
        for q in &self.blocked_queue {
            let queue = q.read();
            for t in queue.iter() {
                ids.push(*t);
            }
        }

        // Zombie tasks
        for q in &self.zombie_queue {
            let queue = q.read();
            for t in queue.iter() {
                ids.push(*t);
            }
        }

        ids
    }

    /// Perform kernel context switch between tasks
    ///
    /// This function handles the low-level kernel context switching between
    /// the current task and the next selected task. It also saves/restores
    /// FPU/SIMD/Vector context for user-space tasks.
    ///
    /// CRITICAL: No scheduler locks should be held when calling this function
    /// to prevent deadlock during context switches.
    ///
    /// # Arguments
    /// * `cpu_id` - The CPU ID
    /// * `from_task_id` - Current task ID
    /// * `to_task_id` - Next task ID
    fn kernel_context_switch(&self, cpu_id: usize, from_task_id: usize, to_task_id: usize) {
        // crate::println!("[SCHED] CPU{}: Switching kernel context from Task {} to Task {}", cpu_id, from_task_id, to_task_id);
        if from_task_id != to_task_id {
            // Find tasks in all queues (ready, blocked, zombie)
            let mut from_ctx_ptr: *mut crate::arch::KernelContext = core::ptr::null_mut();
            let mut to_ctx_ptr: *const crate::arch::KernelContext = core::ptr::null();

            {
                if let Some(from_task) = TaskPool::get_task_mut(from_task_id) {
                    from_ctx_ptr = &mut from_task.kernel_context;

                    #[cfg(feature = "user-fpu")]
                    crate::arch::fpu::kernel_switch_out_user_fpu(&mut from_task.vcpu);

                    #[cfg(feature = "user-vector")]
                    crate::arch::fpu::kernel_switch_out_user_vector(
                        cpu_id,
                        from_task_id,
                        &mut from_task.vcpu,
                    );
                }
                if let Some(to_task) = TaskPool::get_task_mut(to_task_id) {
                    to_ctx_ptr = &to_task.kernel_context;
                }
            }

            if !from_ctx_ptr.is_null() && !to_ctx_ptr.is_null() {
                // Perform kernel context switch
                unsafe {
                    crate::arch::switch::switch_to(from_ctx_ptr, to_ctx_ptr);
                }

                // Execution resumes here when this task is rescheduled
                if let Some(from_task) = TaskPool::get_task_mut(from_task_id) {
                    #[cfg(feature = "user-fpu")]
                    crate::arch::fpu::kernel_switch_in_user_fpu(&mut from_task.vcpu);
                }
            } else {
                // crate::println!("[SCHED] ERROR: Context pointers not found - from: {:p}, to: {:p}", from_ctx_ptr, to_ctx_ptr);
            }
        }
    }

    /// Setup task execution by configuring hardware and user context
    ///
    /// This replaces the old dispatcher functionality with a more direct approach.
    ///
    /// # Arguments
    /// * `cpu` - The CPU architecture state
    /// * `task` - The task to setup for execution
    pub fn setup_task_execution(cpu: &mut Arch, task: &mut Task) {
        // crate::early_println!("[SCHED] Setting up Task {} for execution", task.get_id());
        // crate::early_println!("[SCHED]   before CPU {:#x?}", cpu);
        // let trapframe = cpu.get_trapframe();
        // crate::early_println!("[SCHED]   before Trapframe {:#x?}", trapframe);

        // Prefer the high-VA kernel stack window if available
        let sp = if let Some((_slot, base)) = task.get_kernel_stack_window_base() {
            // top = base + guard + TASK_KERNEL_STACK_SIZE
            (base + crate::environment::PAGE_SIZE + crate::environment::TASK_KERNEL_STACK_SIZE)
                as u64
        } else {
            task.get_kernel_stack_bottom_paddr()
        };

        // crate::early_println!("[SCHED]   Setting kernel stack to {:#x}", sp);
        cpu.set_kernel_stack(sp);

        // Handle trapframe and vcpu switching - use raw pointer to avoid borrow checker issues
        // This is safe because we're accessing different fields of the same struct
        let task_ptr = task as *mut Task;
        unsafe {
            let trapframe = (*task_ptr).get_trapframe();
            (*task_ptr).vcpu.switch(trapframe);
        }

        cpu.set_trap_handler(get_user_trap_handler());
        cpu.set_next_address_space(task.vm_manager.get_asid());
        set_next_mode(task.vcpu.get_mode());
        // Setup trap vector
        set_trapvector(get_trampoline_trap_vector());

        // crate::early_println!("[SCHED]   after  CPU {:#x?}", cpu);
        // crate::early_println!("[SCHED]   after  Trapframe {:#x?}", cpu.get_trapframe());

        // Note: User context (VCPU) will be restored in schedule() after run() returns
    }

    /// Reset the scheduler to initial state (test-only)
    ///
    /// Clears all queues, resets current task IDs, and resets the task pool.
    /// This should ONLY be called in tests to clean up state between test cases.
    #[cfg(test)]
    pub fn reset(&self) {
        // Clear all queues
        for cpu_id in 0..NUM_OF_CPUS {
            self.ready_queue[cpu_id].write().clear();
            self.blocked_queue[cpu_id].write().clear();
            self.zombie_queue[cpu_id].write().clear();
            *self.current_task_id[cpu_id].write() = None;
        }

        // Reset the task pool
        get_task_pool().reset();
    }
}

pub fn make_test_tasks() {
    println!("Making test tasks...");
    let sched = get_scheduler();
    let mut task0 = new_kernel_task("Task0".to_string(), 0, || {
        println!("Task0");
        let mut counter: usize = 0;
        loop {
            if counter % 500000 == 0 {
                print!("\nTask0: ");
            }
            if counter % 10000 == 0 {
                print!(".");
            }
            counter += 1;
            if counter >= 100000000 {
                break;
            }
        }
        println!("");
        println!("Task0: Done");
        idle();
    });
    task0.init();
    sched.add_task(task0, 0);

    let mut task1 = new_kernel_task("Task1".to_string(), 0, || {
        println!("Task1");
        let mut counter: usize = 0;
        loop {
            if counter % 500000 == 0 {
                print!("\nTask1: {} %", counter / 1000000);
            }
            counter += 1;
            if counter >= 100000000 {
                break;
            }
        }
        println!("\nTask1: 100 %");
        println!("Task1: Completed");
        idle();
    });
    task1.init();
    sched.add_task(task1, 0);

    let mut task2 = new_kernel_task("Task2".to_string(), 0, || {
        println!("Task2");
        /* Fizz Buzz */
        for i in 1..=1000000 {
            if i % 1000 > 0 {
                continue;
            }
            let c = i / 1000;
            if c % 15 == 0 {
                println!("FizzBuzz");
            } else if c % 3 == 0 {
                println!("Fizz");
            } else if c % 5 == 0 {
                println!("Buzz");
            } else {
                println!("{}", c);
            }
        }
        println!("Task2: Done");
        idle();
    });
    task2.init();
    sched.add_task(task2, 0);
}

// late_initcall!(make_test_tasks);

#[cfg(test)]
mod tests {
    use crate::task::TaskType;

    use super::*;

    #[test_case]
    fn test_add_task() {
        let scheduler = Scheduler::new();
        let task = Task::new("TestTask".to_string(), 1, TaskType::Kernel);
        scheduler.add_task(task, 0);
        assert_eq!(scheduler.ready_queue[0].read().len(), 1);
    }
}
