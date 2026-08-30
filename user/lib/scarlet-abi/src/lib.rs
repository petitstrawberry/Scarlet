//! Scarlet Native ABI definitions.
//!
//! This crate contains raw ABI definitions shared by Scarlet userland
//! libraries and, eventually, Rust `std`'s Scarlet PAL. It intentionally avoids
//! syscall assembly or safe wrappers so it can stay `no_std` and dependency-free.

#![no_std]

/// Raw kernel object handle value used at the Scarlet Native ABI boundary.
pub type RawHandle = i32;

/// Raw process identifier exposed by Scarlet Native process syscalls.
pub type Pid = u32;

/// Raw thread identifier exposed by Scarlet Native thread syscalls.
pub type Tid = u32;

/// Require `GetRandom` to use a registered entropy source instead of the
/// non-cryptographic emergency fallback.
pub const GET_RANDOM_FLAG_REQUIRE_ENTROPY: usize = 1 << 0;

/// Version of the task-debug snapshot ABI implemented by Scarlet.
pub const TASK_DEBUG_INFO_VERSION_V1: u16 = 1;
/// The snapshot contains a valid last-observed instruction address.
pub const TASK_DEBUG_FLAG_PC_VALID: u32 = 1 << 0;
/// The last-observed instruction address was sampled in privileged mode.
pub const TASK_DEBUG_FLAG_PC_PRIVILEGED: u32 = 1 << 1;
/// The snapshot contains information about an entered system call.
pub const TASK_DEBUG_FLAG_SYSCALL_VALID: u32 = 1 << 2;
/// The task has not yet returned from the reported system call.
pub const TASK_DEBUG_FLAG_SYSCALL_ACTIVE: u32 = 1 << 3;
/// The task is configured for periodic deadline scheduling.
pub const TASK_DEBUG_FLAG_DEADLINE: u32 = 1 << 4;
/// The deadline task has exhausted its current runtime budget.
pub const TASK_DEBUG_FLAG_DEADLINE_THROTTLED: u32 = 1 << 5;
/// At least one task-owned software timer is currently registered.
pub const TASK_DEBUG_FLAG_SOFTWARE_TIMER_ARMED: u32 = 1 << 6;
/// Deadline state could not be sampled without waiting for its lock.
pub const TASK_DEBUG_FLAG_DEADLINE_UNAVAILABLE: u32 = 1 << 7;

/// Raw v1 task execution snapshot returned by `GetTaskDebugInfo`.
///
/// This interface is available only in kernels built with `sync-debug`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RawTaskDebugInfoV1 {
    /// Size of this entry in bytes.
    pub size: u32,
    /// ABI version, currently [`TASK_DEBUG_INFO_VERSION_V1`].
    pub version: u16,
    /// Task-state discriminant.
    pub state: u8,
    /// Task type: 0 = kernel, 1 = user.
    pub task_type: u8,
    /// Combination of `TASK_DEBUG_FLAG_*` values.
    pub flags: u32,
    /// Last scheduler CPU, or `u32::MAX` when unknown.
    pub cpu_id: u32,
    /// Namespace-local thread ID.
    pub pid: usize,
    /// Namespace-local thread-group ID.
    pub tgid: usize,
    /// Most recent timer-sampled instruction address.
    pub observed_pc: u64,
    /// Most recently entered system-call number, or `u64::MAX`.
    pub syscall_number: u64,
    /// User instruction address from which the system call was entered.
    pub syscall_pc: u64,
    /// Cumulative task CPU time in nanoseconds.
    pub cpu_time_ns: u64,
}

const _: [(); 64] = [(); core::mem::size_of::<RawTaskDebugInfoV1>()];

/// Version of the per-CPU debug snapshot ABI implemented by Scarlet.
pub const CPU_DEBUG_INFO_VERSION_V1: u16 = 1;
/// The snapshot contains a namespace-visible current task ID.
pub const CPU_DEBUG_FLAG_CURRENT_TASK_VALID: u16 = 1 << 0;
/// The CPU's published current task is its idle task.
pub const CPU_DEBUG_FLAG_IDLE: u16 = 1 << 1;
/// The CPU has a deferred reschedule request pending.
pub const CPU_DEBUG_FLAG_PENDING_RESCHEDULE: u16 = 1 << 2;
/// The CPU's local hardware timer has a programmed deadline.
pub const CPU_DEBUG_FLAG_TIMER_ARMED: u16 = 1 << 3;

/// Raw v1 lock-free per-CPU snapshot returned by `GetCpuDebugInfo`.
///
/// This interface is available only in kernels built with `sync-debug`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RawCpuDebugInfoV1 {
    /// Size of this entry in bytes.
    pub size: u32,
    /// ABI version, currently [`CPU_DEBUG_INFO_VERSION_V1`].
    pub version: u16,
    /// Combination of `CPU_DEBUG_FLAG_*` values.
    pub flags: u16,
    /// Logical CPU ID represented by this snapshot.
    pub cpu_id: u32,
    /// Low 32 bits of the breadcrumb commit sequence.
    pub reserved: u32,
    /// Namespace-local current task ID, or zero when unavailable.
    pub current_task_id: usize,
    /// Number of local timer interrupts observed by this CPU.
    pub timer_irq_count: u64,
    /// Last lock-free kernel execution breadcrumb phase.
    pub breadcrumb_phase: u64,
    /// First context value associated with `breadcrumb_phase`.
    pub breadcrumb_aux: u64,
    /// Second context value associated with `breadcrumb_phase`.
    pub breadcrumb_aux2: u64,
    /// Last requested local timer deadline, or zero when stopped.
    pub timer_deadline_ns: u64,
}

const _: [(); 64] = [(); core::mem::size_of::<RawCpuDebugInfoV1>()];

/// Scheduler utilization scale used by Scarlet Native util-clamp syscalls.
///
/// A task with `util_min == SCHED_UTIL_SCALE` requires a CPU with full
/// scheduler capacity, which excludes lower-capacity efficiency cores on
/// heterogeneous systems.
pub const SCHED_UTIL_SCALE: u32 = 1024;

/// Highest-priority nice value accepted by Scarlet Native scheduler controls.
pub const SCHED_NICE_MIN: i32 = -20;
/// Lowest-priority nice value accepted by Scarlet Native scheduler controls.
pub const SCHED_NICE_MAX: i32 = 19;

/// Fixed-layout periodic deadline reservation parameters.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RawTaskDeadlineParams {
    /// CPU runtime available during each period, in nanoseconds.
    pub runtime_ns: u64,
    /// Relative completion deadline, in nanoseconds.
    pub deadline_ns: u64,
    /// Reservation period, in nanoseconds.
    pub period_ns: u64,
}

const _: [(); 24] = [(); core::mem::size_of::<RawTaskDeadlineParams>()];

/// Scheduler-control ABI version implemented by the v1 raw structures.
pub const SCHEDULER_CONTROL_VERSION_V1: u32 = 1;

/// Fair, weighted EEVDF scheduler policy.
pub const SCHED_POLICY_FAIR: u32 = 0;
/// Periodic deadline-reservation scheduler policy.
pub const SCHED_POLICY_DEADLINE: u32 = 1;

/// Allow execution on any online CPU.
pub const SCHED_AFFINITY_ANY: u32 = 0;
/// Allow execution only on the CPU identified by `cpu_id`.
pub const SCHED_AFFINITY_SINGLE: u32 = 1;
/// Allow execution on the CPUs selected by a user-provided bit mask.
pub const SCHED_AFFINITY_MASK: u32 = 2;

/// Sentinel used when a scheduler state has no associated CPU.
pub const SCHED_CPU_ID_NONE: u32 = u32::MAX;

/// The only valid scheduler-attribute flag value in v1.
///
/// Callers must set [`RawSchedulerAttrV1::flags`] to this value. No scheduler
/// attribute flag bits are defined in v1; nonzero values are reserved for a
/// future ABI version and must be rejected. In particular, implicit-deadline
/// validation is a kernel policy and is not selected by a user-visible flag.
pub const SCHED_ATTR_FLAGS_NONE: u32 = 0;

/// Stable result code returned by a scheduler-control syscall.
///
/// This type describes the success or failure of a scheduler-control request.
/// It is intentionally distinct from [`RawSchedulerStatus`], which describes
/// the calling task's runtime scheduler state in [`RawSchedulerStateV1`].
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawSchedulerResult {
    /// The request completed successfully.
    Ok = 0,
    /// A user-provided structure or affinity-mask address was invalid.
    BadAddress = 1,
    /// The supplied structure size was invalid for the requested ABI version.
    BadSize = 2,
    /// The requested scheduler-control ABI version is not implemented.
    UnsupportedVersion = 3,
    /// The request contained one or more undefined scheduler attribute flags.
    InvalidFlags = 4,
    /// The request selected an unsupported scheduler policy.
    InvalidPolicy = 5,
    /// The request contained invalid scheduler parameters.
    InvalidArgument = 6,
    /// A requested CPU is not online.
    CpuOffline = 7,
    /// A requested CPU affinity mask selected no online CPUs.
    EmptyCpuMask = 8,
    /// Deadline admission control rejected the requested reservation.
    AdmissionFailed = 9,
    /// The request cannot complete while the current task is busy.
    Busy = 10,
    /// A user-provided output or affinity-mask buffer was too small.
    BufferTooSmall = 11,
}

impl RawSchedulerResult {
    /// Convert a raw ABI result value into a recognized scheduler result.
    ///
    /// # Arguments
    ///
    /// * `value` - Raw `u32` value returned by a scheduler-control syscall.
    ///
    /// # Returns
    ///
    /// The matching result code, or `None` for a value reserved by a newer ABI.
    pub const fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Ok),
            1 => Some(Self::BadAddress),
            2 => Some(Self::BadSize),
            3 => Some(Self::UnsupportedVersion),
            4 => Some(Self::InvalidFlags),
            5 => Some(Self::InvalidPolicy),
            6 => Some(Self::InvalidArgument),
            7 => Some(Self::CpuOffline),
            8 => Some(Self::EmptyCpuMask),
            9 => Some(Self::AdmissionFailed),
            10 => Some(Self::Busy),
            11 => Some(Self::BufferTooSmall),
            _ => None,
        }
    }

    /// Return the stable raw ABI representation of this result.
    ///
    /// # Returns
    ///
    /// The `u32` value returned by a scheduler-control syscall.
    pub const fn as_raw(self) -> u32 {
        self as u32
    }
}

/// Stable runtime status reported by [`RawSchedulerStateV1`].
///
/// The raw state structure stores this value as `u32` so later ABI revisions
/// can add statuses without making an older user library interpret an unknown
/// discriminant as a known one.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawSchedulerStatus {
    /// The kernel did not report a recognized scheduler status.
    Unknown = 0,
    /// The task is executing on [`RawSchedulerStateV1::current_cpu_id`].
    Running = 1,
    /// The task is runnable on [`RawSchedulerStateV1::queued_cpu_id`].
    Queued = 2,
    /// The task is blocked and not runnable.
    Blocked = 3,
    /// The task is suspended after exhausting a deadline runtime budget.
    Throttled = 4,
}

impl RawSchedulerStatus {
    /// Convert a raw ABI status value into a recognized scheduler status.
    ///
    /// # Arguments
    ///
    /// * `value` - Raw `u32` value supplied by the scheduler state ABI.
    ///
    /// # Returns
    ///
    /// The matching status, or `None` for a value reserved by a newer ABI.
    pub const fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Unknown),
            1 => Some(Self::Running),
            2 => Some(Self::Queued),
            3 => Some(Self::Blocked),
            4 => Some(Self::Throttled),
            _ => None,
        }
    }

    /// Return the stable raw ABI representation of this status.
    ///
    /// # Returns
    ///
    /// The `u32` discriminant stored in [`RawSchedulerStateV1::status`].
    pub const fn as_raw(self) -> u32 {
        self as u32
    }
}

/// Raw v1 requested scheduler attributes for the calling task.
///
/// This is a current-task-only ABI: it deliberately contains no process ID,
/// thread ID, handle, or other cross-task selector. Set and get scheduler
/// attribute syscalls exchange this exact fixed layout. All integer fields are
/// fixed-width so the layout is identical on supported 64-bit architectures.
///
/// Fair fallback affinity, nice, and utilization are present for both active
/// policies. Deadline requests add the reservation fields and a separate
/// [`RawSchedulerAttrV1::deadline_cpu_id`], allowing a complete configuration
/// to be atomically reapplied without losing the Fair fallback.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawSchedulerAttrV1 {
    /// Size of this user-provided structure in bytes.
    ///
    /// Set this to [`RAW_SCHEDULER_ATTR_V1_SIZE`] for v1. The kernel uses it to
    /// verify the available layout before reading or writing the structure.
    pub size: u32,
    /// ABI version, which must be [`SCHEDULER_CONTROL_VERSION_V1`] for v1.
    pub version: u32,
    /// Requested policy: [`SCHED_POLICY_FAIR`] or [`SCHED_POLICY_DEADLINE`].
    pub policy: u32,
    /// Requested flags, which must be [`SCHED_ATTR_FLAGS_NONE`] in v1.
    pub flags: u32,
    /// Fair fallback affinity encoding: [`SCHED_AFFINITY_ANY`],
    /// [`SCHED_AFFINITY_SINGLE`], or [`SCHED_AFFINITY_MASK`].
    pub affinity_kind: u32,
    /// Fair fallback CPU ID for [`SCHED_AFFINITY_SINGLE`].
    ///
    /// Set this to [`SCHED_CPU_ID_NONE`] for any-CPU or mask affinity.
    pub cpu_id: u32,
    /// Requested Fair fallback nice value in the inclusive
    /// [`SCHED_NICE_MIN`]..=[`SCHED_NICE_MAX`] range.
    pub nice: i32,
    /// Requested Fair fallback minimum scheduler utilization in
    /// `0..=[SCHED_UTIL_SCALE]` capacity units.
    pub util_min: u32,
    /// User virtual address of the first byte of a Fair fallback affinity mask.
    ///
    /// This is a `u64` ABI address rather than a Rust pointer. It is used only
    /// with [`SCHED_AFFINITY_MASK`] and must be zero otherwise.
    pub cpu_mask_ptr: u64,
    /// Number of bytes readable at [`RawSchedulerAttrV1::cpu_mask_ptr`].
    ///
    /// This must be sufficient for [`RawSchedulerAttrV1::cpu_mask_nbits`] and
    /// zero for any-CPU or single-CPU affinity.
    pub cpu_mask_bytes: u32,
    /// Number of meaningful bits in the Fair fallback affinity mask.
    ///
    /// Bit `n` is the least-significant bit of byte `n / 8` shifted by
    /// `n % 8`, and permits scheduler CPU `n`. This must be zero for any-CPU
    /// or single-CPU affinity.
    pub cpu_mask_nbits: u32,
    /// Active Deadline runtime budget per period, in nanoseconds.
    ///
    /// This is zero when deadline scheduling is disabled.
    pub runtime_ns: u64,
    /// Active Deadline relative deadline, in nanoseconds.
    ///
    /// This is zero when deadline scheduling is disabled. v1 leaves
    /// implicit-deadline enforcement to the kernel rather than a flag bit.
    pub deadline_ns: u64,
    /// Active Deadline reservation period, in nanoseconds.
    ///
    /// This is zero when deadline scheduling is disabled.
    pub period_ns: u64,
    /// Sole CPU used by the active Deadline reservation.
    ///
    /// Set this to [`SCHED_CPU_ID_NONE`] when [`RawSchedulerAttrV1::policy`]
    /// is [`SCHED_POLICY_FAIR`]. The Fair fallback affinity remains encoded by
    /// [`RawSchedulerAttrV1::affinity_kind`] and related mask fields regardless
    /// of the active policy.
    pub deadline_cpu_id: u32,
    /// Reserved fixed-width field following [`RawSchedulerAttrV1::deadline_cpu_id`].
    ///
    /// Callers must initialize this to zero. The kernel must reject a nonzero
    /// input value and return zero for output.
    pub reserved0: u32,
    /// Reserved for future ABI expansion.
    ///
    /// Callers must initialize every element to zero. The kernel must reject a
    /// nonzero input element and must return zero for every output element.
    pub reserved: [u64; 6],
}

/// Wire size of [`RawSchedulerAttrV1`] in bytes.
pub const RAW_SCHEDULER_ATTR_V1_SIZE: u32 = 128;

impl RawSchedulerAttrV1 {
    /// Create zeroed v1 scheduler attributes with the required size and version.
    ///
    /// # Returns
    ///
    /// A fair-policy, any-CPU attribute block with all optional scheduler
    /// controls disabled.
    pub const fn new() -> Self {
        Self {
            size: RAW_SCHEDULER_ATTR_V1_SIZE,
            version: SCHEDULER_CONTROL_VERSION_V1,
            policy: SCHED_POLICY_FAIR,
            flags: SCHED_ATTR_FLAGS_NONE,
            affinity_kind: SCHED_AFFINITY_ANY,
            cpu_id: SCHED_CPU_ID_NONE,
            nice: 0,
            util_min: 0,
            cpu_mask_ptr: 0,
            cpu_mask_bytes: 0,
            cpu_mask_nbits: 0,
            runtime_ns: 0,
            deadline_ns: 0,
            period_ns: 0,
            deadline_cpu_id: SCHED_CPU_ID_NONE,
            reserved0: 0,
            reserved: [0; 6],
        }
    }
}

impl Default for RawSchedulerAttrV1 {
    fn default() -> Self {
        Self::new()
    }
}

const _: [(); 128] = [(); core::mem::size_of::<RawSchedulerAttrV1>()];
const _: [(); 72] = [(); core::mem::offset_of!(RawSchedulerAttrV1, deadline_cpu_id)];
const _: [(); 76] = [(); core::mem::offset_of!(RawSchedulerAttrV1, reserved0)];
const _: [(); 80] = [(); core::mem::offset_of!(RawSchedulerAttrV1, reserved)];

/// Raw v1 runtime scheduler state for the calling task.
///
/// This structure reports scheduler-observed state separately from the
/// requested configuration in [`RawSchedulerAttrV1`]. It contains no
/// cross-task selector and therefore always describes the current task.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawSchedulerStateV1 {
    /// Size of this user-provided structure in bytes.
    ///
    /// Set this to [`RAW_SCHEDULER_STATE_V1_SIZE`] before a state query.
    pub size: u32,
    /// ABI version, which must be [`SCHEDULER_CONTROL_VERSION_V1`] for v1.
    pub version: u32,
    /// Runtime status encoded as a [`RawSchedulerStatus`] discriminant.
    pub status: u32,
    /// Active policy: [`SCHED_POLICY_FAIR`] or [`SCHED_POLICY_DEADLINE`].
    pub policy: u32,
    /// Configured scheduler flags, which are always [`SCHED_ATTR_FLAGS_NONE`]
    /// in v1.
    pub flags: u32,
    /// Active placement encoding.
    ///
    /// This is always single-CPU placement for Deadline. Query
    /// [`RawSchedulerAttrV1`] for the retained Fair fallback affinity.
    pub affinity_kind: u32,
    /// Active single CPU, or [`SCHED_CPU_ID_NONE`] for Fair any-CPU or mask
    /// placement. Query [`RawSchedulerAttrV1`] for the complete Fair fallback.
    pub configured_cpu_id: u32,
    /// CPU currently executing the task, or [`SCHED_CPU_ID_NONE`] when it is
    /// not executing.
    pub current_cpu_id: u32,
    /// CPU whose ready queue owns the task, or [`SCHED_CPU_ID_NONE`] when the
    /// task is not queued.
    pub queued_cpu_id: u32,
    /// Configured Fair fallback nice value.
    pub nice: i32,
    /// Configured Fair fallback minimum utilization in capacity units.
    pub util_min: u32,
    /// Reserved for future fixed-width state fields.
    ///
    /// The kernel must return this as zero; callers must ignore it.
    pub reserved0: u32,
    /// Current EEVDF virtual runtime, in scheduler virtual-time units.
    pub fair_vruntime_ns: u64,
    /// Current EEVDF virtual deadline, in scheduler virtual-time units.
    pub fair_vdeadline_ns: u64,
    /// Wall-clock fair slice remaining before the task should be reconsidered,
    /// in nanoseconds.
    pub fair_slice_remaining_ns: u64,
    /// Runtime budget remaining in the active deadline period, in nanoseconds.
    pub deadline_runtime_remaining_ns: u64,
    /// Active deadline's absolute monotonic timestamp, in nanoseconds.
    pub deadline_absolute_ns: u64,
    /// Monotonic timestamp of the next deadline budget replenishment, in
    /// nanoseconds.
    pub deadline_replenishment_ns: u64,
    /// Deadline bandwidth admission currently reserved for this task.
    ///
    /// This is a scheduler-defined fixed-point capacity value, not a duration
    /// or a count. It is zero when deadline scheduling is disabled.
    pub deadline_admission_units: u32,
    /// Reserved for future deadline-state flags or metrics.
    ///
    /// The kernel must return this as zero; callers must ignore it.
    pub reserved1: u32,
    /// Number of deadline periods observed after their absolute deadline.
    pub deadline_miss_count: u64,
    /// Number of deadline runtime-budget overruns observed for this task.
    pub deadline_overrun_count: u64,
    /// Reserved for future ABI expansion.
    ///
    /// The kernel must return every element as zero; callers must ignore all
    /// elements until a later ABI version assigns them meaning.
    pub reserved: [u64; 5],
}

/// Wire size of [`RawSchedulerStateV1`] in bytes.
pub const RAW_SCHEDULER_STATE_V1_SIZE: u32 = 160;

impl RawSchedulerStateV1 {
    /// Create a v1 state-query buffer with the required size and version.
    ///
    /// # Returns
    ///
    /// A zeroed state buffer whose header is ready to pass to the kernel.
    pub const fn new() -> Self {
        Self {
            size: RAW_SCHEDULER_STATE_V1_SIZE,
            version: SCHEDULER_CONTROL_VERSION_V1,
            status: RawSchedulerStatus::Unknown.as_raw(),
            policy: SCHED_POLICY_FAIR,
            flags: SCHED_ATTR_FLAGS_NONE,
            affinity_kind: SCHED_AFFINITY_ANY,
            configured_cpu_id: SCHED_CPU_ID_NONE,
            current_cpu_id: SCHED_CPU_ID_NONE,
            queued_cpu_id: SCHED_CPU_ID_NONE,
            nice: 0,
            util_min: 0,
            reserved0: 0,
            fair_vruntime_ns: 0,
            fair_vdeadline_ns: 0,
            fair_slice_remaining_ns: 0,
            deadline_runtime_remaining_ns: 0,
            deadline_absolute_ns: 0,
            deadline_replenishment_ns: 0,
            deadline_admission_units: 0,
            reserved1: 0,
            deadline_miss_count: 0,
            deadline_overrun_count: 0,
            reserved: [0; 5],
        }
    }

    /// Decode the stable scheduler status reported by this state snapshot.
    ///
    /// # Returns
    ///
    /// The recognized status, or `None` if a newer kernel returned an unknown
    /// raw value.
    pub const fn scheduler_status(&self) -> Option<RawSchedulerStatus> {
        RawSchedulerStatus::from_raw(self.status)
    }
}

impl Default for RawSchedulerStateV1 {
    fn default() -> Self {
        Self::new()
    }
}

const _: [(); 160] = [(); core::mem::size_of::<RawSchedulerStateV1>()];

/// Raw regular file type value used in [`RawFileMetadata::file_type`].
pub const FILE_TYPE_REGULAR: u32 = 0;
/// Raw directory file type value used in [`RawFileMetadata::file_type`].
pub const FILE_TYPE_DIRECTORY: u32 = 1;
/// Raw symbolic link file type value used in [`RawFileMetadata::file_type`].
pub const FILE_TYPE_SYMLINK: u32 = 2;
/// Raw character device file type value used in [`RawFileMetadata::file_type`].
pub const FILE_TYPE_CHAR_DEVICE: u32 = 3;
/// Raw block device file type value used in [`RawFileMetadata::file_type`].
pub const FILE_TYPE_BLOCK_DEVICE: u32 = 4;
/// Raw pipe file type value used in [`RawFileMetadata::file_type`].
pub const FILE_TYPE_PIPE: u32 = 5;
/// Raw socket file type value used in [`RawFileMetadata::file_type`].
pub const FILE_TYPE_SOCKET: u32 = 6;
/// Raw unknown file type value used in [`RawFileMetadata::file_type`].
pub const FILE_TYPE_UNKNOWN: u32 = 7;

/// Raw read permission bit used in [`RawFileMetadata::permissions`].
pub const FILE_PERMISSION_READ: u32 = 1 << 0;
/// Raw write permission bit used in [`RawFileMetadata::permissions`].
pub const FILE_PERMISSION_WRITE: u32 = 1 << 1;
/// Raw execute permission bit used in [`RawFileMetadata::permissions`].
pub const FILE_PERMISSION_EXECUTE: u32 = 1 << 2;

/// Scarlet-private control opcode for setting non-blocking mode.
pub const SCTL_SOCKET_SET_NONBLOCK: u32 = 0x5353_0007;
/// Scarlet-private socket control opcode for querying non-blocking mode.
pub const SCTL_SOCKET_GET_NONBLOCK: u32 = 0x5353_000B;
/// Scarlet-private socket control opcode for setting read timeout in milliseconds.
pub const SCTL_SOCKET_SET_READ_TIMEOUT_MS: u32 = 0x5353_000C;
/// Scarlet-private socket control opcode for setting write timeout in milliseconds.
pub const SCTL_SOCKET_SET_WRITE_TIMEOUT_MS: u32 = 0x5353_000D;
/// Scarlet-private socket control opcode for querying read timeout in milliseconds.
pub const SCTL_SOCKET_GET_READ_TIMEOUT_MS: u32 = 0x5353_000E;
/// Scarlet-private socket control opcode for querying write timeout in milliseconds.
pub const SCTL_SOCKET_GET_WRITE_TIMEOUT_MS: u32 = 0x5353_000F;

/// Fixed-layout file metadata returned by Scarlet Native metadata syscalls.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RawFileMetadata {
    /// File size in bytes.
    pub size: u64,
    /// File type encoded as one of the `FILE_TYPE_*` constants.
    pub file_type: u32,
    /// Permission bits encoded as `FILE_PERMISSION_*` flags.
    pub permissions: u32,
    /// Creation timestamp in seconds.
    pub created: u64,
    /// Last modification timestamp in seconds.
    pub modified: u64,
    /// Last access timestamp in seconds.
    pub accessed: u64,
    /// Filesystem-local stable file identifier.
    pub file_id: u64,
    /// Number of hard links to this file.
    pub link_count: u32,
    /// Reserved for future ABI expansion.
    pub _reserved: u32,
}

/// Scarlet Native syscall numbers.
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syscall {
    Invalid = 0,
    Exit = 1,
    Clone = 2,
    Execve = 3,
    ExecveABI = 4,
    Waitpid = 5,
    Kill = 6,
    Getpid = 7,
    Getppid = 8,
    Brk = 12,
    Sbrk = 13,

    // Basic I/O
    Putchar = 16,
    Getchar = 17,

    Sleep = 20,
    Yield = 21,
    GetRandom = 22,
    ExitGroup = 23,
    MonotonicTime = 35,
    GetCpuUsageInfo = 36,
    SystemTime = 37,
    SetTaskUtilMin = 38,
    GetTaskUtilMin = 39,
    SetTaskNice = 40,
    GetTaskNice = 41,
    SetTaskCpuAffinity = 42,
    GetTaskCpuAffinity = 43,
    SetTaskDeadline = 44,
    GetTaskDeadline = 45,
    SetSchedulerAttr = 46,
    GetSchedulerAttr = 47,
    GetSchedulerState = 48,
    FutexWait = 49,
    FutexWake = 50,

    // Process information
    GetTaskInfoCount = 24,
    GetTaskInfoList = 25,
    CreateSession = 26,
    GetSessionId = 27,
    GetProcessGroupId = 28,
    SetProcessGroup = 29,

    // TLS management
    SetTls = 30,
    GetTls = 31,
    SetTidAddress = 32,
    ThreadDetach = 33,
    ThreadExitCleanup = 34,

    // ABI zone management
    RegisterAbiZone = 90,
    UnregisterAbiZone = 91,

    // Namespace management
    CreateNamespace = 92,

    // Handle management
    HandleQuery = 100,
    HandleSetRole = 101,
    HandleClose = 102,
    HandleDuplicate = 103,
    HandleControl = 110,

    // Core capabilities
    StreamRead = 200,
    StreamWrite = 201,
    Poll = 202,

    // FileObject capability
    FileSeek = 300,
    FileTruncate = 301,
    FileMetadata = 302,

    // VFS operations
    VfsOpen = 400,
    VfsRemove = 401,
    VfsCreateFile = 402,
    VfsCreateDirectory = 403,
    VfsChangeDirectory = 404,
    VfsTruncate = 405,
    VfsCreateSymlink = 406,
    VfsReadlink = 407,
    VfsGetCwdPath = 408,
    VfsRename = 409,
    VfsMetadata = 410,
    VfsCreateHardlink = 411,

    // Filesystem operations
    FsMount = 500,
    FsUmount = 501,
    FsPivotRoot = 502,

    // IPC operations
    Pipe = 600,
    EventSendDirect = 615,
    EventSendGroup = 616,

    // Shared memory
    SharedMemoryCreate = 620,
    SharedMemoryResize = 621,

    // Socket handle transfer
    SocketSendHandle = 630,
    SocketRecvHandle = 631,
    SocketSendHandleAndData = 632,
    SocketRecvHandleAndData = 633,

    // Scarlet Native event handling
    EventHandlerRegister = 640,
    EventHandlerUnregister = 641,
    EventMask = 642,
    EventReturn = 643,
    /// Register a native event handler with an executable event-return restorer.
    EventHandlerRegisterWithRestorer = 644,

    // Memory mapping operations
    MemoryMap = 700,
    MemoryUnmap = 701,

    // Socket operations
    SocketCreate = 900,
    SocketBind = 901,
    SocketListen = 902,
    SocketConnect = 903,
    SocketAccept = 904,
    Socketpair = 905,
    SocketShutdown = 906,

    // Datagram operations
    SocketRecvFrom = 907,
    SocketSendTo = 908,
    SocketBindInterface = 909,

    // Network configuration
    NetworkSetIpv4 = 910,
    NetworkSetGateway = 911,
    NetworkSetNetmask = 913,
    NetworkListInterfaces = 914,
    NetworkConfigureIpv4 = 915,
    NetworkListInterfacesV2 = 916,
    NetworkClearIpv4 = 917,

    // Debug/profiler operations
    GetCpuDebugInfo = 997,
    GetTaskDebugInfo = 998,
    ProfilerDump = 999,

    // System control operations
    Shutdown = 1000,

    // Hypervisor operations
    ShvVmCreate = 1100,
    ShvVcpuCreate = 1101,
    ShvVcpuRun = 1102,

    // Loadable module operations
    LsmLoad = 1200,
    LsmUnload = 1201,
    LsmList = 1202,
}
