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
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use alloc::{boxed::Box, collections::vec_deque::VecDeque, string::ToString};

use crate::arch::ArchCpuState;
use crate::arch::get_trapvector;
use crate::arch::set_next_mode;
use crate::print;
use crate::println;
use crate::task::TaskType;
use crate::{arch::set_trapvector, vm::get_trampoline_trap_vector};
use crate::{
    arch::{
        Arch, Trapframe, get_cpu, get_user_trap_handler, instruction::idle,
        trap::user::arch_switch_to_user,
    },
    environment::MAX_NUM_CPUS,
    sync::{CpuLocal, IrqGuard},
    task::{TaskState, new_kernel_task, wake_parent_waiters, wake_task_waiters},
    timer::get_kernel_timer,
};

use crate::task::Task;

/// Task pool that stores tasks in fixed positions
/// With each Task being 824 bytes, 1024 tasks consume approximately 824 KiB of memory,
/// which is very reasonable for general-purpose systems.
/// TODO: Refactor Task struct to use fine-grained Mutex on individual fields
///       (e.g., state: Mutex<TaskState>, time_slice: Mutex<usize>) and change
///       TaskPool to use Arc<Task> for safe sharing across threads/contexts.
///       This would also eliminate the fixed-size limitation.
pub const MAX_TASKS: usize = 1024;

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
/// - `get_task_by_id()` which is the preferred public API
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
        crate::println!("[SCHED] TaskPool::new() starting...");

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

        crate::println!("[SCHED] TaskPool created (heap allocation, stable pointers)");

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
    pub(crate) fn remove_task(&self, task_id: usize) -> Option<Task> {
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

static CURRENT_TASK_IDS: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];
static READY_QUEUES: [spin::Mutex<VecDeque<usize>>; MAX_NUM_CPUS] =
    [const { spin::Mutex::new(VecDeque::new()) }; MAX_NUM_CPUS];
static ZOMBIE_QUEUE: spin::Mutex<VecDeque<usize>> = spin::Mutex::new(VecDeque::new());
static DEBUG_TICK: AtomicU64 = AtomicU64::new(0);

#[inline]
fn assert_valid_cpu_id(cpu_id: usize) {
    debug_assert!(cpu_id < CpuLocal::<usize>::cpu_count());
}

#[inline]
fn encode_task_id(task_id: Option<usize>) -> usize {
    task_id.unwrap_or(0)
}

#[inline]
fn decode_task_id(task_id: usize) -> Option<usize> {
    if task_id == 0 { None } else { Some(task_id) }
}

#[inline]
fn ready_queue(cpu_id: usize) -> &'static spin::Mutex<VecDeque<usize>> {
    assert_valid_cpu_id(cpu_id);
    &READY_QUEUES[cpu_id]
}

#[inline]
fn set_current_task_id(cpu_id: usize, task_id: Option<usize>) {
    assert_valid_cpu_id(cpu_id);
    CURRENT_TASK_IDS[cpu_id].store(encode_task_id(task_id), Ordering::SeqCst);
}

#[inline]
fn push_ready_task(cpu_id: usize, task_id: usize) {
    let mut queue = ready_queue(cpu_id).lock();
    if !queue.contains(&task_id) {
        queue.push_back(task_id);
    }
}

fn ready_queue_contains(task_id: usize) -> bool {
    for cpu_id in 0..MAX_NUM_CPUS {
        if ready_queue(cpu_id).lock().contains(&task_id) {
            return true;
        }
    }
    false
}

fn finalize_zombie(task_id: usize, parent_id: Option<usize>) {
    {
        let mut zombie_queue = ZOMBIE_QUEUE.lock();
        if !zombie_queue.contains(&task_id) {
            zombie_queue.push_back(task_id);
        }
    }
    wake_task_waiters(task_id);
    if let Some(parent_id) = parent_id {
        wake_parent_waiters(parent_id);
    }
}

fn pick_next(cpu: &Arch) -> (Option<usize>, Option<usize>) {
    let _irq_guard = IrqGuard::new();
    let cpu_id = cpu.get_cpuid();
    let old_current_task_id = current_task_id(cpu_id);

    if let Some(current_id) = old_current_task_id {
        if let Some(task) = TaskPool::get_task_mut(current_id) {
            match task.state.load(Ordering::SeqCst) {
                TaskState::Ready => push_ready_task(cpu_id, current_id),
                TaskState::Running => {
                    task.state.store(TaskState::Ready, Ordering::SeqCst);
                    push_ready_task(cpu_id, current_id);
                }
                TaskState::Zombie => finalize_zombie(current_id, task.get_parent_id()),
                TaskState::Terminated | TaskState::Blocked(_) | TaskState::NotInitialized => {}
            }
        }
    }

    loop {
        let task_id = { ready_queue(cpu_id).lock().pop_front() };

        let Some(task_id) = task_id else {
            set_current_task_id(cpu_id, None);
            return (old_current_task_id, None);
        };

        let Some(task) = TaskPool::get_task_mut(task_id) else {
            continue;
        };

        match task.state.load(Ordering::SeqCst) {
            TaskState::NotInitialized => panic!("Task must be initialized before scheduling"),
            TaskState::Zombie => {
                finalize_zombie(task.get_id(), task.get_parent_id());
                continue;
            }
            TaskState::Terminated => {
                let _ = get_task_pool().remove_task(task_id);
                continue;
            }
            TaskState::Blocked(_) => continue,
            TaskState::Ready | TaskState::Running => {
                task.state.store(TaskState::Running, Ordering::SeqCst);
                task.time_slice.store(
                    task.default_time_slice.load(Ordering::SeqCst),
                    Ordering::SeqCst,
                );
                let next_task_id = task.get_id();
                set_current_task_id(cpu_id, Some(next_task_id));
                push_ready_task(cpu_id, next_task_id);
                return (old_current_task_id, Some(next_task_id));
            }
        }
    }
}

pub fn add_task(task: Task, cpu_id: usize) -> usize {
    let _irq_guard = IrqGuard::new();
    let task_id = match get_task_pool().add_task(task) {
        Ok(id) => id,
        Err(e) => panic!("Failed to add task: {}", e),
    };
    push_ready_task(cpu_id, task_id);
    task_id
}

/// Called every timer tick. Decrements the current task's time_slice.
/// If time_slice reaches 0, triggers a reschedule.
pub fn sched_on_tick(cpu_id: usize, trapframe: &mut Trapframe) {
    let _tick = DEBUG_TICK.fetch_add(1, Ordering::Relaxed);

    if let Some(task_id) = current_task_id(cpu_id) {
        if let Some(task) = TaskPool::get_task_mut(task_id) {
            let current_slice = task.time_slice.load(Ordering::SeqCst);
            if current_slice > 0 {
                task.time_slice.store(current_slice - 1, Ordering::SeqCst);
            }
            if task.time_slice.load(Ordering::SeqCst) == 0 {
                schedule(trapframe);
            }
        }
    } else {
        schedule(trapframe);
    }
}

/// Schedule tasks on the CPU with kernel context switching.
pub fn schedule(trapframe: &mut Trapframe) {
    let cpu = get_cpu();
    let cpu_id = cpu.get_cpuid();
    let (current_task_id, next_task_id) = pick_next(cpu);

    if let Some(next_task_id) = next_task_id {
        if current_task_id != Some(next_task_id) {
            if let Some(current_task_id) = current_task_id {
                let current_task = get_task_by_id(current_task_id).unwrap();
                current_task.vcpu.lock().store(trapframe);

                #[cfg(all(feature = "hypervisor", target_arch = "riscv64"))]
                {
                    use crate::arch::hv::switch::{HypervisorSwitchData, VcpuSwitchData};

                    kernel_context_switch(cpu_id, current_task_id, next_task_id);
                }
                #[cfg(not(all(feature = "hypervisor", target_arch = "riscv64")))]
                {
                    kernel_context_switch(cpu_id, current_task_id, next_task_id);
                }

                let current_task = get_task_by_id(current_task_id).unwrap();
                current_task.vcpu.lock().switch(trapframe);
                set_next_mode(current_task.vcpu.lock().get_mode());
            } else {
                let next_task = get_task_by_id(next_task_id).unwrap();
                setup_task_execution(get_cpu(), next_task);
                arch_switch_to_user(next_task.get_trapframe());
            }
        }
    }

    if let Some(current_task) = current_task(cpu_id) {
        let _ = current_task.process_pending_events();
    }
}

/// Start the scheduler and return the first runnable task ID (if any).
pub fn start_scheduler() -> Option<usize> {
    let cpu = get_cpu();
    let cpu_id = cpu.get_cpuid();
    let timer = get_kernel_timer();
    timer.stop(cpu_id);
    timer.set_interval_us(cpu_id, crate::timer::TICK_INTERVAL_US);
    timer.start(cpu_id);

    let (_current_task_id, next_task_id) = pick_next(cpu);
    next_task_id
}

pub fn current_task(cpu_id: usize) -> Option<&'static Task> {
    current_task_id(cpu_id).and_then(TaskPool::get_task)
}

pub fn current_task_mut(cpu_id: usize) -> Option<&'static mut Task> {
    current_task_id(cpu_id).and_then(TaskPool::get_task_mut)
}

pub fn current_task_id(cpu_id: usize) -> Option<usize> {
    assert_valid_cpu_id(cpu_id);
    decode_task_id(CURRENT_TASK_IDS[cpu_id].load(Ordering::SeqCst))
}

pub fn get_task_by_id(task_id: usize) -> Option<&'static mut Task> {
    TaskPool::get_task_mut(task_id)
}

pub fn wake_task(task_id: usize) -> bool {
    wake_task_on(task_id, get_cpu().get_cpuid())
}

pub fn wake_task_on(task_id: usize, target_cpu: usize) -> bool {
    let _irq_guard = IrqGuard::new();
    let Some(task) = TaskPool::get_task_mut(task_id) else {
        return false;
    };

    if !matches!(task.state.load(Ordering::SeqCst), TaskState::Blocked(_)) {
        return false;
    }

    task.state.store(TaskState::Running, Ordering::SeqCst);
    core::sync::atomic::fence(Ordering::SeqCst);

    if !ready_queue_contains(task_id) {
        push_ready_task(target_cpu, task_id);
    }

    if target_cpu != get_cpu().get_cpuid() {
        crate::arch::send_reschedule_ipi(target_cpu);
    }

    true
}

pub fn cleanup_zombie(task_id: usize) {
    {
        let mut zombie_queue = ZOMBIE_QUEUE.lock();
        if let Some(pos) = zombie_queue.iter().position(|&id| id == task_id) {
            zombie_queue.remove(pos);
            crate::println!("[Scheduler] Removed task {} from zombie_queue", task_id);
        }
    }

    if let Some(_task) = get_task_pool().remove_task(task_id) {
        crate::println!("[Scheduler] Cleaned up zombie task {}", task_id);
    }
}

pub fn remove_task_from_queues(task_id: usize) {
    let _irq_guard = IrqGuard::new();

    for cpu_id in 0..MAX_NUM_CPUS {
        let mut queue = ready_queue(cpu_id).lock();
        while let Some(pos) = queue.iter().position(|&id| id == task_id) {
            queue.remove(pos);
        }

        if current_task_id(cpu_id) == Some(task_id) {
            set_current_task_id(cpu_id, None);
        }
    }

    let mut zombie_queue = ZOMBIE_QUEUE.lock();
    while let Some(pos) = zombie_queue.iter().position(|&id| id == task_id) {
        zombie_queue.remove(pos);
    }
}

/// Get IDs of all tasks across scheduler-visible queues/state.
pub fn get_all_task_ids() -> alloc::vec::Vec<usize> {
    let mut ids = alloc::vec::Vec::new();

    for cpu_id in 0..MAX_NUM_CPUS {
        if let Some(task_id) = current_task_id(cpu_id) {
            if !ids.contains(&task_id) {
                ids.push(task_id);
            }
        }

        let queue = ready_queue(cpu_id).lock();
        for &task_id in queue.iter() {
            if !ids.contains(&task_id) {
                ids.push(task_id);
            }
        }
    }

    let zombie_queue = ZOMBIE_QUEUE.lock();
    for &task_id in zombie_queue.iter() {
        if !ids.contains(&task_id) {
            ids.push(task_id);
        }
    }

    ids
}

/// Perform kernel context switch between tasks.
fn kernel_context_switch(cpu_id: usize, from_task_id: usize, to_task_id: usize) {
    if from_task_id != to_task_id {
        let mut from_ctx_ptr: *mut crate::arch::context::KernelContext = core::ptr::null_mut();
        let mut to_ctx_ptr: *const crate::arch::context::KernelContext = core::ptr::null();

        {
            if let Some(from_task) = TaskPool::get_task_mut(from_task_id) {
                from_ctx_ptr = &mut *from_task.kernel_context.lock();

                #[cfg(feature = "user-fpu")]
                crate::arch::fpu::kernel_switch_out_user_fpu(&mut *from_task.vcpu.lock());

                #[cfg(feature = "user-vector")]
                crate::arch::fpu::kernel_switch_out_user_vector(
                    cpu_id,
                    from_task_id,
                    &mut *from_task.vcpu.lock(),
                );
            }
            if let Some(to_task) = TaskPool::get_task_mut(to_task_id) {
                to_ctx_ptr = &*to_task.kernel_context.lock();
            }
        }

        if !from_ctx_ptr.is_null() && !to_ctx_ptr.is_null() {
            let cpu = get_cpu();
            let saved_arch_cpu_state = ArchCpuState::save(cpu);
            let saved_trapvector = get_trapvector();

            #[cfg(feature = "hypervisor")]
            let guest_vcpu_switch_data = crate::arch::hv::switch::VcpuSwitchData::save();
            #[cfg(feature = "hypervisor")]
            let hypervisor_switch_data = crate::arch::hv::switch::HypervisorSwitchData::save();

            unsafe {
                crate::arch::switch::switch_to(from_ctx_ptr, to_ctx_ptr);
            }

            #[cfg(feature = "hypervisor")]
            {
                guest_vcpu_switch_data.restore();
                hypervisor_switch_data.restore();
            }

            let cpu = get_cpu();
            saved_arch_cpu_state.restore(cpu);
            set_trapvector(saved_trapvector);
            if let Some(from_task) = TaskPool::get_task_mut(from_task_id) {
                #[cfg(feature = "user-fpu")]
                crate::arch::fpu::kernel_switch_in_user_fpu(&mut *from_task.vcpu.lock());
            }
        }
    }
}

/// Setup task execution by configuring hardware and user context.
pub fn setup_task_execution(cpu: &mut Arch, task: &mut Task) {
    let sp = if let Some((_slot, base)) = task.get_kernel_stack_window_base() {
        (base + crate::environment::PAGE_SIZE + crate::environment::TASK_KERNEL_STACK_SIZE) as u64
    } else {
        task.get_kernel_stack_bottom_paddr()
    };

    cpu.set_kernel_stack(sp);

    let task_ptr = task as *mut Task;
    unsafe {
        let trapframe = (*task_ptr).get_trapframe();
        (*task_ptr).vcpu.lock().switch(trapframe);
    }

    cpu.set_trap_handler(get_user_trap_handler());
    cpu.set_next_address_space(task.vm_manager.get_asid());
    set_next_mode(task.vcpu.lock().get_mode());
    set_trapvector(get_trampoline_trap_vector());
}

/// Reset the scheduler to initial state (test-only).
#[cfg(test)]
pub fn reset() {
    for cpu_id in 0..MAX_NUM_CPUS {
        ready_queue(cpu_id).lock().clear();
        set_current_task_id(cpu_id, None);
    }
    ZOMBIE_QUEUE.lock().clear();
    get_task_pool().reset();
}

pub fn make_test_tasks() {
    println!("Making test tasks...");
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
    add_task(task0, 0);

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
    add_task(task1, 0);

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
    add_task(task2, 0);
}

// late_initcall!(make_test_tasks);

#[cfg(test)]
mod tests {
    use crate::task::TaskType;

    use super::*;

    #[test_case]
    fn test_add_task() {
        reset();
        let task = Task::new("TestTask".to_string(), 1, TaskType::Kernel);
        add_task(task, 0);
        assert_eq!(READY_QUEUES[0].lock().len(), 1);
    }
}
