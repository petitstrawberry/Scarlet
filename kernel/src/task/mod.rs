//! Task module.
//!
//! The task module defines the structure and behavior of tasks in the system.

pub mod elf_loader;
pub mod namespace;
pub mod syscall;

extern crate alloc;

use alloc::{
    boxed::Box,
    string::{String, ToString},
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{cell::UnsafeCell, sync::atomic};
use spin::mutex::SpinMutex;
use spin::{Mutex, RwLock};

use crate::abi::{AbiModule, EventProcessOutcome, scarlet::ScarletAbi};
use crate::device::char::tty::TtyDevice;
use crate::sync::waker::Waker;
use crate::{
    arch::{
        Trapframe, context::KernelContext, get_cpu, trap::user::arch_switch_to_user, vcpu::Vcpu,
        vm::alloc_virtual_address_space,
    },
    environment::{
        DEAFAULT_MAX_TASK_DATA_SIZE, DEAFAULT_MAX_TASK_STACK_SIZE, DEAFAULT_MAX_TASK_TEXT_SIZE,
        DEFAULT_TIME_SLICE, KERNEL_VM_STACK_END, PAGE_SIZE, USER_STACK_END,
    },
    fs::VfsManager,
    ipc::{EventContent, event::ProcessControlType},
    mem::page::ContiguousPages,
    object::{capability::memory_mapping::anon_owner::ForkCowPageOwner, handle::HandleTable},
    sched::scheduler::{
        cleanup_zombie, current_task, finalize_zombie, get_all_task_ids, get_task_by_id,
        remove_from_ready_queues, schedule, setup_task_execution, unmark_blocked,
    },
    timer::{TimerHandler, add_timer, get_tick},
    vm::{
        addr::{phys_to_virt, virt_to_phys},
        manager::VirtualMemoryManager,
        user_kernel_vm_init, user_vm_init,
        vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryRegion},
    },
};
use alloc::collections::BTreeMap;
use core::ops::Range;
use core::sync::atomic::{
    AtomicBool, AtomicI32, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
use spin::Once;

const INIT_TASK_ID: usize = 1;
const LOG_EXIT_GROUP_SIBLINGS: bool = false;
/// Scheduler utilization scale used for task placement hints.
pub const SCHED_UTIL_SCALE: u32 = 1024;

/// Lock type used for the architecture kernel context.
///
/// The scheduler needs a stable raw pointer to this context while performing
/// low-level context switches. `SpinMutex` exposes `as_mut_ptr()` for that
/// scheduler-only path, while normal setup code still uses `lock()`.
pub type KernelContextMutex = SpinMutex<KernelContext>;

/// Snapshot of task state exposed to user space via the `GetTaskInfo` syscall.
///
/// This is a fixed-size, `#[repr(C)]` structure so that the kernel and user
/// library can agree on the layout without sharing a header.
///
/// Kernel and user space must agree on this layout because
/// `GetTaskInfoList` does not take a per-entry size argument. Append fields
/// only as part of a coordinated kernel/user ABI update.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct TaskInfo {
    /// Namespace-local PID visible to user space.
    pub pid: usize,
    /// Namespace-local parent PID (0 if none).
    pub ppid: usize,
    /// Task state as a discriminant (see `TaskState::to_u8`):
    ///   0 = NotInitialized, 1 = Ready, 2 = Running,
    ///   3 = Blocked(Interruptible), 4 = Blocked(Uninterruptible),
    ///   5 = Zombie, 6 = Terminated.
    pub state: u8,
    /// Task type: 0 = Kernel, 1 = User.
    pub task_type: u8,
    /// CPU the task last ran on (MAX_CPU = no CPU).
    pub cpu_id: u8,
    /// Reserved for future use.
    pub _reserved: u8,
    /// Exit status (meaningful only when `state == Zombie`).
    pub exit_status: i32,
    /// Thread-group ID (process ID for multi-threaded tasks).
    pub tgid: usize,
    /// Null-terminated task name (truncated to fit).
    pub name: [u8; 64],
    /// Cumulative CPU time consumed by this task, in nanoseconds.
    pub cpu_time_ns: u64,
}

impl TaskInfo {
    /// Maximum task name length (excluding null terminator).
    pub const NAME_CAP: usize = 63;
}

/// Snapshot of system-wide CPU usage exposed to user space.
///
/// All time fields are cumulative nanoseconds since scheduler accounting
/// started. `busy_time_ns + idle_time_ns` is the accounted CPU capacity across
/// all online CPUs.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct CpuUsageInfo {
    /// Number of CPUs currently known to the scheduler.
    pub online_cpus: usize,
    /// Cumulative non-idle CPU time in nanoseconds.
    pub busy_time_ns: u64,
    /// Cumulative idle task CPU time in nanoseconds.
    pub idle_time_ns: u64,
    /// Total accounted CPU time in nanoseconds.
    pub total_time_ns: u64,
    /// Busy percentage in permille (1000 = 100.0%).
    pub usage_per_mille: u32,
    /// Reserved for future use.
    pub _reserved: u32,
}

/// Global registry of task-specific wakers for waitpid
static WAITPID_WAKERS: Once<Mutex<BTreeMap<usize, Waker>>> = Once::new();

/// Note: task ID counters live in `TaskPool` for better ID management,
/// including recycling of freed task IDs.
///
/// Global registry of parent task wakers for waitpid(-1) operations
/// Each parent task has a waker that gets triggered when any of its children exit
static PARENT_WAITPID_WAKERS: Once<Mutex<BTreeMap<usize, Waker>>> = Once::new();

/// Initialize the waitpid wakers registry
fn init_waitpid_wakers() -> Mutex<BTreeMap<usize, Waker>> {
    Mutex::new(BTreeMap::new())
}

/// Initialize the parent waitpid waker registry
fn init_parent_waitpid_wakers() -> Mutex<BTreeMap<usize, Waker>> {
    Mutex::new(BTreeMap::new())
}

/// Get or create a waker for waitpid/wait operations for a specific task
///
/// This function returns a reference to the waker associated with the given task ID,
/// used exclusively for waitpid/wait (child termination wait) synchronization.
/// If no waker exists for the task, a new one is created.
///
/// # Arguments
///
/// * `task_id` - The ID of the task to get a waitpid/wait waker for
///
/// # Returns
///
/// A reference to the waker for the specified task
pub fn get_waitpid_waker(task_id: usize) -> &'static Waker {
    let wakers_mutex = WAITPID_WAKERS.call_once(init_waitpid_wakers);
    let mut wakers = wakers_mutex.lock();
    if !wakers.contains_key(&task_id) {
        let waker_name = alloc::format!("task_{}", task_id);
        // We need to create a static string for the waker name
        let static_name = Box::leak(waker_name.into_boxed_str());
        wakers.insert(task_id, Waker::new_interruptible(static_name));
    }
    // This is safe because we know the waker exists and won't be removed
    // until the task is cleaned up
    unsafe {
        let waker_ptr = wakers.get(&task_id).unwrap() as *const Waker;
        &*waker_ptr
    }
}

// pub fn get_select_waker(...) was removed; use object-level Selectable::wait_until_ready

/// Get or create a parent waker for waitpid(-1) operations
///
/// This waker is used when a parent process calls waitpid(-1) to wait for any child to exit.
/// It is separate from the task-specific waitpid wakers to avoid conflicts, and is used
/// exclusively for waitpid(-1) (any child termination wait) synchronization.
///
/// # Arguments
///
/// * `parent_id` - The ID of the parent task
///
/// # Returns
///
/// A reference to the parent waker
pub fn get_parent_waitpid_waker(parent_id: usize) -> &'static Waker {
    let wakers_mutex = PARENT_WAITPID_WAKERS.call_once(init_parent_waitpid_wakers);
    let mut wakers = wakers_mutex.lock();

    // Create a new waker if it doesn't exist
    if !wakers.contains_key(&parent_id) {
        let waker_name = alloc::format!("parent_waker_{}", parent_id);
        // We need to leak the string to make it 'static
        let static_name = alloc::boxed::Box::leak(waker_name.into_boxed_str());
        wakers.insert(parent_id, Waker::new_interruptible(static_name));
    }

    // Return a reference to the waker
    // This is safe because the BTreeMap is never dropped and the Waker is never moved
    unsafe {
        let waker_ptr = wakers.get(&parent_id).unwrap() as *const Waker;
        &*waker_ptr
    }
}

/// Wake up any processes waiting for a specific task
///
/// This function should be called when a task exits to wake up
/// any parent processes that are waiting for this specific task.
///
/// # Arguments
///
/// * `task_id` - The ID of the task that has exited
pub fn wake_task_waiters(task_id: usize) {
    let wakers_mutex = WAITPID_WAKERS.call_once(init_waitpid_wakers);
    let wakers = wakers_mutex.lock();
    if let Some(waker) = wakers.get(&task_id) {
        waker.wake_all();
    }
}

/// Wake up a parent process waiting for any child (waitpid(-1))
///
/// This function should be called when any child of a parent exits.
///
/// # Arguments
///
/// * `parent_id` - The ID of the parent task
pub fn wake_parent_waiters(parent_id: usize) {
    let wakers_mutex = PARENT_WAITPID_WAKERS.call_once(init_parent_waitpid_wakers);
    let wakers = wakers_mutex.lock();
    if let Some(waker) = wakers.get(&parent_id) {
        waker.wake_all();
    }
}

/// Clean up the waker for a specific task
///
/// This function should be called when a task is completely cleaned up
/// to remove its waker from the global registry.
///
/// # Arguments
///
/// * `task_id` - The ID of the task to clean up
pub fn cleanup_task_waker(task_id: usize) {
    let wakers_mutex = WAITPID_WAKERS.call_once(init_waitpid_wakers);
    let mut wakers = wakers_mutex.lock();
    wakers.remove(&task_id);
}

/// Clean up the parent waker for a specific task
///
/// This function should be called when a parent task is completely cleaned up.
///
/// # Arguments
///
/// * `parent_id` - The ID of the parent task to clean up
pub fn cleanup_parent_waker(parent_id: usize) {
    let wakers_mutex = PARENT_WAITPID_WAKERS.call_once(init_parent_waitpid_wakers);
    let mut wakers = wakers_mutex.lock();
    wakers.remove(&parent_id);
}

// pub fn cleanup_select_waker(...) was removed along with task-level select waker

/// Types of blocked states for tasks
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum BlockedType {
    /// Interruptible blocking - can be interrupted by signals
    Interruptible,
    /// Uninterruptible blocking - cannot be interrupted, must wait for completion
    Uninterruptible,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum TaskState {
    NotInitialized,
    Ready,
    Running,
    Blocked(BlockedType),
    Zombie,
    Terminated,
}

impl TaskState {
    /// Convert TaskState to u8 for atomic storage
    pub const fn to_u8(self) -> u8 {
        match self {
            TaskState::NotInitialized => 0,
            TaskState::Ready => 1,
            TaskState::Running => 2,
            TaskState::Blocked(bt) => match bt {
                BlockedType::Interruptible => 3,
                BlockedType::Uninterruptible => 4,
            },
            TaskState::Zombie => 5,
            TaskState::Terminated => 6,
        }
    }

    /// Convert u8 to TaskState
    pub const fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(TaskState::NotInitialized),
            1 => Some(TaskState::Ready),
            2 => Some(TaskState::Running),
            3 => Some(TaskState::Blocked(BlockedType::Interruptible)),
            4 => Some(TaskState::Blocked(BlockedType::Uninterruptible)),
            5 => Some(TaskState::Zombie),
            6 => Some(TaskState::Terminated),
            _ => None,
        }
    }
}

/// Atomic task state for thread-safe state management
pub struct AtomicTaskState {
    inner: AtomicU8,
}

impl AtomicTaskState {
    pub const fn new(state: TaskState) -> Self {
        Self {
            inner: AtomicU8::new(state.to_u8()),
        }
    }

    pub fn load(&self, ordering: Ordering) -> TaskState {
        TaskState::from_u8(self.inner.load(ordering)).unwrap_or(TaskState::NotInitialized)
    }

    pub fn store(&self, state: TaskState, ordering: Ordering) {
        self.inner.store(state.to_u8(), ordering);
    }

    pub fn compare_exchange(
        &self,
        current: TaskState,
        new: TaskState,
        success: Ordering,
        failure: Ordering,
    ) -> Result<TaskState, TaskState> {
        match self
            .inner
            .compare_exchange(current.to_u8(), new.to_u8(), success, failure)
        {
            Ok(_) => Ok(new),
            Err(actual) => Err(TaskState::from_u8(actual).unwrap_or(TaskState::NotInitialized)),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum TaskType {
    Kernel,
    User,
}

/// Scheduler hint for heterogeneous CPU placement.
#[derive(Debug, PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum TaskCorePreference {
    /// No explicit core-class preference.
    Any = 0,
    /// Prefer energy-efficient cores when load permits.
    Efficiency = 1,
    /// Prefer higher-capacity cores.
    Performance = 2,
}

impl TaskCorePreference {
    pub(crate) const fn to_u8(self) -> u8 {
        self as u8
    }

    pub(crate) const fn from_u8(value: u8) -> Self {
        match value {
            1 => TaskCorePreference::Efficiency,
            2 => TaskCorePreference::Performance,
            _ => TaskCorePreference::Any,
        }
    }
}

/// ABI Zone structure holding a memory range with an owned ABI module.
pub struct AbiZone {
    pub range: Range<usize>,
    pub abi: Box<dyn AbiModule + Send + Sync>,
}

/// A cell type for task-local data that is only accessed by the hart currently
/// executing the task.
///
/// # Safety
///
/// This type uses `UnsafeCell` internally and is `Sync` so it can live inside
/// `Task` (which is `Send + Sync`).  The safety invariant is:
///
/// * **Only the hart that is currently running this task may access the
///   contents.**  Because a task is scheduled on exactly one hart at a time,
///   there is no concurrent access and no lock is needed.
/// * During `clone_task`, the **parent** accesses its own `TaskLocal` fields
///   (safe – it is the running hart) and writes to the **child's** `TaskLocal`
///   fields (safe – the child has not been added to the scheduler yet, so no
///   other hart can touch it).
pub struct TaskLocal<T> {
    inner: UnsafeCell<T>,
}

// SAFETY: Access is restricted to the hart executing the owning task.
// See the doc comment on `TaskLocal` for the full safety argument.
unsafe impl<T> Sync for TaskLocal<T> {}

impl<T> TaskLocal<T> {
    /// Create a new `TaskLocal` with the given value.
    pub fn new(value: T) -> Self {
        Self {
            inner: UnsafeCell::new(value),
        }
    }

    /// Get an immutable reference to the contained value.
    ///
    /// # Safety
    ///
    /// The caller must be the hart currently executing the owning task,
    /// or the task must not yet be visible to the scheduler.
    #[inline]
    pub unsafe fn get(&self) -> &T {
        // SAFETY: Upheld by caller (single-hart-per-task invariant).
        unsafe { &*self.inner.get() }
    }

    /// Get a mutable reference to the contained value.
    ///
    /// # Safety
    ///
    /// The caller must be the hart currently executing the owning task,
    /// or the task must not yet be visible to the scheduler.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_mut(&self) -> &mut T {
        // SAFETY: Upheld by caller (single-hart-per-task invariant).
        unsafe { &mut *self.inner.get() }
    }
}

pub struct Task {
    // === Read-only fields (set at creation) ===
    id: usize,
    /// Task ID within the task's namespace (may differ from global ID)
    namespace_id: atomic::AtomicUsize,
    /// Task namespace for ID management
    namespace: RwLock<Arc<namespace::TaskNamespace>>,
    pub task_type: TaskType,
    pub entry: usize,
    parent_id: AtomicUsize,
    /// Thread Group ID (TGID) - identifies tasks in the same thread group
    thread_group_id: usize,
    /// Legacy task group ID mirror.
    ///
    /// New job-control code should use `process_group_id`; this field is kept
    /// during the migration so older Scarlet-private controls continue to see
    /// the same value.
    task_group_id: AtomicUsize,
    /// POSIX session ID (SID), stored as a global task ID.
    session_id: AtomicUsize,
    /// POSIX process group ID (PGID), stored as a global task ID.
    process_group_id: AtomicUsize,
    /// Controlling terminal for this task, if any.
    controlling_tty: RwLock<Option<Weak<TtyDevice>>>,
    /// Whether this task is the leader of its POSIX session.
    is_session_leader: AtomicBool,
    pub max_stack_size: usize,
    pub max_data_size: usize,
    pub max_text_size: usize,

    // === Atomic fields (lock-free) ===
    /// Task state with atomic transitions
    pub state: AtomicTaskState,
    /// Task priority
    pub priority: AtomicU32,
    /// Scheduler hint for heterogeneous CPU placement.
    core_preference: AtomicU8,
    /// Minimum scheduler utilization required by this task.
    sched_util_min: AtomicU32,
    /// Time slice for scheduling
    pub time_slice: AtomicU32,
    pub default_time_slice: AtomicU32,
    /// Cumulative CPU time charged to this task, in nanoseconds.
    pub cpu_time_ns: AtomicU64,
    /// Monotonic timestamp at which the current CPU run began.
    cpu_run_start_ns: AtomicU64,
    /// Stack size in bytes
    pub stack_size: AtomicUsize,
    /// Data segment size in bytes
    pub data_size: AtomicUsize,
    /// Text segment size in bytes
    pub text_size: AtomicUsize,
    /// Exit status (i32::MIN represents None)
    pub exit_status: AtomicI32,
    /// Set when a process-control stop should be observable by waitpid.
    process_control_stopped: AtomicBool,
    /// Set after the current process-control stop has been reported once.
    process_control_stop_reported: AtomicBool,
    /// Program break (already thread-safe)
    pub brk: Arc<AtomicUsize>,

    // === RwLock fields (frequent reads) ===
    /// Task name
    pub name: RwLock<String>,
    /// List of child task IDs
    pub children: RwLock<Vec<usize>>,
    /// Contiguous page allocations (PMM-backed, auto-freed on drop).
    ///
    /// Each entry is a `ContiguousPages` RAII wrapper that returns its pages to the
    /// buddy-system PMM when dropped. Used for ELF segment and anonymous mappings
    /// that require physically contiguous memory.
    pub page_allocations: RwLock<Vec<ContiguousPages>>,
    /// Non-contiguous individual page allocations (PMM-backed, auto-freed on drop).
    ///
    /// Each entry is a `TaskPages` RAII wrapper holding a list of individual
    /// physical page addresses. Used for anonymous private mappings where
    /// physical contiguity is not required and partial reclaim on unmap is needed.
    pub task_pages: RwLock<Vec<crate::mem::page::TaskPages>>,
    /// Virtual File System Manager
    ///
    /// # Usage Patterns
    ///
    /// - `None`: Task uses global filesystem namespace (traditional Unix-like behavior)
    /// - `Some(Arc<VfsManager>)`: Task has isolated filesystem namespace (container-like behavior)
    ///
    /// # Thread Safety
    ///
    /// VfsManager is thread-safe and can be shared between tasks using Arc.
    /// All internal operations use RwLock for concurrent access protection.
    pub vfs: RwLock<Option<Arc<VfsManager>>>,
    /// Software timer handlers
    pub software_timers_handlers: RwLock<Vec<Arc<dyn TimerHandler>>>,

    // === Mutex fields (complex operations) ===
    /// VCPU state for context switching
    pub vcpu: Mutex<Vcpu>,
    /// Kernel context for context switching
    pub kernel_context: KernelContextMutex,
    /// Virtual memory manager (already thread-safe internally)
    pub vm_manager: VirtualMemoryManager,
    /// Default ABI module (task-local: only accessed by the executing hart)
    pub default_abi: TaskLocal<Option<Box<dyn AbiModule + Send + Sync>>>,
    /// ABI zones map (task-local: only accessed by the executing hart)
    pub abi_zones: TaskLocal<BTreeMap<usize, AbiZone>>,
    /// Handle table for kernel objects (already thread-safe internally)
    pub handle_table: HandleTable,
    /// Waker for sleep operations (already thread-safe internally)
    pub sleep_waker: Waker,
    /// Kernel stack window base (slot_index, base_vaddr)
    pub kernel_stack_window_base: Mutex<Option<(usize, usize)>>,
    pub pinned_cpu: Option<usize>,
    pub last_cpu: atomic::AtomicUsize,
    /// CPU that currently "owns" this task (has saved its context or is
    /// actively running it). `usize::MAX` means unowned / available.
    /// Used as a CAS claim token to prevent double-scheduling on SMP.
    pub running_cpu: atomic::AtomicUsize,

    // === Already protected fields ===
    /// Task-local event queue with priority ordering
    pub event_queue: Mutex<crate::ipc::event::TaskEventQueue>,
    /// Event processing enabled flag
    pub events_enabled: Mutex<bool>,
}

pub enum CloneFlagsDef {
    Vm = 0b00000001,      // Clone the VM
    Fs = 0b00000010,      // Clone the filesystem
    Files = 0b00000100,   // Clone the file descriptors
    Thread = 0b00001000,  // Join thread group (share TGID) - Linux CLONE_THREAD semantics
    SetTls = 0b000010000, // Set TLS pointer for cloned task
}

#[derive(Debug, Clone, Copy)]
pub struct CloneFlags {
    raw: u64,
}

impl CloneFlags {
    pub fn new() -> Self {
        CloneFlags { raw: 0 }
    }

    pub fn from_raw(raw: u64) -> Self {
        CloneFlags { raw }
    }

    pub fn set(&mut self, flag: CloneFlagsDef) {
        self.raw |= flag as u64;
    }

    pub fn clear(&mut self, flag: CloneFlagsDef) {
        self.raw &= !(flag as u64);
    }

    pub fn is_set(&self, flag: CloneFlagsDef) -> bool {
        (self.raw & (flag as u64)) != 0
    }

    pub fn get_raw(&self) -> u64 {
        self.raw
    }
}

impl Default for CloneFlags {
    fn default() -> Self {
        let raw = CloneFlagsDef::Fs as u64 | CloneFlagsDef::Files as u64;
        CloneFlags { raw }
    }
}

impl Task {
    /// Create a new task with the root namespace.
    ///
    /// # Arguments
    /// * `name` - Task name
    /// * `priority` - Task priority
    /// * `task_type` - Task type (Kernel or User)
    ///
    /// # Returns
    /// A new task in the root namespace
    pub fn new(name: String, priority: u32, task_type: TaskType) -> Self {
        Self::new_with_namespace(
            name,
            priority,
            task_type,
            namespace::get_root_namespace().clone(),
        )
    }

    /// Create a new task with a specific namespace.
    ///
    /// # Arguments
    /// * `name` - Task name
    /// * `priority` - Task priority
    /// * `task_type` - Task type (Kernel or User)
    /// * `ns` - Task namespace
    ///
    /// # Returns
    /// A new task in the specified namespace
    pub fn new_with_namespace(
        name: String,
        priority: u32,
        task_type: TaskType,
        ns: Arc<namespace::TaskNamespace>,
    ) -> Self {
        Task {
            // Read-only fields
            id: 0,
            namespace_id: AtomicUsize::new(0),
            namespace: RwLock::new(ns),
            task_type,
            entry: 0,
            parent_id: AtomicUsize::new(0),
            thread_group_id: 0,
            task_group_id: AtomicUsize::new(0),
            session_id: AtomicUsize::new(0),
            process_group_id: AtomicUsize::new(0),
            controlling_tty: RwLock::new(None),
            is_session_leader: AtomicBool::new(false),
            max_stack_size: DEAFAULT_MAX_TASK_STACK_SIZE,
            max_data_size: DEAFAULT_MAX_TASK_DATA_SIZE,
            max_text_size: DEAFAULT_MAX_TASK_TEXT_SIZE,
            // Atomic fields
            state: AtomicTaskState::new(TaskState::NotInitialized),
            priority: AtomicU32::new(priority),
            core_preference: AtomicU8::new(TaskCorePreference::Any.to_u8()),
            sched_util_min: AtomicU32::new(0),
            time_slice: AtomicU32::new(DEFAULT_TIME_SLICE),
            default_time_slice: AtomicU32::new(DEFAULT_TIME_SLICE),
            cpu_time_ns: AtomicU64::new(0),
            cpu_run_start_ns: AtomicU64::new(0),
            stack_size: AtomicUsize::new(0),
            data_size: AtomicUsize::new(0),
            text_size: AtomicUsize::new(0),
            exit_status: AtomicI32::new(i32::MIN),
            process_control_stopped: AtomicBool::new(false),
            process_control_stop_reported: AtomicBool::new(false),
            brk: Arc::new(AtomicUsize::new(usize::MAX)),
            // RwLock fields
            name: RwLock::new(name),
            children: RwLock::new(Vec::new()),
            page_allocations: RwLock::new(Vec::new()),
            task_pages: RwLock::new(Vec::new()),
            vfs: RwLock::new(None),
            software_timers_handlers: RwLock::new(Vec::new()),
            // Mutex fields
            vcpu: Mutex::new(Vcpu::new(match task_type {
                TaskType::Kernel => crate::arch::Mode::Kernel,
                TaskType::User => crate::arch::Mode::User,
            })),
            kernel_context: KernelContextMutex::new(KernelContext::new()),
            vm_manager: VirtualMemoryManager::new(),
            default_abi: TaskLocal::new(Some(Box::new(ScarletAbi::default()))),
            abi_zones: TaskLocal::new(BTreeMap::new()),
            handle_table: HandleTable::new(),
            sleep_waker: Waker::new_interruptible("task_sleep_waker"),
            kernel_stack_window_base: Mutex::new(None),
            pinned_cpu: None,
            last_cpu: atomic::AtomicUsize::new(0),
            running_cpu: atomic::AtomicUsize::new(usize::MAX),
            // Already protected
            event_queue: Mutex::new(crate::ipc::event::TaskEventQueue::new()),
            events_enabled: Mutex::new(true),
        }
    }

    pub fn init(&self) {
        // Initialize kernel context with the task's entry point
        // The kernel stack is allocated within the KernelContext
        *self.kernel_context.lock() = KernelContext::new();

        match self.task_type {
            TaskType::Kernel => {
                user_kernel_vm_init(self);
                /* Set sp to the top of the kernel stack */
                self.vcpu.lock().set_sp(KERNEL_VM_STACK_END + 1);
                /* Set pc to the task's entry point */
                self.vcpu.lock().set_pc(self.entry as u64);
            }
            TaskType::User => {
                user_vm_init(self);
                /* Set sp to the top of the user stack */
                self.vcpu.lock().set_sp(USER_STACK_END);
                /* PC will be set when loading the ELF binary */
            }
        }

        /* Set the task state to Ready */
        self.state.store(TaskState::Ready, Ordering::SeqCst);
        self.time_slice.store(
            self.default_time_slice.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
    }

    /// Return the scheduler core preference hint.
    ///
    /// # Returns
    ///
    /// The current heterogeneous CPU placement preference.
    pub fn core_preference(&self) -> TaskCorePreference {
        TaskCorePreference::from_u8(self.core_preference.load(Ordering::SeqCst))
    }

    /// Set the scheduler core preference hint.
    ///
    /// # Arguments
    ///
    /// * `preference` - Core class preference used for future CPU placement.
    pub fn set_core_preference(&self, preference: TaskCorePreference) {
        self.core_preference
            .store(preference.to_u8(), Ordering::SeqCst);
    }

    /// Return the minimum scheduler utilization requested by this task.
    ///
    /// # Returns
    ///
    /// Minimum utilization in scheduler capacity units, where
    /// [`SCHED_UTIL_SCALE`] represents a full-capacity CPU.
    pub fn sched_util_min(&self) -> u32 {
        self.sched_util_min.load(Ordering::SeqCst)
    }

    /// Set the minimum scheduler utilization requested by this task.
    ///
    /// # Arguments
    ///
    /// * `util_min` - Minimum utilization in scheduler capacity units.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an error if `util_min` is outside the supported
    /// range.
    pub fn set_sched_util_min(&self, util_min: u32) -> Result<(), &'static str> {
        if util_min > SCHED_UTIL_SCALE {
            return Err("scheduler util_min out of range");
        }
        self.sched_util_min.store(util_min, Ordering::SeqCst);
        Ok(())
    }

    /// Mark the task as running for CPU accounting.
    ///
    /// # Arguments
    ///
    /// * `now_ns` - Current monotonic timestamp in nanoseconds.
    pub fn start_cpu_accounting(&self, now_ns: u64) {
        self.cpu_run_start_ns.store(now_ns, Ordering::SeqCst);
    }

    /// Stop charging CPU time to this task and return the elapsed delta.
    ///
    /// # Arguments
    ///
    /// * `now_ns` - Current monotonic timestamp in nanoseconds.
    ///
    /// # Returns
    ///
    /// The nanoseconds charged by this stop operation.
    pub fn stop_cpu_accounting(&self, now_ns: u64) -> u64 {
        let start_ns = self.cpu_run_start_ns.swap(0, Ordering::SeqCst);
        if start_ns == 0 {
            return 0;
        }
        let delta_ns = now_ns.saturating_sub(start_ns);
        self.cpu_time_ns.fetch_add(delta_ns, Ordering::SeqCst);
        delta_ns
    }

    /// Return the current CPU time snapshot for this task.
    ///
    /// # Arguments
    ///
    /// * `now_ns` - Current monotonic timestamp in nanoseconds.
    ///
    /// # Returns
    ///
    /// Cumulative CPU time, including the current running interval if any.
    pub fn cpu_time_snapshot_ns(&self, now_ns: u64) -> u64 {
        self.cpu_time_ns
            .load(Ordering::SeqCst)
            .saturating_add(self.current_cpu_delta_ns(now_ns))
    }

    /// Return the current uncommitted running interval for this task.
    ///
    /// # Arguments
    ///
    /// * `now_ns` - Current monotonic timestamp in nanoseconds.
    ///
    /// # Returns
    ///
    /// Nanoseconds elapsed since the task was last scheduled in.
    pub fn current_cpu_delta_ns(&self, now_ns: u64) -> u64 {
        let start_ns = self.cpu_run_start_ns.load(Ordering::SeqCst);
        if start_ns == 0 {
            0
        } else {
            now_ns.saturating_sub(start_ns)
        }
    }

    pub fn get_id(&self) -> usize {
        assert!(
            self.id != 0,
            "Task ID is 0 - task may not have been added to scheduler yet"
        );
        self.id
    }

    /// Set the task ID (used by TaskPool during task addition)
    pub fn set_id(&mut self, id: usize) {
        self.id = id;
        if self.thread_group_id == 0 {
            self.thread_group_id = id;
        }
        if self.task_group_id.load(Ordering::SeqCst) == 0 {
            self.task_group_id.store(id, Ordering::SeqCst);
        }
        if self.process_group_id.load(Ordering::SeqCst) == 0 {
            self.process_group_id.store(id, Ordering::SeqCst);
        }
        if self.session_id.load(Ordering::SeqCst) == 0 {
            self.session_id.store(id, Ordering::SeqCst);
            self.is_session_leader.store(true, Ordering::SeqCst);
        }
    }

    pub fn get_task_group_id(&self) -> usize {
        self.get_process_group_id()
    }

    pub fn set_task_group_id(&self, task_group_id: usize) {
        self.set_process_group_id(task_group_id);
    }

    /// Get the POSIX process group ID (PGID).
    ///
    /// # Returns
    /// The global task ID that names this task's process group.
    pub fn get_process_group_id(&self) -> usize {
        let pgid = self.process_group_id.load(Ordering::SeqCst);
        if pgid == 0 {
            self.task_group_id.load(Ordering::SeqCst)
        } else {
            pgid
        }
    }

    /// Set the POSIX process group ID (PGID).
    ///
    /// # Arguments
    /// * `task_group_id` - Global task ID that names the target process group.
    pub fn set_process_group_id(&self, task_group_id: usize) {
        self.process_group_id.store(task_group_id, Ordering::SeqCst);
        self.task_group_id.store(task_group_id, Ordering::SeqCst);
    }

    /// Get the POSIX session ID (SID).
    ///
    /// # Returns
    /// The global task ID that names this task's session.
    pub fn get_session_id(&self) -> usize {
        self.session_id.load(Ordering::SeqCst)
    }

    /// Set the POSIX session ID (SID).
    ///
    /// # Arguments
    /// * `session_id` - Global task ID that names the session.
    pub fn set_session_id(&self, session_id: usize) {
        self.session_id.store(session_id, Ordering::SeqCst);
        self.is_session_leader
            .store(session_id == self.id && self.id != 0, Ordering::SeqCst);
    }

    /// Returns true if this task is a POSIX session leader.
    pub fn is_session_leader(&self) -> bool {
        self.is_session_leader.load(Ordering::SeqCst)
    }

    /// Create a new POSIX session led by this task.
    ///
    /// This implements the kernel-side part of `setsid(2)`: the caller must
    /// not already be a process group leader, and on success SID and PGID both
    /// become the caller's task ID while the controlling terminal is dropped.
    ///
    /// # Returns
    /// The new global SID on success.
    pub fn create_session(&self) -> Result<usize, &'static str> {
        let id = self.get_id();
        if self.get_process_group_id() == id {
            return Err("process group leader cannot create a new session");
        }

        self.session_id.store(id, Ordering::SeqCst);
        self.set_process_group_id(id);
        *self.controlling_tty.write() = None;
        self.is_session_leader.store(true, Ordering::SeqCst);
        Ok(id)
    }

    /// Set the task's controlling terminal.
    ///
    /// # Arguments
    /// * `tty` - Weak reference to the controlling terminal, or `None` to
    ///   detach from any controlling terminal.
    pub fn set_controlling_tty(&self, tty: Option<Weak<TtyDevice>>) {
        *self.controlling_tty.write() = tty;
    }

    /// Get the task's controlling terminal, if it is still alive.
    ///
    /// # Returns
    /// Strong reference to the controlling TTY, or `None`.
    pub fn get_controlling_tty(&self) -> Option<Arc<TtyDevice>> {
        self.controlling_tty.read().as_ref().and_then(Weak::upgrade)
    }

    /// Detach the task from its controlling terminal.
    pub fn clear_controlling_tty(&self) {
        *self.controlling_tty.write() = None;
    }

    /// Set the namespace ID (used by TaskPool during task addition)
    pub fn set_namespace_id(&self, namespace_id: usize) {
        self.namespace_id
            .store(namespace_id, atomic::Ordering::SeqCst);
    }

    /// Get the task ID within its namespace.
    ///
    /// This ID is local to the task's namespace and may differ from the global ID.
    /// This is the ID that should be exposed to user space and ABI syscalls.
    ///
    /// # Returns
    /// The namespace-local task ID
    pub fn get_namespace_id(&self) -> usize {
        let namespace_id = self.namespace_id.load(atomic::Ordering::SeqCst);
        assert!(
            namespace_id != 0,
            "Task namespace_id is 0 - task may not have been added to scheduler yet"
        );
        namespace_id
    }

    /// Get the task's namespace.
    ///
    /// # Returns
    /// Reference to the task's namespace
    pub fn get_namespace(&self) -> Arc<namespace::TaskNamespace> {
        self.namespace.read().clone()
    }

    /// Set the task's namespace.
    ///
    /// This allows changing a task's namespace, useful for ABI transitions
    /// or when moving tasks between namespace contexts.
    ///
    /// **Warning**: This method allocates a new namespace-local ID each time
    /// it's called. Changing a task's namespace multiple times may lead to
    /// ID conflicts or unexpected behavior. This method should typically only
    /// be called once during task initialization or ABI transition.
    ///
    /// # Arguments
    /// * `ns` - New namespace for the task
    pub fn set_namespace(&self, ns: Arc<namespace::TaskNamespace>) {
        *self.namespace.write() = ns;
        // Allocate a new namespace-local ID (and register translation mapping)
        self.namespace_id.store(
            self.namespace.write().allocate_task_id_for(self.id),
            atomic::Ordering::SeqCst,
        );
    }

    /// Get the Thread Group ID (TGID)
    ///
    /// The TGID identifies the thread group (process). For tasks created with
    /// CLONE_VM (threads), all threads in the group share the same TGID.
    /// For standalone tasks (no CLONE_VM), TGID equals the task ID.
    ///
    /// # Returns
    pub fn get_thread_group_id(&self) -> usize {
        self.thread_group_id
    }

    pub fn set_thread_group_id(&mut self, thread_group_id: usize) {
        self.thread_group_id = thread_group_id;
    }

    /// Set the task state
    ///
    /// # Arguments
    /// * `state` - The new task state
    ///
    pub fn set_state(&self, state: TaskState) {
        self.state.store(state, Ordering::SeqCst);
    }

    /// Get the task state
    ///
    /// # Returns
    /// The task state
    ///
    pub fn get_state(&self) -> TaskState {
        self.state.load(Ordering::SeqCst)
    }

    /// Mark this task as stopped by a process-control event.
    pub fn mark_process_control_stopped(&self) {
        self.process_control_stopped.store(true, Ordering::SeqCst);
        self.process_control_stop_reported
            .store(false, Ordering::SeqCst);
    }

    /// Clear process-control stopped state after a continue event.
    pub fn clear_process_control_stopped(&self) {
        self.process_control_stopped.store(false, Ordering::SeqCst);
        self.process_control_stop_reported
            .store(false, Ordering::SeqCst);
    }

    /// Return whether this task has an unreported process-control stop.
    pub fn take_process_control_stop_report(&self) -> bool {
        if !self.process_control_stopped.load(Ordering::SeqCst) {
            return false;
        }
        self.process_control_stop_reported
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Get the size of the task.
    ///
    /// # Returns
    /// The size of the task in bytes.
    pub fn get_size(&self) -> usize {
        self.stack_size.load(Ordering::SeqCst)
            + self.text_size.load(Ordering::SeqCst)
            + self.data_size.load(Ordering::SeqCst)
    }

    /// Get the program break (NOT work in Kernel task)
    ///
    /// # Returns
    /// The program break address
    pub fn get_brk(&self) -> usize {
        // Return brk if set (represents program end address)
        // Otherwise fallback to legacy size-based calculation for compatibility
        let brk = self.brk.load(Ordering::SeqCst);
        if brk == usize::MAX {
            self.text_size.load(Ordering::SeqCst) + self.data_size.load(Ordering::SeqCst)
        } else {
            brk
        }
    }

    /// Set the program break (NOT work in Kernel task)
    ///
    /// # Arguments
    /// * `brk` - The new program break address
    ///
    /// # Returns
    /// If successful, returns Ok(()), otherwise returns an error.
    pub fn set_brk(&self, brk: usize) -> Result<(), &'static str> {
        let prev_brk = self.get_brk();
        if brk < prev_brk {
            /* Free pages */
            /* Round address to the page boundary */
            let prev_addr = (prev_brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            let addr = (brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            let num_of_pages = (prev_addr - addr) / PAGE_SIZE;
            self.free_data_pages(addr, num_of_pages);
        } else if brk > prev_brk {
            /* Allocate pages */
            /* Round address to the page boundary */
            let prev_addr = (prev_brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            let addr = (brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            let num_of_pages = (addr - prev_addr) / PAGE_SIZE;

            // crate::println!("[set_brk] Expanding: prev_brk={:#x} -> brk={:#x}", prev_brk, brk);
            // crate::println!("[set_brk] Page allocation: prev_addr={:#x}, addr={:#x}, num_pages={}",
            //     prev_addr, addr, num_of_pages);

            if num_of_pages > 0 {
                match self.vm_manager.search_memory_map(prev_addr) {
                    Some(_existing_map) => {
                        // crate::println!("[set_brk] Existing mapping found: VA {:#x}-{:#x}, skipping allocation",
                        //     existing_map.vmarea.start, existing_map.vmarea.end);
                    }
                    None => {
                        // crate::println!("[set_brk] No existing mapping, allocating {} pages at {:#x}",
                        //     num_of_pages, prev_addr);
                        match self.allocate_data_pages(prev_addr, num_of_pages) {
                            Ok(_) => {
                                // crate::println!("[set_brk] Successfully allocated {} pages", num_of_pages);
                            }
                            Err(_e) => {
                                // crate::println!("[set_brk] Failed to allocate pages: {}", e);
                                return Err("Failed to allocate pages");
                            }
                        }
                    }
                }
            }
        }
        self.brk.store(brk, Ordering::SeqCst);
        Ok(())
    }

    /// Allocate pages for the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to allocate pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to allocate
    /// * `segment` - The segment type to allocate pages
    ///
    /// # Returns
    /// The memory map of the allocated pages, if successful.
    ///
    /// # Errors
    /// If the address is not page aligned, or if the pages cannot be allocated.
    ///
    /// # Note
    /// This function don't increment the size of the task.
    /// You must increment the size of the task manually.
    ///
    pub fn allocate_pages(
        &self,
        vaddr: usize,
        num_of_pages: usize,
        permissions: usize,
    ) -> Result<VirtualMemoryMap, &'static str> {
        if vaddr % PAGE_SIZE != 0 {
            return Err("Address is not page aligned");
        }

        let page_alloc = ContiguousPages::new(num_of_pages).ok_or("Failed to allocate pages")?;
        let size = num_of_pages * PAGE_SIZE;
        let paddr = virt_to_phys(page_alloc.as_ptr() as usize);
        let mmap = VirtualMemoryMap {
            pmarea: MemoryArea {
                start: paddr,
                end: paddr + size - 1,
            },
            vmarea: MemoryArea {
                start: vaddr,
                end: vaddr + size - 1,
            },
            vm_start: vaddr,
            permissions,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: None,
        };
        self.vm_manager
            .add_memory_map(mmap.clone())
            .map_err(|e| panic!("Failed to add memory map: {}", e))?;

        self.page_allocations.write().push(page_alloc);

        Ok(mmap)
    }

    /// Free pages for the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to free pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to free
    pub fn free_pages(&self, vaddr: usize, num_of_pages: usize) {
        let page = vaddr / PAGE_SIZE;
        for p in 0..num_of_pages {
            let vaddr = (page + p) * PAGE_SIZE;
            match self.vm_manager.remove_memory_map_by_addr(vaddr) {
                Some(mmap) => {
                    if p == 0 && mmap.vmarea.start < vaddr {
                        /* Re add the first part of the memory map */
                        let size = vaddr - mmap.vmarea.start;
                        let paddr = mmap.pmarea.start;
                        let mmap1 = VirtualMemoryMap {
                            pmarea: MemoryArea {
                                start: paddr,
                                end: paddr + size - 1,
                            },
                            vmarea: MemoryArea {
                                start: mmap.vmarea.start,
                                end: vaddr - 1,
                            },
                            vm_start: mmap.vm_start,
                            permissions: mmap.permissions,
                            is_shared: mmap.is_shared,
                            memory_attribute: mmap.memory_attribute,
                            owner: mmap.owner.clone(),
                        };
                        self.vm_manager
                            .add_memory_map(mmap1)
                            .map_err(|e| panic!("Failed to add memory map: {}", e))
                            .unwrap();
                        // println!("Removed map : {:#x} - {:#x}", mmap.vmarea.start, mmap.vmarea.end);
                        // println!("Re added map: {:#x} - {:#x}", mmap1.vmarea.start, mmap1.vmarea.end);
                    }
                    if p == num_of_pages - 1 && mmap.vmarea.end > vaddr + PAGE_SIZE - 1 {
                        /* Re add the second part of the memory map */
                        let size = mmap.vmarea.end - (vaddr + PAGE_SIZE) + 1;
                        let paddr = mmap.pmarea.start + (vaddr + PAGE_SIZE - mmap.vmarea.start);
                        let mmap2 = VirtualMemoryMap {
                            pmarea: MemoryArea {
                                start: paddr,
                                end: paddr + size - 1,
                            },
                            vmarea: MemoryArea {
                                start: vaddr + PAGE_SIZE,
                                end: mmap.vmarea.end,
                            },
                            vm_start: mmap.vm_start,
                            permissions: mmap.permissions,
                            is_shared: mmap.is_shared,
                            memory_attribute: mmap.memory_attribute,
                            owner: mmap.owner.clone(),
                        };
                        self.vm_manager
                            .add_memory_map(mmap2)
                            .map_err(|e| panic!("Failed to add memory map: {}", e))
                            .unwrap();
                        // println!("Removed map : {:#x} - {:#x}", mmap.vmarea.start, mmap.vmarea.end);
                        // println!("Re added map: {:#x} - {:#x}", mmap2.vmarea.start, mmap2.vmarea.end);
                    }
                }
                None => {}
            }
        }
        /* Unmap pages */
        let asid = self.vm_manager.get_asid();
        let root_pagetable = self.vm_manager.get_root_page_table().unwrap();
        if num_of_pages > 0 {
            let vaddr_start = page * PAGE_SIZE;
            let vaddr_end = vaddr_start + num_of_pages * PAGE_SIZE - 1;
            root_pagetable.unmap_range(asid, vaddr_start, vaddr_end);
        }
    }

    /// Allocate text pages for the task. And increment the size of the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to allocate pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to allocate
    ///
    /// # Returns
    /// The memory map of the allocated pages, if successful.
    ///
    /// # Errors
    /// If the address is not page aligned, or if the pages cannot be allocated.
    ///
    pub fn allocate_text_pages(
        &self,
        vaddr: usize,
        num_of_pages: usize,
    ) -> Result<VirtualMemoryMap, &'static str> {
        let permissions = VirtualMemoryRegion::Text.default_permissions();
        let res = self.allocate_pages(vaddr, num_of_pages, permissions);
        if res.is_ok() {
            self.text_size
                .fetch_add(num_of_pages * PAGE_SIZE, Ordering::SeqCst);
        }
        res
    }

    /// Free text pages for the task. And decrement the size of the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to free pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to free
    ///
    pub fn free_text_pages(&self, vaddr: usize, num_of_pages: usize) {
        self.free_pages(vaddr, num_of_pages);
        self.text_size
            .fetch_sub(num_of_pages * PAGE_SIZE, Ordering::SeqCst);
    }

    /// Allocate stack pages for the task. And increment the size of the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to allocate pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to allocate
    ///
    /// # Returns
    /// The memory map of the allocated pages, if successful.
    ///
    /// # Errors
    /// If the address is not page aligned, or if the pages cannot be allocated.
    ///
    pub fn allocate_stack_pages(
        &self,
        vaddr: usize,
        num_of_pages: usize,
    ) -> Result<VirtualMemoryMap, &'static str> {
        let permissions = VirtualMemoryRegion::Stack.default_permissions();
        let res = self.allocate_pages(vaddr, num_of_pages, permissions)?;
        self.stack_size
            .fetch_add(num_of_pages * PAGE_SIZE, Ordering::SeqCst);
        Ok(res)
    }

    /// Free stack pages for the task. And decrement the size of the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to free pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to free
    ///
    pub fn free_stack_pages(&self, vaddr: usize, num_of_pages: usize) {
        self.free_pages(vaddr, num_of_pages);
        self.stack_size
            .fetch_sub(num_of_pages * PAGE_SIZE, Ordering::SeqCst);
    }

    /// Allocate data pages for the task. And increment the size of the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to allocate pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to allocate
    ///
    /// # Returns
    /// The memory map of the allocated pages, if successful.
    ///
    /// # Errors
    /// If the address is not page aligned, or if the pages cannot be allocated.
    ///
    pub fn allocate_data_pages(
        &self,
        vaddr: usize,
        num_of_pages: usize,
    ) -> Result<VirtualMemoryMap, &'static str> {
        let permissions = VirtualMemoryRegion::Data.default_permissions();
        let res = self.allocate_pages(vaddr, num_of_pages, permissions)?;
        self.data_size
            .fetch_add(num_of_pages * PAGE_SIZE, Ordering::SeqCst);
        Ok(res)
    }

    /// Free data pages for the task. And decrement the size of the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to free pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to free
    ///
    pub fn free_data_pages(&self, vaddr: usize, num_of_pages: usize) {
        self.free_pages(vaddr, num_of_pages);
        self.data_size
            .fetch_sub(num_of_pages * PAGE_SIZE, Ordering::SeqCst);
    }

    /// Allocate guard pages for the task.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to allocate pages (NOTE: The address must be page aligned)
    /// * `num_of_pages` - The number of pages to allocate
    ///
    /// # Returns
    /// The memory map of the allocated pages, if successful.
    ///
    /// # Errors
    /// If the address is not page aligned, or if the pages cannot be allocated.
    ///
    /// # Note
    /// Gurad pages are not allocated in the physical memory space.
    /// This function only maps the pages to the virtual memory space.
    ///
    pub fn allocate_guard_pages(
        &self,
        vaddr: usize,
        num_of_pages: usize,
    ) -> Result<VirtualMemoryMap, &'static str> {
        let permissions = VirtualMemoryRegion::Guard.default_permissions();
        let mmap = VirtualMemoryMap {
            pmarea: MemoryArea { start: 0, end: 0 },
            vmarea: MemoryArea {
                start: vaddr,
                end: vaddr + num_of_pages * PAGE_SIZE - 1,
            },
            vm_start: vaddr,
            permissions,
            is_shared: VirtualMemoryRegion::Guard.is_shareable(), // Guard pages can be shared
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: None,
        };
        Ok(mmap)
    }

    fn take_exact_page_allocation(
        &self,
        paddr: usize,
        page_count: usize,
    ) -> Option<ContiguousPages> {
        let mut allocations = self.page_allocations.write();
        let index = allocations
            .iter()
            .position(|alloc| alloc.as_paddr() == paddr && alloc.len() == page_count)?;
        Some(allocations.swap_remove(index))
    }

    // Set the entry point
    pub fn set_entry_point(&self, entry: usize) {
        self.vcpu.lock().set_pc(entry as u64);
    }

    /// Get the parent ID
    ///
    /// # Returns
    /// The parent task ID, or None if there is no parent
    pub fn get_parent_id(&self) -> Option<usize> {
        match self.parent_id.load(Ordering::SeqCst) {
            0 => None,
            id => Some(id),
        }
    }

    /// Set the parent task
    ///
    /// # Arguments
    /// * `parent_id` - The ID of the parent task
    pub fn set_parent_id(&self, parent_id: usize) {
        self.parent_id.store(parent_id, Ordering::SeqCst);
    }

    /// Clear the parent task relationship.
    ///
    /// # Returns
    ///
    /// This function does not return a value.
    pub fn clear_parent_id(&self) {
        self.parent_id.store(0, Ordering::SeqCst);
    }

    fn orphan_reaper(&self) -> Option<&'static Task> {
        if self.thread_group_id != self.id
            && let Some(leader) = get_task_by_id(self.thread_group_id)
            && !matches!(
                leader.get_state(),
                TaskState::Zombie | TaskState::Terminated
            )
        {
            return Some(leader);
        }

        if self.id != INIT_TASK_ID {
            get_task_by_id(INIT_TASK_ID)
        } else {
            None
        }
    }

    fn reparent_children(&self) {
        let child_ids = {
            let mut children = self.children.write();
            if children.is_empty() {
                return;
            }
            let child_ids = children.clone();
            children.clear();
            child_ids
        };

        let reaper = self.orphan_reaper();

        for child_id in child_ids {
            let Some(child) = get_task_by_id(child_id) else {
                continue;
            };
            if child.get_parent_id() != Some(self.id) {
                continue;
            }

            if let Some(reaper) = reaper {
                child.set_parent_id(reaper.get_id());
                reaper.add_child(child_id);
                if child.get_state() == TaskState::Zombie {
                    finalize_zombie(child_id, Some(reaper.get_id()));
                }
            } else {
                child.clear_parent_id();
                if child.get_state() == TaskState::Zombie {
                    child.set_state(TaskState::Terminated);
                    cleanup_zombie(child_id);
                }
            }
        }
    }

    /// Add a child task
    ///
    /// # Arguments
    /// * `child_id` - The ID of the child task
    pub fn add_child(&self, child_id: usize) {
        let mut children = self.children.write();
        if !children.contains(&child_id) {
            children.push(child_id);
        }
    }

    /// Remove a child task
    ///
    /// # Arguments
    /// * `child_id` - The ID of the child task to remove
    ///
    /// # Returns
    /// true if the removal was successful, false if the child task was not found
    pub fn remove_child(&self, child_id: usize) -> bool {
        let mut children = self.children.write();
        if let Some(pos) = children.iter().position(|&id| id == child_id) {
            children.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get the list of child tasks
    ///
    /// # Returns
    /// A vector of child task IDs
    pub fn get_children(&self) -> Vec<usize> {
        self.children.read().clone()
    }

    /// Set the exit status
    ///
    /// # Arguments
    /// * `status` - The exit status
    pub fn set_exit_status(&self, status: i32) {
        self.exit_status.store(status, Ordering::SeqCst);
    }

    /// Get the exit status
    ///
    /// # Returns
    /// The exit status, or None if not set
    pub fn get_exit_status(&self) -> Option<i32> {
        let status = self.exit_status.load(Ordering::SeqCst);
        if status == i32::MIN {
            None
        } else {
            Some(status)
        }
    }

    /// Resolve the ABI to use for the given address
    ///
    /// This method calls a closure with the ABI module that should be used
    /// for a system call issued from the given address. It searches the ABI zones map
    /// and returns the appropriate ABI, falling back to the default ABI if no zone matches.
    ///
    /// # Arguments
    /// * `addr` - The program counter address where the system call was issued
    /// * `f` - Closure to call with the ABI module
    ///
    /// # Returns
    /// The result of the closure
    pub fn with_resolve_abi_mut<R, F>(&self, addr: usize, f: F) -> R
    where
        F: FnOnce(&mut (dyn AbiModule + Send + Sync)) -> R,
    {
        // Search for the zone containing addr using efficient BTreeMap range query
        // SAFETY: This is the currently executing task on this hart
        let abi_zones = unsafe { self.abi_zones.get_mut() };
        if let Some((_start, zone)) = abi_zones.range_mut(..=addr).next_back() {
            if zone.range.contains(&addr) {
                return f(zone.abi.as_mut());
            }
        }
        // No zone found, use default ABI
        // SAFETY: This is the currently executing task on this hart
        let abi = unsafe { self.default_abi.get_mut() };
        f(abi.as_deref_mut().expect("default_abi not set"))
    }

    /// Execute a closure with the default ABI
    ///
    /// # Arguments
    /// * `f` - Closure to call with the default ABI
    ///
    /// # Returns
    /// The result of the closure
    pub fn with_default_abi<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&(dyn AbiModule + Send + Sync)) -> R,
    {
        // SAFETY: This is the currently executing task on this hart
        let abi = unsafe { self.default_abi.get() };
        f(abi.as_deref().expect("default_abi not set"))
    }

    /// Run a closure with mutable access to the default ABI and a reference to the task
    ///
    /// Since `default_abi` is task-local (no lock), we can safely provide both
    /// `&mut AbiModule` and `&Task` without any take/restore dance.
    pub fn with_default_abi_mut<R, F>(&self, f: F) -> R
    where
        F: FnOnce(&mut (dyn AbiModule + Send + Sync), &Task) -> R,
    {
        // SAFETY: This is the currently executing task on this hart
        let abi = unsafe { self.default_abi.get_mut() };
        let abi_ref = abi.as_deref_mut().expect("default_abi not set");
        f(abi_ref, self)
    }

    /// Get the file descriptor table
    ///
    /// # Returns
    /// A reference to the file descriptor table
    ///
    /// Clone this task, creating a near-identical copy
    ///
    /// # Arguments
    ///
    /// # Returns
    /// The cloned task
    ///
    /// # Errors
    /// If the task cannot be cloned, an error is returned.
    ///
    pub fn clone_task(&self, flags: CloneFlags) -> Result<Task, &'static str> {
        // Create a new task in the same namespace as the parent
        let mut child = Task::new_with_namespace(
            self.name.read().clone(),
            self.priority.load(Ordering::SeqCst),
            self.task_type,
            self.namespace.read().clone(),
        );

        // First, set up the virtual memory manager with the same ASID allocation
        match self.task_type {
            TaskType::Kernel => {
                // For kernel tasks, we need to call init to set up the kernel VM
                child.init();
            }
            TaskType::User => {
                if !flags.is_set(CloneFlagsDef::Vm) {
                    // For user tasks, manually set up VM without calling init()
                    // to avoid creating new stack that would overwrite parent's stack content
                    let asid = alloc_virtual_address_space();
                    child.vm_manager.set_asid(asid);
                } else {
                    // CLONE_VM: share the same address space via Arc<VirtualMemoryManager>
                    child.vm_manager = self.vm_manager.clone();
                }
            }
        }

        if !flags.is_set(CloneFlagsDef::Vm) {
            // Copy or share memory maps from parent to child without cloning lists
            let memmaps = self
                .vm_manager
                .with_memmaps(|maps| maps.values().cloned().collect::<Vec<_>>());
            for mmap in memmaps {
                let num_pages =
                    (mmap.vmarea.end - mmap.vmarea.start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;
                if num_pages == 0 {
                    continue;
                }

                let vaddr = mmap.vmarea.start;
                if mmap.is_shared {
                    // Shared memory regions: just reference the same physical pages
                    let shared_mmap = VirtualMemoryMap {
                        pmarea: mmap.pmarea,
                        vmarea: mmap.vmarea,
                        vm_start: mmap.vm_start,
                        permissions: mmap.permissions,
                        is_shared: true,
                        memory_attribute: mmap.memory_attribute,
                        owner: mmap.owner.clone(),
                    };
                    child
                        .vm_manager
                        .add_memory_map(shared_mmap.clone())
                        .map_err(|_| "Failed to add shared memory map to child task")?;

                    // Pre-map trampoline page if applicable
                    if mmap.vmarea.start == 0xffff_ffff_ffff_f000 {
                        if let Some(root_pagetable) = child.vm_manager.get_root_page_table() {
                            root_pagetable
                                .map_memory_area(
                                    child.vm_manager.get_asid(),
                                    shared_mmap,
                                    true,
                                    true,
                                )
                                .map_err(|_| "Failed to map trampoline page")?;
                        }
                    }
                } else if let Some(owner) = &mmap.owner {
                    if let Some(cloned_owner) = owner.fork_clone() {
                        let new_mmap = VirtualMemoryMap {
                            pmarea: MemoryArea { start: 0, end: 0 },
                            vmarea: mmap.vmarea,
                            vm_start: mmap.vm_start,
                            permissions: mmap.permissions,
                            is_shared: false,
                            memory_attribute: mmap.memory_attribute,
                            owner: Some(cloned_owner),
                        };
                        child
                            .vm_manager
                            .add_memory_map(new_mmap)
                            .map_err(|_| "Failed to add owner-based map to child task")?;
                    } else {
                        if mmap.pmarea.start == 0 {
                            // Lazy: clone Arc, child COWs independently on fault
                            let new_mmap = VirtualMemoryMap {
                                pmarea: MemoryArea { start: 0, end: 0 },
                                vmarea: mmap.vmarea,
                                vm_start: mmap.vm_start,
                                permissions: mmap.permissions,
                                is_shared: false,
                                memory_attribute: mmap.memory_attribute,
                                owner: Some(Arc::clone(owner)),
                            };
                            child
                                .vm_manager
                                .add_memory_map(new_mmap)
                                .map_err(|_| "Failed to add owner-based map to child task")?;
                        } else {
                            // Eager: copy physical pages
                            let permissions = mmap.permissions;
                            let page_alloc = ContiguousPages::new(num_pages)
                                .ok_or("Failed to allocate pages for clone")?;
                            let size = num_pages * PAGE_SIZE;
                            let paddr = virt_to_phys(page_alloc.as_ptr() as usize);
                            let new_mmap = VirtualMemoryMap {
                                pmarea: MemoryArea {
                                    start: paddr,
                                    end: paddr + (size - 1),
                                },
                                vmarea: MemoryArea {
                                    start: vaddr,
                                    end: vaddr + (size - 1),
                                },
                                vm_start: vaddr,
                                permissions,
                                is_shared: false,
                                memory_attribute: mmap.memory_attribute,
                                owner: None,
                            };
                            // SAFETY: src/dst are valid page-aligned ranges of `size` bytes.
                            unsafe {
                                let src_start = phys_to_virt(mmap.pmarea.start);
                                let dst_start = phys_to_virt(paddr);
                                core::ptr::copy_nonoverlapping(
                                    src_start as *const u8,
                                    dst_start as *mut u8,
                                    size,
                                );
                            }
                            child.page_allocations.write().push(page_alloc);
                            child
                                .vm_manager
                                .add_memory_map(new_mmap)
                                .map_err(|_| "Failed to add memory map to child task")?;
                        }
                    }
                } else if mmap.pmarea.start != 0 {
                    if let Some(page_alloc) =
                        self.take_exact_page_allocation(mmap.pmarea.start, num_pages)
                    {
                        let base_page_idx = (mmap.vmarea.start - mmap.vm_start) / PAGE_SIZE;
                        let cow_owner = Arc::new(ForkCowPageOwner::new(base_page_idx, page_alloc));
                        let cow_map = VirtualMemoryMap {
                            pmarea: MemoryArea { start: 0, end: 0 },
                            vmarea: mmap.vmarea,
                            vm_start: mmap.vm_start,
                            permissions: mmap.permissions,
                            is_shared: false,
                            memory_attribute: mmap.memory_attribute,
                            owner: Some(cow_owner),
                        };
                        self.vm_manager.with_memmaps_mut(|maps| {
                            if let Some(parent_map) = maps.get_mut(&mmap.vmarea.start) {
                                *parent_map = cow_map.clone();
                            }
                        });
                        self.vm_manager
                            .unmap_range_from_mmu(mmap.vmarea.start, mmap.vmarea.end);
                        child
                            .vm_manager
                            .add_memory_map(cow_map)
                            .map_err(|_| "Failed to add COW memory map to child task")?;
                        continue;
                    }

                    // Private memory regions: allocate new pages and copy contents
                    let permissions = mmap.permissions;
                    let page_alloc = ContiguousPages::new(num_pages)
                        .ok_or("Failed to allocate pages for clone")?;
                    let size = num_pages * PAGE_SIZE;
                    let paddr = virt_to_phys(page_alloc.as_ptr() as usize);
                    let new_mmap = VirtualMemoryMap {
                        pmarea: MemoryArea {
                            start: paddr,
                            end: paddr + (size - 1),
                        },
                        vmarea: MemoryArea {
                            start: vaddr,
                            end: vaddr + (size - 1),
                        },
                        vm_start: vaddr,
                        permissions,
                        is_shared: false,
                        memory_attribute: mmap.memory_attribute,
                        owner: None,
                    };

                    // Copy original contents
                    unsafe {
                        let src_start = phys_to_virt(mmap.pmarea.start);
                        let dst_start = phys_to_virt(paddr);
                        core::ptr::copy_nonoverlapping(
                            src_start as *const u8,
                            dst_start as *mut u8,
                            size,
                        );
                    }

                    child.page_allocations.write().push(page_alloc);

                    child
                        .vm_manager
                        .add_memory_map(new_mmap)
                        .map_err(|_| "Failed to add memory map to child task")?;
                } else {
                    let new_mmap = VirtualMemoryMap {
                        pmarea: mmap.pmarea,
                        vmarea: mmap.vmarea,
                        vm_start: mmap.vm_start,
                        permissions: mmap.permissions,
                        is_shared: false,
                        memory_attribute: mmap.memory_attribute,
                        owner: None,
                    };
                    child
                        .vm_manager
                        .add_memory_map(new_mmap)
                        .map_err(|_| "Failed to add unbacked memory map to child task")?;
                }
            }
        }

        // Copy register states (architecture-specific VCPU state)
        self.vcpu.lock().clone_to(&mut child.vcpu.lock());

        // Clone the default ABI and ABI zones
        // SAFETY: Child task is not yet visible to scheduler, parent is currently executing
        unsafe {
            *child.default_abi.get_mut() = Some(
                self.default_abi
                    .get()
                    .as_ref()
                    .expect("default_abi not set")
                    .clone_boxed(),
            );
            // Clone ABI zones (each zone contains a boxed ABI that needs to be cloned)
            for (start, zone) in self.abi_zones.get().iter() {
                let new_zone = AbiZone {
                    range: zone.range.clone(),
                    abi: zone.abi.clone_boxed(),
                };
                child.abi_zones.get_mut().insert(*start, new_zone);
            }
            // Notify child's default ABI instance that cloning has completed
            // Child is not yet in the scheduler, so direct access is safe.
            if let Some(abi_boxed) = child.default_abi.get_mut().as_mut() {
                let _ = abi_boxed.on_task_cloned(self, &child, flags);
            }
        }

        // Copy state such as data size
        child
            .stack_size
            .store(self.stack_size.load(Ordering::SeqCst), Ordering::SeqCst);
        child
            .data_size
            .store(self.data_size.load(Ordering::SeqCst), Ordering::SeqCst);
        child
            .text_size
            .store(self.text_size.load(Ordering::SeqCst), Ordering::SeqCst);
        child.max_stack_size = self.max_stack_size;
        child.max_data_size = self.max_data_size;
        child.max_text_size = self.max_text_size;
        // Program break must be shared when CLONE_VM is set, because the heap lives in the shared
        // address space. If not shared, the child gets an independent copy of the current brk.
        if flags.is_set(CloneFlagsDef::Vm) {
            child.brk = self.brk.clone();
        } else {
            let parent_brk = self.brk.load(Ordering::SeqCst);
            child.brk = Arc::new(AtomicUsize::new(parent_brk));
        }

        // POSIX job-control identity.  A fork/clone inherits SID, PGID, and
        // controlling TTY.  Session leadership itself is not inherited because
        // the child will receive a distinct task ID when it is registered.
        child
            .session_id
            .store(self.get_session_id(), Ordering::SeqCst);
        child.set_process_group_id(self.get_process_group_id());
        *child.controlling_tty.write() = self.controlling_tty.read().clone();
        child.is_session_leader.store(false, Ordering::SeqCst);

        // Copy scheduling and event handling state
        child
            .time_slice
            .store(self.time_slice.load(Ordering::SeqCst), Ordering::SeqCst);
        child.default_time_slice.store(
            self.default_time_slice.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        child.set_core_preference(self.core_preference());
        child
            .sched_util_min
            .store(self.sched_util_min(), Ordering::SeqCst);
        // Note: software_timers_handlers, sleep_waker, event_queue are NOT copied
        // as they are task-specific runtime state that should start fresh

        // Set the same entry point
        child.entry = self.entry;

        if flags.is_set(CloneFlagsDef::Files) {
            // Share the handle table (CLONE_FILES behavior)
            // clone() creates a shallow copy that shares the same underlying data
            child.handle_table = self.handle_table.clone();
        } else {
            // Create an independent copy of the handle table (fork-like behavior)
            child.handle_table = self.handle_table.deep_clone();
        }

        if flags.is_set(CloneFlagsDef::Fs) {
            if let Some(vfs) = self.vfs.read().clone() {
                *child.vfs.write() = Some(vfs);
            } else {
                *child.vfs.write() = None;
            }
        } else if let Some(vfs) = self.vfs.read().clone() {
            *child.vfs.write() = Some(VfsManager::clone_with_shared_mount_namespace(&vfs));
        }

        // Ensure the cloned task has its own high-VA kernel stack window.
        // Task::new() already allocates a per-task kernel stack (KernelContext), but clone paths
        // intentionally avoid Task::init() (especially for user tasks). That means the trampoline-
        // managed kstack window mapping must be set up here.
        if child.get_kernel_stack_window_base().is_none() {
            crate::vm::setup_trampoline_for_task_kstack_window(&mut child)?;
        }
        // Cloned task starts as Ready regardless of parent's current state
        child.state.store(TaskState::Ready, Ordering::SeqCst);

        // NOTE: Parent-child relationship will be established AFTER add_task()
        // when the child has a valid ID. The caller is responsible for calling:
        //   child.set_parent_id(self.id);
        //   self.add_child(child.get_id());
        // after adding the child to the scheduler.

        // Set TGID: if CLONE_THREAD, share parent's TGID (join thread group)
        // Otherwise, child becomes a new thread group leader (TGID will be set to its own ID)
        // This matches Linux CLONE_THREAD semantics
        if flags.is_set(CloneFlagsDef::Thread) {
            // Thread: share parent's thread group ID
            child.thread_group_id = self.thread_group_id;
        } else {
            child.thread_group_id = 0;
        }

        Ok(child)
    }

    /// Exit the task
    ///
    /// # Arguments
    /// * `status` - The exit status
    ///
    pub fn exit(&self, status: i32) {
        self.exit_with_cleanup(status, |_| {});
    }

    pub(crate) fn exit_with_cleanup<F>(&self, status: i32, cleanup: F)
    where
        F: FnOnce(&Task),
    {
        // Close all open handles only if this task is the sole owner of the
        // handle table.  When CLONE_FILES is used (thread::spawn), multiple
        // tasks share the same Arc<HandleTableInner>.  Closing all handles
        // here would destroy handles that sibling/parent threads still need.
        if self.handle_table.is_sole_owner() {
            self.handle_table.close_all();
        }
        self.clear_process_control_stopped();
        // Let current ABI perform exit-time cleanup (Linux: clear_child_tid, robust list, etc.)
        // Use take/restore to avoid aliasing &mut self and &mut field
        self.with_default_abi_mut(|abi, task| abi.on_task_exit(task));
        cleanup(self);
        self.reparent_children();

        match self.get_parent_id() {
            Some(parent_id) => {
                if get_task_by_id(parent_id).is_none() {
                    // crate::println!("Task {}: Parent {} not found, terminating", self.id, parent_id);
                    self.state.store(TaskState::Terminated, Ordering::SeqCst);
                    return;
                }
                /* Set the exit status */
                self.set_exit_status(status);
                self.state.store(TaskState::Zombie, Ordering::SeqCst);

                // TODO: Notify parent via ABI-specific mechanism
                // crate::println!("Task {}: Set to Zombie state, parent {}", self.id, parent_id);
            }
            None => {
                /* If the task has no parent, it is terminated */
                // crate::println!("Task {}: No parent, terminating", self.id);
                self.state.store(TaskState::Terminated, Ordering::SeqCst);
            }
        }

        // Task cleanup completed - ABI module handles event cleanup

        if mytask().is_none() || mytask().unwrap().get_id() != self.id {
            // Non-current task: finalize zombie state manually
            // (current task path goes through schedule() -> pick_next -> finalize_zombie)
            if matches!(self.state.load(Ordering::SeqCst), TaskState::Zombie) {
                unmark_blocked(self.id);
                finalize_zombie(self.id, self.get_parent_id());
            }
            return;
        }

        // The scheduler will handle saving the current task state internally
        if let Some(current_task) = mytask() {
            schedule(current_task.get_trapframe());
        }
    }

    /// Exit all tasks in the thread group
    ///
    /// This terminates all tasks with the same TGID (thread group).
    /// This is similar to Linux's exit_group system call.
    ///
    /// # Arguments
    /// * `status` - The exit status for all tasks in the group
    ///
    /// # Behavior
    /// - Terminates all tasks with the same thread group ID
    /// - The calling task is set to Zombie/Terminated
    /// - Other tasks in the group are forcefully terminated
    pub fn exit_group(&self, status: i32) {
        let thread_group_id = self.thread_group_id;
        let my_id = self.id;
        let leader_id = thread_group_id;

        // The process is exiting, so shared file tables must be closed even
        // while sibling Task objects still hold HandleTable Arc references.
        self.handle_table.close_all();

        let all_task_ids = get_all_task_ids();
        let mut leader_finalized = false;

        // Terminate all tasks with the same thread group ID (except self).
        // The thread-group leader is the process wait target, so keep it as
        // the observable zombie even when another thread initiates exit_group.
        for task_id in all_task_ids {
            if task_id == my_id {
                continue;
            }

            if let Some(task) = get_task_by_id(task_id) {
                if task.get_thread_group_id() == thread_group_id {
                    if task_id == leader_id {
                        leader_finalized = true;
                        remove_from_ready_queues(task_id);
                        unmark_blocked(task_id);
                        task.exit(status);
                        continue;
                    }

                    task.reparent_children();
                    if LOG_EXIT_GROUP_SIBLINGS {
                        crate::println!(
                            "[exit_group] Task {} terminating sibling task {} (thread_group_id={})",
                            my_id,
                            task_id,
                            thread_group_id
                        );
                    }
                    // Set state to Terminated directly (bypass normal exit)
                    // Use unsafe to modify state through immutable reference
                    // This is safe because we are in a termination context
                    let task_ptr = task as *const Task as *mut Task;
                    unsafe {
                        (*task_ptr)
                            .state
                            .store(TaskState::Terminated, Ordering::SeqCst);
                        (*task_ptr).exit_status.store(status, Ordering::SeqCst);
                        // Close handles to prevent resource leaks
                        (*task_ptr).handle_table.close_all();
                    }
                    remove_from_ready_queues(task_id);
                    unmark_blocked(task_id);
                }
            }
        }

        if my_id == leader_id {
            self.exit(status);
            return;
        }

        if !leader_finalized
            && let Some(leader) = get_task_by_id(leader_id)
            && leader.get_thread_group_id() == thread_group_id
            && !matches!(
                leader.get_state(),
                TaskState::Zombie | TaskState::Terminated
            )
        {
            remove_from_ready_queues(leader_id);
            unmark_blocked(leader_id);
            leader.exit(status);
        }

        self.clear_process_control_stopped();
        self.with_default_abi_mut(|abi, task| abi.on_task_exit(task));
        self.reparent_children();
        self.set_exit_status(status);
        self.state.store(TaskState::Terminated, Ordering::SeqCst);
        remove_from_ready_queues(my_id);
        unmark_blocked(my_id);

        if mytask().is_some_and(|task| task.get_id() == self.id) {
            if let Some(current_task) = mytask() {
                schedule(current_task.get_trapframe());
            }
        }
    }

    /// Wait for a child task to exit and collect its status
    ///
    /// # Arguments
    /// * `child_id` - The ID of the child task to wait for
    ///
    /// # Returns
    /// The exit status of the child task, or an error if the child is not found or not in Zombie state
    pub fn wait(&self, child_id: usize) -> Result<i32, WaitError> {
        if !self.children.read().contains(&child_id) {
            crate::println!("[Task {}] wait: No such child task: {}", self.id, child_id);
            return Err(WaitError::NoSuchChild("No such child task".to_string()));
        }

        if let Some(child_task) = get_task_by_id(child_id) {
            if child_task.get_state() == TaskState::Zombie {
                let status = child_task.get_exit_status().unwrap_or(-1);
                child_task.set_state(TaskState::Terminated);
                self.remove_child(child_id);
                crate::sched::scheduler::cleanup_zombie(child_id);
                Ok(status)
            } else {
                Err(WaitError::ChildNotExited(
                    "Child has not exited or is not a zombie".to_string(),
                ))
            }
        } else {
            Err(WaitError::ChildTaskNotFound(
                "Child task not found".to_string(),
            ))
        }
    }

    /// Sleep the current task for the specified number of ticks.
    /// This blocks the task and registers a timer to wake it up.
    ///
    /// # Arguments
    /// * `trapframe` - The trapframe of the current CPU state
    /// * `ticks` - The number of ticks to sleep
    ///
    pub fn sleep(&self, trapframe: &mut Trapframe, ticks: u64) {
        struct SleepWakerHandler {
            task_id: usize,
            _start_tick: u64,
        }

        impl TimerHandler for SleepWakerHandler {
            fn on_timer_expired(self: Arc<Self>, _context: usize) {
                if let Some(task) = get_task_by_id(self.task_id) {
                    let handler: Arc<dyn TimerHandler> = self.clone();
                    task.remove_software_timer_handler(&handler);
                    // Memory barrier to ensure state change is visible
                    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
                    // crate::println!("Task {} woke up after {} ticks", self.task_id, get_tick() - self.start_tick);
                    task.sleep_waker.wake_all();
                }
            }
        }

        let wake_tick = get_tick() + ticks;
        let handler: Arc<dyn crate::timer::TimerHandler> = Arc::new(SleepWakerHandler {
            task_id: self.id,
            _start_tick: get_tick(),
        });
        add_timer(wake_tick, &handler, 0);

        self.add_software_timer_handler(handler);
        // Memory barrier to ensure timer handler registration is visible
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.sleep_waker.wait(self.get_id(), trapframe);
    }

    // VFS Helper Methods

    /// Set the VFS manager
    ///
    /// # Arguments
    /// * `vfs` - The VfsManager to set as the VFS
    pub fn set_vfs(&self, vfs: Arc<VfsManager>) {
        *self.vfs.write() = Some(vfs);
    }

    /// Get a reference to the VFS
    pub fn get_vfs(&self) -> Option<Arc<VfsManager>> {
        self.vfs.read().clone()
    }

    pub fn add_software_timer_handler(&self, timer: Arc<dyn TimerHandler>) {
        self.software_timers_handlers.write().push(timer);
    }

    pub fn remove_software_timer_handler(&self, timer: &Arc<dyn TimerHandler>) {
        let mut handlers = self.software_timers_handlers.write();
        if let Some(pos) = handlers.iter().position(|x| Arc::ptr_eq(x, timer)) {
            handlers.remove(pos);
        }
    }

    /// Enable event processing for this task (similar to enabling interrupts)
    pub fn enable_events(&self) {
        let mut enabled = self.events_enabled.lock();
        *enabled = true;
    }

    /// Disable event processing for this task (similar to disabling interrupts)
    pub fn disable_events(&self) {
        let mut enabled = self.events_enabled.lock();
        *enabled = false;
    }

    /// Check if events are enabled for this task
    pub fn events_enabled(&self) -> bool {
        *self.events_enabled.lock()
    }

    /// Process pending events if events are enabled
    /// This should be called by the scheduler before resuming the task
    ///
    /// Following signal-like semantics:
    /// - Process a limited number of events per scheduler cycle to avoid starvation
    /// - Critical events (like KILL) are processed immediately
    /// - Normal events are batched and processed in priority order
    pub fn process_pending_events(&self) -> Result<EventProcessOutcome, &'static str> {
        // Check if events are enabled
        if !self.events_enabled() {
            return Ok(EventProcessOutcome::Continue); // Events disabled, skip processing
        }

        // Delegate to ABI module for event processing
        self.with_default_abi_mut(|abi, _| {
            const MAX_EVENTS_PER_CYCLE: usize = 8; // Prevent scheduler starvation
            let mut processed_count = 0;

            // Process events with limits to prevent infinite loops
            while processed_count < MAX_EVENTS_PER_CYCLE {
                let event = {
                    let mut queue = self.event_queue.lock();
                    queue.dequeue()
                };

                match event {
                    Some(event) => {
                        processed_count += 1;

                        // Check if this is a critical event that requires immediate attention
                        let is_critical = self.is_critical_event(&event);

                        // Let ABI handle the event
                        let outcome = abi.handle_event(event, self.id as u32)?;
                        match outcome {
                            EventProcessOutcome::Continue | EventProcessOutcome::Pending => {}
                            EventProcessOutcome::UserHandlerArmed
                            | EventProcessOutcome::NeedReschedule
                            | EventProcessOutcome::Exited => return Ok(outcome),
                        }

                        // Check if events were disabled during handling
                        if !self.events_enabled() {
                            break;
                        }

                        // If we processed a critical event, we can stop here
                        // to allow the ABI module to take appropriate action
                        if is_critical {
                            break;
                        }
                    }
                    None => break, // No more events
                }
            }

            // If we hit the limit and there are still events, the scheduler
            // will call us again on the next cycle
            if processed_count == MAX_EVENTS_PER_CYCLE {
                let queue = self.event_queue.lock();
                if !queue.is_empty() {
                    // Log that we're deferring events to next cycle
                    // crate::println!("Task {}: Deferring {} events to next scheduler cycle",
                    //                      self.id, queue.len());
                }
            }

            Ok(EventProcessOutcome::Continue)
        })
    }

    /// Check if an event is critical and should be processed immediately
    /// Critical events typically cannot be ignored and affect task state directly
    fn is_critical_event(&self, event: &crate::ipc::event::Event) -> bool {
        use crate::ipc::event::EventPriority;

        // High/Critical priority events are always considered critical
        match event.metadata.priority {
            EventPriority::Critical => return true,
            EventPriority::High => {
                // Some high priority events are critical depending on content
                match &event.content {
                    EventContent::ProcessControl(ProcessControlType::Kill) => true,
                    EventContent::Custom { event_id, .. } => {
                        // Could map specific event IDs to critical signals
                        *event_id == 9 // SIGKILL-like event
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    // /// Get a mutable reference to the kernel context for context switching
    // pub fn get_kernel_context_mut(&mut self) -> &mut KernelContext {
    //     self.kernel_context.lock().deref_mut()
    // }

    // /// Get a reference to the kernel context
    // pub fn get_kernel_context(&self) -> &KernelContext {
    //     self.kernel_context.lock().deref()
    // }

    pub fn with_kernel_context<R>(&self, f: impl FnOnce(&mut KernelContext) -> R) -> R {
        let mut kctx = self.kernel_context.lock();
        f(&mut kctx)
    }

    /// Get the kernel stack bottom address for this task
    ///
    /// # Returns
    /// The kernel stack bottom address as u64, or 0 if no kernel stack is allocated
    pub fn get_kernel_stack_bottom_paddr(&self) -> u64 {
        self.kernel_context.lock().get_kernel_stack_bottom_paddr()
    }

    /// Get the kernel stack memory area for this task
    ///
    /// # Returns
    /// The kernel stack memory area as a MemoryArea
    ///
    pub fn get_kernel_stack_memory_area_paddr(&self) -> MemoryArea {
        self.kernel_context
            .lock()
            .get_kernel_stack_memory_area_paddr()
    }

    /// Get a mutable reference to the trapframe for this task
    ///
    /// The trapframe contains the user-space register state and is located
    /// at the top of the kernel stack. This provides access to modify the
    /// user context during system calls, interrupts, and context switches.
    ///
    /// If a kernel stack window is mapped (high-VA), this returns a reference
    /// via the mapped virtual address. Otherwise, it returns a reference via
    /// the physical address directly.
    ///
    /// # Returns
    /// A mutable reference to the Trapframe
    pub fn get_trapframe(&self) -> &mut Trapframe {
        // If we have a kernel stack window mapped, use the high-VA address
        if let Some((_slot, base)) = *self.kernel_stack_window_base.lock() {
            let trapframe_offset = crate::environment::PAGE_SIZE
                + crate::environment::TASK_KERNEL_STACK_SIZE
                - core::mem::size_of::<Trapframe>();
            let trapframe_vaddr = base + trapframe_offset;
            unsafe { &mut *(trapframe_vaddr as *mut Trapframe) }
        } else {
            // Fallback to physical address (should not happen for user tasks after init)
            // self.kernel_context.lock().get_trapframe()
            panic!("get_trapframe: No kernel stack window mapped");
        }
    }

    /// Internal: set kernel stack window base (slot index and base vaddr)
    pub fn set_kernel_stack_window_base(&self, base: Option<(usize, usize)>) {
        *self.kernel_stack_window_base.lock() = base;
    }

    /// Get kernel stack window base (slot index and base vaddr)
    pub fn get_kernel_stack_window_base(&self) -> Option<(usize, usize)> {
        *self.kernel_stack_window_base.lock()
    }
}

#[derive(Debug)]
pub enum WaitError {
    NoSuchChild(String),
    ChildNotExited(String),
    ChildTaskNotFound(String),
}

impl WaitError {
    pub fn message(&self) -> &str {
        match self {
            WaitError::NoSuchChild(msg) => msg,
            WaitError::ChildNotExited(msg) => msg,
            WaitError::ChildTaskNotFound(msg) => msg,
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        crate::vm::teardown_trampoline_for_task_kstack_window(self);
    }
}

/// Create a new kernel task.
///
/// # Arguments
/// * `name` - The name of the task
/// * `priority` - The priority of the task
/// * `func` - The function to run in the task
///
/// # Returns
/// The new task.
pub fn new_kernel_task(name: String, priority: u32, func: fn()) -> Task {
    let mut task = Task::new(name, priority, TaskType::Kernel);
    task.entry = func as usize;
    task
}

/// Create a new user task.
///
/// # Arguments
/// * `name` - The name of the task
/// * `priority` - The priority of the task
///
/// # Returns
/// The new task.
pub fn new_user_task(name: String, priority: u32) -> Task {
    Task::new(name, priority, TaskType::User)
}

#[cfg(test)]
static mut MOCK_CURRENT_TASK: Option<*mut Task> = None;

#[cfg(test)]
/// Set a mock current task for testing purposes
///
/// This function allows tests to override the return value of mytask()
/// for controlled testing scenarios.
///
/// # Arguments
/// * `task` - The task to return from mytask()
///
/// # Safety
/// The caller must ensure the task pointer remains valid for the duration
/// of the test and that clear_mock_current_task() is called when done.
/// This function is only safe to call in single-threaded test environments.
pub unsafe fn set_mock_current_task(task: &'static mut Task) {
    unsafe {
        MOCK_CURRENT_TASK = Some(task as *mut Task);
    }
}

#[cfg(test)]
/// Clear the mock current task, reverting to normal scheduler behavior
///
/// # Safety
/// This function is only safe to call in single-threaded test environments.
pub unsafe fn clear_mock_current_task() {
    unsafe {
        MOCK_CURRENT_TASK = None;
    }
}

/// Get the current task.
///
/// # Returns
/// The current task if it exists.
pub fn mytask() -> Option<&'static Task> {
    #[cfg(test)]
    {
        unsafe {
            if let Some(task_ptr) = MOCK_CURRENT_TASK {
                return Some(&*task_ptr);
            }
        }
    }

    let cpu = get_cpu();
    current_task(cpu.get_cpuid())
}

/// Set the current working directory for the current task via VfsManager
///
/// This function sets the current working directory of the calling task
/// using the VfsManager's path-based API.
///
/// # Arguments
/// * `path` - The new working directory path
///
/// # Returns
/// * `true` if successful, `false` if no current task or VfsManager
pub fn set_current_task_cwd(path: String) -> bool {
    if let Some(task) = mytask() {
        if let Some(vfs) = task.vfs.read().as_ref() {
            // Use VfsManager to set current working directory
            vfs.set_cwd_by_path(&path).is_ok()
        } else {
            false // No VfsManager available
        }
    } else {
        false
    }
}

/// Internal function to perform kernel context switch between tasks
/// This function is called when a task is first scheduled.
pub fn task_initial_kernel_entrypoint() -> ! {
    let cpu = get_cpu();
    crate::sched::scheduler::complete_deferred_context_switch(cpu.get_cpuid());
    if let Some(current_task) = current_task(cpu.get_cpuid()) {
        if crate::sched::scheduler::DEBUG_SMP_TASK_FLOW {
            let vcpu = current_task.vcpu.lock();
            crate::println!(
                "[SMPDBG task-entry] cpu={} task={} name={} type={:?} state={:?} running_cpu={} last_cpu={} pc={:#x} mode={:?}",
                cpu.get_cpuid(),
                current_task.get_id(),
                current_task.name.read().as_str(),
                current_task.task_type,
                current_task.state.load(Ordering::SeqCst),
                current_task.running_cpu.load(Ordering::SeqCst),
                current_task.last_cpu.load(Ordering::SeqCst),
                vcpu.get_pc(),
                vcpu.get_mode(),
            );
        }
        if current_task.task_type == TaskType::Kernel {
            let entry = current_task.vcpu.lock().get_pc();
            // SAFETY: `new_kernel_task` stores a valid `fn()` entry pointer in
            // the kernel-mode VCPU PC before the task is made runnable.
            let entry: fn() = unsafe { core::mem::transmute(entry as usize) };
            if crate::sched::scheduler::DEBUG_SMP_TASK_FLOW {
                crate::println!(
                    "[SMPDBG task-entry-kernel-jump] cpu={} task={} entry={:#x}",
                    cpu.get_cpuid(),
                    current_task.get_id(),
                    entry as usize,
                );
            }
            entry();
            current_task.exit(0);
            loop {
                crate::arch::instruction::idle();
            }
        }

        if crate::sched::scheduler::DEBUG_SMP_TASK_FLOW {
            crate::println!(
                "[SMPDBG task-entry-user-jump] cpu={} task={} name={}",
                cpu.get_cpuid(),
                current_task.get_id(),
                current_task.name.read().as_str(),
            );
        }
        setup_task_execution(cpu, current_task);
        arch_switch_to_user(current_task.get_trapframe());
    }

    loop {
        crate::arch::instruction::idle();
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::sync::Arc;
    use core::sync::atomic::Ordering;

    use super::INIT_TASK_ID;
    use crate::object::capability::memory_mapping::AccessOp;
    use crate::sched::scheduler::{add_task, finalize_zombie, get_task_by_id, reset};
    use crate::task::{CloneFlags, CloneFlagsDef, TaskState};
    use crate::vm::addr::{phys_to_virt, virt_to_phys};

    #[test_case]
    fn test_set_brk() {
        let mut task = super::new_user_task("Task0".to_string(), 0);
        task.init();
        assert_eq!(task.get_brk(), 0);
        task.set_brk(0x1000).unwrap();
        assert_eq!(task.get_brk(), 0x1000);
        task.set_brk(0x2000).unwrap();
        assert_eq!(task.get_brk(), 0x2000);
        task.set_brk(0x1008).unwrap();
        assert_eq!(task.get_brk(), 0x1008);
        task.set_brk(0x1000).unwrap();
        assert_eq!(task.get_brk(), 0x1000);
    }

    #[test_case]
    fn test_task_parent_child_relationship() {
        // Reset scheduler state before test
        reset();

        let mut parent_task = super::new_user_task("ParentTask".to_string(), 0);
        parent_task.init();

        let mut child_task = super::new_user_task("ChildTask".to_string(), 0);
        child_task.init();

        // Add tasks to scheduler to allocate IDs
        let parent_id = add_task(parent_task, 0);
        let child_id = add_task(child_task, 0);

        // Set parent-child relationship using allocated IDs
        // We need to do this sequentially due to borrow checker
        {
            let child_task = get_task_by_id(child_id).unwrap();
            child_task.set_parent_id(parent_id);
        }
        {
            let parent_task = get_task_by_id(parent_id).unwrap();
            parent_task.add_child(child_id);
        }

        // Verify parent-child relationship
        {
            let child_task = get_task_by_id(child_id).unwrap();
            assert_eq!(child_task.get_parent_id(), Some(parent_id));
        }
        {
            let parent_task = get_task_by_id(parent_id).unwrap();
            assert!(parent_task.get_children().contains(&child_id));
        }

        // Remove child and verify
        {
            let parent_task = get_task_by_id(parent_id).unwrap();
            assert!(parent_task.remove_child(child_id));
            assert!(!parent_task.get_children().contains(&child_id));
        }
    }

    #[test_case]
    fn test_task_reparents_children_to_init_on_exit() {
        reset();

        let mut init_task = super::new_user_task("InitTask".to_string(), 0);
        init_task.init();
        let init_id = add_task(init_task, 0);
        assert_eq!(init_id, INIT_TASK_ID);

        let mut parent_task = super::new_user_task("ParentTask".to_string(), 0);
        parent_task.init();
        let parent_id = add_task(parent_task, 0);

        let mut child_task = super::new_user_task("ChildTask".to_string(), 0);
        child_task.init();
        let child_id = add_task(child_task, 0);

        let parent = get_task_by_id(parent_id).unwrap();
        let child = get_task_by_id(child_id).unwrap();
        child.set_parent_id(parent_id);
        parent.add_child(child_id);

        parent.exit(0);

        let init = get_task_by_id(init_id).unwrap();
        let child = get_task_by_id(child_id).unwrap();
        assert_eq!(child.get_parent_id(), Some(init_id));
        assert!(init.get_children().contains(&child_id));
        assert!(!parent.get_children().contains(&child_id));
    }

    #[test_case]
    fn test_task_reparents_zombie_children_to_init_on_exit() {
        reset();

        let mut init_task = super::new_user_task("InitTask".to_string(), 0);
        init_task.init();
        let init_id = add_task(init_task, 0);
        assert_eq!(init_id, INIT_TASK_ID);

        let mut parent_task = super::new_user_task("ParentTask".to_string(), 0);
        parent_task.init();
        let parent_id = add_task(parent_task, 0);

        let mut child_task = super::new_user_task("ChildTask".to_string(), 0);
        child_task.init();
        let child_id = add_task(child_task, 0);

        let parent = get_task_by_id(parent_id).unwrap();
        let child = get_task_by_id(child_id).unwrap();
        child.set_parent_id(parent_id);
        parent.add_child(child_id);
        child.set_exit_status(7);
        child.set_state(TaskState::Zombie);
        finalize_zombie(child_id, Some(parent_id));

        parent.exit(0);

        let init = get_task_by_id(init_id).unwrap();
        let child = get_task_by_id(child_id).unwrap();
        assert_eq!(child.get_parent_id(), Some(init_id));
        assert_eq!(child.get_state(), TaskState::Zombie);
        assert!(init.get_children().contains(&child_id));

        match init.wait(child_id) {
            Ok(status) => assert_eq!(status, 7),
            Err(error) => panic!("wait failed: {:?}", error),
        }
        assert!(get_task_by_id(child_id).is_none());
    }

    #[test_case]
    fn test_task_session_and_process_group_defaults() {
        reset();

        let mut task = super::new_user_task("SessionDefaults".to_string(), 0);
        task.init();
        let task_id = add_task(task, 0);

        let task = get_task_by_id(task_id).unwrap();
        assert_eq!(task.get_session_id(), task_id);
        assert_eq!(task.get_process_group_id(), task_id);
        assert_eq!(task.get_task_group_id(), task_id);
        assert!(task.is_session_leader());
        assert!(task.get_controlling_tty().is_none());
    }

    #[test_case]
    fn test_clone_inherits_session_and_process_group() {
        reset();

        let mut parent = super::new_user_task("SessionParent".to_string(), 0);
        parent.init();
        let parent_id = add_task(parent, 0);

        let parent = get_task_by_id(parent_id).unwrap();
        let child = parent.clone_task(CloneFlags::default()).unwrap();
        let child_id = add_task(child, 0);

        let child = get_task_by_id(child_id).unwrap();
        assert_eq!(child.get_session_id(), parent.get_session_id());
        assert_eq!(child.get_process_group_id(), parent.get_process_group_id());
        assert_eq!(child.get_task_group_id(), parent.get_process_group_id());
        assert!(!child.is_session_leader());
        assert!(child.get_controlling_tty().is_none());
    }

    #[test_case]
    fn test_fork_clone_becomes_new_thread_group_leader() {
        reset();

        let mut parent = super::new_user_task("ThreadGroupParent".to_string(), 0);
        parent.init();
        let parent_id = add_task(parent, 0);

        let parent = get_task_by_id(parent_id).unwrap();
        let child = parent.clone_task(CloneFlags::default()).unwrap();
        assert_eq!(child.thread_group_id, 0);

        let child_id = add_task(child, 0);
        let child = get_task_by_id(child_id).unwrap();
        assert_eq!(child.get_thread_group_id(), child_id);
    }

    #[test_case]
    fn test_thread_clone_inherits_thread_group() {
        reset();

        let mut parent = super::new_user_task("ThreadGroupParent".to_string(), 0);
        parent.init();
        let parent_id = add_task(parent, 0);

        let parent = get_task_by_id(parent_id).unwrap();
        let mut flags = CloneFlags::default();
        flags.set(CloneFlagsDef::Thread);
        let child = parent.clone_task(flags).unwrap();
        assert_eq!(child.get_thread_group_id(), parent.get_thread_group_id());

        let child_id = add_task(child, 0);
        let child = get_task_by_id(child_id).unwrap();
        assert_eq!(child.get_thread_group_id(), parent.get_thread_group_id());
    }

    #[test_case]
    fn test_exit_group_from_non_leader_makes_leader_waitable() {
        reset();

        let mut parent = super::new_user_task("WaitParent".to_string(), 0);
        parent.init();
        let parent_id = add_task(parent, 0);

        let mut leader = super::new_user_task("ProcessLeader".to_string(), 0);
        leader.init();
        let leader_id = add_task(leader, 0);

        let parent = get_task_by_id(parent_id).unwrap();
        let leader = get_task_by_id(leader_id).unwrap();
        leader.set_parent_id(parent_id);
        parent.add_child(leader_id);

        let mut flags = CloneFlags::default();
        flags.set(CloneFlagsDef::Thread);
        let worker = leader.clone_task(flags).unwrap();
        let worker_id = add_task(worker, 0);

        let worker = get_task_by_id(worker_id).unwrap();
        worker.exit_group(130);

        let leader = get_task_by_id(leader_id).unwrap();
        assert_eq!(leader.get_state(), TaskState::Zombie);
        assert_eq!(worker.get_state(), TaskState::Terminated);

        match parent.wait(leader_id) {
            Ok(status) => assert_eq!(status, 130),
            Err(error) => panic!("wait failed: {:?}", error),
        }
        assert!(get_task_by_id(leader_id).is_none());
    }

    #[test_case]
    fn test_create_session_requires_non_process_group_leader() {
        reset();

        let mut task = super::new_user_task("SetsidTask".to_string(), 0);
        task.init();
        let task_id = add_task(task, 0);

        let task = get_task_by_id(task_id).unwrap();
        assert!(task.create_session().is_err());

        task.set_process_group_id(task_id + 1);
        assert_eq!(task.create_session(), Ok(task_id));
        assert_eq!(task.get_session_id(), task_id);
        assert_eq!(task.get_process_group_id(), task_id);
        assert!(task.is_session_leader());
        assert!(task.get_controlling_tty().is_none());
    }

    #[test_case]
    fn test_process_control_stop_report_latches_once() {
        let mut task = super::new_user_task("StopReportTask".to_string(), 0);
        task.init();

        assert!(!task.take_process_control_stop_report());

        task.mark_process_control_stopped();
        assert!(task.take_process_control_stop_report());
        assert!(!task.take_process_control_stop_report());

        task.clear_process_control_stopped();
        assert!(!task.take_process_control_stop_report());
    }

    #[test_case]
    fn test_task_exit_status() {
        let mut task = super::new_user_task("TaskWithExitStatus".to_string(), 0);
        task.init();

        // Verify initial exit status is None
        assert_eq!(task.get_exit_status(), None);

        // Set and verify exit status
        task.set_exit_status(0);
        assert_eq!(task.get_exit_status(), Some(0));

        task.set_exit_status(1);
        assert_eq!(task.get_exit_status(), Some(1));
    }

    #[test_case]
    fn test_clone_task_memory_copy() {
        // Reset scheduler state before test
        reset();

        let mut parent_task = super::new_user_task("ParentTask".to_string(), 0);
        parent_task.init();

        // Allocate some memory pages for the parent task
        let vaddr = 0x1000;
        let num_pages = 2;
        let mmap = parent_task.allocate_data_pages(vaddr, num_pages).unwrap();

        // Save the physical address and permissions before adding to scheduler
        let parent_paddr = mmap.pmarea.start;
        let parent_vaddr_start = mmap.vmarea.start;
        let parent_vaddr_end = mmap.vmarea.end;
        let parent_perms = mmap.permissions;

        // Write test data to parent's memory
        let test_data: [u8; 8] = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0];
        unsafe {
            let dst_ptr = phys_to_virt(mmap.pmarea.start) as *mut u8;
            core::ptr::copy_nonoverlapping(test_data.as_ptr(), dst_ptr, test_data.len());
        }

        // Get parent memory map count before cloning
        let parent_memmap_count = parent_task.vm_manager.memmap_len();

        // Clone the parent task
        let child_task = parent_task.clone_task(CloneFlags::default()).unwrap();

        // For fork-like clones (no CLONE_VM), brk must NOT be shared.
        assert!(
            !Arc::ptr_eq(&child_task.brk, &parent_task.brk),
            "Child should not share brk with parent unless CLONE_VM is set"
        );

        // Get child memory map count after cloning
        let child_memmap_count = child_task.vm_manager.memmap_len();

        // Verify that the number of memory maps are identical
        assert_eq!(
            child_memmap_count, parent_memmap_count,
            "Child should have the same number of memory maps as parent: child={}, parent={}",
            child_memmap_count, parent_memmap_count
        );

        // Save values that will be needed after add_task
        let parent_pc = parent_task.vcpu.lock().get_pc();
        let parent_entry = parent_task.entry;
        let parent_state = parent_task.state.load(Ordering::SeqCst);
        let child_pc = child_task.vcpu.lock().get_pc();
        let child_entry = child_task.entry;
        let child_state = child_task.state.load(Ordering::SeqCst);

        // Add both tasks to scheduler to establish parent-child relationship
        let parent_id = add_task(parent_task, 0);
        let child_id = add_task(child_task, 0);

        // Establish parent-child relationship
        {
            let child = get_task_by_id(child_id).unwrap();
            child.set_parent_id(parent_id);
        }
        {
            let parent = get_task_by_id(parent_id).unwrap();
            parent.add_child(child_id);
        }

        // Verify parent-child relationship was established (in separate scopes)
        {
            let child = get_task_by_id(child_id).unwrap();
            assert_eq!(child.get_parent_id(), Some(parent_id));
        }
        {
            let parent = get_task_by_id(parent_id).unwrap();
            assert!(parent.get_children().contains(&child_id));
        }

        // Get references for further verification (in separate scopes)
        let child_stack_size = {
            let child = get_task_by_id(child_id).unwrap();
            child.stack_size.load(Ordering::SeqCst)
        };
        let child_data_size = {
            let child = get_task_by_id(child_id).unwrap();
            child.data_size.load(Ordering::SeqCst)
        };
        let child_text_size = {
            let child = get_task_by_id(child_id).unwrap();
            child.text_size.load(Ordering::SeqCst)
        };
        let parent_stack_size = {
            let parent = get_task_by_id(parent_id).unwrap();
            parent.stack_size.load(Ordering::SeqCst)
        };
        let parent_data_size = {
            let parent = get_task_by_id(parent_id).unwrap();
            parent.data_size.load(Ordering::SeqCst)
        };
        let parent_text_size = {
            let parent = get_task_by_id(parent_id).unwrap();
            parent.text_size.load(Ordering::SeqCst)
        };

        // Verify memory sizes were copied
        assert_eq!(child_stack_size, parent_stack_size);
        assert_eq!(child_data_size, parent_data_size);
        assert_eq!(child_text_size, parent_text_size);

        // Find the corresponding memory maps that match our test allocation.
        let parent_mmap_after_fork = {
            let mut found = None;
            let parent = get_task_by_id(parent_id).unwrap();
            parent.vm_manager.with_memmaps(|mm| {
                for m in mm.values() {
                    if m.vmarea.start == vaddr
                        && m.vmarea.end == vaddr + num_pages * crate::environment::PAGE_SIZE - 1
                    {
                        found = Some(m.clone());
                        break;
                    }
                }
            });
            found.expect("Test memory map not found in parent task")
        };
        let child_mmap = {
            let mut found = None;
            let child = get_task_by_id(child_id).unwrap();
            child.vm_manager.with_memmaps(|mm| {
                for m in mm.values() {
                    if m.vmarea.start == vaddr
                        && m.vmarea.end == vaddr + num_pages * crate::environment::PAGE_SIZE - 1
                    {
                        found = Some(m.clone());
                        break;
                    }
                }
            });
            found.expect("Test memory map not found in child task")
        };

        // Verify the virtual memory ranges match
        assert_eq!(child_mmap.vmarea.start, parent_vaddr_start);
        assert_eq!(child_mmap.vmarea.end, parent_vaddr_end);
        assert_eq!(child_mmap.permissions, parent_perms);
        assert_eq!(parent_mmap_after_fork.pmarea.start, 0);
        assert!(parent_mmap_after_fork.owner.is_some());
        assert_eq!(child_mmap.pmarea.start, 0);
        assert!(child_mmap.owner.is_some());

        use crate::object::capability::memory_mapping::{AccessKind, AccessOp};
        let parent = get_task_by_id(parent_id).unwrap();
        let child = get_task_by_id(child_id).unwrap();
        parent
            .vm_manager
            .lazy_map_page_with(AccessKind {
                op: AccessOp::Load,
                vaddr,
                size: None,
            })
            .unwrap();
        child
            .vm_manager
            .lazy_map_page_with(AccessKind {
                op: AccessOp::Load,
                vaddr,
                size: None,
            })
            .unwrap();

        assert_eq!(
            parent.vm_manager.translate_to_phys(vaddr),
            Some(parent_paddr)
        );
        assert_eq!(
            child.vm_manager.translate_to_phys(vaddr),
            Some(parent_paddr)
        );

        child
            .vm_manager
            .lazy_map_page_with(AccessKind {
                op: AccessOp::Store,
                vaddr,
                size: None,
            })
            .unwrap();
        let child_private_paddr = child.vm_manager.translate_to_phys(vaddr).unwrap();
        assert_ne!(
            child_private_paddr, parent_paddr,
            "Child store fault should allocate a private page"
        );

        // Verify that modifying child's private page doesn't affect parent's COW backing.
        unsafe {
            let parent_ptr = phys_to_virt(mmap.pmarea.start) as *mut u8;
            let original_value = *parent_ptr;

            let child_ptr = phys_to_virt(child_private_paddr) as *mut u8;
            *child_ptr = 0xFF;

            let parent_first_byte = *parent_ptr;
            assert_eq!(
                parent_first_byte, original_value,
                "Child private write should not modify parent backing"
            );
        }

        // Verify register states were copied
        assert_eq!(child_pc, parent_pc);

        // Verify entry point was copied
        assert_eq!(child_entry, parent_entry);

        // Verify state was copied
        assert_eq!(child_state, parent_state);

        let child_private_mmap = child.vm_manager.search_memory_map(vaddr).unwrap();
        assert_eq!(child_private_mmap.pmarea.start, child_private_paddr);
        assert!(child_private_mmap.owner.is_none());
        assert!(
            child
                .page_allocations
                .read()
                .iter()
                .any(|alloc| { alloc.as_paddr() == child_private_paddr && alloc.len() == 1 }),
            "Child COW private page should be tracked for reclaim"
        );
    }

    #[test_case]
    fn test_clone_task_stack_copy() {
        // Reset scheduler state before test
        reset();

        let mut parent_task = super::new_user_task("ParentWithStack".to_string(), 0);
        parent_task.init();

        // Find the stack memory map in parent
        let stack_mmap = {
            let mut found = None;
            parent_task.vm_manager.with_memmaps(|mm| {
                for mmap in mm.values() {
                    // Stack should be near USER_STACK_END and have stack permissions
                    use crate::vm::vmem::VirtualMemoryRegion;
                    if mmap.vmarea.end == crate::environment::USER_STACK_END - 1
                        && mmap.permissions == VirtualMemoryRegion::Stack.default_permissions()
                    {
                        found = Some(mmap.clone());
                        break;
                    }
                }
            });
            found.expect("Stack memory map not found in parent task")
        };

        let stack_data_vaddr = stack_mmap.vmarea.start + crate::environment::PAGE_SIZE;

        // Write test data to parent's stack before clone. At this point the
        // parent owns the physical stack allocation directly.
        let stack_test_data: [u8; 16] = [
            0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
            0x99, 0x00,
        ];
        unsafe {
            let stack_ptr =
                phys_to_virt(stack_mmap.pmarea.start + crate::environment::PAGE_SIZE) as *mut u8;
            core::ptr::copy_nonoverlapping(
                stack_test_data.as_ptr(),
                stack_ptr,
                stack_test_data.len(),
            );
        }

        // Clone the parent task
        let child_task = parent_task.clone_task(CloneFlags::default()).unwrap();

        // Find the corresponding stack memory map in child
        let child_stack_mmap = {
            let mut found = None;
            child_task.vm_manager.with_memmaps(|mm| {
                for mmap in mm.values() {
                    use crate::vm::vmem::VirtualMemoryRegion;
                    if mmap.vmarea.start == stack_mmap.vmarea.start
                        && mmap.vmarea.end == stack_mmap.vmarea.end
                        && mmap.permissions == VirtualMemoryRegion::Stack.default_permissions()
                    {
                        found = Some(mmap.clone());
                        break;
                    }
                }
            });
            found.expect("Stack memory map not found in child task")
        };

        // Fork converts private stack pages to a COW owner. Reads in parent and
        // child should resolve to the same backing page until one side stores.
        let parent_shared_paddr = parent_task
            .vm_manager
            .translate_to_phys(stack_data_vaddr)
            .expect("Parent stack COW backing not resolved");
        let child_shared_paddr = child_task
            .vm_manager
            .translate_to_phys(stack_data_vaddr)
            .expect("Child stack COW backing not resolved");
        assert_eq!(
            parent_shared_paddr, child_shared_paddr,
            "Parent and child should share stack backing before a COW store"
        );
        assert_eq!(
            child_stack_mmap.pmarea.start, 0,
            "Child COW stack map should not expose a direct physical range"
        );
        assert!(
            child_stack_mmap.owner.is_some(),
            "Child COW stack map should keep an owner for fault resolution"
        );

        // Verify that stack content is visible through the COW backing.
        unsafe {
            let parent_stack_ptr = phys_to_virt(parent_shared_paddr) as *const u8;
            let child_stack_ptr = phys_to_virt(child_shared_paddr) as *const u8;

            // Check that the stack data content is identical
            for i in 0..stack_test_data.len() {
                let parent_byte = *parent_stack_ptr.offset(i as isize);
                let child_byte = *child_stack_ptr.offset(i as isize);
                assert_eq!(
                    parent_byte, child_byte,
                    "Stack data mismatch at offset {}: parent={:#x}, child={:#x}",
                    i, parent_byte, child_byte
                );
            }
        }

        // Verify that modifying parent's stack triggers COW and doesn't affect child's stack.
        let parent_private_paddr = parent_task
            .vm_manager
            .translate_to_phys_with_access(stack_data_vaddr, AccessOp::Store)
            .expect("Parent stack store COW did not allocate a private page");
        assert_ne!(
            parent_private_paddr, child_shared_paddr,
            "Parent store should allocate a private stack page"
        );

        unsafe {
            let parent_stack_ptr = phys_to_virt(parent_private_paddr) as *mut u8;
            let original_value = *parent_stack_ptr;
            *parent_stack_ptr = 0xFE; // Modify first byte in parent stack

            let child_stack_paddr = child_task
                .vm_manager
                .translate_to_phys(stack_data_vaddr)
                .expect("Child stack backing disappeared after parent COW");
            let child_stack_ptr = phys_to_virt(child_stack_paddr) as *const u8;
            let child_first_byte = *child_stack_ptr;

            // Child's first byte should still be the original value
            assert_eq!(
                child_first_byte, original_value,
                "Child stack should be independent from parent stack"
            );
        }

        // Verify stack sizes match
        assert_eq!(
            child_task.stack_size.load(Ordering::SeqCst),
            parent_task.stack_size.load(Ordering::SeqCst),
            "Child and parent should have the same stack size"
        );
    }

    #[test_case]
    fn test_clone_task_shared_memory() {
        // Reset scheduler state before test
        reset();

        use crate::environment::PAGE_SIZE;
        use crate::mem::page::allocate_raw_pages;
        use crate::vm::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission};

        let mut parent_task = super::new_user_task("ParentWithShared".to_string(), 0);
        parent_task.init();

        // Manually add a shared memory region to test sharing behavior
        let shared_vaddr = 0x5000;
        let num_pages = 1;
        let pages = allocate_raw_pages(num_pages);
        let paddr = virt_to_phys(pages as usize);

        let shared_mmap = VirtualMemoryMap {
            pmarea: MemoryArea {
                start: paddr,
                end: paddr + PAGE_SIZE - 1,
            },
            vmarea: MemoryArea {
                start: shared_vaddr,
                end: shared_vaddr + PAGE_SIZE - 1,
            },
            vm_start: shared_vaddr,
            permissions: VirtualMemoryPermission::Read as usize
                | VirtualMemoryPermission::Write as usize,
            is_shared: true, // This should be shared between parent and child
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: None,
        };

        // Add shared memory map to parent
        parent_task
            .vm_manager
            .add_memory_map(shared_mmap.clone())
            .unwrap();

        // Write test data to shared memory
        let test_data: [u8; 8] = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22];
        unsafe {
            let shared_ptr = phys_to_virt(paddr) as *mut u8;
            core::ptr::copy_nonoverlapping(test_data.as_ptr(), shared_ptr, test_data.len());
        }

        // Clone the parent task
        let child_task = parent_task.clone_task(CloneFlags::default()).unwrap();

        // Find the shared memory map in child
        let child_shared_mmap = {
            let mut found = None;
            child_task.vm_manager.with_memmaps(|mm| {
                for mmap in mm.values() {
                    if mmap.vmarea.start == shared_vaddr && mmap.is_shared {
                        found = Some(mmap.clone());
                        break;
                    }
                }
            });
            found.expect("Shared memory map not found in child task")
        };

        // Verify that the physical addresses are the same (shared memory)
        assert_eq!(
            child_shared_mmap.pmarea.start, shared_mmap.pmarea.start,
            "Shared memory should have the same physical address in parent and child"
        );

        // Verify that the virtual addresses are the same
        assert_eq!(child_shared_mmap.vmarea.start, shared_mmap.vmarea.start);
        assert_eq!(child_shared_mmap.vmarea.end, shared_mmap.vmarea.end);

        // Verify that is_shared flag is preserved
        assert!(
            child_shared_mmap.is_shared,
            "Shared memory should remain marked as shared"
        );

        // Verify that modifying shared memory from child affects parent
        unsafe {
            let child_shared_ptr = phys_to_virt(child_shared_mmap.pmarea.start) as *mut u8;
            let original_value = *child_shared_ptr;
            *child_shared_ptr = 0xFF; // Modify first byte through child reference

            let parent_shared_ptr = phys_to_virt(shared_mmap.pmarea.start) as *const u8;
            let parent_first_byte = *parent_shared_ptr;

            // Parent should see the change made by child (shared memory)
            assert_eq!(
                parent_first_byte, 0xFF,
                "Parent should see changes made through child's shared memory reference"
            );

            // Restore original value
            *child_shared_ptr = original_value;
        }

        // Verify that the shared data content is accessible from both
        unsafe {
            let child_ptr = phys_to_virt(child_shared_mmap.pmarea.start) as *const u8;
            let parent_ptr = phys_to_virt(shared_mmap.pmarea.start) as *const u8;

            // Check that the data content is identical and accessible from both
            for i in 0..test_data.len() {
                let parent_byte = *parent_ptr.offset(i as isize);
                let child_byte = *child_ptr.offset(i as isize);
                assert_eq!(
                    parent_byte, child_byte,
                    "Shared memory data should be identical from both parent and child views"
                );
            }
        }
    }

    #[test_case]
    fn test_clone_task_with_clone_vm_shares_address_space() {
        // Reset scheduler state before test
        reset();

        use crate::environment::PAGE_SIZE;

        let mut parent = super::new_user_task("ParentCloneVm".to_string(), 0);
        parent.init();

        // Allocate one page initially in the parent
        let base_vaddr = 0x4000;
        parent.allocate_data_pages(base_vaddr, 1).unwrap();
        let parent_len_before = parent.vm_manager.memmap_len();

        // Clone with CLONE_VM flag (share the address space only)
        let mut flags = super::CloneFlags::new();
        flags.set(super::CloneFlagsDef::Vm);
        let child = parent.clone_task(flags).unwrap();

        // CLONE_VM: brk must be shared because heap lives in the shared address space.
        assert!(
            Arc::ptr_eq(&child.brk, &parent.brk),
            "CLONE_VM tasks must share brk"
        );

        // Indirectly verify that both share the same ASID/address space
        assert_eq!(child.vm_manager.get_asid(), parent.vm_manager.get_asid());
        assert_eq!(child.vm_manager.memmap_len(), parent_len_before);

        // Adding another page in the parent should be immediately visible to the child
        parent
            .allocate_data_pages(base_vaddr + PAGE_SIZE, 1)
            .unwrap();
        assert_eq!(
            child.vm_manager.memmap_len(),
            parent.vm_manager.memmap_len()
        );

        // Page allocations are per-task; child should not acquire new page allocations
        // when sharing VM (physical memory isn't privately managed by the child)
        assert!(child.page_allocations.read().len() <= parent.page_allocations.read().len());
    }

    #[test_case]
    fn test_task_namespace_creation() {
        // Reset scheduler state before test
        reset();

        // Create task in root namespace
        let task = super::new_user_task("TestTask".to_string(), 0);
        assert_eq!(task.get_namespace().get_name(), "root");
        assert!(task.get_namespace().is_root());

        // Add task to scheduler to allocate namespace ID
        let task_id = add_task(task, 0);

        // Verify namespace-local ID was allocated
        let ns_id = get_task_by_id(task_id).unwrap().get_namespace_id();
        assert!(ns_id >= 1); // Should start from 1
    }

    #[test_case]
    fn test_task_namespace_inheritance() {
        // Reset scheduler state before test
        reset();

        let mut parent = super::new_user_task("Parent".to_string(), 0);
        parent.init();

        // Clone should inherit parent's namespace
        let child = parent.clone_task(CloneFlags::default()).unwrap();

        // Both should be in same namespace
        assert_eq!(
            parent.get_namespace().get_id(),
            child.get_namespace().get_id()
        );

        // Add both to scheduler to allocate namespace IDs
        let parent_id = add_task(parent, 0);
        let child_id = add_task(child, 0);

        // But should have different namespace-local IDs
        let parent_ns_id = get_task_by_id(parent_id).unwrap().get_namespace_id();
        let child_ns_id = get_task_by_id(child_id).unwrap().get_namespace_id();
        assert_ne!(parent_ns_id, child_ns_id);
    }

    #[test_case]
    fn test_task_namespace_id_allocation() {
        // Reset scheduler state before test
        reset();

        use super::namespace;

        // Create custom namespace
        let custom_ns = namespace::TaskNamespace::new_child(
            namespace::get_root_namespace().clone(),
            "test_ns".to_string(),
        );

        // Create multiple tasks in the same namespace
        let mut task1 = super::Task::new_with_namespace(
            "Task1".to_string(),
            0,
            super::TaskType::User,
            custom_ns.clone(),
        );
        let mut task2 = super::Task::new_with_namespace(
            "Task2".to_string(),
            0,
            super::TaskType::User,
            custom_ns.clone(),
        );
        let mut task3 = super::Task::new_with_namespace(
            "Task3".to_string(),
            0,
            super::TaskType::User,
            custom_ns.clone(),
        );

        // Initialize tasks before adding to scheduler
        task1.init();
        task2.init();
        task3.init();

        // Add tasks to scheduler to allocate IDs
        let id1 = add_task(task1, 0);
        let id2 = add_task(task2, 0);
        let id3 = add_task(task3, 0);

        // All should have sequential namespace-local IDs
        let ns_id1 = get_task_by_id(id1).unwrap().get_namespace_id();
        let ns_id2 = get_task_by_id(id2).unwrap().get_namespace_id();
        let ns_id3 = get_task_by_id(id3).unwrap().get_namespace_id();
        assert_eq!(ns_id1, 1);
        assert_eq!(ns_id2, 2);
        assert_eq!(ns_id3, 3);

        // All should have different global IDs
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[test_case]
    fn test_namespace_hierarchy() {
        use super::namespace;

        let root = namespace::get_root_namespace();
        let child_ns = namespace::TaskNamespace::new_child(root.clone(), "child".to_string());
        let grandchild_ns =
            namespace::TaskNamespace::new_child(child_ns.clone(), "grandchild".to_string());

        // Verify hierarchy
        assert!(root.is_root());
        assert!(!child_ns.is_root());
        assert!(!grandchild_ns.is_root());

        // Verify parent relationships
        assert!(child_ns.get_parent().is_some());
        assert_eq!(child_ns.get_parent().unwrap().get_id(), root.get_id());
        assert_eq!(
            grandchild_ns.get_parent().unwrap().get_id(),
            child_ns.get_id()
        );
    }

    #[test_case]
    fn test_all_abis_share_root_namespace_by_default() {
        // Reset scheduler state before test
        reset();

        // Create tasks using default Task::new (which uses root namespace)
        let mut task1 = super::new_user_task("Task1".to_string(), 0);
        let mut task2 = super::new_user_task("Task2".to_string(), 0);
        let mut task3 = super::new_user_task("Task3".to_string(), 0);

        // Initialize tasks before adding to scheduler
        task1.init();
        task2.init();
        task3.init();

        // Add tasks to scheduler to allocate namespace IDs
        let id1 = add_task(task1, 0);
        let id2 = add_task(task2, 0);
        let id3 = add_task(task3, 0);

        // Verify all tasks have valid IDs after being added to scheduler
        assert_ne!(id1, 0, "Task ID should be non-zero after add_task");
        assert_ne!(id2, 0, "Task ID should be non-zero after add_task");
        assert_ne!(id3, 0, "Task ID should be non-zero after add_task");

        // Get namespace IDs to verify (in separate scopes to avoid borrow issues)
        let ns_id1 = get_task_by_id(id1).unwrap().get_namespace_id();
        assert_ne!(ns_id1, 0, "Namespace ID should be non-zero after add_task");

        let ns_id2 = get_task_by_id(id2).unwrap().get_namespace_id();
        assert_ne!(ns_id2, 0, "Namespace ID should be non-zero after add_task");

        let ns_id3 = get_task_by_id(id3).unwrap().get_namespace_id();
        assert_ne!(ns_id3, 0, "Namespace ID should be non-zero after add_task");

        // Verify namespace IDs are unique
        assert_ne!(ns_id1, ns_id2, "Namespace IDs should be unique");
        assert_ne!(ns_id2, ns_id3, "Namespace IDs should be unique");

        // Verify all tasks are in root namespace (in separate scopes)
        {
            let task1 = get_task_by_id(id1).unwrap();
            assert_eq!(task1.get_namespace().get_name(), "root");
        }
        {
            let task2 = get_task_by_id(id2).unwrap();
            assert_eq!(task2.get_namespace().get_name(), "root");
        }
        {
            let task3 = get_task_by_id(id3).unwrap();
            assert_eq!(task3.get_namespace().get_name(), "root");
        }

        // Verify all tasks share the same namespace instance
        let ns1_id = get_task_by_id(id1).unwrap().get_namespace().get_id();
        let ns2_id = get_task_by_id(id2).unwrap().get_namespace().get_id();
        let ns3_id = get_task_by_id(id3).unwrap().get_namespace().get_id();
        assert_eq!(ns1_id, ns2_id, "All tasks should share root namespace");
        assert_eq!(ns2_id, ns3_id, "All tasks should share root namespace");
    }
}
