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
//! The pool provides `get_task()` which returns `&'static`
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

use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use alloc::{boxed::Box, collections::vec_deque::VecDeque, string::ToString};

use crate::abi::EventProcessOutcome;
use crate::arch::ArchCpuState;
use crate::arch::get_trapvector;
use crate::arch::set_next_mode;
use crate::print;
use crate::println;
use crate::{arch::set_trapvector, vm::get_trampoline_trap_vector};
use crate::{
    arch::{
        Arch, Trapframe, get_cpu, get_user_trap_handler, instruction::idle,
        trap::user::arch_switch_to_user,
    },
    environment::MAX_NUM_CPUS,
    sync::{CpuLocal, IrqGuard},
    task::{
        SCHED_UTIL_SCALE, Task, TaskCorePreference, TaskState, new_kernel_task,
        wake_parent_waiters, wake_task_waiters,
    },
    timer::{get_kernel_timer, get_time_ns},
};

/// Task pool that stores tasks in fixed positions
/// With each Task being 824 bytes, 1024 tasks consume approximately 824 KiB of memory,
/// which is very reasonable for general-purpose systems.
/// TODO: Refactor Task struct to use fine-grained Mutex on individual fields
///       (e.g., state: Mutex<TaskState>, time_slice: Mutex<usize>) and change
///       TaskPool to use Arc<Task> for safe sharing across threads/contexts.
///       This would also eliminate the fixed-size limitation.
pub const MAX_TASKS: usize = 1024;
const KERNEL_TASK_ID_START: usize = MAX_TASKS - 128;

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
/// This struct provides unsafe access to tasks through `get_task()`
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
    // ⚠️ DO NOT ACCESS DIRECTLY - Use get_task() methods
    tasks: spin::Mutex<Box<[Option<Task>; MAX_TASKS]>>,

    // Monotonically increasing generation for each task slot.
    // Incremented every time a slot receives a new task so recycled IDs can
    // be distinguished from stale deferred references.
    slot_generations: Box<[AtomicUsize; MAX_TASKS]>,

    // Free lists of recyclable task IDs. User task IDs are kept in the low range
    // so init remains PID 1 even if kernel workers are created early.
    free_user_ids: spin::Mutex<VecDeque<usize>>,
    free_kernel_ids: spin::Mutex<VecDeque<usize>>,

    // Next IDs to allocate when free lists are empty. Kernel workers allocate
    // downward from the high reserved range.
    next_user_id: core::sync::atomic::AtomicUsize,
    next_kernel_id: core::sync::atomic::AtomicUsize,
}

impl TaskPool {
    fn new() -> Self {
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

        TaskPool {
            tasks: spin::Mutex::new(tasks),
            slot_generations: Box::new([const { AtomicUsize::new(0) }; MAX_TASKS]),
            free_user_ids: spin::Mutex::new(VecDeque::new()),
            free_kernel_ids: spin::Mutex::new(VecDeque::new()),
            next_user_id: core::sync::atomic::AtomicUsize::new(1), // Start from 1, ID 0 is invalid
            next_kernel_id: core::sync::atomic::AtomicUsize::new(MAX_TASKS - 1),
        }
    }

    /// Allocate a new task ID
    /// Tries to reuse freed IDs first, then allocates new ones sequentially
    /// Uses atomic operations for lock-free allocation
    pub fn allocate_id(&self, task_type: crate::task::TaskType) -> Option<usize> {
        match task_type {
            crate::task::TaskType::Kernel => self.allocate_kernel_id(),
            crate::task::TaskType::User => self.allocate_user_id(),
        }
    }

    fn allocate_user_id(&self) -> Option<usize> {
        // Try to reuse freed IDs first
        {
            let mut free_user_ids = self.free_user_ids.lock();
            if let Some(id) = free_user_ids.pop_front() {
                return Some(id);
            }
        }

        // Allocate new ID using atomic fetch_add
        let id = self
            .next_user_id
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if id >= KERNEL_TASK_ID_START {
            // Rollback on overflow
            self.next_user_id
                .fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
            None
        } else {
            Some(id)
        }
    }

    fn allocate_kernel_id(&self) -> Option<usize> {
        {
            let mut free_kernel_ids = self.free_kernel_ids.lock();
            if let Some(id) = free_kernel_ids.pop_front() {
                return Some(id);
            }
        }

        let id = self
            .next_kernel_id
            .fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
        if id < KERNEL_TASK_ID_START {
            self.next_kernel_id
                .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            None
        } else {
            Some(id)
        }
    }

    /// Add a task to the pool
    /// Allocates an ID, sets it on the task, and returns the ID
    fn add_task(&self, mut task: Task) -> Result<usize, &'static str> {
        // Allocate ID for this task
        let task_id = self
            .allocate_id(task.task_type)
            .ok_or("Task pool exhausted")?;

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
        task.vm_manager.set_owner_task_id_if_unset(task_id);

        let generation = self.slot_generations[task_id]
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        debug_assert_ne!(generation, 0, "task slot generation wrapped to 0");

        tasks[task_id] = Some(task);
        Ok(task_id)
    }

    fn task_generation(&self, task_id: usize) -> Option<usize> {
        if task_id >= MAX_TASKS {
            return None;
        }

        let tasks = self.tasks.lock();
        tasks[task_id].as_ref()?;
        Some(self.slot_generations[task_id].load(Ordering::SeqCst))
    }

    fn get_task_if_generation(&self, task_id: usize, generation: usize) -> Option<&'static Task> {
        if task_id >= MAX_TASKS || generation == 0 {
            return None;
        }

        let tasks = self.tasks.lock();
        let task = tasks[task_id].as_ref()?;
        if self.slot_generations[task_id].load(Ordering::SeqCst) != generation {
            return None;
        }

        // SAFETY: The Box<[T]> ensures the array pointer is stable.
        // Once a Box is allocated, its underlying pointer never changes.
        let ptr = task as *const Task;
        Some(unsafe { &*ptr })
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
    /// Always use this method to ensure proper safety.
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

    /// Remove a task from the pool
    ///
    /// # Safety
    ///
    /// **CRITICAL**: This method invalidates all `&'static` references returned by
    /// `get_task()` for this task_id. The scheduler must ensure:
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
        let task = tasks[task_id].as_ref()?;
        if task.running_cpu.load(Ordering::SeqCst) != NO_CPU {
            return None;
        }
        if !matches!(task.state.load(Ordering::SeqCst), TaskState::Terminated) {
            return None;
        }
        let task = tasks[task_id].take().unwrap();
        match task.task_type {
            crate::task::TaskType::Kernel => self.free_kernel_ids.lock().push_back(task_id),
            crate::task::TaskType::User => self.free_user_ids.lock().push_back(task_id),
        }
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
            self.slot_generations[i].store(0, Ordering::SeqCst);
        }
        drop(tasks);

        // Reset ID allocation to start from each range's beginning
        self.next_user_id
            .store(1, core::sync::atomic::Ordering::Relaxed);
        self.next_kernel_id
            .store(MAX_TASKS - 1, core::sync::atomic::Ordering::Relaxed);

        // Clear free lists
        self.free_user_ids.lock().clear();
        self.free_kernel_ids.lock().clear();
    }
}

static CURRENT_TASK_IDS: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];
static READY_QUEUES: [spin::Mutex<VecDeque<usize>>; MAX_NUM_CPUS] =
    [const { spin::Mutex::new(VecDeque::new()) }; MAX_NUM_CPUS];
static ZOMBIE_QUEUE: spin::Mutex<VecDeque<usize>> = spin::Mutex::new(VecDeque::new());
static BLOCKED_QUEUE: spin::Mutex<VecDeque<usize>> = spin::Mutex::new(VecDeque::new());
static ONLINE_CPUS: spin::Mutex<alloc::vec::Vec<usize>> = spin::Mutex::new(alloc::vec::Vec::new());
static IDLE_TASK_IDS: [AtomicUsize; MAX_NUM_CPUS] = [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];
static PENDING_IDLE_TO_USER_TRAP_TASK: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];
static TOTAL_BUSY_CPU_TIME_NS: AtomicU64 = AtomicU64::new(0);
static TOTAL_IDLE_CPU_TIME_NS: AtomicU64 = AtomicU64::new(0);
static CPU_UTIL_AVG: [AtomicU32; MAX_NUM_CPUS] = [const { AtomicU32::new(0) }; MAX_NUM_CPUS];
static CPU_UTIL_MIN: [AtomicU32; MAX_NUM_CPUS] = [const { AtomicU32::new(0) }; MAX_NUM_CPUS];
static CPU_RUNNABLE_TASKS: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];

const DEFAULT_CPU_CAPACITY: u32 = 1024;
const MIN_CPU_CAPACITY: u32 = 128;
const EFFICIENCY_CPU_CAPACITY: u32 = 512;
const PERFORMANCE_CPU_CAPACITY: u32 = 1536;
const TASK_LOAD_SCALE: u64 = 1024;
const MAX_PRIORITY_LOAD_BONUS: u64 = 1024;
const SCHED_MIGRATION_COOLDOWN_NS: u64 = 100_000_000;
const SCHED_DEMOTION_MARGIN: u32 = 128;
const SCHED_STEAL_SCAN_LIMIT: usize = 8;

static CPU_CORE_CLASSES: [AtomicU8; MAX_NUM_CPUS] =
    [const { AtomicU8::new(CpuCoreClass::Balanced as u8) }; MAX_NUM_CPUS];
static CPU_CAPACITIES: [AtomicU32; MAX_NUM_CPUS] =
    [const { AtomicU32::new(DEFAULT_CPU_CAPACITY) }; MAX_NUM_CPUS];
const INVALID_CPU_TOPOLOGY_DOMAIN: u32 = u32::MAX;
static CPU_TOPOLOGY_DOMAINS: [AtomicU32; MAX_NUM_CPUS] =
    [const { AtomicU32::new(INVALID_CPU_TOPOLOGY_DOMAIN) }; MAX_NUM_CPUS];
static SCHED_MIGRATIONS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SCHED_MIGRATION_PROMOTIONS: AtomicU64 = AtomicU64::new(0);
static SCHED_MIGRATION_DEMOTIONS: AtomicU64 = AtomicU64::new(0);
static SCHED_MIGRATION_COOLDOWN_SKIPS: AtomicU64 = AtomicU64::new(0);
static SCHED_WORK_STEALS: AtomicU64 = AtomicU64::new(0);

/// Coarse CPU core class used for heterogeneous scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CpuCoreClass {
    /// Energy-efficient core.
    Efficiency = 0,
    /// Default homogeneous core.
    Balanced = 1,
    /// Higher-performance core.
    Performance = 2,
}

impl CpuCoreClass {
    /// Return a stable lowercase name for this core class.
    ///
    /// # Returns
    ///
    /// `"efficiency"`, `"balanced"`, or `"performance"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            CpuCoreClass::Efficiency => "efficiency",
            CpuCoreClass::Balanced => "balanced",
            CpuCoreClass::Performance => "performance",
        }
    }

    /// Return the default relative capacity for this core class.
    ///
    /// # Returns
    ///
    /// Capacity in scheduler units where `1024` is a normal homogeneous CPU.
    pub const fn default_capacity(self) -> u32 {
        match self {
            CpuCoreClass::Efficiency => EFFICIENCY_CPU_CAPACITY,
            CpuCoreClass::Balanced => DEFAULT_CPU_CAPACITY,
            CpuCoreClass::Performance => PERFORMANCE_CPU_CAPACITY,
        }
    }

    const fn from_u8(value: u8) -> Self {
        match value {
            0 => CpuCoreClass::Efficiency,
            2 => CpuCoreClass::Performance,
            _ => CpuCoreClass::Balanced,
        }
    }
}

/// Scheduler-visible CPU topology information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTopology {
    /// Scheduler CPU ID.
    pub cpu_id: usize,
    /// Coarse CPU core class.
    pub core_class: CpuCoreClass,
    /// Relative compute capacity in scheduler units.
    pub capacity: u32,
    /// Optional scheduler topology domain ID.
    pub domain_id: Option<u32>,
    /// CPU mask for CPUs registered in the same topology domain.
    pub domain_cpus_mask: u64,
}

/// Scheduler CPU accounting snapshot.
#[derive(Debug, Clone, Copy)]
pub struct CpuUsageSnapshot {
    /// Number of CPUs currently known to the scheduler.
    pub online_cpus: usize,
    /// Cumulative non-idle CPU time in nanoseconds.
    pub busy_time_ns: u64,
    /// Cumulative idle task CPU time in nanoseconds.
    pub idle_time_ns: u64,
}

/// Scheduler utilization snapshot for one CPU.
#[derive(Debug, Clone, Copy)]
pub struct CpuUtilSnapshot {
    /// Scheduler CPU ID.
    pub cpu_id: usize,
    /// Exponentially weighted utilization average in [`SCHED_UTIL_SCALE`] units.
    pub util_avg: u32,
    /// Maximum minimum-utilization clamp from runnable tasks on this CPU.
    pub util_min: u32,
    /// Relative CPU capacity in scheduler units.
    pub capacity: u32,
    /// Number of non-idle runnable tasks seen by this CPU.
    pub runnable_tasks: usize,
}

/// Scheduler migration accounting snapshot.
#[derive(Debug, Clone, Copy)]
pub struct SchedulerMigrationStats {
    /// Number of capacity-directed scheduler migrations.
    pub total: u64,
    /// Number of migrations to a higher-capacity CPU.
    pub promotions: u64,
    /// Number of migrations to a lower-capacity CPU.
    pub demotions: u64,
    /// Number of migration opportunities skipped by cooldown.
    pub cooldown_skips: u64,
    /// Number of ready tasks moved by idle-core work stealing.
    pub work_steals: u64,
}

pub fn note_idle_to_user_handoff(cpu_id: usize, task_id: usize) {
    if cpu_id < MAX_NUM_CPUS {
        PENDING_IDLE_TO_USER_TRAP_TASK[cpu_id].store(task_id, Ordering::SeqCst);
    }
}

pub fn take_idle_to_user_handoff(cpu_id: usize, current_task_id: usize) -> bool {
    if cpu_id >= MAX_NUM_CPUS {
        return false;
    }

    PENDING_IDLE_TO_USER_TRAP_TASK[cpu_id]
        .compare_exchange(current_task_id, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}
static DEBUG_TICK: AtomicU64 = AtomicU64::new(0);
static NEXT_CPU: AtomicUsize = AtomicUsize::new(0);

pub const DEBUG_SMP_TASK_FLOW: bool = false;

static DEBUG_ENQUEUE_SEQ: AtomicUsize = AtomicUsize::new(0);
static DEBUG_REMOTE_ENQUEUE_TASK: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];
static DEBUG_REMOTE_ENQUEUE_FROM_CPU: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(NO_CPU) }; MAX_NUM_CPUS];
static DEBUG_REMOTE_ENQUEUE_SEQ: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];

const NO_CPU: usize = usize::MAX;

const TASK_ID_MASK: usize = MAX_TASKS - 1;

static SCHEDULE_PREV_TASK: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];

fn debug_remote_enqueue_snapshot(cpu_id: usize) -> (Option<usize>, usize, usize) {
    let task_id = decode_task_id(DEBUG_REMOTE_ENQUEUE_TASK[cpu_id].load(Ordering::SeqCst));
    let from_cpu = DEBUG_REMOTE_ENQUEUE_FROM_CPU[cpu_id].load(Ordering::SeqCst);
    let seq = DEBUG_REMOTE_ENQUEUE_SEQ[cpu_id].load(Ordering::SeqCst);
    (task_id, from_cpu, seq)
}

fn debug_task_name(task_id: usize) -> alloc::string::String {
    TaskPool::get_task(task_id)
        .map(|task| task.name.read().clone())
        .unwrap_or_else(|| "<missing>".to_string())
}

pub fn debug_log_reschedule_ipi(cpu_id: usize, from_kernel: bool, can_schedule: bool) {
    if !DEBUG_SMP_TASK_FLOW {
        return;
    }

    let (expected_task, from_cpu, seq) = debug_remote_enqueue_snapshot(cpu_id);
    println!(
        "[SMPDBG ipi-recv] cpu={} from_kernel={} can_schedule={} current={:?} expected_task={:?} expected_name={} expected_from={} seq={} ready_len={}",
        cpu_id,
        from_kernel,
        can_schedule,
        current_task_id(cpu_id),
        expected_task,
        expected_task
            .map(debug_task_name)
            .unwrap_or_else(|| "<none>".to_string()),
        from_cpu,
        seq,
        ready_queue(cpu_id).lock().len(),
    );
}

fn release_deferred_prev(cpu_id: usize) {
    let prev = decode_prev_task(SCHEDULE_PREV_TASK[cpu_id].swap(0, Ordering::SeqCst));
    let Some((prev_id, prev_generation)) = prev else {
        return;
    };
    let Some(task) = get_task_pool().get_task_if_generation(prev_id, prev_generation) else {
        return;
    };
    if task
        .running_cpu
        .compare_exchange(cpu_id, NO_CPU, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        // Idle tasks are never enqueued into the ready queue. They are
        // selected by the fallback in pick_next() when the queue is empty.
        let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
        if prev_id == idle_id {
            task.state.store(TaskState::Ready, Ordering::SeqCst);
            return;
        }
        // Requeue the previous task after its context has been saved. At this
        // point `running_cpu` has been released, so another CPU may safely
        // claim the saved kernel context from its ready queue.
        if matches!(task.state.load(Ordering::SeqCst), TaskState::Ready) {
            let now_ns = get_time_ns();
            let target_cpu = migration_target_for_task(task, cpu_id, now_ns, true)
                .filter(|&target_cpu| is_cpu_online(target_cpu))
                .unwrap_or(cpu_id);
            if target_cpu != cpu_id {
                record_scheduler_migration(task, cpu_id, target_cpu, now_ns);
            }
            task.last_cpu.store(target_cpu, Ordering::SeqCst);
            push_ready_task(target_cpu, prev_id);
            notify_remote_ready_task(target_cpu, prev_id, "migrate-ipi-send");
        }
    }
}

fn charge_finished_cpu_time(cpu_id: usize, task_id: usize, delta_ns: u64) {
    if delta_ns == 0 {
        return;
    }

    let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
    if idle_id != 0 && task_id == idle_id {
        TOTAL_IDLE_CPU_TIME_NS.fetch_add(delta_ns, Ordering::SeqCst);
    } else {
        TOTAL_BUSY_CPU_TIME_NS.fetch_add(delta_ns, Ordering::SeqCst);
    }
}

fn task_util_min_by_id(task_id: usize) -> u32 {
    TaskPool::get_task(task_id)
        .map(Task::sched_util_min)
        .unwrap_or(0)
}

fn cpu_instant_util(cpu_id: usize) -> (u32, u32, usize) {
    let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
    let mut util = 0u32;
    let mut util_min = 0u32;
    let mut runnable_tasks = 0usize;

    if let Some(task_id) = current_task_id(cpu_id)
        && task_id != idle_id
    {
        runnable_tasks = runnable_tasks.saturating_add(1);
        util = SCHED_UTIL_SCALE;
        util_min = util_min.max(task_util_min_by_id(task_id));
    }

    let queue = ready_queue(cpu_id).lock();
    for &task_id in queue.iter() {
        if task_id == idle_id {
            continue;
        }
        runnable_tasks = runnable_tasks.saturating_add(1);
        util_min = util_min.max(task_util_min_by_id(task_id));
    }
    drop(queue);

    (
        util.max(util_min).min(SCHED_UTIL_SCALE),
        util_min,
        runnable_tasks,
    )
}

fn update_cpu_util_avg(cpu_id: usize) {
    let (instant, util_min, runnable_tasks) = cpu_instant_util(cpu_id);
    let prev = CPU_UTIL_AVG[cpu_id].load(Ordering::SeqCst);
    let next = if instant > prev {
        prev.saturating_add(instant).div_ceil(2)
    } else {
        prev.saturating_mul(7).saturating_add(instant) / 8
    };
    CPU_UTIL_AVG[cpu_id].store(next.min(SCHED_UTIL_SCALE), Ordering::SeqCst);
    CPU_UTIL_MIN[cpu_id].store(util_min, Ordering::SeqCst);
    CPU_RUNNABLE_TASKS[cpu_id].store(runnable_tasks, Ordering::SeqCst);
}

/// Return scheduler utilization for one CPU.
///
/// # Arguments
///
/// * `cpu_id` - Scheduler CPU ID.
///
/// # Returns
///
/// Current utilization snapshot, or `None` if the CPU ID is invalid.
pub fn cpu_util_snapshot(cpu_id: usize) -> Option<CpuUtilSnapshot> {
    if cpu_id >= MAX_NUM_CPUS {
        return None;
    }

    Some(CpuUtilSnapshot {
        cpu_id,
        util_avg: CPU_UTIL_AVG[cpu_id].load(Ordering::SeqCst),
        util_min: CPU_UTIL_MIN[cpu_id].load(Ordering::SeqCst),
        capacity: cpu_capacity(cpu_id),
        runnable_tasks: CPU_RUNNABLE_TASKS[cpu_id].load(Ordering::SeqCst),
    })
}

/// Return scheduler migration counters.
///
/// # Returns
///
/// Current scheduler migration accounting snapshot.
pub fn scheduler_migration_stats() -> SchedulerMigrationStats {
    SchedulerMigrationStats {
        total: SCHED_MIGRATIONS_TOTAL.load(Ordering::SeqCst),
        promotions: SCHED_MIGRATION_PROMOTIONS.load(Ordering::SeqCst),
        demotions: SCHED_MIGRATION_DEMOTIONS.load(Ordering::SeqCst),
        cooldown_skips: SCHED_MIGRATION_COOLDOWN_SKIPS.load(Ordering::SeqCst),
        work_steals: SCHED_WORK_STEALS.load(Ordering::SeqCst),
    }
}

fn account_task_switch(cpu_id: usize, old_id: Option<usize>, next_id: Option<usize>) {
    if old_id == next_id {
        return;
    }

    let now_ns = get_time_ns();

    if let Some(old_id) = old_id {
        if let Some(task) = TaskPool::get_task(old_id) {
            let delta_ns = task.stop_cpu_accounting(now_ns);
            charge_finished_cpu_time(cpu_id, old_id, delta_ns);
            let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
            if old_id != idle_id {
                task.account_sched_util_running(now_ns);
            }
        }
    }

    if let Some(next_id) = next_id {
        if let Some(task) = TaskPool::get_task(next_id) {
            task.start_cpu_accounting(now_ns);
        }
    }
}

/// Return a system-wide CPU accounting snapshot.
///
/// # Returns
///
/// Cumulative busy and idle CPU time, including currently running task deltas.
pub fn cpu_usage_snapshot() -> CpuUsageSnapshot {
    let now_ns = get_time_ns();
    let mut busy_time_ns = TOTAL_BUSY_CPU_TIME_NS.load(Ordering::SeqCst);
    let mut idle_time_ns = TOTAL_IDLE_CPU_TIME_NS.load(Ordering::SeqCst);

    for_each_online_cpu(|cpu_id| {
        let Some(task_id) = current_task_id(cpu_id) else {
            return;
        };
        let Some(task) = TaskPool::get_task(task_id) else {
            return;
        };
        let delta_ns = task.current_cpu_delta_ns(now_ns);
        let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
        if idle_id != 0 && task_id == idle_id {
            idle_time_ns = idle_time_ns.saturating_add(delta_ns);
        } else {
            busy_time_ns = busy_time_ns.saturating_add(delta_ns);
        }
    });

    CpuUsageSnapshot {
        online_cpus: num_online_cpus(),
        busy_time_ns,
        idle_time_ns,
    }
}

pub fn complete_deferred_context_switch(cpu_id: usize) {
    release_deferred_prev(cpu_id);
}

fn try_claim_ready_task(task: &Task, cpu_id: usize) -> bool {
    if task
        .running_cpu
        .compare_exchange(NO_CPU, cpu_id, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return false;
    }
    if task
        .state
        .compare_exchange(
            TaskState::Ready,
            TaskState::Running,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        task.running_cpu.store(NO_CPU, Ordering::SeqCst);
        return false;
    }
    task.last_cpu.store(cpu_id, Ordering::SeqCst);
    task.time_slice.store(
        task.default_time_slice.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    true
}

pub fn register_online_cpu(cpu_id: usize) {
    let mut cpus = ONLINE_CPUS.lock();
    if !cpus.contains(&cpu_id) {
        cpus.push(cpu_id);
    }
}

fn cpu_mask_bit(cpu_id: usize) -> u64 {
    if cpu_id >= u64::BITS as usize {
        0
    } else {
        1u64 << cpu_id
    }
}

/// Return a bitmask of scheduler-online CPUs.
///
/// # Returns
///
/// A CPU mask with bit `n` set when scheduler CPU `n` is online. CPU IDs that
/// do not fit in a 64-bit mask are omitted.
pub fn online_cpu_mask() -> u64 {
    let cpus = ONLINE_CPUS.lock();
    cpus.iter()
        .fold(0u64, |mask, &cpu_id| mask | cpu_mask_bit(cpu_id))
}

fn sanitize_cpu_capacity(core_class: CpuCoreClass, capacity: u32) -> u32 {
    let capacity = if capacity == 0 {
        core_class.default_capacity()
    } else {
        capacity
    };
    capacity.max(MIN_CPU_CAPACITY)
}

/// Register scheduler topology information for a CPU.
///
/// Architecture code can call this during boot after discovering CPU topology
/// from FDT, ACPI, firmware, or platform-specific tables. CPUs default to
/// [`CpuCoreClass::Balanced`] with capacity `1024` if no topology is registered.
///
/// # Arguments
///
/// * `cpu_id` - Scheduler CPU ID.
/// * `core_class` - Coarse core class used for placement tie-breaking.
/// * `capacity` - Relative compute capacity. Pass `0` to use the class default.
///
/// # Returns
///
/// `Ok(())` on success, or an error if `cpu_id` is outside the supported range.
pub fn register_cpu_topology(
    cpu_id: usize,
    core_class: CpuCoreClass,
    capacity: u32,
) -> Result<(), &'static str> {
    if cpu_id >= MAX_NUM_CPUS {
        return Err("CPU ID out of bounds");
    }

    CPU_CORE_CLASSES[cpu_id].store(core_class as u8, Ordering::SeqCst);
    CPU_CAPACITIES[cpu_id].store(
        sanitize_cpu_capacity(core_class, capacity),
        Ordering::SeqCst,
    );
    Ok(())
}

/// Register scheduler topology domain information for a CPU.
///
/// Architecture code should call this when firmware exposes a stable CPU
/// grouping, such as an Apple Silicon performance/DVFS domain. The scheduler
/// treats missing domain information as "unknown" rather than as a stable ABI.
///
/// # Arguments
///
/// * `cpu_id` - Scheduler CPU ID.
/// * `domain_id` - Platform topology domain identifier.
///
/// # Returns
///
/// `Ok(())` on success, or an error if the CPU ID or domain ID is invalid.
pub fn register_cpu_topology_domain(cpu_id: usize, domain_id: u32) -> Result<(), &'static str> {
    if cpu_id >= MAX_NUM_CPUS {
        return Err("CPU ID out of bounds");
    }
    if domain_id == INVALID_CPU_TOPOLOGY_DOMAIN {
        return Err("Invalid CPU topology domain");
    }

    CPU_TOPOLOGY_DOMAINS[cpu_id].store(domain_id, Ordering::SeqCst);
    Ok(())
}

/// Return the registered topology domain for a CPU.
///
/// # Arguments
///
/// * `cpu_id` - Scheduler CPU ID.
///
/// # Returns
///
/// The registered domain ID, or `None` if the CPU ID is invalid or the
/// platform did not provide domain information.
pub fn cpu_topology_domain(cpu_id: usize) -> Option<u32> {
    if cpu_id >= MAX_NUM_CPUS {
        return None;
    }

    let domain_id = CPU_TOPOLOGY_DOMAINS[cpu_id].load(Ordering::SeqCst);
    (domain_id != INVALID_CPU_TOPOLOGY_DOMAIN).then_some(domain_id)
}

fn cpu_topology_domain_mask(domain_id: u32) -> u64 {
    let mut mask = 0u64;
    for cpu_id in 0..MAX_NUM_CPUS {
        if cpu_topology_domain(cpu_id) == Some(domain_id) {
            mask |= cpu_mask_bit(cpu_id);
        }
    }
    mask
}

/// Return the online CPU mask for a scheduler topology domain.
///
/// # Arguments
///
/// * `domain_id` - Platform topology domain identifier.
///
/// # Returns
///
/// A CPU mask containing online CPUs in the requested domain. CPU IDs that do
/// not fit in a 64-bit mask are omitted.
pub fn cpu_topology_domain_online_mask(domain_id: u32) -> u64 {
    let cpus = ONLINE_CPUS.lock();
    cpus.iter()
        .filter(|&&cpu_id| cpu_topology_domain(cpu_id) == Some(domain_id))
        .fold(0u64, |mask, &cpu_id| mask | cpu_mask_bit(cpu_id))
}

/// Return scheduler topology information for a CPU.
///
/// # Arguments
///
/// * `cpu_id` - Scheduler CPU ID.
///
/// # Returns
///
/// The registered topology information, or `None` if `cpu_id` is invalid.
pub fn cpu_topology(cpu_id: usize) -> Option<CpuTopology> {
    if cpu_id >= MAX_NUM_CPUS {
        return None;
    }

    let core_class = CpuCoreClass::from_u8(CPU_CORE_CLASSES[cpu_id].load(Ordering::SeqCst));
    let domain_id = cpu_topology_domain(cpu_id);
    Some(CpuTopology {
        cpu_id,
        core_class,
        capacity: sanitize_cpu_capacity(core_class, CPU_CAPACITIES[cpu_id].load(Ordering::SeqCst)),
        domain_id,
        domain_cpus_mask: domain_id.map(cpu_topology_domain_mask).unwrap_or(0),
    })
}

fn cpu_capacity(cpu_id: usize) -> u32 {
    cpu_topology(cpu_id)
        .map(|topology| topology.capacity)
        .unwrap_or(DEFAULT_CPU_CAPACITY)
}

fn task_load_weight(task: Option<&Task>) -> u64 {
    let Some(task) = task else {
        return TASK_LOAD_SCALE;
    };

    let priority = task.priority.load(Ordering::SeqCst) as u64;
    let bonus = core::cmp::min(priority, MAX_PRIORITY_LOAD_BONUS);
    TASK_LOAD_SCALE.saturating_add(bonus)
}

fn runnable_task_weight(task_id: usize) -> u64 {
    task_load_weight(TaskPool::get_task(task_id))
}

fn cpu_runnable_weight(cpu_id: usize) -> u64 {
    let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
    let mut weight = 0u64;

    if let Some(task_id) = current_task_id(cpu_id) {
        if task_id != idle_id {
            weight = weight.saturating_add(runnable_task_weight(task_id));
        }
    }

    let queue = ready_queue(cpu_id).lock();
    for &task_id in queue.iter() {
        if task_id != idle_id {
            weight = weight.saturating_add(runnable_task_weight(task_id));
        }
    }

    weight
}

fn cpu_load_score(cpu_id: usize) -> u64 {
    let capacity = cpu_capacity(cpu_id) as u64;
    cpu_runnable_weight(cpu_id).saturating_mul(TASK_LOAD_SCALE) / capacity
}

fn task_effective_util(task: Option<&Task>, now_ns: u64) -> u32 {
    let Some(task) = task else {
        return 0;
    };

    core::cmp::max(task.sched_util_avg_snapshot(now_ns), task.sched_util_min())
        .min(SCHED_UTIL_SCALE)
}

fn task_min_cpu_capacity_at(task: Option<&Task>, now_ns: u64) -> u32 {
    let preference_min = match task.map(Task::core_preference) {
        Some(TaskCorePreference::Performance) => DEFAULT_CPU_CAPACITY,
        _ => MIN_CPU_CAPACITY,
    };
    core::cmp::max(preference_min, task_effective_util(task, now_ns))
}

fn cpu_better_for_preference(
    candidate_cpu: usize,
    best_cpu: usize,
    preference: TaskCorePreference,
) -> bool {
    let candidate_capacity = cpu_capacity(candidate_cpu);
    let best_capacity = cpu_capacity(best_cpu);

    match preference {
        TaskCorePreference::Efficiency => candidate_capacity < best_capacity,
        TaskCorePreference::Performance => candidate_capacity > best_capacity,
        TaskCorePreference::Any => candidate_capacity < best_capacity,
    }
}

fn select_target_cpu_at(task: Option<&Task>, now_ns: u64) -> usize {
    let cpus = ONLINE_CPUS.lock();
    if cpus.is_empty() {
        return 0;
    }

    let preference = task
        .map(Task::core_preference)
        .unwrap_or(TaskCorePreference::Any);
    let min_capacity = task_min_cpu_capacity_at(task, now_ns);
    let start = NEXT_CPU.fetch_add(1, Ordering::Relaxed);
    let mut fallback: Option<(usize, u64)> = None;
    let mut best: Option<(usize, u64)> = None;

    for offset in 0..cpus.len() {
        let cpu_id = cpus[(start + offset) % cpus.len()];
        let score = cpu_load_score(cpu_id);

        if fallback
            .map(|(best_cpu, best_score)| {
                score < best_score
                    || (score == best_score
                        && cpu_better_for_preference(cpu_id, best_cpu, preference))
            })
            .unwrap_or(true)
        {
            fallback = Some((cpu_id, score));
        }

        if cpu_capacity(cpu_id) < min_capacity {
            continue;
        }

        if best
            .map(|(best_cpu, best_score)| {
                score < best_score
                    || (score == best_score
                        && cpu_better_for_preference(cpu_id, best_cpu, preference))
            })
            .unwrap_or(true)
        {
            best = Some((cpu_id, score));
        }
    }

    best.or(fallback).map(|(cpu_id, _)| cpu_id).unwrap_or(0)
}

fn select_target_cpu(task: Option<&Task>) -> usize {
    select_target_cpu_at(task, get_time_ns())
}

fn select_enqueue_cpu_for_task(task: &Task, requested_cpu: usize, now_ns: u64) -> usize {
    if let Some(pinned_cpu) = task.pinned_cpu {
        return pinned_cpu;
    }

    let selected = select_target_cpu_at(Some(task), now_ns);
    let selected_score = cpu_load_score(selected);
    if is_cpu_online(requested_cpu)
        && cpu_capacity(requested_cpu) >= task_min_cpu_capacity_at(Some(task), now_ns)
        && cpu_load_score(requested_cpu) <= selected_score
    {
        requested_cpu
    } else {
        selected
    }
}

fn task_can_run_on_cpu(task: &Task, cpu_id: usize, now_ns: u64) -> bool {
    if task
        .pinned_cpu
        .is_some_and(|pinned_cpu| pinned_cpu != cpu_id)
    {
        return false;
    }

    cpu_capacity(cpu_id) >= task_min_cpu_capacity_at(Some(task), now_ns)
}

fn steal_candidate_from_cpu(
    victim_cpu: usize,
    target_cpu: usize,
    now_ns: u64,
) -> Option<(usize, u64)> {
    let idle_id = IDLE_TASK_IDS[victim_cpu].load(Ordering::SeqCst);
    let queue = ready_queue(victim_cpu).lock();
    let mut weight = 0u64;
    let mut candidate = None;

    for &task_id in queue.iter() {
        if task_id == idle_id {
            continue;
        }

        weight = weight.saturating_add(runnable_task_weight(task_id));
        if candidate.is_some() {
            continue;
        }

        let Some(task) = TaskPool::get_task(task_id) else {
            continue;
        };
        if task.running_cpu.load(Ordering::SeqCst) != NO_CPU {
            continue;
        }
        if !matches!(task.state.load(Ordering::SeqCst), TaskState::Ready) {
            continue;
        }
        if !task_can_run_on_cpu(task, target_cpu, now_ns) {
            continue;
        }
        if migration_cooldown_active(task, now_ns, false) {
            continue;
        }

        candidate = Some(task_id);
    }

    candidate.map(|task_id| (task_id, weight))
}

fn remove_ready_task_from_cpu(cpu_id: usize, task_id: usize) -> bool {
    let mut queue = ready_queue(cpu_id).lock();
    let Some(index) = queue.iter().position(|&queued_id| queued_id == task_id) else {
        return false;
    };
    queue.remove(index).is_some()
}

fn steal_ready_task_for_cpu(target_cpu: usize, now_ns: u64) -> Option<usize> {
    let cpus = ONLINE_CPUS.lock();
    if cpus.len() <= 1 || !cpus.contains(&target_cpu) {
        return None;
    }

    let start = NEXT_CPU.fetch_add(1, Ordering::Relaxed);
    let mut scanned = 0usize;
    let mut best: Option<(usize, usize, u64)> = None;

    for offset in 0..cpus.len() {
        if scanned >= SCHED_STEAL_SCAN_LIMIT {
            break;
        }

        let victim_cpu = cpus[(start + offset) % cpus.len()];
        if victim_cpu == target_cpu {
            continue;
        }
        scanned = scanned.saturating_add(1);

        let Some((task_id, weight)) = steal_candidate_from_cpu(victim_cpu, target_cpu, now_ns)
        else {
            continue;
        };
        if weight == 0 {
            continue;
        }

        if best
            .map(|(_, _, best_weight)| weight > best_weight)
            .unwrap_or(true)
        {
            best = Some((victim_cpu, task_id, weight));
        }
    }
    drop(cpus);

    let (victim_cpu, task_id, _) = best?;
    if !remove_ready_task_from_cpu(victim_cpu, task_id) {
        return None;
    }

    let Some(task) = TaskPool::get_task(task_id) else {
        return None;
    };
    if !try_claim_ready_task(task, target_cpu) {
        if matches!(task.state.load(Ordering::SeqCst), TaskState::Ready)
            && task.running_cpu.load(Ordering::SeqCst) == NO_CPU
        {
            push_ready_task(victim_cpu, task_id);
        }
        return None;
    }

    record_work_steal(task, now_ns);
    Some(task_id)
}

fn select_wake_cpu_for_task(task: &Task, now_ns: u64) -> usize {
    if let Some(pinned_cpu) = task.pinned_cpu {
        return pinned_cpu;
    }

    let min_capacity = task_min_cpu_capacity_at(Some(task), now_ns);
    let selected = select_target_cpu_at(Some(task), now_ns);
    let selected_score = cpu_load_score(selected);
    let last = task.last_cpu.load(Ordering::SeqCst);
    if is_cpu_online(last)
        && cpu_capacity(last) >= min_capacity
        && cpu_load_score(last) <= selected_score
    {
        last
    } else {
        let current = get_cpu().get_cpuid();
        if is_cpu_online(current)
            && cpu_capacity(current) >= min_capacity
            && cpu_load_score(current) <= selected_score
        {
            current
        } else {
            selected
        }
    }
}

fn select_lower_capacity_cpu(task: &Task, current_cpu: usize, min_capacity: u32) -> Option<usize> {
    let cpus = ONLINE_CPUS.lock();
    let current_capacity = cpu_capacity(current_cpu);
    let preference = task.core_preference();
    let mut best: Option<(usize, u64)> = None;

    for &cpu_id in cpus.iter() {
        if cpu_id == current_cpu {
            continue;
        }

        let capacity = cpu_capacity(cpu_id);
        if capacity >= current_capacity || capacity < min_capacity {
            continue;
        }

        let score = cpu_load_score(cpu_id);
        if best
            .map(|(best_cpu, best_score)| {
                score < best_score
                    || (score == best_score
                        && (cpu_better_for_preference(cpu_id, best_cpu, preference)
                            || cpu_capacity(cpu_id) < cpu_capacity(best_cpu)))
            })
            .unwrap_or(true)
        {
            best = Some((cpu_id, score));
        }
    }

    best.map(|(cpu_id, _)| cpu_id)
}

fn migration_cooldown_active(task: &Task, now_ns: u64, record_skip: bool) -> bool {
    let last_migration_ns = task.sched_last_migration_ns();
    let active = last_migration_ns != 0
        && now_ns.saturating_sub(last_migration_ns) < SCHED_MIGRATION_COOLDOWN_NS;
    if active && record_skip {
        SCHED_MIGRATION_COOLDOWN_SKIPS.fetch_add(1, Ordering::SeqCst);
    }
    active
}

fn migration_target_for_task(
    task: &Task,
    current_cpu: usize,
    now_ns: u64,
    record_skip: bool,
) -> Option<usize> {
    if task.pinned_cpu.is_some() || !is_cpu_online(current_cpu) {
        return None;
    }

    let current_capacity = cpu_capacity(current_cpu);
    let required_capacity = task_min_cpu_capacity_at(Some(task), now_ns);

    if required_capacity > current_capacity {
        let target_cpu = select_target_cpu_at(Some(task), now_ns);
        let target_capacity = cpu_capacity(target_cpu);
        if target_cpu != current_cpu
            && target_capacity > current_capacity
            && target_capacity >= required_capacity
        {
            if migration_cooldown_active(task, now_ns, record_skip) {
                return None;
            }
            return Some(target_cpu);
        }
        return None;
    }

    if current_capacity > required_capacity.saturating_add(SCHED_DEMOTION_MARGIN) {
        let target_cpu = select_lower_capacity_cpu(task, current_cpu, required_capacity)?;
        if migration_cooldown_active(task, now_ns, record_skip) {
            return None;
        }
        return Some(target_cpu);
    }

    None
}

fn record_work_steal(task: &Task, now_ns: u64) {
    SCHED_WORK_STEALS.fetch_add(1, Ordering::SeqCst);
    task.mark_sched_migrated(now_ns);
}

fn record_scheduler_migration(task: &Task, from_cpu: usize, to_cpu: usize, now_ns: u64) {
    if from_cpu == to_cpu {
        return;
    }

    let from_capacity = cpu_capacity(from_cpu);
    let to_capacity = cpu_capacity(to_cpu);
    SCHED_MIGRATIONS_TOTAL.fetch_add(1, Ordering::SeqCst);
    if to_capacity > from_capacity {
        SCHED_MIGRATION_PROMOTIONS.fetch_add(1, Ordering::SeqCst);
    } else if to_capacity < from_capacity {
        SCHED_MIGRATION_DEMOTIONS.fetch_add(1, Ordering::SeqCst);
    }
    task.mark_sched_migrated(now_ns);
}

fn notify_remote_ready_task(target_cpu: usize, task_id: usize, label: &'static str) {
    if !is_cpu_online(target_cpu) || target_cpu == get_cpu().get_cpuid() {
        return;
    }

    let seq = DEBUG_ENQUEUE_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    DEBUG_REMOTE_ENQUEUE_TASK[target_cpu].store(encode_task_id(Some(task_id)), Ordering::SeqCst);
    DEBUG_REMOTE_ENQUEUE_FROM_CPU[target_cpu].store(get_cpu().get_cpuid(), Ordering::SeqCst);
    DEBUG_REMOTE_ENQUEUE_SEQ[target_cpu].store(seq, Ordering::SeqCst);
    if DEBUG_SMP_TASK_FLOW {
        println!(
            "[SMPDBG {}] seq={} from_cpu={} target_cpu={} task={} name={} ready_len={}",
            label,
            seq,
            get_cpu().get_cpuid(),
            target_cpu,
            task_id,
            debug_task_name(task_id),
            ready_queue(target_cpu).lock().len(),
        );
    }
    crate::arch::send_reschedule_ipi(target_cpu);
}

pub fn for_each_online_cpu<F: FnMut(usize)>(mut f: F) {
    let cpus = ONLINE_CPUS.lock();
    for &cpu_id in cpus.iter() {
        f(cpu_id);
    }
}

pub fn num_online_cpus() -> usize {
    ONLINE_CPUS.lock().len()
}

/// Select a target CPU for a new task.
///
/// This uses registered CPU capacity and the task's heterogeneous scheduling
/// hint. If no CPU is online yet, CPU 0 is returned as the boot-time fallback.
///
/// # Arguments
///
/// * `task` - Task to place.
///
/// # Returns
///
/// Selected scheduler CPU ID.
pub fn select_cpu_for_task(task: &Task) -> usize {
    if let Some(pinned_cpu) = task.pinned_cpu {
        return pinned_cpu;
    }

    select_target_cpu(Some(task))
}

/// Select a target CPU when no task-specific placement data is available.
///
/// # Returns
///
/// Selected scheduler CPU ID.
pub fn select_cpu() -> usize {
    select_target_cpu(None)
}

/// Return whether a scheduler CPU is online.
///
/// # Arguments
///
/// * `cpu_id` - Scheduler CPU ID.
///
/// # Returns
///
/// `true` if the CPU is registered as online.
pub fn is_cpu_online(cpu_id: usize) -> bool {
    ONLINE_CPUS.lock().contains(&cpu_id)
}

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
fn encode_prev_task(task_id: usize, generation: usize) -> usize {
    debug_assert!(task_id <= TASK_ID_MASK);
    debug_assert_ne!(generation, 0);
    (generation << MAX_TASKS.ilog2()) | task_id
}

#[inline]
fn decode_prev_task(encoded: usize) -> Option<(usize, usize)> {
    if encoded == 0 {
        return None;
    }

    let task_id = encoded & TASK_ID_MASK;
    let generation = encoded >> MAX_TASKS.ilog2();
    if task_id == 0 || generation == 0 {
        return None;
    }

    Some((task_id, generation))
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
pub fn push_ready_task(cpu_id: usize, task_id: usize) {
    let mut queue = ready_queue(cpu_id).lock();
    if !queue.contains(&task_id) {
        queue.push_back(task_id);
    }
}

pub fn mark_blocked(task_id: usize) {
    let mut queue = BLOCKED_QUEUE.lock();
    if !queue.contains(&task_id) {
        queue.push_back(task_id);
    }
}

pub fn unmark_blocked(task_id: usize) {
    let mut queue = BLOCKED_QUEUE.lock();
    if let Some(pos) = queue.iter().position(|&id| id == task_id) {
        queue.remove(pos);
    }
}

pub fn finalize_zombie(task_id: usize, parent_id: Option<usize>) {
    {
        let mut zombie_queue = ZOMBIE_QUEUE.lock();
        if !zombie_queue.contains(&task_id) {
            zombie_queue.push_back(task_id);
        }
    }
    wake_task_waiters(task_id);
    if let Some(parent_id) = parent_id {
        wake_parent_waiters(parent_id);
        if let Some(parent) = get_task_by_id(parent_id) {
            let parent_thread_group = parent.get_thread_group_id();
            for thread_id in get_all_task_ids() {
                if thread_id == parent_id {
                    continue;
                }
                if let Some(thread) = get_task_by_id(thread_id) {
                    if thread.get_thread_group_id() == parent_thread_group {
                        wake_parent_waiters(thread_id);
                    }
                }
            }
        }
    }
}

fn pick_next(cpu: &Arch) -> (Option<usize>, Option<usize>) {
    let _irq_guard = IrqGuard::new();
    let cpu_id = cpu.get_cpuid();
    release_deferred_prev(cpu_id);

    let old_id = current_task_id(cpu_id);
    let old_task = old_id.and_then(TaskPool::get_task);

    let mut next_id: Option<usize> = None;
    'outer: loop {
        let candidate = { ready_queue(cpu_id).lock().pop_front() };
        match candidate {
            Some(task_id) => {
                let Some(task) = TaskPool::get_task(task_id) else {
                    continue;
                };
                if task.pinned_cpu.is_some_and(|p| p != cpu_id) {
                    push_ready_task(task.pinned_cpu.unwrap(), task_id);
                    continue;
                }
                match task.state.load(Ordering::SeqCst) {
                    TaskState::NotInitialized => {
                        panic!("Task must be initialized before scheduling")
                    }
                    TaskState::Zombie => {
                        finalize_zombie(task.get_id(), task.get_parent_id());
                        continue;
                    }
                    TaskState::Terminated => {
                        continue;
                    }
                    TaskState::Blocked(blocked) => {
                        let _ = blocked;
                        continue;
                    }
                    TaskState::Ready | TaskState::Running => {
                        if task.running_cpu.load(Ordering::SeqCst) != NO_CPU {
                            continue;
                        }
                        if try_claim_ready_task(task, cpu_id) {
                            if DEBUG_SMP_TASK_FLOW {
                                let (expected_task, _from_cpu, seq) =
                                    debug_remote_enqueue_snapshot(cpu_id);
                                if expected_task.is_some() {
                                    println!(
                                        "[SMPDBG pick-selected] cpu={} old={:?} next={} next_name={} expected_task={:?} expected_match={} seq={}",
                                        cpu_id,
                                        old_id,
                                        task_id,
                                        task.name.read().as_str(),
                                        expected_task,
                                        expected_task == Some(task_id),
                                        seq,
                                    );
                                }
                            }
                            next_id = Some(task_id);
                            break 'outer;
                        }
                        continue;
                    }
                }
            }
            None => {
                break 'outer;
            }
        }
    }

    if next_id.is_none() {
        let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
        let current_is_idle = old_id
            .map(|task_id| idle_id != 0 && task_id == idle_id)
            .unwrap_or(true);
        if current_is_idle {
            next_id = steal_ready_task_for_cpu(cpu_id, get_time_ns());
        }
    }

    if next_id.is_none() {
        if let (Some(oid), Some(ot)) = (old_id, old_task) {
            if ot.running_cpu.load(Ordering::SeqCst) == cpu_id
                && matches!(
                    ot.state.load(Ordering::SeqCst),
                    TaskState::Running | TaskState::Ready
                )
            {
                if migration_target_for_task(ot, cpu_id, get_time_ns(), false).is_some() {
                    let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
                    if idle_id != 0
                        && oid != idle_id
                        && let Some(idle_task) = TaskPool::get_task(idle_id)
                        && try_claim_ready_task(idle_task, cpu_id)
                    {
                        next_id = Some(idle_id);
                    }
                }
                if next_id.is_none() {
                    ot.state.store(TaskState::Running, Ordering::SeqCst);
                    ot.time_slice.store(
                        ot.default_time_slice.load(Ordering::SeqCst),
                        Ordering::SeqCst,
                    );
                    set_current_task_id(cpu_id, Some(oid));
                    return (old_id, Some(oid));
                }
                // Fall through to the normal context-switch path. The old task
                // will be released and migrated from release_deferred_prev().
            }
        }
    }

    if next_id.is_none() {
        let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
        if idle_id != 0 {
            if let Some(task) = TaskPool::get_task(idle_id) {
                if try_claim_ready_task(task, cpu_id) {
                    next_id = Some(idle_id);
                }
            }
        }
    }

    let Some(next_id) = next_id else {
        account_task_switch(cpu_id, old_id, None);
        set_current_task_id(cpu_id, None);
        return (old_id, None);
    };

    if let (Some(oid), Some(ot), Some(nid)) = (old_id, old_task, Some(next_id)) {
        if oid != nid {
            match ot.state.load(Ordering::SeqCst) {
                TaskState::Running | TaskState::Ready => {
                    let _ = ot.state.compare_exchange(
                        TaskState::Running,
                        TaskState::Ready,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    );
                }
                TaskState::Zombie => finalize_zombie(oid, ot.get_parent_id()),
                TaskState::Terminated | TaskState::Blocked(_) | TaskState::NotInitialized => {}
            }
            if let Some(generation) = get_task_pool().task_generation(oid) {
                SCHEDULE_PREV_TASK[cpu_id]
                    .store(encode_prev_task(oid, generation), Ordering::SeqCst);
            }
        }
    }

    account_task_switch(cpu_id, old_id, Some(next_id));
    set_current_task_id(cpu_id, Some(next_id));
    if DEBUG_SMP_TASK_FLOW {
        let (expected_task, from_cpu, seq) = debug_remote_enqueue_snapshot(cpu_id);
        if expected_task.is_some() && expected_task != Some(next_id) {
            println!(
                "[SMPDBG pick-mismatch] cpu={} old={:?} next={} next_name={} expected_task={:?} expected_from={} seq={}",
                cpu_id,
                old_id,
                next_id,
                debug_task_name(next_id),
                expected_task,
                from_cpu,
                seq,
            );
        }
    }
    (old_id, Some(next_id))
}

pub fn add_task(task: Task, cpu_id: usize) -> usize {
    let _irq_guard = IrqGuard::new();
    let task_id = register_task(task);
    enqueue_task(task_id, cpu_id);
    task_id
}

/// Add a task to the global task pool without making it runnable yet.
///
/// This is useful for fork/clone paths that need the allocated task ID before
/// finalizing parent/child relationships or ABI-specific setup.  The caller
/// must eventually call [`enqueue_task`] to make the task visible to the
/// scheduler.
pub fn register_task(task: Task) -> usize {
    let task_id = match get_task_pool().add_task(task) {
        Ok(id) => id,
        Err(e) => panic!("Failed to add task: {}", e),
    };
    task_id
}

/// Make a registered task runnable on the specified CPU.
pub fn enqueue_task(task_id: usize, cpu_id: usize) {
    let _irq_guard = IrqGuard::new();
    let current_cpu = get_cpu().get_cpuid();
    let target_cpu = TaskPool::get_task(task_id)
        .map(|task| select_enqueue_cpu_for_task(task, cpu_id, get_time_ns()))
        .unwrap_or(cpu_id);
    let seq = DEBUG_ENQUEUE_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    if let Some(task) = TaskPool::get_task(task_id) {
        task.last_cpu.store(target_cpu, Ordering::SeqCst);
    }
    push_ready_task(target_cpu, task_id);
    if DEBUG_SMP_TASK_FLOW {
        println!(
            "[SMPDBG enqueue] seq={} from_cpu={} target_cpu={} task={} name={} remote={} ready_len={}",
            seq,
            current_cpu,
            target_cpu,
            task_id,
            debug_task_name(task_id),
            target_cpu != current_cpu,
            ready_queue(target_cpu).lock().len(),
        );
    }
    if is_cpu_online(target_cpu) && target_cpu != get_cpu().get_cpuid() {
        DEBUG_REMOTE_ENQUEUE_TASK[target_cpu]
            .store(encode_task_id(Some(task_id)), Ordering::SeqCst);
        DEBUG_REMOTE_ENQUEUE_FROM_CPU[target_cpu].store(current_cpu, Ordering::SeqCst);
        DEBUG_REMOTE_ENQUEUE_SEQ[target_cpu].store(seq, Ordering::SeqCst);
        if DEBUG_SMP_TASK_FLOW {
            println!(
                "[SMPDBG ipi-send] seq={} from_cpu={} target_cpu={} task={} name={} ready_len={}",
                seq,
                current_cpu,
                target_cpu,
                task_id,
                debug_task_name(task_id),
                ready_queue(target_cpu).lock().len(),
            );
        }
        crate::arch::send_reschedule_ipi(target_cpu);
    }
}

/// Called every timer tick. Decrements the current task's time_slice.
/// If time_slice reaches 0, triggers a reschedule.
pub fn sched_on_tick(cpu_id: usize, trapframe: &mut Trapframe) {
    let _tick = DEBUG_TICK.fetch_add(1, Ordering::Relaxed);
    update_cpu_util_avg(cpu_id);
    let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
    if let Some(task_id) = current_task_id(cpu_id)
        && task_id != idle_id
        && let Some(task) = TaskPool::get_task(task_id)
    {
        task.account_sched_util_running(get_time_ns());
    }
    crate::device::cpufreq::on_scheduler_tick(cpu_id);

    if let Some(task_id) = current_task_id(cpu_id) {
        if let Some(task) = TaskPool::get_task(task_id) {
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
    let idle_task_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);

    if let Some(next_task_id) = next_task_id {
        if current_task_id != Some(next_task_id) {
            if current_task_id == Some(idle_task_id) && next_task_id != idle_task_id {
                note_idle_to_user_handoff(cpu_id, next_task_id);
            }

            if let Some(current_task_id) = current_task_id {
                if let Some(current_task) = TaskPool::get_task(current_task_id) {
                    current_task.last_cpu.store(cpu_id, Ordering::SeqCst);
                    current_task.vcpu.lock().store(trapframe);

                    kernel_context_switch(cpu_id, current_task_id, next_task_id);

                    current_task.vcpu.lock().switch(trapframe);
                    set_next_mode(current_task.vcpu.lock().get_mode());
                } else {
                    set_current_task_id(cpu_id, None);
                    if let Some(next_task) = TaskPool::get_task(next_task_id) {
                        setup_task_execution(get_cpu(), next_task);
                        arch_switch_to_user(next_task.get_trapframe());
                    }
                }
            } else {
                if let Some(next_task) = TaskPool::get_task(next_task_id) {
                    setup_task_execution(get_cpu(), next_task);
                    arch_switch_to_user(next_task.get_trapframe());
                }
            }
        }
    }

    process_pending_events_before_user_return(trapframe);
}

/// Process events that must be delivered before returning to userspace.
pub fn process_pending_events_before_user_return(trapframe: &mut Trapframe) {
    let cpu_id = get_cpu().get_cpuid();
    let Some(current_task) = current_task(cpu_id) else {
        return;
    };

    match current_task.process_pending_events() {
        Ok(EventProcessOutcome::NeedReschedule | EventProcessOutcome::Exited) => {
            schedule(trapframe)
        }
        Ok(
            EventProcessOutcome::Continue
            | EventProcessOutcome::Pending
            | EventProcessOutcome::UserHandlerArmed,
        ) => {}
        Err(_) => {}
    }
}

/// Start the scheduler and return the first runnable task ID (if any).
fn idle_loop() -> ! {
    loop {
        idle();
    }
}

fn idle_entry() {
    register_online_cpu(get_cpu().get_cpuid());
    idle_loop()
}

pub fn spawn_idle_task(cpu_id: usize) -> usize {
    let name = alloc::format!("idle{}", cpu_id);
    let mut task = new_kernel_task(name, 0, idle_entry);
    task.pinned_cpu = Some(cpu_id);
    task.init();
    // Idle task is used as a fallback in pick_next when no real tasks are available.
    let task_id = match get_task_pool().add_task(task) {
        Ok(id) => id,
        Err(e) => panic!("Failed to add idle task: {}", e),
    };
    IDLE_TASK_IDS[cpu_id].store(task_id, Ordering::SeqCst);
    task_id
}

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

pub fn first_switch_to_kernel_task(task_id: usize) -> ! {
    let Some(task) = TaskPool::get_task(task_id) else {
        panic!("Kernel task {} not found", task_id);
    };

    let mut boot_context = crate::arch::context::KernelContext::new();
    let to_ctx_ptr = task.kernel_context.as_mut_ptr() as *const crate::arch::context::KernelContext;

    // SAFETY: `task_id` is the current runnable task selected by `start_scheduler`,
    // and `to_ctx_ptr` points to its initialized kernel context. The scheduler
    // has claimed this task before the first switch, so no other CPU mutates the
    // target context concurrently. `boot_context` is a temporary save area for
    // the boot/AP context and remains valid here.
    unsafe {
        crate::arch::switch::switch_to(&mut boot_context as *mut _, to_ctx_ptr);
    }

    loop {
        idle();
    }
}

pub fn current_task(cpu_id: usize) -> Option<&'static Task> {
    current_task_id(cpu_id).and_then(TaskPool::get_task)
}

pub fn current_task_id(cpu_id: usize) -> Option<usize> {
    assert_valid_cpu_id(cpu_id);
    decode_task_id(CURRENT_TASK_IDS[cpu_id].load(Ordering::SeqCst))
}

pub fn current_task_is_idle(cpu_id: usize) -> bool {
    let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
    idle_id != 0 && current_task_id(cpu_id) == Some(idle_id)
}

pub fn has_ready_tasks(cpu_id: usize) -> bool {
    assert_valid_cpu_id(cpu_id);
    !READY_QUEUES[cpu_id].lock().is_empty()
}

pub fn get_task_by_id(task_id: usize) -> Option<&'static Task> {
    TaskPool::get_task(task_id)
}

pub fn wake_task(task_id: usize) -> bool {
    let target_cpu = {
        let Some(task) = TaskPool::get_task(task_id) else {
            return false;
        };
        select_wake_cpu_for_task(task, get_time_ns())
    };
    wake_task_on(task_id, target_cpu)
}

pub fn wake_task_on(task_id: usize, target_cpu: usize) -> bool {
    let _irq_guard = IrqGuard::new();
    let Some(task) = TaskPool::get_task(task_id) else {
        return false;
    };

    let mut state = task.state.load(Ordering::SeqCst);
    loop {
        match state {
            TaskState::Blocked(_) => {}
            _ => {
                return false;
            }
        }
        match task.state.compare_exchange(
            state,
            TaskState::Ready,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => break,
            Err(actual) => state = actual,
        }
    }

    unmark_blocked(task_id);
    if DEBUG_SMP_TASK_FLOW {
        println!(
            "[SMPDBG wake-task-on] current_cpu={} target_cpu={} task={} name={} state=Ready enqueue",
            get_cpu().get_cpuid(),
            target_cpu,
            task_id,
            debug_task_name(task_id),
        );
    }
    push_ready_task(target_cpu, task_id);

    if target_cpu != get_cpu().get_cpuid() {
        let seq = DEBUG_ENQUEUE_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
        DEBUG_REMOTE_ENQUEUE_TASK[target_cpu]
            .store(encode_task_id(Some(task_id)), Ordering::SeqCst);
        DEBUG_REMOTE_ENQUEUE_FROM_CPU[target_cpu].store(get_cpu().get_cpuid(), Ordering::SeqCst);
        DEBUG_REMOTE_ENQUEUE_SEQ[target_cpu].store(seq, Ordering::SeqCst);
        if DEBUG_SMP_TASK_FLOW {
            println!(
                "[SMPDBG wake-ipi-send] seq={} from_cpu={} target_cpu={} task={} name={} ready_len={}",
                seq,
                get_cpu().get_cpuid(),
                target_cpu,
                task_id,
                debug_task_name(task_id),
                ready_queue(target_cpu).lock().len(),
            );
        }
        crate::arch::send_reschedule_ipi(target_cpu);
    }

    true
}

pub fn cleanup_zombie(task_id: usize) {
    {
        let mut zombie_queue = ZOMBIE_QUEUE.lock();
        if let Some(pos) = zombie_queue.iter().position(|&id| id == task_id) {
            zombie_queue.remove(pos);
        }
    }

    let _ = get_task_pool().remove_task(task_id);
}

pub fn remove_from_ready_queues(task_id: usize) {
    let _irq_guard = IrqGuard::new();

    for_each_online_cpu(|cpu_id| {
        let mut queue = ready_queue(cpu_id).lock();
        while let Some(pos) = queue.iter().position(|&id| id == task_id) {
            queue.remove(pos);
        }
    });
}

pub fn remove_from_zombie_queue(task_id: usize) {
    let mut zombie_queue = ZOMBIE_QUEUE.lock();
    while let Some(pos) = zombie_queue.iter().position(|&id| id == task_id) {
        zombie_queue.remove(pos);
    }
}

pub fn remove_task_from_queues(task_id: usize) {
    remove_from_ready_queues(task_id);
    remove_from_zombie_queue(task_id);
    unmark_blocked(task_id);
}

/// Get IDs of all tasks across scheduler-visible queues/state.
pub fn get_all_task_ids() -> alloc::vec::Vec<usize> {
    let _irq_guard = IrqGuard::new();
    let mut ids = alloc::vec::Vec::new();

    for_each_online_cpu(|cpu_id| {
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
    });

    let zombie_queue = ZOMBIE_QUEUE.lock();
    for &task_id in zombie_queue.iter() {
        if !ids.contains(&task_id) {
            ids.push(task_id);
        }
    }

    let blocked_queue = BLOCKED_QUEUE.lock();
    for &task_id in blocked_queue.iter() {
        if !ids.contains(&task_id) {
            ids.push(task_id);
        }
    }

    ids
}

/// Perform kernel context switch between tasks.
fn kernel_context_switch(cpu_id: usize, from_task_id: usize, to_task_id: usize) {
    if from_task_id != to_task_id {
        if DEBUG_SMP_TASK_FLOW {
            let (expected_task, from_cpu, seq) = debug_remote_enqueue_snapshot(cpu_id);
            if expected_task.is_some() {
                println!(
                    "[SMPDBG kctx-enter] cpu={} from={} from_name={} to={} to_name={} expected_task={:?} expected_from={} expected_match={} seq={}",
                    cpu_id,
                    from_task_id,
                    debug_task_name(from_task_id),
                    to_task_id,
                    debug_task_name(to_task_id),
                    expected_task,
                    from_cpu,
                    expected_task == Some(to_task_id),
                    seq,
                );
            }
        }
        let mut from_ctx_ptr: *mut crate::arch::context::KernelContext = core::ptr::null_mut();
        let mut to_ctx_ptr: *const crate::arch::context::KernelContext = core::ptr::null();

        if let Some(from_task) = TaskPool::get_task(from_task_id) {
            // The currently running task is owned by this CPU, and its kernel
            // context is the save target of the low-level switch code. Do not
            // take `kernel_context`'s spin mutex here: a task can be switched
            // out while code higher in its kernel stack still owns unrelated
            // locks, and blocking in the scheduler makes SMP remote wakeups
            // deadlock-prone. The `running_cpu` ownership token provides the
            // required exclusion for context switching.
            from_ctx_ptr = from_task.kernel_context.as_mut_ptr();

            #[cfg(feature = "user-fpu")]
            crate::arch::fpu::kernel_switch_out_user_fpu(&mut *from_task.vcpu.lock());

            #[cfg(feature = "user-vector")]
            crate::arch::fpu::kernel_switch_out_user_vector(
                cpu_id,
                from_task_id,
                &mut *from_task.vcpu.lock(),
            );
        }
        if let Some(to_task) = TaskPool::get_task(to_task_id) {
            // `pick_next()` successfully claimed `to_task.running_cpu` for this
            // CPU before reaching this point, so no other CPU may save/restore
            // this kernel context concurrently. Read it locklessly for the same
            // reason as `from_ctx_ptr` above.
            to_ctx_ptr = to_task.kernel_context.as_mut_ptr() as *const _;
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
                if DEBUG_SMP_TASK_FLOW {
                    let (expected_task, _from_cpu, seq) = debug_remote_enqueue_snapshot(cpu_id);
                    if expected_task.is_some() {
                        println!(
                            "[SMPDBG kctx-switch-to] cpu={} from={} to={} to_name={} expected_task={:?} expected_match={} seq={}",
                            cpu_id,
                            from_task_id,
                            to_task_id,
                            debug_task_name(to_task_id),
                            expected_task,
                            expected_task == Some(to_task_id),
                            seq,
                        );
                    }
                }
                crate::arch::switch::switch_to(from_ctx_ptr, to_ctx_ptr);
            }

            let resumed_cpu_id = get_cpu().get_cpuid();

            if DEBUG_SMP_TASK_FLOW {
                let (expected_task, _from_cpu, seq) = debug_remote_enqueue_snapshot(resumed_cpu_id);
                if expected_task.is_some() {
                    println!(
                        "[SMPDBG kctx-resume] from_cpu={} resumed_cpu={} resumed={} resumed_name={} switched_from={} expected_task={:?} seq={}",
                        cpu_id,
                        resumed_cpu_id,
                        from_task_id,
                        debug_task_name(from_task_id),
                        to_task_id,
                        expected_task,
                        seq,
                    );
                }
            }

            // The saved kernel context may resume on a different CPU after
            // scheduler migration. Release the deferred previous task for the
            // CPU we are actually running on now, not for the CPU that saved
            // this stack frame before the switch.
            release_deferred_prev(resumed_cpu_id);

            #[cfg(feature = "hypervisor")]
            {
                guest_vcpu_switch_data.restore();
                hypervisor_switch_data.restore();
            }

            let cpu = get_cpu();
            saved_arch_cpu_state.restore(cpu);
            set_trapvector(saved_trapvector);
            if let Some(from_task) = TaskPool::get_task(from_task_id) {
                #[cfg(feature = "user-fpu")]
                crate::arch::fpu::kernel_switch_in_user_fpu(&mut *from_task.vcpu.lock());
            }
        }
    }
}

/// Setup task execution by configuring hardware and user context.
pub fn setup_task_execution(cpu: &mut Arch, task: &Task) {
    if DEBUG_SMP_TASK_FLOW {
        let cpu_id = cpu.get_cpuid();
        let (expected_task, _from_cpu, seq) = debug_remote_enqueue_snapshot(cpu_id);
        if expected_task.is_some() {
            let vcpu = task.vcpu.lock();
            println!(
                "[SMPDBG setup-task-exec] cpu={} task={} name={} mode={:?} pc={:#x} expected_task={:?} expected_match={} seq={}",
                cpu_id,
                task.get_id(),
                task.name.read().as_str(),
                vcpu.get_mode(),
                vcpu.get_pc(),
                expected_task,
                expected_task == Some(task.get_id()),
                seq,
            );
        }
    }

    let sp = if let Some((_slot, base)) = task.get_kernel_stack_window_base() {
        (base + crate::environment::PAGE_SIZE + crate::environment::TASK_KERNEL_STACK_SIZE) as u64
    } else {
        task.get_kernel_stack_bottom_paddr()
    };

    crate::arch::set_arch(crate::vm::get_trampoline_arch(cpu.get_cpuid()));
    cpu.set_kernel_stack(sp);

    let task_mode = task.vcpu.lock().get_mode();
    let trapframe = task.get_trapframe();
    task.vcpu.lock().switch(trapframe);

    cpu.set_trap_handler(get_user_trap_handler());
    cpu.set_next_address_space(task.vm_manager.get_asid());
    set_next_mode(task_mode);
    set_trapvector(get_trampoline_trap_vector());
}

/// Reset the scheduler to initial state (test-only).
#[cfg(test)]
pub fn reset() {
    for cpu_id in 0..MAX_NUM_CPUS {
        ready_queue(cpu_id).lock().clear();
        set_current_task_id(cpu_id, None);
        SCHEDULE_PREV_TASK[cpu_id].store(0, Ordering::SeqCst);
        IDLE_TASK_IDS[cpu_id].store(0, Ordering::SeqCst);
        PENDING_IDLE_TO_USER_TRAP_TASK[cpu_id].store(0, Ordering::SeqCst);
        CPU_CORE_CLASSES[cpu_id].store(CpuCoreClass::Balanced as u8, Ordering::SeqCst);
        CPU_CAPACITIES[cpu_id].store(DEFAULT_CPU_CAPACITY, Ordering::SeqCst);
        CPU_TOPOLOGY_DOMAINS[cpu_id].store(INVALID_CPU_TOPOLOGY_DOMAIN, Ordering::SeqCst);
        DEBUG_REMOTE_ENQUEUE_TASK[cpu_id].store(0, Ordering::SeqCst);
        DEBUG_REMOTE_ENQUEUE_FROM_CPU[cpu_id].store(NO_CPU, Ordering::SeqCst);
        DEBUG_REMOTE_ENQUEUE_SEQ[cpu_id].store(0, Ordering::SeqCst);
    }
    NEXT_CPU.store(0, Ordering::SeqCst);
    DEBUG_TICK.store(0, Ordering::SeqCst);
    DEBUG_ENQUEUE_SEQ.store(0, Ordering::SeqCst);
    SCHED_MIGRATIONS_TOTAL.store(0, Ordering::SeqCst);
    SCHED_MIGRATION_PROMOTIONS.store(0, Ordering::SeqCst);
    SCHED_MIGRATION_DEMOTIONS.store(0, Ordering::SeqCst);
    SCHED_MIGRATION_COOLDOWN_SKIPS.store(0, Ordering::SeqCst);
    SCHED_WORK_STEALS.store(0, Ordering::SeqCst);
    TOTAL_BUSY_CPU_TIME_NS.store(0, Ordering::SeqCst);
    TOTAL_IDLE_CPU_TIME_NS.store(0, Ordering::SeqCst);
    ONLINE_CPUS.lock().clear();
    ZOMBIE_QUEUE.lock().clear();
    BLOCKED_QUEUE.lock().clear();
    get_task_pool().reset();
}

pub fn make_test_tasks() {
    println!("Making test tasks...");
    let task0 = new_kernel_task("Task0".to_string(), 0, || {
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

    let task1 = new_kernel_task("Task1".to_string(), 0, || {
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

    let task2 = new_kernel_task("Task2".to_string(), 0, || {
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
    use crate::task::{TaskCorePreference, TaskType};

    use super::*;

    #[test_case]
    fn test_add_task() {
        reset();
        let task = Task::new("TestTask".to_string(), 1, TaskType::Kernel);
        add_task(task, 0);
        assert_eq!(READY_QUEUES[0].lock().len(), 1);
    }

    #[test_case]
    fn test_register_cpu_topology() {
        reset();
        register_cpu_topology(0, CpuCoreClass::Efficiency, 0).unwrap();
        let topology = cpu_topology(0).unwrap();
        assert_eq!(topology.cpu_id, 0);
        assert_eq!(topology.core_class, CpuCoreClass::Efficiency);
        assert_eq!(
            topology.capacity,
            CpuCoreClass::Efficiency.default_capacity()
        );
        assert_eq!(topology.domain_id, None);
        assert_eq!(topology.domain_cpus_mask, 0);
    }

    #[test_case]
    fn test_register_cpu_topology_domain() {
        reset();
        register_cpu_topology(0, CpuCoreClass::Efficiency, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Performance, 0).unwrap();
        register_cpu_topology(2, CpuCoreClass::Performance, 0).unwrap();
        register_online_cpu(0);
        register_online_cpu(1);
        register_online_cpu(2);

        register_cpu_topology_domain(0, 0xa).unwrap();
        register_cpu_topology_domain(2, 0xa).unwrap();
        register_cpu_topology_domain(1, 0xd).unwrap();

        let topology = cpu_topology(0).unwrap();
        assert_eq!(topology.domain_id, Some(0xa));
        assert_eq!(topology.domain_cpus_mask, 0b101);
        assert_eq!(cpu_topology_domain_online_mask(0xa), 0b101);
        assert_eq!(cpu_topology_domain_online_mask(0xd), 0b010);
        assert_eq!(online_cpu_mask(), 0b111);
    }

    #[test_case]
    fn test_select_cpu_for_performance_task_prefers_big_core() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Efficiency, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Performance, 0).unwrap();

        let task = Task::new("PerfTask".to_string(), 1, TaskType::Kernel);
        task.set_core_preference(TaskCorePreference::Performance);

        assert_eq!(select_cpu_for_task(&task), 1);
    }

    #[test_case]
    fn test_select_cpu_for_efficiency_task_prefers_little_core_on_tie() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Performance, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Efficiency, 0).unwrap();

        let task = Task::new("EffTask".to_string(), 1, TaskType::Kernel);
        task.set_core_preference(TaskCorePreference::Efficiency);

        assert_eq!(select_cpu_for_task(&task), 1);
    }

    #[test_case]
    fn test_select_cpu_for_light_any_task_prefers_efficiency_on_tie() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Performance, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Efficiency, 0).unwrap();

        let task = Task::new("LightTask".to_string(), 1, TaskType::Kernel);

        assert_eq!(select_cpu_for_task(&task), 1);
    }

    #[test_case]
    fn test_select_cpu_for_requested_heavy_task_prefers_capacity() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Efficiency, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Performance, 0).unwrap();

        let task = Task::new("HeavyTask".to_string(), 1, TaskType::Kernel);
        task.set_sched_util_min(SCHED_UTIL_SCALE).unwrap();

        assert_eq!(select_cpu_for_task(&task), 1);
    }

    #[test_case]
    fn test_enqueue_prefers_idle_cpu_over_requested_busy_cpu() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Balanced, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Balanced, 0).unwrap();

        let first_id = register_task(Task::new("FirstTask".to_string(), 1, TaskType::Kernel));
        enqueue_task(first_id, 0);
        assert!(ready_queue(0).lock().contains(&first_id));

        let second_id = register_task(Task::new("SecondTask".to_string(), 1, TaskType::Kernel));
        enqueue_task(second_id, 0);
        assert!(ready_queue(1).lock().contains(&second_id));
    }

    #[test_case]
    fn test_idle_cpu_steals_ready_task_from_busy_queue() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Balanced, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Balanced, 0).unwrap();

        let task_id = register_task(Task::new("StealableTask".to_string(), 1, TaskType::Kernel));
        let task = TaskPool::get_task(task_id).unwrap();
        task.state.store(TaskState::Ready, Ordering::SeqCst);
        push_ready_task(0, task_id);

        assert_eq!(steal_ready_task_for_cpu(1, 20_000_000), Some(task_id));
        assert!(!ready_queue(0).lock().contains(&task_id));
        assert_eq!(task.running_cpu.load(Ordering::SeqCst), 1);
        assert_eq!(task.last_cpu.load(Ordering::SeqCst), 1);

        let stats = scheduler_migration_stats();
        assert_eq!(stats.total, 0);
        assert_eq!(stats.work_steals, 1);
    }

    #[test_case]
    fn test_idle_cpu_does_not_steal_recently_migrated_task() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Balanced, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Balanced, 0).unwrap();

        let task_id = register_task(Task::new("CoolingTask".to_string(), 1, TaskType::Kernel));
        let task = TaskPool::get_task(task_id).unwrap();
        task.state.store(TaskState::Ready, Ordering::SeqCst);
        task.mark_sched_migrated(10_000_000);
        push_ready_task(0, task_id);

        assert_eq!(steal_ready_task_for_cpu(1, 20_000_000), None);
        assert!(ready_queue(0).lock().contains(&task_id));

        let stats = scheduler_migration_stats();
        assert_eq!(stats.total, 0);
        assert_eq!(stats.work_steals, 0);
        assert_eq!(stats.cooldown_skips, 0);
    }

    #[test_case]
    fn test_migration_target_promotes_over_capacity_task() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Efficiency, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Performance, 0).unwrap();

        let task = Task::new("MigratingTask".to_string(), 1, TaskType::Kernel);
        task.set_sched_util_min(SCHED_UTIL_SCALE).unwrap();

        assert_eq!(
            migration_target_for_task(&task, 0, 20_000_000, false),
            Some(1)
        );
    }

    #[test_case]
    fn test_migration_cooldown_skip_counts_when_target_exists() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Efficiency, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Performance, 0).unwrap();

        let task = Task::new("CoolingPromotionTask".to_string(), 1, TaskType::Kernel);
        task.set_sched_util_min(SCHED_UTIL_SCALE).unwrap();
        task.mark_sched_migrated(10_000_000);

        assert_eq!(migration_target_for_task(&task, 0, 20_000_000, true), None);
        assert_eq!(scheduler_migration_stats().cooldown_skips, 1);
    }

    #[test_case]
    fn test_migration_target_demotes_low_util_task() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Performance, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Efficiency, 0).unwrap();

        let task = Task::new("LowUtilTask".to_string(), 1, TaskType::Kernel);

        assert_eq!(
            migration_target_for_task(&task, 0, 10_000_000, false),
            Some(1)
        );
    }

    #[test_case]
    fn test_migration_cooldown_skip_requires_target_cpu() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Balanced, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Balanced, 0).unwrap();

        let task = Task::new("NoTargetTask".to_string(), 1, TaskType::Kernel);
        task.mark_sched_migrated(10_000_000);

        assert_eq!(migration_target_for_task(&task, 0, 20_000_000, true), None);
        assert_eq!(scheduler_migration_stats().cooldown_skips, 0);
    }

    #[test_case]
    fn test_deferred_release_migrates_ready_task() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Efficiency, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Performance, 0).unwrap();

        let task_id = register_task(Task::new(
            "DeferredMigrationTask".to_string(),
            1,
            TaskType::Kernel,
        ));
        let task = TaskPool::get_task(task_id).unwrap();
        task.set_sched_util_min(SCHED_UTIL_SCALE).unwrap();
        task.state.store(TaskState::Ready, Ordering::SeqCst);
        task.running_cpu.store(0, Ordering::SeqCst);
        let generation = get_task_pool().task_generation(task_id).unwrap();
        SCHEDULE_PREV_TASK[0].store(encode_prev_task(task_id, generation), Ordering::SeqCst);

        release_deferred_prev(0);

        assert_eq!(task.running_cpu.load(Ordering::SeqCst), NO_CPU);
        assert_eq!(task.last_cpu.load(Ordering::SeqCst), 1);
        assert!(ready_queue(1).lock().contains(&task_id));
        let stats = scheduler_migration_stats();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.promotions, 1);
    }
}
