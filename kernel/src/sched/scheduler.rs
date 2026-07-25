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
//! # TaskPool Ownership
//!
//! The global `TaskPool` stores active tasks in a map keyed by stable global
//! task IDs. User IDs increase from one while kernel IDs decrease from a
//! disjoint high range. IDs are never recycled, so stale bare IDs cannot alias
//! a later task.
//!
//! The pool owns one `Arc<Task>` for every registered task. Lookups clone that
//! handle while holding the pool lock, so callers retain ownership even after a
//! concurrent zombie cleanup removes the task from its slot. Removed handles
//! move to retirement until no lookup owns another `Arc`; only then does the
//! dedicated task-reaper worker drop the task. Stable IDs are never recycled.
//! This makes task lookup safe across CPUs without extending borrowed-reference
//! lifetimes or running `Task::drop` in a lookup context.
//!
//! **IMPORTANT**: Never access `TaskPool::tasks` directly. Always use the
//! provided methods so lookups retain task ownership.

extern crate alloc;

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use alloc::{
    collections::{BTreeMap, BTreeSet, vec_deque::VecDeque},
    string::ToString,
    sync::Arc,
    vec::Vec,
};

use crate::abi::EventProcessOutcome;
use crate::arch::get_kernel_trapvector_paddr;
use crate::arch::set_next_mode;
use crate::print;
use crate::println;
use crate::sync::{IrqSpinLock, Once};
use crate::{arch::set_trapvector, vm::get_trampoline_trap_vector};
use crate::{
    arch::{
        Arch, Trapframe, get_cpu, get_user_trap_handler, instruction::idle,
        trap::user::arch_switch_to_user,
    },
    environment::MAX_NUM_CPUS,
    sync::{CpuLocal, IrqGuard},
    task::{
        CurrentTaskRef, NICE_0_LOAD, SCHED_UTIL_SCALE, Task, TaskCorePreference, TaskState,
        new_kernel_task, wake_parent_waiters, wake_task_waiters,
    },
    timer::{
        SCHEDULER_ACCOUNTING_QUANTUM_NS, TimerHandle, TimerHandler, add_timer, cancel_timer,
        get_time_ns,
    },
};

/// Maximum number of concurrently active user tasks.
pub const MAX_ACTIVE_USER_TASKS: usize = 895;
/// Maximum number of concurrently active kernel tasks.
pub const MAX_ACTIVE_KERNEL_TASKS: usize = 128;

/// Global task pool storing all tasks
/// Using Once with Box-ed tasks array to avoid large stack usage.
static TASK_POOL: Once<TaskPool> = Once::new();
static TASK_REAPER_STARTED: AtomicBool = AtomicBool::new(false);
static TASK_REAPER_WAKER: crate::sync::Waker =
    crate::sync::Waker::new_uninterruptible("task-reaper");
static SLICE_CALLBACK_TOKENS: AtomicU64 = AtomicU64::new(1);
static SLICE_CALLBACK_CONTEXTS: Once<IrqSpinLock<BTreeMap<u64, SliceCallbackContext>>> =
    Once::new();
static SLICE_STATES: Once<[IrqSpinLock<SliceState>; MAX_NUM_CPUS]> = Once::new();
static SLICE_TIMER_HANDLER: Once<Arc<SliceTimerHandler>> = Once::new();

#[derive(Clone, Copy)]
struct SliceCallbackContext {
    cpu_id: usize,
    task_id: usize,
    task_generation: usize,
    generation: u64,
}

#[derive(Clone, Copy)]
struct ActiveSlice {
    handle: Option<TimerHandle>,
    token: u64,
}

struct SliceState {
    active: Option<ActiveSlice>,
    generation: u64,
    task_id: Option<usize>,
    task_generation: Option<usize>,
    need_resched: bool,
}

impl SliceState {
    fn new() -> Self {
        Self {
            active: None,
            generation: 0,
            task_id: None,
            task_generation: None,
            need_resched: false,
        }
    }
}

struct SliceTimerHandler;

impl TimerHandler for SliceTimerHandler {
    fn on_timer_expired(self: Arc<Self>, context: usize) {
        let token = context as u64;
        let Some(context) = slice_callback_contexts().lock().remove(&token) else {
            return;
        };
        let mut state = slice_states()[context.cpu_id].lock();
        if state.generation == context.generation
            && state.task_id == Some(context.task_id)
            && state.task_generation == Some(context.task_generation)
            && state.active.is_some_and(|active| active.token == token)
        {
            state.active = None;
            state.need_resched = true;
        }
    }
}

fn slice_callback_contexts() -> &'static IrqSpinLock<BTreeMap<u64, SliceCallbackContext>> {
    SLICE_CALLBACK_CONTEXTS.call_once(|| IrqSpinLock::new(BTreeMap::new()))
}

fn slice_states() -> &'static [IrqSpinLock<SliceState>; MAX_NUM_CPUS] {
    SLICE_STATES.call_once(|| core::array::from_fn(|_| IrqSpinLock::new(SliceState::new())))
}

fn slice_timer_handler() -> Arc<dyn TimerHandler> {
    SLICE_TIMER_HANDLER
        .call_once(|| Arc::new(SliceTimerHandler))
        .clone()
}
static FORK_TRACE_TASKS: Once<IrqSpinLock<BTreeSet<usize>>> = Once::new();
static FORK_TRACE_PICKED_TASKS: Once<IrqSpinLock<BTreeSet<usize>>> = Once::new();
const FORK_TRACE_ATOMIC_SLOTS: usize = 1024;
static FORK_TRACE_ATOMIC_TASKS: [AtomicUsize; FORK_TRACE_ATOMIC_SLOTS] =
    [const { AtomicUsize::new(0) }; FORK_TRACE_ATOMIC_SLOTS];
static FORK_TRACE_ATOMIC_CPU_MASKS: [AtomicU64; FORK_TRACE_ATOMIC_SLOTS] =
    [const { AtomicU64::new(0) }; FORK_TRACE_ATOMIC_SLOTS];

/// Get the global task pool (lazy initialization on first call)
pub fn get_task_pool() -> &'static TaskPool {
    TASK_POOL.call_once(|| TaskPool::new())
}

fn fork_trace_tasks() -> &'static IrqSpinLock<BTreeSet<usize>> {
    FORK_TRACE_TASKS.call_once(|| IrqSpinLock::new(BTreeSet::new()))
}

fn fork_trace_picked_tasks() -> &'static IrqSpinLock<BTreeSet<usize>> {
    FORK_TRACE_PICKED_TASKS.call_once(|| IrqSpinLock::new(BTreeSet::new()))
}

struct TaskEntry {
    generation: usize,
    task: Arc<Task>,
}

struct TaskPoolState {
    tasks: BTreeMap<usize, TaskEntry>,
    active_user_tasks: usize,
    active_kernel_tasks: usize,
    pending_user_tasks: usize,
    pending_kernel_tasks: usize,
    next_user_id: usize,
    next_kernel_id: usize,
}

/// Global task pool storing active tasks in a map.
///
/// # Ownership
///
/// Tasks are stored as `Arc<Task>` handles in a map. `get_task()` clones an
/// entry's handle while holding the pool lock, so the returned task
/// remains alive if another CPU concurrently removes the pool's handle during
/// zombie cleanup. Removed entry handles enter retirement and are dropped only
/// by the dedicated normal-context task reaper after no outstanding lookup
/// retains the task. A task ID is never recycled.
///
/// **IMPORTANT**: Do NOT directly access the `tasks` map. Always use:
/// - `TaskPool::get_task()` for owned task handles
/// - `get_task_by_id()` which is the preferred public API
///
/// Direct map access can bypass the ownership and synchronization guarantees.
///
/// # Memory Layout
///
/// The active-task map owns one `Arc<Task>` per entry:
/// - Every active entry owns an `Arc<Task>`
/// - Cloned handles keep the corresponding task allocation alive
struct RetiredTask {
    task_id: usize,
    task: Arc<Task>,
}

pub struct TaskPool {
    // Each map entry owns one handle while the task is registered.
    //
    // ⚠️ DO NOT ACCESS DIRECTLY - Use get_task() methods
    tasks: IrqSpinLock<TaskPoolState>,

    // Removed slot handles remain here until no outstanding lookup owns the
    // task. The task-reaper worker moves entries out of this lock before
    // dropping them.
    retired_tasks: IrqSpinLock<Vec<RetiredTask>>,
}

impl TaskPool {
    fn new() -> Self {
        TaskPool {
            tasks: IrqSpinLock::new(TaskPoolState {
                tasks: BTreeMap::new(),
                active_user_tasks: 0,
                active_kernel_tasks: 0,
                pending_user_tasks: 0,
                pending_kernel_tasks: 0,
                next_user_id: 1,
                // Zero encodes no task, and usize::MAX is intentionally never assigned.
                next_kernel_id: usize::MAX - 1,
            }),
            retired_tasks: IrqSpinLock::new(Vec::new()),
        }
    }

    fn reserve_id(
        state: &mut TaskPoolState,
        task_type: crate::task::TaskType,
    ) -> Result<usize, &'static str> {
        match task_type {
            crate::task::TaskType::User => {
                if state.active_user_tasks + state.pending_user_tasks >= MAX_ACTIVE_USER_TASKS {
                    return Err("Maximum active user task count reached");
                }
                let id = state.next_user_id;
                if id == 0 || id >= state.next_kernel_id {
                    return Err("User task ID space exhausted");
                }
                state.next_user_id = id.checked_add(1).ok_or("User task ID overflow")?;
                state.pending_user_tasks += 1;
                Ok(id)
            }
            crate::task::TaskType::Kernel => {
                if state.active_kernel_tasks + state.pending_kernel_tasks >= MAX_ACTIVE_KERNEL_TASKS
                {
                    return Err("Maximum active kernel task count reached");
                }
                let id = state.next_kernel_id;
                if id == 0 || id <= state.next_user_id {
                    return Err("Kernel task ID space exhausted");
                }
                state.next_kernel_id = id.checked_sub(1).ok_or("Kernel task ID overflow")?;
                state.pending_kernel_tasks += 1;
                Ok(id)
            }
        }
    }

    /// Add a task to the pool
    /// Allocates an ID, sets it on the task, and returns the ID
    fn add_task(&self, mut task: Task) -> Result<usize, &'static str> {
        let task_type = task.task_type;
        let task_id = {
            let mut tasks = self.tasks.lock();
            Self::reserve_id(&mut tasks, task_type)?
        };

        // Namespace allocation and VMM owner registration may take their own
        // locks, so do not hold the TaskPool lock across these operations.
        let namespace_id = task.get_namespace().allocate_task_id_for(task_id);
        task.set_id(task_id);
        task.set_namespace_id(namespace_id);
        task.vm_manager.set_owner_task_id_if_unset(task_id);
        let task = Arc::new(task);

        let mut tasks = self.tasks.lock();
        match task_type {
            crate::task::TaskType::User => {
                tasks.pending_user_tasks -= 1;
                tasks.active_user_tasks += 1;
            }
            crate::task::TaskType::Kernel => {
                tasks.pending_kernel_tasks -= 1;
                tasks.active_kernel_tasks += 1;
            }
        }
        tasks.tasks.insert(
            task_id,
            TaskEntry {
                generation: task_id,
                task,
            },
        );
        Ok(task_id)
    }

    fn task_generation(&self, task_id: usize) -> Option<usize> {
        let tasks = self.tasks.lock();
        Some(tasks.tasks.get(&task_id)?.generation)
    }

    fn get_task_if_generation(&self, task_id: usize, generation: usize) -> Option<Arc<Task>> {
        if generation == 0 {
            return None;
        }

        let tasks = self.tasks.lock();
        let entry = tasks.tasks.get(&task_id)?;
        if entry.generation != generation {
            return None;
        }

        Some(entry.task.clone())
    }

    /// Get an owned task handle by ID.
    ///
    /// The returned `Arc<Task>` keeps the task alive even if zombie cleanup
    /// removes the pool's handle concurrently.
    ///
    /// # Returns
    ///
    /// An owned handle for the task currently registered at `task_id`, or `None`.
    pub fn get_task(task_id: usize) -> Option<Arc<Task>> {
        let pool = get_task_pool();
        let tasks = pool.tasks.lock();

        tasks.tasks.get(&task_id).map(|entry| entry.task.clone())
    }

    /// Retire a terminated task from the pool.
    ///
    /// This removes the pool's active entry handle and transfers it to retirement.
    /// Any handles previously returned by `get_task()` keep the task alive
    /// until their owners release them. The dedicated task-reaper worker drops
    /// the task only after retirement is the sole strong
    /// owner. The task must be terminated and not running when removed.
    ///
    /// # Returns
    ///
    /// `true` when the task was retired, or `false` when it is absent, running,
    /// or not yet terminated.
    pub(crate) fn remove_task(&self, task_id: usize) -> bool {
        let retired_task = {
            let mut tasks = self.tasks.lock();
            let Some(entry) = tasks.tasks.get(&task_id) else {
                return false;
            };
            if entry.task.running_cpu.load(Ordering::SeqCst) != NO_CPU
                || !matches!(
                    entry.task.state.load(Ordering::SeqCst),
                    TaskState::Terminated
                )
            {
                return false;
            }

            let entry = tasks
                .tasks
                .remove(&task_id)
                .expect("task entry was checked as occupied");
            match entry.task.task_type {
                crate::task::TaskType::User => tasks.active_user_tasks -= 1,
                crate::task::TaskType::Kernel => tasks.active_kernel_tasks -= 1,
            }
            RetiredTask {
                task_id,
                task: entry.task,
            }
        };

        {
            self.retired_tasks.lock().push(retired_task);
        }
        TASK_REAPER_WAKER.wake_one();
        true
    }

    /// Reap retired tasks no longer referenced by lookup owners.
    ///
    /// This is called only by the dedicated task-reaper worker in normal task
    /// context. Entries are removed from the retirement list before dropping so
    /// `Task::drop` never executes while either task-pool lock is held.
    ///
    /// # Returns
    ///
    /// The number of retired tasks reclaimed during this call.
    pub(crate) fn reap_retired_tasks(&self) -> usize {
        let mut reclaimable = Vec::new();
        {
            let mut retired_tasks = self.retired_tasks.lock();
            let mut index = 0;
            while index < retired_tasks.len() {
                if Arc::strong_count(&retired_tasks[index].task) == 1 {
                    reclaimable.push(retired_tasks.swap_remove(index));
                } else {
                    index += 1;
                }
            }
        }

        let reclaimed = reclaimable.len();
        for retired_task in reclaimable {
            let task_id = retired_task.task_id;
            let trace_fork_exit = DEBUG_FORK_TRACE_LOGGING && is_fork_trace_task(task_id);
            retired_task
                .task
                .get_namespace()
                .unregister_mapping_for_global(task_id);
            crate::task::cleanup_task_waker(task_id);
            crate::task::cleanup_parent_waker(task_id);
            if DEBUG_FORK_TRACE_LOGGING {
                clear_fork_trace_task(task_id);
            }
            crate::breadcrumb::drop(crate::breadcrumb::REAPER_DROP_BEGIN, task_id as u64, 0);
            if trace_fork_exit {
                crate::early_println!(
                    "[fork-trace] child_task_id={} reaper-drop-enter cpu={}",
                    task_id,
                    get_cpu().get_cpuid(),
                );
            }
            drop(retired_task);
            crate::breadcrumb::drop(crate::breadcrumb::REAPER_DROP_DONE, task_id as u64, 0);
            if trace_fork_exit {
                crate::early_println!(
                    "[fork-trace] child_task_id={} reaper-drop-done cpu={}",
                    task_id,
                    get_cpu().get_cpuid(),
                );
            }
        }
        reclaimed
    }

    /// Drop reclaimable retired tasks immediately from normal task context.
    ///
    /// This is used by resource allocation slow paths that can recover by
    /// releasing tasks already retired by the scheduler. It preserves the
    /// reaper's ownership rule: only retirement's sole `Arc<Task>` is dropped.
    pub(crate) fn reclaim_retired_tasks_now(&self) -> usize {
        self.reap_retired_tasks()
    }

    fn has_retired_tasks(&self) -> bool {
        !self.retired_tasks.lock().is_empty()
    }

    #[cfg(test)]
    fn reap_retired_tasks_for_test(&self) -> usize {
        self.reap_retired_tasks()
    }

    #[allow(dead_code)]
    fn contains_task(&self, task_id: usize) -> bool {
        let tasks = self.tasks.lock();
        tasks.tasks.contains_key(&task_id)
    }

    /// Return a snapshot of active global task IDs for shutdown and diagnostics.
    pub fn task_ids_snapshot(&self) -> Vec<usize> {
        self.tasks.lock().tasks.keys().copied().collect()
    }

    /// Reset the task pool to initial state (test-only)
    ///
    /// Retires all tasks, reclaims them, and resets test-only ID allocation.
    /// This should ONLY be called in tests to clean up state between test
    /// cases when no task handles remain and no tasks are running. Test reset
    /// invokes reaping directly because the kernel worker is not scheduled by
    /// unit tests.
    #[cfg(test)]
    pub fn reset(&self) {
        let mut retired_tasks = Vec::new();
        {
            let mut tasks = self.tasks.lock();
            for (task_id, entry) in core::mem::take(&mut tasks.tasks) {
                retired_tasks.push(RetiredTask {
                    task_id,
                    task: entry.task,
                });
            }
            tasks.active_user_tasks = 0;
            tasks.active_kernel_tasks = 0;
            tasks.pending_user_tasks = 0;
            tasks.pending_kernel_tasks = 0;
            tasks.next_user_id = 1;
            tasks.next_kernel_id = usize::MAX - 1;
        }
        {
            self.retired_tasks.lock().append(&mut retired_tasks);
        }
        self.reap_retired_tasks_for_test();
        assert!(
            self.retired_tasks.lock().is_empty(),
            "cannot reset task pool while task handles remain"
        );
    }
}

const TASK_REAPER_RETRY_NS: u64 = SCHEDULER_ACCOUNTING_QUANTUM_NS;

fn task_reaper_worker_entry() {
    loop {
        let task_pool = get_task_pool();
        task_pool.reap_retired_tasks();

        let Some(task) = crate::task::mytask() else {
            crate::arch::instruction::idle();
        };
        if task_pool.has_retired_tasks() {
            TASK_REAPER_WAKER.wait_with_timeout(
                task.get_id(),
                task.get_trapframe(),
                Some(TASK_REAPER_RETRY_NS),
            );
        } else {
            TASK_REAPER_WAKER.wait(task.get_id(), task.get_trapframe());
        }
    }
}

fn start_task_reaper_worker() {
    if TASK_REAPER_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    let task = new_kernel_task("task-reaper".to_string(), 1, task_reaper_worker_entry);
    task.init();
    add_task(task, 0);
}

crate::late_initcall!(start_task_reaper_worker);

static CURRENT_TASK_IDS: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];
static SCHEDULER_READY: [AtomicBool; MAX_NUM_CPUS] =
    [const { AtomicBool::new(false) }; MAX_NUM_CPUS];
static BOOT_CPU_ID: AtomicUsize = AtomicUsize::new(0);
static READY_QUEUES: [IrqSpinLock<VecDeque<usize>>; MAX_NUM_CPUS] =
    [const { IrqSpinLock::new(VecDeque::new()) }; MAX_NUM_CPUS];
static ZOMBIE_QUEUE: IrqSpinLock<VecDeque<usize>> = IrqSpinLock::new(VecDeque::new());
static BLOCKED_QUEUE: IrqSpinLock<VecDeque<usize>> = IrqSpinLock::new(VecDeque::new());
static ONLINE_CPUS: IrqSpinLock<alloc::vec::Vec<usize>> = IrqSpinLock::new(alloc::vec::Vec::new());
static IDLE_TASK_IDS: [AtomicUsize; MAX_NUM_CPUS] = [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];
static PENDING_IDLE_TO_USER_TRAP_TASK: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];
static PENDING_RESCHEDULE: [AtomicBool; MAX_NUM_CPUS] =
    [const { AtomicBool::new(false) }; MAX_NUM_CPUS];
// Tracks an outstanding hardware reschedule kick. This is deliberately
// separate from PENDING_RESCHEDULE, which records deferred scheduler work.
static PENDING_RESCHEDULE_IPI: [AtomicBool; MAX_NUM_CPUS] =
    [const { AtomicBool::new(false) }; MAX_NUM_CPUS];
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
const SCHED_DEMOTION_SUSTAIN_NS: u64 = 250_000_000;
const SCHED_STEAL_SCAN_LIMIT: usize = 8;
const SCHED_LATERAL_BALANCE_MARGIN: u64 = TASK_LOAD_SCALE / 4;

/// Target fair-scheduling period in nanoseconds. With few runners the queue
/// aims to cycle every task within this window.
const SCHED_LATENCY_NS: u64 = 5_000_000;
/// Per-task minimum quantum in nanoseconds. Below this a task cannot be
/// preempted mid-run, which bounds pick frequency under load.
const SCHED_MIN_GRANULARITY_NS: u64 = 750_000;

/// Compute the fair-scheduling period for a queue with `nr_running` entities.
///
/// Matches Linux's `__sched_period`: when the queue fits inside
/// [`SCHED_LATENCY_NS`], every entity gets a slice each period; otherwise the
/// period grows so each entity receives at least [`SCHED_MIN_GRANULARITY_NS`].
#[inline]
const fn sched_period(nr_running: usize) -> u64 {
    if nr_running * (SCHED_MIN_GRANULARITY_NS as usize) > SCHED_LATENCY_NS as usize {
        (nr_running as u64).saturating_mul(SCHED_MIN_GRANULARITY_NS)
    } else {
        SCHED_LATENCY_NS
    }
}

/// Translate a real-time delta into virtual time for an entity of `weight`.
///
/// Mirrors Linux's `calc_delta_fair`: heavier weights advance virtual time
/// more slowly, so they receive more real CPU per virtual unit. Uses u128
/// intermediates to avoid overflow at hour-scale uptimes.
#[inline]
fn calc_delta_fair(delta_ns: u64, weight: u32) -> u64 {
    if weight == 0 {
        return delta_ns;
    }
    let result = (delta_ns as u128).saturating_mul(NICE_0_LOAD as u128) / weight as u128;
    result.min(u64::MAX as u128) as u64
}

/// Compute the per-entity quantum for a queue whose total weight is
/// `total_weight` and whose scheduling period is `period_ns`.
///
/// `slice = period * weight / total_weight`, clamped to
/// [`SCHED_MIN_GRANULARITY_NS`] so a low-weight task cannot be starved of
/// runnable time.
#[inline]
fn sched_slice(period_ns: u64, weight: u32, total_weight: u64) -> u64 {
    if total_weight == 0 {
        return SCHED_MIN_GRANULARITY_NS;
    }
    let slice = ((period_ns as u128).saturating_mul(weight as u128) / total_weight as u128) as u64;
    slice.max(SCHED_MIN_GRANULARITY_NS)
}

/// Recompute an entity's virtual deadline.
///
/// `deadline = vruntime + calc_delta_fair(slice, weight)`. The fair scheduler
/// picks the eligible entity with the smallest deadline.
#[inline]
fn fair_deadline(vruntime: u64, slice_ns: u64, weight: u32) -> u64 {
    vruntime.saturating_add(calc_delta_fair(slice_ns, weight))
}

/// Composite key ordering fair-queue entities.
///
/// Primary key is the virtual deadline (EEVDF picks the smallest eligible
/// deadline). Ties fall back to virtual runtime, then to task id, so two
/// entities with identical EEVDF coordinates still have a deterministic
/// `BTreeMap` order.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Debug)]
struct FairKey {
    deadline: u64,
    vruntime: u64,
    task_id: usize,
}

impl FairKey {
    const fn new(deadline: u64, vruntime: u64, task_id: usize) -> Self {
        Self {
            deadline,
            vruntime,
            task_id,
        }
    }
}

/// Per-CPU fair run queue.
///
/// Holds runnable task ids keyed by [`FairKey`] for O(log n) eligible
/// min-deadline picks. Authoritative per-entity state (`vruntime`,
/// `deadline`, `slice`, `weight`) lives on [`Task`]; the queue caches only
/// the aggregates needed to advance virtual time without re-scanning.
struct FairQueue {
    tree: BTreeMap<FairKey, usize>,
    entries: BTreeMap<usize, FairKey>,
    min_vruntime: u64,
    sum_w_vruntime: i128,
    avg_load: u64,
    nr_running: usize,
}

impl FairQueue {
    const fn new() -> Self {
        Self {
            tree: BTreeMap::new(),
            entries: BTreeMap::new(),
            min_vruntime: 0,
            sum_w_vruntime: 0,
            avg_load: 0,
            nr_running: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.nr_running == 0
    }

    /// Weighted average virtual runtime over queued entities.
    ///
    /// EEVDF defines `lag_i = avg_vruntime - vruntime_i`; an entity is
    /// eligible iff its lag is non-negative. Returns [`FairQueue::min_vruntime`]
    /// when the queue is empty so the value stays monotonic and sensible.
    fn avg_vruntime(&self) -> u64 {
        if self.avg_load == 0 {
            return self.min_vruntime;
        }
        let avg = self.sum_w_vruntime / self.avg_load as i128;
        if avg < 0 {
            self.min_vruntime
        } else {
            avg as u64
        }
    }

    /// Return the entity key for `task_id`, if it is currently queued.
    fn key_for(&self, task_id: usize) -> Option<FairKey> {
        self.entries.get(&task_id).copied()
    }

    /// Insert a freshly-placed entity into the queue.
    ///
    /// Caller is responsible for setting `vruntime`, `deadline`, and `on_rq`
    /// on the [`Task`] before calling this; the queue only indexes what is
    /// already authoritative.
    fn insert(&mut self, task_id: usize, key: FairKey, vruntime: u64, weight: u32) {
        debug_assert!(!self.entries.contains_key(&task_id), "double enqueue");
        self.tree.insert(key, task_id);
        self.entries.insert(task_id, key);
        self.sum_w_vruntime = self
            .sum_w_vruntime
            .saturating_add((vruntime as i128).saturating_mul(weight as i128));
        self.avg_load = self.avg_load.saturating_add(weight as u64);
        self.nr_running = self.nr_running.saturating_add(1);
        self.bump_min_vruntime(vruntime);
    }

    /// Remove an entity from the queue. Caller updates authoritative Task
    /// state separately; this only drops the queue index.
    fn remove(&mut self, task_id: usize) -> Option<FairKey> {
        let key = self.entries.remove(&task_id)?;
        self.tree.remove(&key);
        Some(key)
    }

    /// Re-insert under a new key after `vruntime` or `deadline` advanced,
    /// applying the resulting weighted-sum delta to `sum_w_vruntime`.
    fn rekey(&mut self, task_id: usize, vruntime: u64, weight: u32, new_key: FairKey) {
        let prev = self
            .entries
            .remove(&task_id)
            .expect("rekey on missing entity");
        let removed = self.tree.remove(&prev);
        debug_assert!(removed.is_some(), "rekey missing tree entry");
        self.tree.insert(new_key, task_id);
        self.entries.insert(task_id, new_key);
        let delta = (vruntime as i128).saturating_sub(prev.vruntime as i128);
        self.sum_w_vruntime = self
            .sum_w_vruntime
            .saturating_add(delta.saturating_mul(weight as i128));
        self.bump_min_vruntime(vruntime);
    }

    /// Advance the queue's monotonic floor.
    ///
    /// `min_vruntime` never decreases; it tracks the smaller of the just-updated
    /// entity and the previous floor, then refuses to go backwards. This is
    /// the same invariant Linux's `update_min_vruntime` maintains and is what
    /// lets migrating entities be re-placed without gaining virtual time.
    fn bump_min_vruntime(&mut self, candidate: u64) {
        let floor = match self.tree.first_key_value() {
            Some((k, _)) => k.vruntime.min(candidate),
            None => candidate,
        };
        if floor > self.min_vruntime {
            self.min_vruntime = floor;
        }
    }

    /// Pick the eligible entity with the smallest virtual deadline.
    ///
    /// Returns `None` for an empty queue. If every queued entity is currently
    /// over-served (`lag < 0`), the queue snaps `avg_vruntime` forward to the
    /// smallest queued `vruntime` and re-evaluates, mirroring the
    /// "no eligible entity" rule from Zircon and the EEVDF paper.
    fn pick_eligible_min_deadline(&self) -> Option<FairKey> {
        let avg = self.avg_vruntime();
        // BTreeMap iterates in FairKey order (deadline, vruntime, task_id),
        // so the first queued entity whose vruntime is at or below avg is
        // already the smallest-deadline eligible pick.
        if let Some((&key, _)) = self.tree.iter().find(|(k, _)| k.vruntime <= avg) {
            return Some(key);
        }
        // Nobody eligible: snap avg forward to the smallest queued vruntime
        // and return the smallest-deadline entity at that point.
        let snap = self.tree.keys().map(|k| k.vruntime).min()?;
        self.tree
            .iter()
            .find(|(k, _)| k.vruntime == snap)
            .map(|(k, _)| *k)
    }
}

// DIAGNOSTIC: Set these constants to false after the Apple SMP fork/migration
// experiment. The normal placement and work-stealing code remains below.
const DIAGNOSTIC_PIN_FORK_CHILD_TO_PARENT_CPU: bool = false;
const DIAGNOSTIC_PIN_FORK_CHILD_TO_BSP: bool = false;
const DIAGNOSTIC_DISABLE_IDLE_WORK_STEALING: bool = false;
const DIAGNOSTIC_DISABLE_TASK_MIGRATION: bool = false;
const DIAGNOSTIC_RUN_UNPINNED_TASKS_ON_BSP: bool = false;
// Route selected user task classes to the BSP while leaving the normal
// placement code available for the complementary class.
const DIAGNOSTIC_RUN_ALL_USER_TASKS_ON_BSP: bool = false;
const DIAGNOSTIC_RUN_USER_PROCESS_LEADERS_ON_BSP: bool = false;
const DIAGNOSTIC_RUN_USER_THREADS_ON_BSP: bool = false;
const DIAGNOSTIC_RETAIN_TERMINATED_TASKS: bool = false;

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

/// Defer a reschedule request until the current CPU reaches a safe scheduling point.
///
/// # Arguments
///
/// * `cpu_id` - CPU that consumed the reschedule interrupt.
pub fn defer_reschedule(cpu_id: usize) {
    if cpu_id < MAX_NUM_CPUS {
        PENDING_RESCHEDULE[cpu_id].store(true, Ordering::Release);
    }
}

/// Take a deferred reschedule request for a CPU.
///
/// # Arguments
///
/// * `cpu_id` - CPU about to reach a safe scheduling point.
///
/// # Returns
///
/// `true` when a deferred request was pending.
pub fn take_deferred_reschedule(cpu_id: usize) -> bool {
    cpu_id < MAX_NUM_CPUS && PENDING_RESCHEDULE[cpu_id].swap(false, Ordering::AcqRel)
}

fn reserve_reschedule_ipi(cpu_id: usize) -> bool {
    cpu_id < MAX_NUM_CPUS
        && PENDING_RESCHEDULE_IPI[cpu_id]
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
}

/// Acknowledge a consumed hardware reschedule IPI on its target CPU.
///
/// The architecture interrupt handler must call this only after it has
/// acknowledged the hardware IPI source and before it processes or defers the
/// reschedule request. A request that races before this clear is covered by the
/// current handler; one that races after it sends a fresh hardware IPI.
///
/// # Arguments
///
/// * `cpu_id` - Target CPU that consumed the reschedule IPI.
pub fn acknowledge_reschedule_ipi(cpu_id: usize) {
    if cpu_id < MAX_NUM_CPUS {
        PENDING_RESCHEDULE_IPI[cpu_id].swap(false, Ordering::AcqRel);
    }
}

fn request_remote_reschedule(target_cpu: usize) {
    let source_cpu = get_cpu().get_cpuid();
    if target_cpu >= MAX_NUM_CPUS
        || !is_cpu_online(target_cpu)
        || target_cpu == source_cpu
        || !reserve_reschedule_ipi(target_cpu)
    {
        return;
    }

    if !crate::arch::send_reschedule_ipi(target_cpu) {
        panic!(
            "online CPU must accept reschedule IPI: source_cpu={} target_cpu={}",
            source_cpu, target_cpu
        );
    }
}

/// Return whether a CPU has completed its initial scheduler task publication.
///
/// A CPU becomes ready only after `start_scheduler()` returns from its first
/// `pick_next()`, which publishes that CPU's current-task state. Interrupt
/// handlers must not enter scheduling before then.
///
/// # Arguments
///
/// * `cpu_id` - Scheduler CPU ID.
///
/// # Returns
///
/// `true` when interrupt-originated scheduling is safe on `cpu_id`.
pub fn scheduler_ready(cpu_id: usize) -> bool {
    cpu_id < MAX_NUM_CPUS && SCHEDULER_READY[cpu_id].load(Ordering::Acquire)
}

/// Return whether an interrupt handler may enter the scheduler on a CPU.
///
/// # Arguments
///
/// * `cpu_id` - Scheduler CPU ID handling the interrupt.
///
/// # Returns
///
/// `true` after the CPU has published its initial current task and the current
/// execution context holds no preemption-disabling guards.
pub fn may_schedule_from_interrupt(cpu_id: usize) -> bool {
    scheduler_ready(cpu_id) && crate::sync::preemptible()
}

#[inline]
fn set_scheduler_ready(cpu_id: usize, ready: bool) {
    assert_valid_cpu_id(cpu_id);
    SCHEDULER_READY[cpu_id].store(ready, Ordering::Release);
}

/// Apply the temporary fork-child affinity used by the SMP diagnostic mode.
///
/// # Arguments
///
/// * `child` - Fully initialized child task that has not been published yet.
/// * `parent_cpu` - CPU currently executing the parent task.
pub fn apply_fork_child_diagnostic_affinity(child: &mut Task, parent_cpu: usize) {
    if DIAGNOSTIC_PIN_FORK_CHILD_TO_BSP {
        child.pinned_cpu = Some(BOOT_CPU_ID.load(Ordering::Acquire));
    } else if DIAGNOSTIC_PIN_FORK_CHILD_TO_PARENT_CPU {
        child.pinned_cpu = Some(parent_cpu);
    }
}

fn diagnostic_run_task_on_bsp(task: &Task) -> bool {
    if task.task_type != crate::task::TaskType::User {
        return false;
    }
    if DIAGNOSTIC_RUN_ALL_USER_TASKS_ON_BSP {
        return true;
    }

    task.registered_id().is_some_and(|task_id| {
        let is_process_leader = task_id == task.get_thread_group_id();
        (DIAGNOSTIC_RUN_USER_PROCESS_LEADERS_ON_BSP && is_process_leader)
            || (DIAGNOSTIC_RUN_USER_THREADS_ON_BSP && !is_process_leader)
    })
}

static DEBUG_TICK: AtomicU64 = AtomicU64::new(0);
static NEXT_CPU: AtomicUsize = AtomicUsize::new(0);

pub const DEBUG_SMP_TASK_FLOW: bool = false;
/// Enable focused fork-child lifecycle logging and bookkeeping.
pub const DEBUG_FORK_TRACE_LOGGING: bool = false;

static DEBUG_ENQUEUE_SEQ: AtomicUsize = AtomicUsize::new(0);
static DEBUG_REMOTE_ENQUEUE_TASK: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];
static DEBUG_REMOTE_ENQUEUE_FROM_CPU: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(NO_CPU) }; MAX_NUM_CPUS];
static DEBUG_REMOTE_ENQUEUE_SEQ: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];

const NO_CPU: usize = usize::MAX;

static SCHEDULE_PREV_TASK: [AtomicUsize; MAX_NUM_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_NUM_CPUS];

/// Lock-free scheduler state sampled for a single CPU.
#[derive(Clone, Copy, Debug)]
pub struct SchedulerDiagnosticSnapshot {
    /// Current task ID, or zero when no task is published.
    pub current_task_id: usize,
    /// Whether the published current task is the CPU's idle task.
    pub is_idle: bool,
    /// Whether the CPU has a deferred reschedule request.
    pub pending_reschedule: bool,
}

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
    let prev_id = SCHEDULE_PREV_TASK[cpu_id].swap(0, Ordering::SeqCst);
    let Some(prev_id) = decode_task_id(prev_id) else {
        return;
    };
    crate::breadcrumb::drop(
        crate::breadcrumb::RELEASE_PREV_ENTER,
        prev_id as u64,
        cpu_id as u64,
    );
    // The lock-free breadcrumb above retains release diagnostics without
    // serializing every traced task switch through the early-console lock.
    // if is_fork_trace_task(prev_id) {
    //     crate::early_println!(
    //         "[fork-trace] child_task_id={} release-prev-enter cpu={}",
    //         prev_id,
    //         cpu_id,
    //     );
    // }
    let Some(task) = TaskPool::get_task(prev_id) else {
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
        let state = task.state.load(Ordering::SeqCst);
        match state {
            // Requeue the previous task after its context has been saved. At
            // this point `running_cpu` has been released, so another CPU may
            // safely claim the saved kernel context from its ready queue.
            TaskState::Ready => {
                let now_ns = get_time_ns();
                let target_cpu = migration_target_for_task(&task, cpu_id, now_ns, true)
                    .filter(|&target_cpu| is_cpu_online(target_cpu))
                    .unwrap_or(cpu_id);
                if target_cpu != cpu_id {
                    record_scheduler_migration(&task, cpu_id, target_cpu, now_ns);
                }
                task.last_cpu.store(target_cpu, Ordering::SeqCst);
                push_ready_task(target_cpu, prev_id);
                notify_remote_ready_task(target_cpu, prev_id, "migrate-ipi-send");
            }
            TaskState::Zombie => {
                finalize_zombie(prev_id, task.get_parent_id());
            }
            TaskState::Terminated => {
                cleanup_zombie(prev_id);
            }
            TaskState::Running | TaskState::Blocked(_) | TaskState::NotInitialized => {}
        }
        crate::breadcrumb::drop(
            crate::breadcrumb::RELEASE_PREV_DONE,
            prev_id as u64,
            match state {
                TaskState::NotInitialized => 0,
                TaskState::Ready => 1,
                TaskState::Running => 2,
                TaskState::Blocked(_) => 3,
                TaskState::Zombie => 5,
                TaskState::Terminated => 6,
            },
        );
        // if is_fork_trace_task(prev_id) {
        //     crate::early_println!(
        //         "[fork-trace] child_task_id={} release-prev-done cpu={} state={:?}",
        //         prev_id,
        //         cpu_id,
        //         state,
        //     );
        // }
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
        .map(|task| task.sched_util_min())
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

fn update_scheduler_observers(cpu_id: usize) {
    if !scheduler_ready(cpu_id) {
        return;
    }
    update_cpu_util_avg(cpu_id);
    crate::device::cpufreq::on_scheduler_tick(cpu_id);
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

fn account_current_task_slice_boundary(cpu_id: usize) {
    let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
    let Some(task_id) = current_task_id(cpu_id) else {
        return;
    };
    if task_id == idle_id {
        return;
    }
    if let Some(task) = TaskPool::get_task(task_id) {
        task.account_sched_util_running(get_time_ns());
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
        match task.state.load(Ordering::SeqCst) {
            TaskState::Zombie => finalize_zombie(task.get_id(), task.get_parent_id()),
            TaskState::Terminated => cleanup_zombie(task.get_id()),
            TaskState::Running
            | TaskState::Ready
            | TaskState::Blocked(_)
            | TaskState::NotInitialized => {}
        }
        return false;
    }
    task.last_cpu.store(cpu_id, Ordering::SeqCst);
    true
}

fn invalidate_local_slice(cpu_id: usize) {
    let active = {
        let mut state = slice_states()[cpu_id].lock();
        state.generation = state.generation.wrapping_add(1);
        state.need_resched = false;
        state.task_id = None;
        state.task_generation = None;
        state.active.take()
    };
    if let Some(active) = active {
        slice_callback_contexts().lock().remove(&active.token);
        if let Some(handle) = active.handle {
            let _ = cancel_timer(handle);
        }
    }
}

fn arm_local_slice(cpu_id: usize, task_id: usize) {
    let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
    if task_id == idle_id {
        return;
    }
    let Some(task) = TaskPool::get_task(task_id) else {
        return;
    };
    let Some(task_generation) = get_task_pool().task_generation(task_id) else {
        return;
    };
    let duration_ns = task.time_slice_duration_ns.load(Ordering::SeqCst);
    drop(task);
    let token = SLICE_CALLBACK_TOKENS.fetch_add(1, Ordering::Relaxed);
    let generation = {
        let mut state = slice_states()[cpu_id].lock();
        state.generation = state.generation.wrapping_add(1);
        state.need_resched = false;
        state.task_id = Some(task_id);
        state.task_generation = Some(task_generation);
        state.active = Some(ActiveSlice {
            handle: None,
            token,
        });
        state.generation
    };
    slice_callback_contexts().lock().insert(
        token,
        SliceCallbackContext {
            cpu_id,
            task_id,
            task_generation,
            generation,
        },
    );
    let handler = slice_timer_handler();
    let handle = add_timer(
        get_time_ns().saturating_add(duration_ns),
        crate::timer::TimerPrecision::Exact,
        &handler,
        token as usize,
    );
    let keep_handle = {
        let mut state = slice_states()[cpu_id].lock();
        if state.generation == generation
            && state.task_id == Some(task_id)
            && state.active.is_some_and(|active| active.token == token)
        {
            state.active = Some(ActiveSlice {
                handle: Some(handle),
                token,
            });
            true
        } else {
            false
        }
    };
    if !keep_handle {
        let _ = cancel_timer(handle);
    }
}

fn replace_local_slice(cpu_id: usize, next_task_id: Option<usize>) {
    invalidate_local_slice(cpu_id);
    if let Some(task_id) = next_task_id {
        arm_local_slice(cpu_id, task_id);
    }
}

fn take_local_slice_reschedule(cpu_id: usize) -> bool {
    if cpu_id >= MAX_NUM_CPUS {
        return false;
    }

    let mut state = slice_states()[cpu_id].lock();
    let requested = state.need_resched;
    state.need_resched = false;
    requested
}

/// Consume a scheduler slice-expiry request after local software timer callbacks.
///
/// Architecture interrupt layers retain the trapframe and pass whether entering
/// the scheduler is safe for this interrupt origin. A deferred request is never
/// lost when the interrupted context is kernel or guest state.
pub fn handle_timer_reschedule(
    cpu_id: usize,
    trapframe: &mut Trapframe,
    can_schedule: bool,
) -> bool {
    let requested = take_local_slice_reschedule(cpu_id);
    let needs_idle_handoff = current_task_is_idle(cpu_id) && has_ready_tasks(cpu_id);
    if !(requested || needs_idle_handoff) {
        return false;
    }
    if can_schedule && may_schedule_from_interrupt(cpu_id) {
        schedule(trapframe);
        true
    } else {
        defer_reschedule(cpu_id);
        false
    }
}

/// Consume a local slice expiry while the CPU is executing a guest.
///
/// Guest trapframes describe guest state and must never be passed to the host
/// scheduler. This helper consumes the local slice request, records deferred
/// host scheduler work, and reports whether guest execution must exit.
///
/// # Arguments
///
/// * `cpu_id` - CPU handling the guest-originated host timer IRQ.
///
/// # Returns
///
/// `true` when the guest must return to the host so the deferred schedule can
/// be consumed at a host-safe point.
pub fn consume_guest_timer_reschedule(cpu_id: usize) -> bool {
    if cpu_id >= MAX_NUM_CPUS {
        return false;
    }

    let requested = take_local_slice_reschedule(cpu_id);
    let needs_idle_handoff = current_task_is_idle(cpu_id) && has_ready_tasks(cpu_id);
    if requested || needs_idle_handoff {
        defer_reschedule(cpu_id);
        true
    } else {
        false
    }
}

/// Refresh the current non-idle task's local slice after its duration changes.
pub fn refresh_current_task_slice(cpu_id: usize) {
    if current_task_is_idle(cpu_id) {
        return;
    }
    replace_local_slice(cpu_id, current_task_id(cpu_id));
}

/// Register a CPU as available to the scheduler.
///
/// Architecture boot code must initialize the CPU's local interrupt controller
/// and reschedule-IPI transport before calling this function. Once registered,
/// remote scheduler operations may immediately send an IPI to the CPU.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU that has completed per-CPU interrupt setup.
///
/// # Returns
///
/// This function returns no value.
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
    task_load_weight(TaskPool::get_task(task_id).as_deref())
}

fn task_load_score_on_cpu(task: &Task, cpu_id: usize) -> u64 {
    task_load_weight(Some(task)).saturating_mul(TASK_LOAD_SCALE) / cpu_capacity(cpu_id) as u64
}

fn task_present_on_cpu(cpu_id: usize, task_id: usize) -> bool {
    if task_id == 0 {
        return false;
    }

    if current_task_id(cpu_id) == Some(task_id) {
        return true;
    }

    ready_queue(cpu_id).lock().contains(&task_id)
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

fn cpu_load_score_with_task(cpu_id: usize, task: &Task) -> u64 {
    let score = cpu_load_score(cpu_id);
    if task
        .registered_id()
        .is_some_and(|task_id| task_present_on_cpu(cpu_id, task_id))
    {
        score
    } else {
        score.saturating_add(task_load_score_on_cpu(task, cpu_id))
    }
}

fn cpu_load_score_without_task(cpu_id: usize, task: &Task) -> u64 {
    let score = cpu_load_score(cpu_id);
    if task
        .registered_id()
        .is_some_and(|task_id| task_present_on_cpu(cpu_id, task_id))
    {
        score.saturating_sub(task_load_score_on_cpu(task, cpu_id))
    } else {
        score
    }
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

/// Return the scheduler capacity required by a task at a point in time.
///
/// This combines measured utilization, the task's `util_min` clamp, and its
/// core preference hint. It is intended for diagnostics and should match the
/// placement policy's capacity floor.
///
/// # Arguments
///
/// * `task` - Task whose placement capacity should be reported.
/// * `now_ns` - Current monotonic timestamp in nanoseconds.
///
/// # Returns
///
/// Required CPU capacity in scheduler units.
pub fn task_required_capacity_snapshot(task: &Task, now_ns: u64) -> u32 {
    task_min_cpu_capacity_at(Some(task), now_ns)
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
    if task.is_some_and(diagnostic_run_task_on_bsp) {
        return BOOT_CPU_ID.load(Ordering::Acquire);
    }
    if DIAGNOSTIC_RUN_UNPINNED_TASKS_ON_BSP && task.is_none_or(|task| task.pinned_cpu.is_none()) {
        return BOOT_CPU_ID.load(Ordering::Acquire);
    }

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
    if diagnostic_run_task_on_bsp(task) {
        return BOOT_CPU_ID.load(Ordering::Acquire);
    }
    if let Some(pinned_cpu) = task.pinned_cpu {
        return pinned_cpu;
    }
    if DIAGNOSTIC_RUN_UNPINNED_TASKS_ON_BSP {
        return BOOT_CPU_ID.load(Ordering::Acquire);
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
    if diagnostic_run_task_on_bsp(task) && cpu_id != BOOT_CPU_ID.load(Ordering::Acquire) {
        return false;
    }
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
        if !task_can_run_on_cpu(&task, target_cpu, now_ns) {
            continue;
        }
        if migration_cooldown_active(&task, now_ns, false) {
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
    if !try_claim_ready_task(&task, target_cpu) {
        if matches!(task.state.load(Ordering::SeqCst), TaskState::Ready)
            && task.running_cpu.load(Ordering::SeqCst) == NO_CPU
        {
            push_ready_task(victim_cpu, task_id);
        }
        return None;
    }

    record_work_steal(&task, now_ns);
    Some(task_id)
}

fn select_wake_cpu_for_task(task: &Task, now_ns: u64) -> usize {
    if diagnostic_run_task_on_bsp(task) {
        return BOOT_CPU_ID.load(Ordering::Acquire);
    }
    if let Some(pinned_cpu) = task.pinned_cpu {
        return pinned_cpu;
    }
    if DIAGNOSTIC_RUN_UNPINNED_TASKS_ON_BSP {
        return BOOT_CPU_ID.load(Ordering::Acquire);
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

/// Normalize an explicit wake target against the same placement policy used by
/// regular wakeups. Explicit callers must not enqueue a task on an offline CPU
/// or override affinity and diagnostic placement constraints.
fn normalize_wake_cpu_for_task(task: &Task, requested_cpu: usize, now_ns: u64) -> Option<usize> {
    if is_cpu_online(requested_cpu) && task_can_run_on_cpu(task, requested_cpu, now_ns) {
        return Some(requested_cpu);
    }

    let fallback_cpu = select_wake_cpu_for_task(task, now_ns);
    (is_cpu_online(fallback_cpu) && task_can_run_on_cpu(task, fallback_cpu, now_ns))
        .then_some(fallback_cpu)
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

fn same_balance_domain(current_cpu: usize, candidate_cpu: usize) -> bool {
    match (
        cpu_topology_domain(current_cpu),
        cpu_topology_domain(candidate_cpu),
    ) {
        (Some(current_domain), Some(candidate_domain)) => current_domain == candidate_domain,
        (None, None) => cpu_capacity(current_cpu) == cpu_capacity(candidate_cpu),
        _ => false,
    }
}

fn select_lateral_balance_cpu(
    task: &Task,
    current_cpu: usize,
    now_ns: u64,
    required_capacity: u32,
) -> Option<usize> {
    let cpus = ONLINE_CPUS.lock();
    if cpus.len() <= 1 {
        return None;
    }

    let current_score = cpu_load_score_with_task(current_cpu, task);
    let current_after = cpu_load_score_without_task(current_cpu, task);
    let mut best: Option<(usize, u64)> = None;

    for &cpu_id in cpus.iter() {
        if cpu_id == current_cpu {
            continue;
        }
        if !same_balance_domain(current_cpu, cpu_id) {
            continue;
        }
        if cpu_capacity(cpu_id) < required_capacity {
            continue;
        }
        if !task_can_run_on_cpu(task, cpu_id, now_ns) {
            continue;
        }

        let target_score = cpu_load_score(cpu_id);
        let target_after = target_score.saturating_add(task_load_score_on_cpu(task, cpu_id));
        let before_max = current_score.max(target_score);
        let after_max = current_after.max(target_after);
        if after_max.saturating_add(SCHED_LATERAL_BALANCE_MARGIN) >= before_max {
            continue;
        }

        if best
            .map(|(_, best_after)| target_after < best_after)
            .unwrap_or(true)
        {
            best = Some((cpu_id, target_after));
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

fn demotion_low_util_sustained(task: &Task, now_ns: u64) -> bool {
    let low_util_since_ns = task.note_sched_low_util(now_ns);
    now_ns.saturating_sub(low_util_since_ns) >= SCHED_DEMOTION_SUSTAIN_NS
}

fn migration_target_for_task(
    task: &Task,
    current_cpu: usize,
    now_ns: u64,
    record_skip: bool,
) -> Option<usize> {
    if DIAGNOSTIC_DISABLE_TASK_MIGRATION {
        return None;
    }
    if task.pinned_cpu.is_some() || !is_cpu_online(current_cpu) {
        return None;
    }

    let current_capacity = cpu_capacity(current_cpu);
    let required_capacity = task_min_cpu_capacity_at(Some(task), now_ns);

    if required_capacity > current_capacity {
        task.clear_sched_low_util();
        let target_cpu = select_target_cpu_at(Some(task), now_ns);
        let target_capacity = cpu_capacity(target_cpu);
        if target_cpu != current_cpu
            && target_capacity > current_capacity
            && target_capacity >= required_capacity
        {
            if migration_cooldown_active(&task, now_ns, record_skip) {
                return None;
            }
            return Some(target_cpu);
        }
        return None;
    }

    if current_capacity > required_capacity.saturating_add(SCHED_DEMOTION_MARGIN) {
        if let Some(target_cpu) = select_lower_capacity_cpu(task, current_cpu, required_capacity) {
            if demotion_low_util_sustained(task, now_ns) {
                if migration_cooldown_active(&task, now_ns, record_skip) {
                    return None;
                }
                return Some(target_cpu);
            }
        } else {
            task.clear_sched_low_util();
        }
    } else {
        task.clear_sched_low_util();
    }

    if let Some(target_cpu) =
        select_lateral_balance_cpu(task, current_cpu, now_ns, required_capacity)
    {
        if migration_cooldown_active(&task, now_ns, record_skip) {
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
    request_remote_reschedule(target_cpu);
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

/// Register the logical CPU that entered the architecture-independent kernel.
///
/// # Arguments
///
/// * `cpu_id` - Logical scheduler CPU ID supplied in [`crate::BootInfo`].
pub fn register_boot_cpu(cpu_id: usize) {
    debug_assert!(cpu_id < MAX_NUM_CPUS);
    BOOT_CPU_ID.store(cpu_id, Ordering::Release);
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

/// Return a lock-free diagnostic snapshot for a scheduler CPU.
///
/// # Arguments
///
/// * `cpu_id` - Scheduler CPU whose atomic state should be sampled.
///
/// # Returns
///
/// The published current task, idle state, and pending-reschedule state, or
/// `None` when `cpu_id` is outside the supported CPU range.
///
/// This accessor takes no scheduler locks and performs no task lookup, so it
/// may be used by interrupt-context diagnostics.
pub fn diagnostic_snapshot(cpu_id: usize) -> Option<SchedulerDiagnosticSnapshot> {
    if cpu_id >= MAX_NUM_CPUS {
        return None;
    }

    let current_task_id = CURRENT_TASK_IDS[cpu_id].load(Ordering::Relaxed);
    let idle_task_id = IDLE_TASK_IDS[cpu_id].load(Ordering::Relaxed);
    Some(SchedulerDiagnosticSnapshot {
        current_task_id,
        is_idle: idle_task_id != 0 && current_task_id == idle_task_id,
        pending_reschedule: PENDING_RESCHEDULE[cpu_id].load(Ordering::Relaxed),
    })
}

#[inline]
fn ready_queue(cpu_id: usize) -> &'static IrqSpinLock<VecDeque<usize>> {
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
    let cpu_id =
        if TaskPool::get_task(task_id).is_some_and(|task| diagnostic_run_task_on_bsp(&task)) {
            BOOT_CPU_ID.load(Ordering::Acquire)
        } else {
            cpu_id
        };
    let mut queue = ready_queue(cpu_id).lock();
    if !queue.contains(&task_id) {
        queue.push_back(task_id);
    }
}

/// Mark a process-fork child for focused first-run tracing.
///
/// # Arguments
///
/// * `task_id` - Global task ID assigned to the child.
pub fn mark_fork_trace_task(task_id: usize) {
    let start = task_id % FORK_TRACE_ATOMIC_SLOTS;
    for offset in 0..FORK_TRACE_ATOMIC_SLOTS {
        let slot = (start + offset) % FORK_TRACE_ATOMIC_SLOTS;
        let registered = FORK_TRACE_ATOMIC_TASKS[slot].load(Ordering::Acquire);
        if registered == task_id {
            break;
        }
        if registered == 0
            && FORK_TRACE_ATOMIC_TASKS[slot]
                .compare_exchange(0, task_id, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            break;
        }
    }

    let mut traced = fork_trace_tasks().lock();
    let mut picked = fork_trace_picked_tasks().lock();
    picked.remove(&task_id);
    traced.insert(task_id);
}

/// Check whether a task is a process-fork child being traced.
///
/// # Arguments
///
/// * `task_id` - Global task ID to check.
///
/// # Returns
///
/// `true` when focused fork tracing is enabled for the task.
pub fn is_fork_trace_task(task_id: usize) -> bool {
    fork_trace_tasks().lock().contains(&task_id)
}

/// Return whether this is the first observed user trap for a traced fork child
/// on the executing CPU.
///
/// The fixed atomic table suppresses duplicate output without allocation or
/// lock acquisition. A child that migrates before another trap may be logged
/// once on each CPU, which is useful diagnostic information.
///
/// # Arguments
///
/// * `cpu_id` - CPU handling the user trap.
/// * `task_id` - Current task ID.
///
/// # Returns
///
/// `true` when the caller should emit the first-trap diagnostic.
pub fn take_fork_trace_first_user_trap(cpu_id: usize, task_id: usize) -> bool {
    if cpu_id >= MAX_NUM_CPUS || cpu_id >= u64::BITS as usize {
        return false;
    }

    let start = task_id % FORK_TRACE_ATOMIC_SLOTS;
    for offset in 0..FORK_TRACE_ATOMIC_SLOTS {
        let slot = (start + offset) % FORK_TRACE_ATOMIC_SLOTS;
        if FORK_TRACE_ATOMIC_TASKS[slot].load(Ordering::Acquire) != task_id {
            continue;
        }

        let cpu_bit = 1u64 << cpu_id;
        return FORK_TRACE_ATOMIC_CPU_MASKS[slot].fetch_or(cpu_bit, Ordering::AcqRel) & cpu_bit
            == 0;
    }

    false
}

#[allow(dead_code)]
fn take_fork_trace_first_pick(task_id: usize) -> bool {
    let traced = fork_trace_tasks().lock();
    if !traced.contains(&task_id) {
        return false;
    }
    fork_trace_picked_tasks().lock().insert(task_id)
}

fn clear_fork_trace_task(task_id: usize) {
    let start = task_id % FORK_TRACE_ATOMIC_SLOTS;
    for offset in 0..FORK_TRACE_ATOMIC_SLOTS {
        let slot = (start + offset) % FORK_TRACE_ATOMIC_SLOTS;
        if FORK_TRACE_ATOMIC_TASKS[slot].load(Ordering::Acquire) != task_id {
            continue;
        }

        FORK_TRACE_ATOMIC_CPU_MASKS[slot].store(0, Ordering::Release);
        let _ = FORK_TRACE_ATOMIC_TASKS[slot].compare_exchange(
            task_id,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        break;
    }

    let mut traced = fork_trace_tasks().lock();
    let mut picked = fork_trace_picked_tasks().lock();
    traced.remove(&task_id);
    picked.remove(&task_id);
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
    crate::breadcrumb::drop(
        crate::breadcrumb::ZOMBIE_FINALIZE,
        task_id as u64,
        parent_id.unwrap_or(0) as u64,
    );
    {
        let mut zombie_queue = ZOMBIE_QUEUE.lock();
        if !zombie_queue.contains(&task_id) {
            zombie_queue.push_back(task_id);
        }
    }
    wake_task_waiters(task_id);
    if let Some(parent_id) = parent_id {
        wake_parent_waiters(parent_id);
        let parent_thread_group = get_task_by_id(parent_id)
            .map(|parent| parent.get_thread_group_id())
            .or_else(|| get_task_by_id(task_id).and_then(|task| task.get_parent_thread_group_id()));
        if let Some(parent_thread_group) = parent_thread_group {
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
    crate::breadcrumb::drop(
        crate::breadcrumb::PICK_NEXT_ENTER,
        cpu.get_cpuid() as u64,
        0,
    );
    let _irq_guard = IrqGuard::new();
    let cpu_id = cpu.get_cpuid();
    crate::breadcrumb::drop(crate::breadcrumb::PICK_GUARD_DONE, cpu_id as u64, 0);
    release_deferred_prev(cpu_id);
    crate::breadcrumb::drop(crate::breadcrumb::PICK_RELEASE_DONE, cpu_id as u64, 0);

    let old_id = current_task_id(cpu_id);
    let old_task = old_id.and_then(TaskPool::get_task);
    crate::breadcrumb::drop(
        crate::breadcrumb::PICK_OLD_DONE,
        cpu_id as u64,
        old_id.unwrap_or(0) as u64,
    );

    let mut next_id: Option<usize> = None;
    'outer: loop {
        let candidate = { ready_queue(cpu_id).lock().pop_front() };
        crate::breadcrumb::drop(
            crate::breadcrumb::PICK_QUEUE_DONE,
            cpu_id as u64,
            candidate.unwrap_or(0) as u64,
        );
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
                        if try_claim_ready_task(&task, cpu_id) {
                            // if take_fork_trace_first_pick(task_id) {
                            //     crate::early_println!(
                            //         "[fork-trace] child_task_id={} picked cpu={}",
                            //         task_id,
                            //         cpu_id
                            //     );
                            // }
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
        if current_is_idle && !DIAGNOSTIC_DISABLE_IDLE_WORK_STEALING {
            crate::breadcrumb::drop(crate::breadcrumb::PICK_STEAL_BEGIN, cpu_id as u64, 0);
            next_id = steal_ready_task_for_cpu(cpu_id, get_time_ns());
            crate::breadcrumb::drop(
                crate::breadcrumb::PICK_STEAL_DONE,
                cpu_id as u64,
                next_id.unwrap_or(0) as u64,
            );
        }
    }

    if next_id.is_none() {
        if let (Some(oid), Some(ot)) = (old_id, old_task.as_deref()) {
            if ot.running_cpu.load(Ordering::SeqCst) == cpu_id
                && matches!(
                    ot.state.load(Ordering::SeqCst),
                    TaskState::Running | TaskState::Ready
                )
            {
                if migration_target_for_task(&ot, cpu_id, get_time_ns(), false).is_some() {
                    let idle_id = IDLE_TASK_IDS[cpu_id].load(Ordering::SeqCst);
                    if idle_id != 0
                        && oid != idle_id
                        && let Some(idle_task) = TaskPool::get_task(idle_id)
                        && try_claim_ready_task(&idle_task, cpu_id)
                    {
                        next_id = Some(idle_id);
                    }
                }
                if next_id.is_none() {
                    ot.state.store(TaskState::Running, Ordering::SeqCst);
                    account_current_task_slice_boundary(cpu_id);
                    replace_local_slice(cpu_id, Some(oid));
                    set_current_task_id(cpu_id, Some(oid));
                    update_scheduler_observers(cpu_id);
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
                if try_claim_ready_task(&task, cpu_id) {
                    next_id = Some(idle_id);
                }
            }
        }
    }

    let Some(next_id) = next_id else {
        invalidate_local_slice(cpu_id);
        account_task_switch(cpu_id, old_id, None);
        set_current_task_id(cpu_id, None);
        update_scheduler_observers(cpu_id);
        return (old_id, None);
    };

    if let (Some(oid), Some(ot), Some(nid)) = (old_id, old_task.as_deref(), Some(next_id)) {
        if oid != nid {
            // Invalidate before publishing a new running owner. A returning
            // kernel switch can be delayed indefinitely, so this cannot wait
            // for release_deferred_prev().
            invalidate_local_slice(cpu_id);
            // Publish every real switch-out before state-specific handling.
            // `release_deferred_prev()` runs only after the low-level context
            // switch and releases the ownership token for blocked, zombie, and
            // terminated tasks as well as runnable tasks.
            SCHEDULE_PREV_TASK[cpu_id].store(oid, Ordering::SeqCst);
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
        }
    }

    account_task_switch(cpu_id, old_id, Some(next_id));
    set_current_task_id(cpu_id, Some(next_id));
    update_scheduler_observers(cpu_id);
    if old_id != Some(next_id) {
        arm_local_slice(cpu_id, next_id);
    }
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
///
/// # Arguments
///
/// * `task` - The unpublished task to register.
///
/// # Returns
///
/// The stable global task ID, or an error if active capacity or ID space is exhausted.
pub fn try_register_task(task: Task) -> Result<usize, &'static str> {
    get_task_pool().add_task(task)
}

/// Add a task to the global task pool without making it runnable yet.
///
/// This infallible wrapper is for boot and other critical callers that cannot
/// recover from task-pool exhaustion. Recoverable userspace clone paths should
/// use [`try_register_task`] instead.
pub fn register_task(task: Task) -> usize {
    let task_id = match try_register_task(task) {
        Ok(id) => id,
        Err(e) => panic!("Failed to add task: {}", e),
    };
    task_id
}

/// Make a registered task runnable on the specified CPU.
pub fn enqueue_task(task_id: usize, cpu_id: usize) {
    let irq_guard = IrqGuard::new();
    let current_cpu = get_cpu().get_cpuid();
    let target_cpu = TaskPool::get_task(task_id)
        .map(|task| select_enqueue_cpu_for_task(&task, cpu_id, get_time_ns()))
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
    let remote = is_cpu_online(target_cpu) && target_cpu != get_cpu().get_cpuid();
    if remote {
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
        request_remote_reschedule(target_cpu);
    }
    drop(irq_guard);
    if remote {
        crate::breadcrumb::drop(
            crate::breadcrumb::ENQUEUE_IRQ_RESTORED,
            task_id as u64,
            target_cpu as u64,
        );
    }
}

/// Schedule tasks on the CPU with kernel context switching.
pub fn schedule(trapframe: &mut Trapframe) {
    assert!(
        crate::sync::preemptible(),
        "schedule called while preemption is disabled"
    );
    crate::breadcrumb::drop(
        crate::breadcrumb::SCHED_ENTER,
        get_cpu().get_cpuid() as u64,
        0,
    );
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
                let current_task_generation = {
                    let Some(current_task) = TaskPool::get_task(current_task_id) else {
                        set_current_task_id(cpu_id, None);
                        switch_to_user_task(cpu, next_task_id);
                    };
                    current_task.last_cpu.store(cpu_id, Ordering::SeqCst);
                    current_task.vcpu.lock().store(trapframe);
                    let generation = get_task_pool()
                        .task_generation(current_task_id)
                        .expect("current task must remain registered before a context switch");

                    // Do not leave an owning Arc<Task> on the outgoing task's
                    // kernel stack. A terminated task never resumes that stack.
                    drop(current_task);
                    generation
                };

                kernel_context_switch(cpu_id, current_task_id, next_task_id);

                // A returning switch resumes this task after it may have been
                // migrated. Reacquire by generation so an ID reuse cannot
                // restore state into a different task.
                if let Some(current_task) =
                    get_task_pool().get_task_if_generation(current_task_id, current_task_generation)
                {
                    current_task.vcpu.lock().switch(trapframe);
                    set_next_mode(current_task.vcpu.lock().get_mode());
                    drop(current_task);
                }
            } else {
                switch_to_user_task(cpu, next_task_id);
            }
        }
    }

    process_pending_events_before_user_return(trapframe);
}

/// Enter userspace for a task already claimed by this CPU.
///
/// This path intentionally extracts the trapframe pointer while an owned pool
/// lookup is in scope, then drops that lookup before the divergent transition.
/// The scheduler's `running_cpu` claim keeps the pool slot alive until this CPU
/// releases it, so the pointer remains valid without retaining an `Arc<Task>`
/// on the task's kernel stack.
fn switch_to_user_task(cpu: &mut Arch, task_id: usize) -> ! {
    let cpu_id = cpu.get_cpuid();
    let trapframe_ptr = {
        let task = TaskPool::get_task(task_id).expect("scheduled task must remain registered");
        assert_eq!(
            task.running_cpu.load(Ordering::SeqCst),
            cpu_id,
            "scheduled task must be owned by the executing CPU"
        );
        setup_task_execution(cpu, &task);
        core::ptr::from_mut(task.get_trapframe())
    };

    // SAFETY: `pick_next()` claimed `task_id` with `running_cpu == cpu_id` and
    // published it as this CPU's current task. TaskPool removal additionally
    // requires `Terminated && running_cpu == NO_CPU`, so the task and its
    // trapframe remain alive after the temporary lookup Arc above is dropped.
    unsafe { arch_switch_to_user(&mut *trapframe_ptr) }
}

/// Process events that must be delivered before returning to userspace.
pub fn process_pending_events_before_user_return(trapframe: &mut Trapframe) {
    let cpu_id = get_cpu().get_cpuid();
    let Some(current_task) = current_task(cpu_id) else {
        return;
    };

    match current_task.process_pending_events() {
        Ok(EventProcessOutcome::NeedReschedule | EventProcessOutcome::Exited(_)) => {
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
    // crate::println!("[sched] entry");
    let cpu = get_cpu();
    let cpu_id = cpu.get_cpuid();
    set_scheduler_ready(cpu_id, false);
    // if cpu_id == 0
    //     && (DIAGNOSTIC_PIN_FORK_CHILD_TO_PARENT_CPU
    //         || DIAGNOSTIC_PIN_FORK_CHILD_TO_BSP
    //         || DIAGNOSTIC_DISABLE_IDLE_WORK_STEALING
    //         || DIAGNOSTIC_DISABLE_TASK_MIGRATION
    //         || DIAGNOSTIC_RUN_UNPINNED_TASKS_ON_BSP
    //         || DIAGNOSTIC_RUN_ALL_USER_TASKS_ON_BSP
    //         || DIAGNOSTIC_RUN_USER_PROCESS_LEADERS_ON_BSP
    //         || DIAGNOSTIC_RUN_USER_THREADS_ON_BSP
    //         || DIAGNOSTIC_RETAIN_TERMINATED_TASKS)
    // {
    //     crate::println!(
    //         "[sched] diagnostic: pin_fork_child={} pin_fork_child_bsp={} disable_idle_work_stealing={} disable_task_migration={} unpinned_tasks_on_bsp={} all_user_tasks_on_bsp={} user_process_leaders_on_bsp={} user_threads_on_bsp={} retain_terminated_tasks={} bsp_cpu={}",
    //         DIAGNOSTIC_PIN_FORK_CHILD_TO_PARENT_CPU,
    //         DIAGNOSTIC_PIN_FORK_CHILD_TO_BSP,
    //         DIAGNOSTIC_DISABLE_IDLE_WORK_STEALING,
    //         DIAGNOSTIC_DISABLE_TASK_MIGRATION,
    //         DIAGNOSTIC_RUN_UNPINNED_TASKS_ON_BSP,
    //         DIAGNOSTIC_RUN_ALL_USER_TASKS_ON_BSP,
    //         DIAGNOSTIC_RUN_USER_PROCESS_LEADERS_ON_BSP,
    //         DIAGNOSTIC_RUN_USER_THREADS_ON_BSP,
    //         DIAGNOSTIC_RETAIN_TERMINATED_TASKS,
    //         BOOT_CPU_ID.load(Ordering::Acquire)
    //     );
    // }
    // crate::println!("[sched] cpu={} pick_next begin", cpu_id);
    let (_current_task_id, next_task_id) = pick_next(cpu);
    // crate::println!(
    //     "[sched] cpu={} pick_next complete current={:?} next={:?}",
    //     cpu_id,
    //     _current_task_id,
    //     next_task_id
    // );
    set_scheduler_ready(cpu_id, true);
    next_task_id
}

pub fn first_switch_to_kernel_task(task_id: usize) -> ! {
    let mut boot_context = crate::arch::context::KernelContext::new();
    let cpu_id = get_cpu().get_cpuid();
    let to_ctx_ptr = {
        let task = TaskPool::get_task(task_id).expect("first kernel task must remain registered");
        assert_eq!(
            task.running_cpu.load(Ordering::SeqCst),
            cpu_id,
            "first kernel task must be owned by the executing CPU"
        );
        // SAFETY: `running_cpu == cpu_id` above grants this CPU exclusive
        // access to the task's kernel context through the first switch.
        unsafe { task.kernel_context.as_mut_ptr() as *const crate::arch::context::KernelContext }
    };

    // SAFETY: `task_id` is the current runnable task selected by
    // `start_scheduler`, and `to_ctx_ptr` points to its initialized kernel
    // context. `running_cpu == cpu_id` prevents pool removal and concurrent
    // context mutation until this CPU releases the task, so the temporary pool
    // Arc above can be dropped before switching. `boot_context` remains valid
    // as the save area for the boot/AP context.
    unsafe {
        crate::arch::switch::switch_to(&mut boot_context as *mut _, to_ctx_ptr);
    }

    loop {
        idle();
    }
}

/// Get a non-owning guard for the task executing on `cpu_id`.
///
/// Current-task guards are valid only for the CPU that is executing this code.
/// Remote CPUs may change task state and release their `running_cpu` token, so
/// this function rejects remote CPU IDs rather than fabricating a borrowed task
/// reference. Use [`get_task_by_id`] for an owned handle to a remote task.
///
/// # Arguments
///
/// * `cpu_id` - The executing CPU's scheduler ID.
///
/// # Returns
///
/// A non-owning guard for the local current task, or `None` when the requested
/// CPU is remote, has no current task, or no longer owns that task.
pub fn current_task(cpu_id: usize) -> Option<CurrentTaskRef> {
    if get_cpu().get_cpuid() != cpu_id {
        return None;
    }

    let task_id = current_task_id(cpu_id)?;
    let task = TaskPool::get_task(task_id)?;
    if task.get_id() != task_id {
        return None;
    }
    let current_task = CurrentTaskRef::from_running_task(&task, cpu_id);

    // The guard is non-owning. The TaskPool slot and running_cpu token retain
    // the task while it executes on this CPU, so this temporary lookup Arc can
    // and must be dropped before returning to the caller.
    drop(task);
    current_task
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

pub fn get_task_by_id(task_id: usize) -> Option<Arc<Task>> {
    TaskPool::get_task(task_id)
}

pub fn wake_task(task_id: usize) -> bool {
    let target_cpu = {
        let Some(task) = TaskPool::get_task(task_id) else {
            return false;
        };
        select_wake_cpu_for_task(&task, get_time_ns())
    };
    wake_task_on(task_id, target_cpu)
}

pub fn wake_task_on(task_id: usize, target_cpu: usize) -> bool {
    let _irq_guard = IrqGuard::new();
    let Some(task) = TaskPool::get_task(task_id) else {
        return false;
    };
    let Some(target_cpu) = normalize_wake_cpu_for_task(&task, target_cpu, get_time_ns()) else {
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
        request_remote_reschedule(target_cpu);
    }

    true
}

pub fn cleanup_zombie(task_id: usize) {
    crate::breadcrumb::drop(crate::breadcrumb::ZOMBIE_CLEANUP, task_id as u64, 0);
    {
        let mut zombie_queue = ZOMBIE_QUEUE.lock();
        if let Some(pos) = zombie_queue.iter().position(|&id| id == task_id) {
            zombie_queue.remove(pos);
        }
    }

    if DIAGNOSTIC_RETAIN_TERMINATED_TASKS {
        return;
    }

    let _ = get_task_pool().remove_task(task_id);
}

/// Complete termination of a task that is not executing on the calling CPU.
///
/// A task with no CPU owner can be finalized immediately. A task still owned
/// by another CPU must remain in the pool until that CPU switches away and
/// [`release_deferred_prev`] releases its `running_cpu` claim.
///
/// # Arguments
///
/// * `task_id` - Global ID of the zombie or terminated task.
pub fn complete_non_current_task_exit(task_id: usize) {
    remove_from_ready_queues(task_id);
    unmark_blocked(task_id);

    let Some(task) = TaskPool::get_task(task_id) else {
        return;
    };
    let running_cpu = task.running_cpu.load(Ordering::SeqCst);
    if running_cpu == NO_CPU {
        match task.state.load(Ordering::SeqCst) {
            TaskState::Zombie => finalize_zombie(task_id, task.get_parent_id()),
            TaskState::Terminated => cleanup_zombie(task_id),
            TaskState::Running
            | TaskState::Ready
            | TaskState::Blocked(_)
            | TaskState::NotInitialized => {}
        }
    } else if is_cpu_online(running_cpu) {
        request_remote_reschedule(running_cpu);
    }
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
        crate::breadcrumb::drop(
            crate::breadcrumb::KCTX_ENTER,
            from_task_id as u64,
            to_task_id as u64,
        );
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
            // SAFETY: `from_task` is the task currently owned and executed by
            // this CPU, so its `running_cpu` token excludes concurrent context
            // mutation while the scheduler saves it.
            from_ctx_ptr = unsafe { from_task.kernel_context.as_mut_ptr() };

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
            // SAFETY: `pick_next()` claimed `to_task.running_cpu` for this CPU,
            // excluding concurrent context mutation until ownership is
            // released after a later switch.
            to_ctx_ptr = unsafe { to_task.kernel_context.as_mut_ptr() as *const _ };
        }

        if !from_ctx_ptr.is_null() && !to_ctx_ptr.is_null() {
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
                crate::breadcrumb::drop(
                    crate::breadcrumb::KCTX_SWITCH_TO,
                    from_task_id as u64,
                    to_task_id as u64,
                );
                crate::arch::switch::switch_to(from_ctx_ptr, to_ctx_ptr);
            }

            let resumed_cpu_id = get_cpu().get_cpuid();
            crate::breadcrumb::drop(
                crate::breadcrumb::KCTX_RESUME,
                resumed_cpu_id as u64,
                from_task_id as u64,
            );

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
            crate::breadcrumb::drop(
                crate::breadcrumb::REL_PREV_DONE,
                resumed_cpu_id as u64,
                from_task_id as u64,
            );

            #[cfg(feature = "hypervisor")]
            {
                guest_vcpu_switch_data.restore();
                hypervisor_switch_data.restore();
            }

            if let Some(from_task) = TaskPool::get_task(from_task_id) {
                crate::breadcrumb::drop(
                    crate::breadcrumb::GETTASK_DONE,
                    resumed_cpu_id as u64,
                    from_task_id as u64,
                );
                setup_task_cpu_state(get_cpu(), &from_task);
                crate::breadcrumb::drop(
                    crate::breadcrumb::SETUP_DONE,
                    resumed_cpu_id as u64,
                    from_task_id as u64,
                );
                set_trapvector(get_kernel_trapvector_paddr());

                #[cfg(feature = "user-fpu")]
                {
                    crate::breadcrumb::drop(
                        crate::breadcrumb::VCPU_LOCK_BEGIN,
                        resumed_cpu_id as u64,
                        from_task_id as u64,
                    );
                    crate::arch::fpu::kernel_switch_in_user_fpu(&mut *from_task.vcpu.lock());
                    crate::breadcrumb::drop(
                        crate::breadcrumb::VCPU_LOCK_DONE,
                        resumed_cpu_id as u64,
                        from_task_id as u64,
                    );
                }
            }
        }
    }
}

/// Bind CPU-local scheduler/trampoline state to the task that is about to run.
fn setup_task_cpu_state(cpu: &mut Arch, task: &Task) {
    let cpuid = cpu.get_cpuid();
    let sp = if let Some((_slot, base)) = task.get_kernel_stack_window_base() {
        (base + crate::environment::PAGE_SIZE + crate::environment::TASK_KERNEL_STACK_SIZE) as u64
    } else {
        task.get_kernel_stack_bottom_paddr()
    };

    let trampoline_arch = crate::vm::get_trampoline_arch(cpuid);
    crate::breadcrumb::drop_cpu(cpuid, crate::breadcrumb::TRAMP_GET, trampoline_arch as u64);
    crate::arch::set_arch(trampoline_arch);
    crate::breadcrumb::drop_cpu(cpuid, crate::breadcrumb::SET_ARCH_DONE, sp);
    cpu.set_kernel_stack(sp);
    crate::breadcrumb::drop_cpu(cpuid, crate::breadcrumb::SETSP_DONE, sp);
    cpu.set_trap_handler(get_user_trap_handler());
    crate::breadcrumb::drop_cpu(cpuid, crate::breadcrumb::SETTH_DONE, 0);
    let asid = task.vm_manager.get_asid();
    crate::breadcrumb::drop_cpu(cpuid, crate::breadcrumb::SETAS_ASID_DONE, asid as u64);
    cpu.set_next_address_space(asid);
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

    setup_task_cpu_state(cpu, task);
    let task_mode = task.vcpu.lock().get_mode();
    let trapframe = task.get_trapframe();
    task.vcpu.lock().switch(trapframe);

    set_next_mode(task_mode);
    set_trapvector(get_trampoline_trap_vector());
}

/// Reset the scheduler to initial state (test-only).
#[cfg(test)]
pub fn reset() {
    for cpu_id in 0..MAX_NUM_CPUS {
        ready_queue(cpu_id).lock().clear();
        set_current_task_id(cpu_id, None);
        set_scheduler_ready(cpu_id, false);
        SCHEDULE_PREV_TASK[cpu_id].store(0, Ordering::SeqCst);
        IDLE_TASK_IDS[cpu_id].store(0, Ordering::SeqCst);
        PENDING_IDLE_TO_USER_TRAP_TASK[cpu_id].store(0, Ordering::SeqCst);
        PENDING_RESCHEDULE[cpu_id].store(false, Ordering::SeqCst);
        PENDING_RESCHEDULE_IPI[cpu_id].store(false, Ordering::SeqCst);
        CPU_CORE_CLASSES[cpu_id].store(CpuCoreClass::Balanced as u8, Ordering::SeqCst);
        CPU_CAPACITIES[cpu_id].store(DEFAULT_CPU_CAPACITY, Ordering::SeqCst);
        CPU_TOPOLOGY_DOMAINS[cpu_id].store(INVALID_CPU_TOPOLOGY_DOMAIN, Ordering::SeqCst);
        DEBUG_REMOTE_ENQUEUE_TASK[cpu_id].store(0, Ordering::SeqCst);
        DEBUG_REMOTE_ENQUEUE_FROM_CPU[cpu_id].store(NO_CPU, Ordering::SeqCst);
        DEBUG_REMOTE_ENQUEUE_SEQ[cpu_id].store(0, Ordering::SeqCst);
        *slice_states()[cpu_id].lock() = SliceState::new();
    }
    slice_callback_contexts().lock().clear();
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
    fork_trace_tasks().lock().clear();
    fork_trace_picked_tasks().lock().clear();
    for slot in 0..FORK_TRACE_ATOMIC_SLOTS {
        FORK_TRACE_ATOMIC_CPU_MASKS[slot].store(0, Ordering::Relaxed);
        FORK_TRACE_ATOMIC_TASKS[slot].store(0, Ordering::Relaxed);
    }
    get_task_pool().reset();
}

#[cfg(test)]
pub(crate) fn set_current_task_for_test(cpu_id: usize, task_id: Option<usize>) {
    set_current_task_id(cpu_id, task_id);
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
    use crate::task::{
        TaskCorePreference, TaskType, cleanup_parent_waker, cleanup_task_waker,
        get_parent_waitpid_waker,
    };

    use super::*;

    #[test_case]
    fn test_deferred_reschedule_round_trip() {
        reset();
        assert!(!take_deferred_reschedule(0));
        defer_reschedule(0);
        assert!(take_deferred_reschedule(0));
        assert!(!take_deferred_reschedule(0));
        defer_reschedule(MAX_NUM_CPUS);
        assert!(!take_deferred_reschedule(MAX_NUM_CPUS));
    }

    #[test_case]
    fn test_reschedule_ipi_reservation_coalesces_until_acknowledged() {
        reset();

        assert!(reserve_reschedule_ipi(0));
        assert!(!reserve_reschedule_ipi(0));
        acknowledge_reschedule_ipi(0);
        assert!(reserve_reschedule_ipi(0));
        acknowledge_reschedule_ipi(0);
    }

    #[test_case]
    fn test_guest_timer_reschedule_defers_without_scheduling() {
        reset();
        slice_states()[0].lock().need_resched = true;

        assert!(consume_guest_timer_reschedule(0));
        assert!(take_deferred_reschedule(0));
        assert!(!consume_guest_timer_reschedule(0));
        assert!(!consume_guest_timer_reschedule(MAX_NUM_CPUS));
    }

    #[test_case]
    fn test_fork_trace_first_user_trap_is_tracked_per_task_and_cpu() {
        reset();
        let first_task = 100;
        let colliding_task = first_task + FORK_TRACE_ATOMIC_SLOTS;

        mark_fork_trace_task(first_task);
        mark_fork_trace_task(colliding_task);

        assert!(take_fork_trace_first_user_trap(0, first_task));
        assert!(!take_fork_trace_first_user_trap(0, first_task));
        assert!(take_fork_trace_first_user_trap(0, colliding_task));
        assert!(!take_fork_trace_first_user_trap(0, colliding_task));

        if MAX_NUM_CPUS > 1 {
            assert!(take_fork_trace_first_user_trap(1, first_task));
            assert!(!take_fork_trace_first_user_trap(1, first_task));
        }

        clear_fork_trace_task(first_task);
        clear_fork_trace_task(colliding_task);
        assert!(!take_fork_trace_first_user_trap(0, first_task));
        assert!(!take_fork_trace_first_user_trap(0, colliding_task));
    }

    #[test_case]
    fn test_scheduler_ready_gate_is_per_cpu_and_resettable() {
        reset();
        assert!(!scheduler_ready(0));
        assert!(!may_schedule_from_interrupt(0));
        assert!(!scheduler_ready(MAX_NUM_CPUS));
        assert!(!may_schedule_from_interrupt(MAX_NUM_CPUS));

        set_scheduler_ready(0, true);
        assert!(scheduler_ready(0));
        assert!(may_schedule_from_interrupt(0));

        if MAX_NUM_CPUS > 1 {
            assert!(!scheduler_ready(1));
            assert!(!may_schedule_from_interrupt(1));
        }

        reset();
        assert!(!scheduler_ready(0));
        assert!(!may_schedule_from_interrupt(0));
    }

    #[test_case]
    fn test_interrupt_schedule_gate_honors_preempt_count() {
        reset();
        set_scheduler_ready(0, true);
        assert!(may_schedule_from_interrupt(0));

        let preempt_guard = crate::sync::PreemptGuard::new();
        assert!(!may_schedule_from_interrupt(0));
        drop(preempt_guard);

        assert!(may_schedule_from_interrupt(0));
    }

    #[test_case]
    fn test_add_task() {
        reset();
        let task = Task::new("TestTask".to_string(), 1, TaskType::Kernel);
        add_task(task, 0);
        assert_eq!(READY_QUEUES[0].lock().len(), 1);
    }

    #[test_case]
    fn test_finalize_zombie_wakes_parent_thread_when_parent_is_missing() {
        reset();

        let parent_id = register_task(Task::new("Parent".to_string(), 1, TaskType::Kernel));
        let mut sibling = Task::new("ParentSibling".to_string(), 1, TaskType::Kernel);
        sibling.set_thread_group_id(parent_id);
        let sibling_id = register_task(sibling);
        let child_id = register_task(Task::new("Child".to_string(), 1, TaskType::Kernel));

        let parent = get_task_by_id(parent_id).unwrap();
        let sibling = get_task_by_id(sibling_id).unwrap();
        let child = get_task_by_id(child_id).unwrap();
        assert!(parent.adopt_registered_child(&child));
        assert_eq!(child.get_parent_thread_group_id(), Some(parent_id));

        sibling.state.store(
            TaskState::Blocked(crate::task::BlockedType::Interruptible),
            Ordering::SeqCst,
        );
        mark_blocked(sibling_id);
        assert!(get_all_task_ids().contains(&sibling_id));

        cleanup_parent_waker(sibling_id);
        let sibling_waker = get_parent_waitpid_waker(sibling_id);
        assert_eq!(sibling_waker.pending_wake_count_for_test(), 0);

        parent.state.store(TaskState::Terminated, Ordering::SeqCst);
        drop(parent);
        complete_non_current_task_exit(parent_id);
        assert!(get_task_by_id(parent_id).is_none());
        assert_eq!(child.get_parent_thread_group_id(), Some(parent_id));

        finalize_zombie(child_id, Some(parent_id));

        assert_eq!(sibling_waker.pending_wake_count_for_test(), 1);
        cleanup_task_waker(child_id);
        cleanup_parent_waker(parent_id);
        cleanup_parent_waker(sibling_id);
    }

    #[test_case]
    fn test_current_task_guard_does_not_clone_pool_arc() {
        reset();
        let cpu_id = get_cpu().get_cpuid();
        let task_id = register_task(Task::new(
            "CurrentTaskRefPool".to_string(),
            1,
            TaskType::Kernel,
        ));
        let task = TaskPool::get_task(task_id).unwrap();
        task.state.store(TaskState::Running, Ordering::SeqCst);
        task.running_cpu.store(cpu_id, Ordering::SeqCst);
        set_current_task_id(cpu_id, Some(task_id));
        let strong_count = Arc::strong_count(&task);

        let current_task_ref = current_task(cpu_id).expect("current task must be local");
        assert_eq!(current_task_ref.get_id(), task_id);
        assert_eq!(Arc::strong_count(&task), strong_count);
        if MAX_NUM_CPUS > 1 {
            let remote_cpu_id = (cpu_id + 1) % MAX_NUM_CPUS;
            assert!(current_task(remote_cpu_id).is_none());
        }

        drop(current_task_ref);
        assert_eq!(Arc::strong_count(&task), strong_count);
        set_current_task_id(cpu_id, None);
        task.running_cpu.store(NO_CPU, Ordering::SeqCst);
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
        assert_eq!(task.sched_migration_count(), 1);

        let stats = scheduler_migration_stats();
        assert_eq!(stats.total, 0);
        assert_eq!(stats.work_steals, 1);
    }

    #[test_case]
    fn test_pinned_task_cannot_migrate_or_be_stolen() {
        if MAX_NUM_CPUS < 2 {
            return;
        }

        reset();
        let source_cpu = 0;
        let target_cpu = 1;
        register_online_cpu(source_cpu);
        register_online_cpu(target_cpu);
        register_cpu_topology(source_cpu, CpuCoreClass::Balanced, 0).unwrap();
        register_cpu_topology(target_cpu, CpuCoreClass::Balanced, 0).unwrap();

        let mut task = Task::new("PinnedTask".to_string(), 1, TaskType::Kernel);
        task.pinned_cpu = Some(source_cpu);
        let task_id = register_task(task);
        let task = TaskPool::get_task(task_id).unwrap();
        task.state.store(TaskState::Ready, Ordering::SeqCst);
        push_ready_task(source_cpu, task_id);

        assert_eq!(
            normalize_wake_cpu_for_task(&task, target_cpu, 20_000_000),
            Some(source_cpu)
        );
        assert_eq!(
            migration_target_for_task(&task, source_cpu, 20_000_000, false),
            None
        );
        assert_eq!(steal_ready_task_for_cpu(target_cpu, 20_000_000), None);
        assert!(ready_queue(source_cpu).lock().contains(&task_id));
    }

    #[test_case]
    fn test_wake_target_normalizes_offline_request() {
        reset();
        let local_cpu = get_cpu().get_cpuid();
        register_online_cpu(local_cpu);

        let task_id = register_task(Task::new("WakeTargetTask".to_string(), 1, TaskType::Kernel));
        let task = TaskPool::get_task(task_id).unwrap();
        task.state.store(
            TaskState::Blocked(crate::task::BlockedType::Interruptible),
            Ordering::SeqCst,
        );
        mark_blocked(task_id);

        assert_eq!(
            normalize_wake_cpu_for_task(&task, MAX_NUM_CPUS, 20_000_000),
            Some(local_cpu)
        );
        assert!(wake_task_on(task_id, MAX_NUM_CPUS));
        assert_eq!(task.state.load(Ordering::SeqCst), TaskState::Ready);
        assert!(ready_queue(local_cpu).lock().contains(&task_id));
    }

    #[test_case]
    fn test_work_stealing_and_migration_are_enabled() {
        assert!(!DIAGNOSTIC_DISABLE_IDLE_WORK_STEALING);
        assert!(!DIAGNOSTIC_DISABLE_TASK_MIGRATION);
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

        assert_eq!(migration_target_for_task(&task, 0, 10_000_000, false), None);
        assert_eq!(task.sched_low_util_since_ns(), 10_000_000);
        assert_eq!(
            migration_target_for_task(&task, 0, 10_000_000 + SCHED_DEMOTION_SUSTAIN_NS - 1, false),
            None
        );
        assert_eq!(
            migration_target_for_task(&task, 0, 10_000_000 + SCHED_DEMOTION_SUSTAIN_NS, false),
            Some(1)
        );
    }

    #[test_case]
    fn test_migration_target_balances_within_topology_domain() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Performance, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Performance, 0).unwrap();
        register_cpu_topology_domain(0, 0xd).unwrap();
        register_cpu_topology_domain(1, 0xd).unwrap();

        let resident_id = register_task(Task::new("ResidentTask".to_string(), 1, TaskType::Kernel));
        let resident = TaskPool::get_task(resident_id).unwrap();
        resident.state.store(TaskState::Running, Ordering::SeqCst);
        resident.running_cpu.store(0, Ordering::SeqCst);
        set_current_task_id(0, Some(resident_id));

        let migrating_id = register_task(Task::new("PackedTask".to_string(), 1, TaskType::Kernel));
        let migrating = TaskPool::get_task(migrating_id).unwrap();
        migrating.state.store(TaskState::Ready, Ordering::SeqCst);

        assert_eq!(
            migration_target_for_task(&migrating, 0, 20_000_000, false),
            Some(1)
        );
    }

    #[test_case]
    fn test_migration_target_keeps_single_task_local() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Performance, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Performance, 0).unwrap();
        register_cpu_topology_domain(0, 0xd).unwrap();
        register_cpu_topology_domain(1, 0xd).unwrap();

        let task_id = register_task(Task::new("SingleTask".to_string(), 1, TaskType::Kernel));
        let task = TaskPool::get_task(task_id).unwrap();
        task.state.store(TaskState::Running, Ordering::SeqCst);
        task.running_cpu.store(0, Ordering::SeqCst);
        set_current_task_id(0, Some(task_id));

        assert_eq!(migration_target_for_task(&task, 0, 20_000_000, false), None);
    }

    #[test_case]
    fn test_lateral_balance_stays_inside_topology_domain() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Performance, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Performance, 0).unwrap();
        register_cpu_topology_domain(0, 0xd).unwrap();
        register_cpu_topology_domain(1, 0xe).unwrap();

        let resident_id =
            register_task(Task::new("DomainResident".to_string(), 1, TaskType::Kernel));
        let resident = TaskPool::get_task(resident_id).unwrap();
        resident.state.store(TaskState::Running, Ordering::SeqCst);
        resident.running_cpu.store(0, Ordering::SeqCst);
        set_current_task_id(0, Some(resident_id));

        let migrating_id =
            register_task(Task::new("DomainPinned".to_string(), 1, TaskType::Kernel));
        let migrating = TaskPool::get_task(migrating_id).unwrap();
        migrating.state.store(TaskState::Ready, Ordering::SeqCst);

        assert_eq!(
            migration_target_for_task(&migrating, 0, 20_000_000, false),
            None
        );
    }

    #[test_case]
    fn test_migration_demotion_tracking_resets_without_lower_target() {
        reset();
        register_online_cpu(0);
        register_online_cpu(1);
        register_cpu_topology(0, CpuCoreClass::Performance, 0).unwrap();
        register_cpu_topology(1, CpuCoreClass::Efficiency, 0).unwrap();

        let task = Task::new("DemotionResetTask".to_string(), 1, TaskType::Kernel);

        assert_eq!(migration_target_for_task(&task, 0, 10_000_000, false), None);
        assert_eq!(task.sched_low_util_since_ns(), 10_000_000);

        task.set_sched_util_min(SCHED_UTIL_SCALE).unwrap();
        assert_eq!(migration_target_for_task(&task, 0, 20_000_000, false), None);
        assert_eq!(task.sched_low_util_since_ns(), 0);

        task.set_sched_util_min(0).unwrap();
        assert_eq!(
            migration_target_for_task(&task, 0, 10_000_000 + SCHED_DEMOTION_SUSTAIN_NS, false),
            None
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
        SCHEDULE_PREV_TASK[0].store(task_id, Ordering::SeqCst);

        release_deferred_prev(0);

        assert_eq!(task.running_cpu.load(Ordering::SeqCst), NO_CPU);
        assert_eq!(task.last_cpu.load(Ordering::SeqCst), 1);
        assert!(ready_queue(1).lock().contains(&task_id));
        assert_eq!(task.sched_migration_count(), 1);
        let stats = scheduler_migration_stats();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.promotions, 1);
    }

    #[test_case]
    fn test_blocked_switch_out_releases_ownership_before_wake_and_reclaim() {
        reset();
        let cpu_id = get_cpu().get_cpuid();
        register_online_cpu(cpu_id);

        let blocked_id = register_task(Task::new(
            "BlockedSwitchOutTask".to_string(),
            1,
            TaskType::Kernel,
        ));
        let blocked = TaskPool::get_task(blocked_id).unwrap();
        blocked.state.store(
            TaskState::Blocked(crate::task::BlockedType::Interruptible),
            Ordering::SeqCst,
        );
        blocked.running_cpu.store(cpu_id, Ordering::SeqCst);
        set_current_task_id(cpu_id, Some(blocked_id));

        let next_id = register_task(Task::new(
            "BlockedSwitchOutNext".to_string(),
            1,
            TaskType::Kernel,
        ));
        let next = TaskPool::get_task(next_id).unwrap();
        next.state.store(TaskState::Ready, Ordering::SeqCst);
        push_ready_task(cpu_id, next_id);

        assert_eq!(pick_next(get_cpu()), (Some(blocked_id), Some(next_id)));
        assert_eq!(
            SCHEDULE_PREV_TASK[cpu_id].load(Ordering::SeqCst),
            blocked_id
        );
        assert_eq!(blocked.running_cpu.load(Ordering::SeqCst), cpu_id);

        release_deferred_prev(cpu_id);
        assert_eq!(blocked.running_cpu.load(Ordering::SeqCst), NO_CPU);
        assert_eq!(
            blocked.state.load(Ordering::SeqCst),
            TaskState::Blocked(crate::task::BlockedType::Interruptible)
        );
        assert!(!ready_queue(cpu_id).lock().contains(&blocked_id));

        assert!(wake_task(blocked_id));
        assert!(remove_ready_task_from_cpu(cpu_id, blocked_id));
        assert!(try_claim_ready_task(&blocked, cpu_id));
        assert_eq!(blocked.running_cpu.load(Ordering::SeqCst), cpu_id);

        set_current_task_id(cpu_id, None);
        blocked.running_cpu.store(NO_CPU, Ordering::SeqCst);
        next.running_cpu.store(NO_CPU, Ordering::SeqCst);
        drop(blocked);
        drop(next);
        reset();
    }

    #[test_case]
    fn test_deferred_release_reaps_terminated_task_after_lookup_drop() {
        reset();
        register_online_cpu(0);

        let task_id = register_task(Task::new(
            "DetachedThreadTask".to_string(),
            1,
            TaskType::Kernel,
        ));
        let task = TaskPool::get_task(task_id).unwrap();
        task.state.store(TaskState::Terminated, Ordering::SeqCst);
        task.running_cpu.store(0, Ordering::SeqCst);
        SCHEDULE_PREV_TASK[0].store(task_id, Ordering::SeqCst);

        release_deferred_prev(0);

        assert!(TaskPool::get_task(task_id).is_none());
        assert_eq!(get_task_pool().retired_tasks.lock().len(), 1);

        drop(task);
        assert_eq!(get_task_pool().reap_retired_tasks_for_test(), 1);
        assert!(get_task_pool().retired_tasks.lock().is_empty());
    }

    #[test_case]
    fn test_failed_ready_claim_retires_concurrently_terminated_task() {
        reset();

        let task_id = register_task(Task::new(
            "ClaimExitRaceTask".to_string(),
            1,
            TaskType::Kernel,
        ));
        let task = TaskPool::get_task(task_id).unwrap();
        task.state.store(TaskState::Terminated, Ordering::SeqCst);

        assert!(!try_claim_ready_task(&task, 0));
        assert_eq!(task.running_cpu.load(Ordering::SeqCst), NO_CPU);
        assert!(TaskPool::get_task(task_id).is_none());
        assert_eq!(get_task_pool().retired_tasks.lock().len(), 1);

        drop(task);
        assert_eq!(get_task_pool().reap_retired_tasks_for_test(), 1);
        assert!(get_task_pool().retired_tasks.lock().is_empty());
    }

    #[test_case]
    fn test_retirement_waits_for_outstanding_lookup_handle() {
        reset();

        let task_id = register_task(Task::new("RetiredTask".to_string(), 1, TaskType::Kernel));
        let lookup_handle = TaskPool::get_task(task_id).unwrap();
        lookup_handle
            .state
            .store(TaskState::Terminated, Ordering::SeqCst);

        let task_pool = get_task_pool();
        assert!(task_pool.remove_task(task_id));
        assert!(TaskPool::get_task(task_id).is_none());
        assert_eq!(task_pool.retired_tasks.lock().len(), 1);
        assert_eq!(task_pool.reap_retired_tasks_for_test(), 0);

        drop(lookup_handle);
        assert_eq!(task_pool.reap_retired_tasks_for_test(), 1);
        assert!(task_pool.retired_tasks.lock().is_empty());
    }

    #[test_case]
    fn test_user_ids_remain_stable_beyond_previous_lifetime_limit() {
        reset();

        let kernel_id = register_task(Task::new(
            "EarlyKernelTask".to_string(),
            1,
            TaskType::Kernel,
        ));
        assert_eq!(kernel_id, usize::MAX - 1);

        let task_pool = get_task_pool();
        for expected_id in 1..=(MAX_ACTIVE_USER_TASKS + 1) {
            let task_id = register_task(Task::new(
                "SequentialUserTask".to_string(),
                1,
                TaskType::User,
            ));
            assert_eq!(task_id, expected_id);
            let task = TaskPool::get_task(task_id).unwrap();
            task.state.store(TaskState::Terminated, Ordering::SeqCst);
            drop(task);
            assert!(task_pool.remove_task(task_id));
            assert_eq!(task_pool.reap_retired_tasks_for_test(), 1);
        }

        let kernel_task = TaskPool::get_task(kernel_id).unwrap();
        kernel_task
            .state
            .store(TaskState::Terminated, Ordering::SeqCst);
        drop(kernel_task);
        assert!(task_pool.remove_task(kernel_id));
        assert_eq!(task_pool.reap_retired_tasks_for_test(), 1);
    }

    #[test_case]
    fn test_retired_task_id_never_aliases_new_task_or_namespace_mapping() {
        reset();

        let task_pool = get_task_pool();
        let old_id = register_task(Task::new("OldStableId".to_string(), 1, TaskType::User));
        let old_task = TaskPool::get_task(old_id).unwrap();
        let namespace = old_task.get_namespace();
        let namespace_id = old_task.get_namespace_id();
        old_task
            .state
            .store(TaskState::Terminated, Ordering::SeqCst);
        drop(old_task);
        assert!(task_pool.remove_task(old_id));
        assert_eq!(task_pool.reap_retired_tasks_for_test(), 1);

        let new_id = register_task(Task::new("NewStableId".to_string(), 1, TaskType::User));
        assert_ne!(old_id, new_id);
        assert!(TaskPool::get_task(old_id).is_none());
        assert!(TaskPool::get_task(new_id).is_some());
        assert_eq!(namespace.resolve_global_id(namespace_id), None);
        assert_eq!(namespace.resolve_local_id(old_id), None);
    }

    #[test_case]
    fn test_user_active_capacity_is_reclaimed_without_reusing_ids() {
        reset();

        let task_pool = get_task_pool();
        let mut ids = Vec::new();
        for _ in 0..MAX_ACTIVE_USER_TASKS {
            ids.push(register_task(Task::new(
                "CapacityUserTask".to_string(),
                1,
                TaskType::User,
            )));
        }
        assert!(
            try_register_task(Task::new("OverflowUserTask".to_string(), 1, TaskType::User))
                .is_err()
        );

        let removed_id = ids.remove(0);
        let removed_task = TaskPool::get_task(removed_id).unwrap();
        removed_task
            .state
            .store(TaskState::Terminated, Ordering::SeqCst);
        drop(removed_task);
        assert!(task_pool.remove_task(removed_id));
        assert_eq!(task_pool.reap_retired_tasks_for_test(), 1);

        let replacement_id = register_task(Task::new(
            "ReplacementUserTask".to_string(),
            1,
            TaskType::User,
        ));
        assert!(replacement_id > removed_id);
        ids.push(replacement_id);

        for task_id in ids {
            let task = TaskPool::get_task(task_id).unwrap();
            task.state.store(TaskState::Terminated, Ordering::SeqCst);
            drop(task);
            assert!(task_pool.remove_task(task_id));
        }
        assert_eq!(
            task_pool.reap_retired_tasks_for_test(),
            MAX_ACTIVE_USER_TASKS
        );
    }
}

#[cfg(test)]
mod fair_tests {
    use super::*;

    fn key(deadline: u64, vruntime: u64, task_id: usize) -> FairKey {
        FairKey::new(deadline, vruntime, task_id)
    }

    fn insert_at(q: &mut FairQueue, task_id: usize, vruntime: u64, deadline: u64, weight: u32) {
        q.insert(task_id, key(deadline, vruntime, task_id), vruntime, weight);
    }

    #[test_case]
    fn sched_period_uses_latency_when_runners_fit() {
        // 5ms / 0.75ms = 6.66, so up to 6 runners stay inside the latency
        // target and the period equals SCHED_LATENCY_NS.
        assert_eq!(sched_period(1), SCHED_LATENCY_NS);
        assert_eq!(sched_period(6), SCHED_LATENCY_NS);
    }

    #[test_case]
    fn sched_period_grows_with_runner_count() {
        // 7 runners at 0.75 ms each exceeds 5 ms, so the period grows to
        // nr_running * SCHED_MIN_GRANULARITY_NS.
        assert_eq!(sched_period(7), 7 * SCHED_MIN_GRANULARITY_NS);
    }

    #[test_case]
    fn calc_delta_fair_scales_by_weight_ratio() {
        // delta * NICE_0_LOAD / weight; heavier weight => slower virtual time.
        assert_eq!(calc_delta_fair(1_000_000, NICE_0_LOAD), 1_000_000);
        // nice -5 weight 3121 vs nice 0 weight 1024: ratio ~3.05x slower
        let heavy = calc_delta_fair(1_000_000, 3121);
        let normal = calc_delta_fair(1_000_000, NICE_0_LOAD);
        assert!(heavy * 3 < normal);
        assert!(heavy * 4 > normal);
    }

    #[test_case]
    fn calc_delta_fair_handles_zero_weight() {
        // A zero weight must not divide by zero; fall back to real time.
        assert_eq!(calc_delta_fair(1_000, 0), 1_000);
    }

    #[test_case]
    fn sched_slice_proportional_to_weight() {
        // Two tasks of weight 1024 each on a queue whose total weight is 2048
        // each get half the period.
        let period = sched_period(2);
        let slice = sched_slice(period, 1024, 2048);
        assert_eq!(slice, period / 2);
    }

    #[test_case]
    fn sched_slice_clamps_to_min_granularity() {
        // A vanishingly-small weight must still receive at least
        // SCHED_MIN_GRANULARITY_NS so it is not starved of runnable time.
        let period = sched_period(2);
        let slice = sched_slice(period, 1, 1_000_000);
        assert_eq!(slice, SCHED_MIN_GRANULARITY_NS);
    }

    #[test_case]
    fn fair_deadline_is_vruntime_plus_virtual_slice() {
        // deadline = vruntime + slice * NICE_0_LOAD / weight.
        let deadline = fair_deadline(1_000, 1_000_000, NICE_0_LOAD);
        assert_eq!(deadline, 1_000 + 1_000_000);
    }

    #[test_case]
    fn empty_queue_picks_none() {
        let q = FairQueue::new();
        assert!(q.pick_eligible_min_deadline().is_none());
        assert!(q.is_empty());
        assert_eq!(q.avg_vruntime(), 0);
    }

    #[test_case]
    fn single_entity_is_always_eligible() {
        let mut q = FairQueue::new();
        insert_at(
            &mut q,
            1,
            /* vruntime */ 100,
            /* deadline */ 200,
            NICE_0_LOAD,
        );
        let picked = q
            .pick_eligible_min_deadline()
            .expect("single task eligible");
        assert_eq!(picked.task_id, 1);
    }

    #[test_case]
    fn pick_returns_smallest_deadline_among_eligible() {
        let mut q = FairQueue::new();
        // Two tasks at vruntime 100, eligible by construction. Their
        // deadlines differ; the smaller-deadline one must be picked first.
        insert_at(&mut q, 1, 100, 300, NICE_0_LOAD);
        insert_at(&mut q, 2, 100, 200, NICE_0_LOAD);
        let picked = q.pick_eligible_min_deadline().expect("non-empty queue");
        assert_eq!(picked.task_id, 2);
    }

    #[test_case]
    fn pick_skips_over_served_entity_then_snaps_forward() {
        let mut q = FairQueue::new();
        // Task 1 ran ahead: high vruntime. Task 2 is fresh: vruntime 0.
        // avg_vruntime is somewhere between; task 2 is the only eligible one.
        insert_at(&mut q, 1, 1_000_000, 1_000_500, NICE_0_LOAD);
        insert_at(&mut q, 2, 0, 500, NICE_0_LOAD);
        let picked = q.pick_eligible_min_deadline().expect("non-empty queue");
        assert_eq!(picked.task_id, 2);
    }

    #[test_case]
    fn min_vruntime_never_decreases_on_removal() {
        let mut q = FairQueue::new();
        insert_at(&mut q, 1, 100, 200, NICE_0_LOAD);
        insert_at(&mut q, 2, 500, 600, NICE_0_LOAD);
        let mid_min = q.min_vruntime;
        // Removing the lower-vruntime entity must not pull min backwards.
        q.remove(1);
        assert!(q.min_vruntime >= mid_min);
    }

    #[test_case]
    fn min_vruntime_advances_on_higher_vruntime_insert() {
        let mut q = FairQueue::new();
        insert_at(&mut q, 1, 100, 200, NICE_0_LOAD);
        let prev_min = q.min_vruntime;
        insert_at(&mut q, 2, 50, 60, NICE_0_LOAD);
        // The newly inserted entity has vruntime 50 < prev_min, so the floor
        // candidate is min(50, leftmost.vruntime=50)=50. min_vruntime only
        // advances, never decreases.
        assert!(q.min_vruntime >= prev_min);
    }

    #[test_case]
    fn rekey_advances_weighted_average() {
        let mut q = FairQueue::new();
        insert_at(&mut q, 1, 100, 200, NICE_0_LOAD);
        let avg_before = q.avg_vruntime();
        // Simulate running: vruntime 100 -> 200, deadline 200 -> 300.
        let new_key = key(300, 200, 1);
        q.rekey(1, 200, NICE_0_LOAD, new_key);
        let avg_after = q.avg_vruntime();
        assert!(avg_after > avg_before);
        assert_eq!(avg_after, 200);
    }

    #[test_case]
    fn rekey_key_for_returns_new_key() {
        let mut q = FairQueue::new();
        insert_at(&mut q, 1, 100, 200, NICE_0_LOAD);
        let new_key = key(300, 200, 1);
        q.rekey(1, 200, NICE_0_LOAD, new_key);
        assert_eq!(q.key_for(1), Some(new_key));
    }

    #[test_case]
    fn pick_with_all_eligible_returns_smallest_deadline() {
        let mut q = FairQueue::new();
        // Three entities all at vruntime 0 (all eligible), distinct deadlines.
        insert_at(&mut q, 1, 0, 900, NICE_0_LOAD);
        insert_at(&mut q, 2, 0, 100, NICE_0_LOAD);
        insert_at(&mut q, 3, 0, 500, NICE_0_LOAD);
        let picked = q.pick_eligible_min_deadline().expect("non-empty");
        assert_eq!(picked.task_id, 2);
    }

    #[test_case]
    fn avg_vruntime_uses_weighted_mean() {
        let mut q = FairQueue::new();
        // Task 1: vruntime 0, weight 1024. Task 2: vruntime 1000, weight 1024.
        // avg = (0*1024 + 1000*1024) / 2048 = 500.
        insert_at(&mut q, 1, 0, 100, NICE_0_LOAD);
        insert_at(&mut q, 2, 1000, 1100, NICE_0_LOAD);
        assert_eq!(q.avg_vruntime(), 500);
    }

    #[test_case]
    fn weighted_avg_scales_with_unbalanced_weights() {
        let mut q = FairQueue::new();
        // Task 1 weight 1024 at vruntime 0, task 2 weight 3072 at vruntime 1000.
        // avg = (0*1024 + 1000*3072) / 4096 = 750.
        insert_at(&mut q, 1, 0, 100, 1024);
        insert_at(&mut q, 2, 1000, 1100, 3072);
        assert_eq!(q.avg_vruntime(), 750);
    }
}
