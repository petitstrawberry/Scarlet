//! Safe current-task scheduler control APIs.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;
use scarlet_sys::{
    RawSchedulerAttrV1, RawSchedulerResult, RawSchedulerStateV1, RawSchedulerStatus,
    SCHED_AFFINITY_ANY, SCHED_AFFINITY_MASK, SCHED_AFFINITY_SINGLE, SCHED_ATTR_FLAGS_NONE,
    SCHED_CPU_ID_NONE, SCHED_NICE_MAX, SCHED_NICE_MIN, SCHED_POLICY_DEADLINE, SCHED_POLICY_FAIR,
    SCHED_UTIL_SCALE, Syscall, syscall1,
};

/// System-wide scheduler CPU accounting snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuUsageInfo {
    online_cpus: usize,
    busy_time_ns: u64,
    idle_time_ns: u64,
    total_time_ns: u64,
    usage_per_mille: u32,
}

impl CpuUsageInfo {
    /// Return the number of CPUs known to the scheduler.
    ///
    /// # Returns
    ///
    /// The number of online CPUs represented by this snapshot.
    pub const fn online_cpus(self) -> usize {
        self.online_cpus
    }

    /// Return cumulative non-idle CPU time.
    ///
    /// # Returns
    ///
    /// Busy scheduler time in nanoseconds.
    pub const fn busy_time_ns(self) -> u64 {
        self.busy_time_ns
    }

    /// Return cumulative idle CPU time.
    ///
    /// # Returns
    ///
    /// Idle scheduler time in nanoseconds.
    pub const fn idle_time_ns(self) -> u64 {
        self.idle_time_ns
    }

    /// Return total accounted CPU capacity time.
    ///
    /// # Returns
    ///
    /// Total scheduler capacity time in nanoseconds.
    pub const fn total_time_ns(self) -> u64 {
        self.total_time_ns
    }

    /// Return the kernel's instantaneous utilization estimate.
    ///
    /// # Returns
    ///
    /// Utilization in per-mille, where `1000` represents 100 percent.
    pub const fn usage_per_mille(self) -> u32 {
        self.usage_per_mille
    }
}

#[repr(C)]
struct RawCpuUsageInfo {
    online_cpus: usize,
    busy_time_ns: u64,
    idle_time_ns: u64,
    total_time_ns: u64,
    usage_per_mille: u32,
    reserved: u32,
}

/// Query system-wide CPU accounting from the scheduler.
///
/// # Returns
///
/// A coherent scheduler snapshot, or `None` when the kernel rejects the
/// query.
pub fn cpu_usage() -> Option<CpuUsageInfo> {
    let mut raw = RawCpuUsageInfo {
        online_cpus: 0,
        busy_time_ns: 0,
        idle_time_ns: 0,
        total_time_ns: 0,
        usage_per_mille: 0,
        reserved: 0,
    };
    let result = syscall1(
        Syscall::GetCpuUsageInfo,
        &mut raw as *mut RawCpuUsageInfo as usize,
    );
    (result != usize::MAX).then_some(CpuUsageInfo {
        online_cpus: raw.online_cpus,
        busy_time_ns: raw.busy_time_ns,
        idle_time_ns: raw.idle_time_ns,
        total_time_ns: raw.total_time_ns,
        usage_per_mille: raw.usage_per_mille,
    })
}

/// Scheduler policy selected for the calling task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerPolicy {
    /// Weighted EEVDF fair scheduling.
    Fair,
    /// Periodic deadline-reservation scheduling.
    Deadline,
}

/// Runtime execution status reported by the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerStatus {
    /// The kernel did not report a more specific status.
    Unknown,
    /// The task is executing on a CPU.
    Running,
    /// The task is runnable on a CPU queue.
    Queued,
    /// The task is blocked and not runnable.
    Blocked,
    /// The task exhausted its deadline runtime budget.
    Throttled,
}

/// Stable scheduler-control result returned by the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerResult {
    /// The request completed successfully.
    Ok,
    /// A supplied user address was invalid.
    BadAddress,
    /// A supplied ABI structure size was invalid.
    BadSize,
    /// The requested ABI version is unsupported.
    UnsupportedVersion,
    /// The request contained unsupported flags.
    InvalidFlags,
    /// The request selected an unsupported policy.
    InvalidPolicy,
    /// The request contained invalid arguments.
    InvalidArgument,
    /// A requested CPU is offline.
    CpuOffline,
    /// The requested CPU mask selected no online CPUs.
    EmptyCpuMask,
    /// Deadline admission control rejected the reservation.
    AdmissionFailed,
    /// The current task was too busy to complete the request.
    Busy,
    /// A supplied output buffer was too small.
    BufferTooSmall,
}

/// Safe scheduler-control failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerError {
    /// A fair nice value was outside the supported range.
    InvalidNice,
    /// A fair utilization clamp exceeded the scheduler scale.
    InvalidUtilization,
    /// A duration was zero or could not fit in ABI nanoseconds.
    InvalidDuration,
    /// A deadline runtime budget exceeded its period.
    RuntimeExceedsPeriod,
    /// A CPU mask contained no selected CPUs.
    EmptyCpuMask,
    /// A CPU mask had non-canonical length or unused bits.
    NonCanonicalCpuMask,
    /// A mask-size probe returned zero, inconsistent, or unreasonable bounds.
    InvalidMaskProbe,
    /// Deadline v1 requires a concrete single CPU.
    InvalidDeadlineCpu,
    /// A recognized kernel scheduler-control result was unsuccessful.
    Kernel(SchedulerResult),
    /// The kernel returned a result code reserved by a newer ABI.
    UnknownResult(usize),
    /// A returned ABI header did not identify scheduler-control v1.
    InvalidResponseHeader,
    /// A returned ABI structure used flags unsupported by this API.
    InvalidResponseFlags(u32),
    /// A returned ABI structure contained nonzero reserved fields.
    InvalidResponseReserved,
    /// A returned policy value is unknown to this API.
    UnknownPolicy(u32),
    /// A returned affinity value is unknown to this API.
    UnknownAffinity(u32),
    /// A returned runtime status is unknown to this API.
    UnknownStatus(u32),
    /// A returned policy used affinity fields invalid for that policy.
    InvalidAffinityForPolicy,
    /// A returned policy used scheduling fields invalid for that policy.
    InvalidFieldsForPolicy,
}

/// A validated borrowed CPU affinity bit mask.
///
/// Bit `n` permits CPU `n`; bits above `nbits` are always zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuMask<'a> {
    bytes: &'a [u8],
    nbits: u32,
}

impl<'a> CpuMask<'a> {
    /// Validate a borrowed canonical CPU mask.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Mask storage in little-endian bit order per byte.
    /// * `nbits` - Number of meaningful CPU bits in `bytes`.
    ///
    /// # Returns
    ///
    /// A borrowed mask, or a local validation error.
    pub fn new(bytes: &'a [u8], nbits: u32) -> Result<Self, SchedulerError> {
        let required_bytes = nbits.div_ceil(8) as usize;
        if nbits == 0 || bytes.len() != required_bytes {
            return Err(SchedulerError::NonCanonicalCpuMask);
        }
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(SchedulerError::EmptyCpuMask);
        }
        let trailing_bits = nbits % 8;
        if trailing_bits != 0 {
            let allowed = (1u8 << trailing_bits) - 1;
            if bytes[required_bytes - 1] & !allowed != 0 {
                return Err(SchedulerError::NonCanonicalCpuMask);
            }
        }
        Ok(Self { bytes, nbits })
    }

    /// Return the canonical mask bytes.
    ///
    /// # Returns
    ///
    /// The borrowed mask storage.
    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    /// Return the number of meaningful CPU bits.
    ///
    /// # Returns
    ///
    /// The mask width in bits.
    pub const fn nbits(self) -> u32 {
        self.nbits
    }
}

/// An owning validated CPU affinity bit mask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedCpuMask {
    bytes: Vec<u8>,
    nbits: u32,
}

impl OwnedCpuMask {
    /// Validate and own a canonical CPU mask.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Owned mask storage in little-endian bit order per byte.
    /// * `nbits` - Number of meaningful CPU bits in `bytes`.
    ///
    /// # Returns
    ///
    /// An owning mask, or a local validation error.
    pub fn new(bytes: Vec<u8>, nbits: u32) -> Result<Self, SchedulerError> {
        CpuMask::new(&bytes, nbits)?;
        Ok(Self { bytes, nbits })
    }

    /// Borrow this mask for a scheduler request.
    ///
    /// # Returns
    ///
    /// A validated borrowed mask that remains valid while `self` is borrowed.
    pub fn as_borrowed(&self) -> CpuMask<'_> {
        CpuMask {
            bytes: &self.bytes,
            nbits: self.nbits,
        }
    }

    /// Return the canonical mask bytes.
    ///
    /// # Returns
    ///
    /// The owned mask storage as a slice.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Return the number of meaningful CPU bits.
    ///
    /// # Returns
    ///
    /// The mask width in bits.
    pub const fn nbits(&self) -> u32 {
        self.nbits
    }
}

/// Fair-policy CPU placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FairAffinity<'a> {
    /// Permit any online CPU.
    Any,
    /// Permit one specific CPU.
    Single(u32),
    /// Permit the CPUs selected by a canonical bit mask.
    Mask(CpuMask<'a>),
}

/// Owning fair-policy CPU placement returned by a configuration query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedFairAffinity {
    /// Permit any online CPU.
    Any,
    /// Permit one specific CPU.
    Single(u32),
    /// Permit the CPUs selected by an owned canonical bit mask.
    Mask(OwnedCpuMask),
}

/// Validated Fair-policy configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FairConfig<'a> {
    nice: i32,
    util_min: u32,
    affinity: FairAffinity<'a>,
}

impl<'a> FairConfig<'a> {
    /// Create a validated Fair-policy configuration.
    ///
    /// # Arguments
    ///
    /// * `nice` - Fair priority from -20 through 19.
    /// * `util_min` - Minimum utilization in `0..=1024` capacity units.
    /// * `affinity` - Any, single-CPU, or mask-based fair placement.
    ///
    /// # Returns
    ///
    /// A validated Fair configuration, or a local validation error.
    pub fn new(
        nice: i32,
        util_min: u32,
        affinity: FairAffinity<'a>,
    ) -> Result<Self, SchedulerError> {
        validate_fair_values(nice, util_min)?;
        Ok(Self {
            nice,
            util_min,
            affinity,
        })
    }

    /// Return the configured fair nice value.
    ///
    /// # Returns
    ///
    /// The fair nice value.
    pub const fn nice(&self) -> i32 {
        self.nice
    }

    /// Return the configured minimum utilization clamp.
    ///
    /// # Returns
    ///
    /// The clamp in scheduler capacity units.
    pub const fn util_min(&self) -> u32 {
        self.util_min
    }

    /// Return the configured fair CPU placement.
    ///
    /// # Returns
    ///
    /// The Fair affinity configuration.
    pub const fn affinity(&self) -> FairAffinity<'a> {
        self.affinity
    }
}

/// Validated v1 deadline-reservation configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadlineConfig {
    runtime: Duration,
    period: Duration,
    cpu_id: u32,
    runtime_ns: u64,
    period_ns: u64,
}

impl DeadlineConfig {
    /// Create a validated implicit-deadline reservation.
    ///
    /// v1 always emits `deadline = period` and permits only a single CPU.
    ///
    /// # Arguments
    ///
    /// * `runtime` - Runtime budget available during each period.
    /// * `period` - Reservation period and v1 relative deadline.
    /// * `cpu_id` - The sole CPU permitted to run the reservation.
    ///
    /// # Returns
    ///
    /// A validated deadline configuration, or a local validation error.
    pub fn new(runtime: Duration, period: Duration, cpu_id: u32) -> Result<Self, SchedulerError> {
        let runtime_ns = duration_to_ns(runtime)?;
        let period_ns = duration_to_ns(period)?;
        if runtime_ns > period_ns {
            return Err(SchedulerError::RuntimeExceedsPeriod);
        }
        if cpu_id == SCHED_CPU_ID_NONE {
            return Err(SchedulerError::InvalidDeadlineCpu);
        }
        Ok(Self {
            runtime,
            period,
            cpu_id,
            runtime_ns,
            period_ns,
        })
    }

    /// Return the runtime budget.
    ///
    /// # Returns
    ///
    /// Runtime available during each period.
    pub const fn runtime(&self) -> Duration {
        self.runtime
    }

    /// Return the reservation period.
    ///
    /// # Returns
    ///
    /// The period, also used as the v1 relative deadline.
    pub const fn period(&self) -> Duration {
        self.period
    }

    /// Return the sole CPU permitted for the reservation.
    ///
    /// # Returns
    ///
    /// The configured CPU ID.
    pub const fn cpu_id(&self) -> u32 {
        self.cpu_id
    }
}

/// A complete scheduler configuration suitable for one atomic apply request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerConfig<'a> {
    /// Activate Fair scheduling with all Fair-owned controls.
    Fair(FairConfig<'a>),
    /// Activate Deadline scheduling with its required Fair fallback.
    Deadline {
        /// The active single-CPU Deadline reservation.
        deadline: DeadlineConfig,
        /// The retained Fair configuration restored when Deadline is cleared.
        fair: FairConfig<'a>,
    },
}

/// The configured scheduler attributes for the calling task.
///
/// Fair fallback settings are always configured, while Deadline adds an active
/// single-CPU reservation. This mirrors the complete v1 ABI request and lets
/// callers update or restore Fair scheduling atomically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredScheduler {
    policy: SchedulerPolicy,
    fair_nice: i32,
    fair_util_min: u32,
    fair_affinity: OwnedFairAffinity,
    deadline: Option<DeadlineConfig>,
}

impl ConfiguredScheduler {
    /// Return the currently active scheduler policy.
    ///
    /// # Returns
    ///
    /// The configured Fair or Deadline policy.
    pub const fn policy(&self) -> SchedulerPolicy {
        self.policy
    }

    /// Return the retained Fair nice value.
    ///
    /// # Returns
    ///
    /// The Fair nice value, including while Deadline is active.
    pub const fn fair_nice(&self) -> i32 {
        self.fair_nice
    }

    /// Return the retained Fair utilization clamp.
    ///
    /// # Returns
    ///
    /// The Fair clamp, including while Deadline is active.
    pub const fn fair_util_min(&self) -> u32 {
        self.fair_util_min
    }

    /// Return the retained Fair placement.
    ///
    /// # Returns
    ///
    /// Any, single-CPU, or owned mask placement.
    pub fn fair_affinity(&self) -> &OwnedFairAffinity {
        &self.fair_affinity
    }

    /// Return the active deadline reservation, if any.
    ///
    /// # Returns
    ///
    /// The deadline configuration when the active policy is Deadline.
    pub const fn deadline(&self) -> Option<DeadlineConfig> {
        self.deadline
    }

    /// Change the retained Fair nice value.
    ///
    /// # Arguments
    ///
    /// * `nice` - Fair priority from -20 through 19.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the value is locally valid.
    pub fn set_fair_nice(&mut self, nice: i32) -> Result<(), SchedulerError> {
        validate_fair_values(nice, self.fair_util_min)?;
        self.fair_nice = nice;
        Ok(())
    }

    /// Change the retained Fair utilization clamp.
    ///
    /// # Arguments
    ///
    /// * `util_min` - Minimum utilization in `0..=1024` capacity units.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the value is locally valid.
    pub fn set_fair_util_min(&mut self, util_min: u32) -> Result<(), SchedulerError> {
        validate_fair_values(self.fair_nice, util_min)?;
        self.fair_util_min = util_min;
        Ok(())
    }

    /// Change the retained Fair CPU placement.
    ///
    /// # Arguments
    ///
    /// * `affinity` - Owned any, single-CPU, or mask placement.
    ///
    /// # Returns
    ///
    /// Nothing; the supplied placement is retained for the next apply.
    pub fn set_fair_affinity(&mut self, affinity: OwnedFairAffinity) {
        self.fair_affinity = affinity;
    }

    /// Activate the retained Fair configuration.
    ///
    /// # Returns
    ///
    /// Nothing; call [`Self::apply`] to atomically submit the change.
    pub fn activate_fair(&mut self) {
        self.policy = SchedulerPolicy::Fair;
        self.deadline = None;
    }

    /// Activate a validated Deadline configuration while retaining Fair values.
    ///
    /// # Arguments
    ///
    /// * `deadline` - The single-CPU v1 reservation to activate.
    ///
    /// # Returns
    ///
    /// Nothing; call [`Self::apply`] to atomically submit the change.
    pub fn activate_deadline(&mut self, deadline: DeadlineConfig) {
        self.policy = SchedulerPolicy::Deadline;
        self.deadline = Some(deadline);
    }

    /// Atomically apply this complete configured state to the current task.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the kernel accepts the configuration.
    pub fn apply(&self) -> Result<(), SchedulerError> {
        apply_raw(encode_configured(self)?)
    }
}

/// Runtime scheduler state observed for the calling task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeSchedulerState {
    status: SchedulerStatus,
    policy: SchedulerPolicy,
    current_cpu_id: Option<u32>,
    queued_cpu_id: Option<u32>,
    fair_vruntime: Duration,
    fair_vdeadline: Duration,
    fair_slice_remaining: Duration,
    deadline_runtime_remaining: Duration,
    deadline_absolute: Duration,
    deadline_replenishment: Duration,
    deadline_admission_units: u32,
    deadline_miss_count: u64,
    deadline_overrun_count: u64,
}

impl RuntimeSchedulerState {
    /// Return the scheduler-observed task status.
    ///
    /// # Returns
    ///
    /// The current execution status.
    pub const fn status(&self) -> SchedulerStatus {
        self.status
    }

    /// Return the active scheduler policy.
    ///
    /// # Returns
    ///
    /// The policy observed by the runtime scheduler.
    pub const fn policy(&self) -> SchedulerPolicy {
        self.policy
    }

    /// Return the CPU currently executing the task.
    ///
    /// # Returns
    ///
    /// The CPU ID, or `None` when the task is not executing.
    pub const fn current_cpu_id(&self) -> Option<u32> {
        self.current_cpu_id
    }

    /// Return the CPU queue currently owning the task.
    ///
    /// # Returns
    ///
    /// The CPU ID, or `None` when the task is not queued.
    pub const fn queued_cpu_id(&self) -> Option<u32> {
        self.queued_cpu_id
    }

    /// Return the current Fair virtual runtime.
    ///
    /// # Returns
    ///
    /// Fair virtual runtime expressed as a duration.
    pub const fn fair_vruntime(&self) -> Duration {
        self.fair_vruntime
    }

    /// Return the current Fair virtual deadline.
    ///
    /// # Returns
    ///
    /// Fair virtual deadline expressed as a duration.
    pub const fn fair_vdeadline(&self) -> Duration {
        self.fair_vdeadline
    }

    /// Return the remaining Fair scheduler slice.
    ///
    /// # Returns
    ///
    /// Remaining Fair slice duration.
    pub const fn fair_slice_remaining(&self) -> Duration {
        self.fair_slice_remaining
    }

    /// Return the remaining Deadline runtime budget.
    ///
    /// # Returns
    ///
    /// Remaining Deadline runtime duration.
    pub const fn deadline_runtime_remaining(&self) -> Duration {
        self.deadline_runtime_remaining
    }

    /// Return the active absolute Deadline timestamp.
    ///
    /// # Returns
    ///
    /// The timestamp as a duration from the scheduler epoch.
    pub const fn deadline_absolute(&self) -> Duration {
        self.deadline_absolute
    }

    /// Return the next Deadline replenishment timestamp.
    ///
    /// # Returns
    ///
    /// The timestamp as a duration from the scheduler epoch.
    pub const fn deadline_replenishment(&self) -> Duration {
        self.deadline_replenishment
    }

    /// Return the currently admitted Deadline bandwidth units.
    ///
    /// # Returns
    ///
    /// The scheduler-defined admission value.
    pub const fn deadline_admission_units(&self) -> u32 {
        self.deadline_admission_units
    }

    /// Return the observed Deadline miss count.
    ///
    /// # Returns
    ///
    /// The cumulative count of missed Deadline periods.
    pub const fn deadline_miss_count(&self) -> u64 {
        self.deadline_miss_count
    }

    /// Return the observed Deadline overrun count.
    ///
    /// # Returns
    ///
    /// The cumulative count of exhausted runtime budgets.
    pub const fn deadline_overrun_count(&self) -> u64 {
        self.deadline_overrun_count
    }
}

/// Atomically apply a scheduler configuration to the current task.
///
/// # Arguments
///
/// * `config` - A validated Fair configuration or Deadline with Fair fallback.
///
/// # Returns
///
/// `Ok(())` when the kernel accepts the configuration.
pub fn apply(config: SchedulerConfig<'_>) -> Result<(), SchedulerError> {
    let raw = match config {
        SchedulerConfig::Fair(fair) => encode_fair(fair),
        SchedulerConfig::Deadline { deadline, fair } => encode_deadline(deadline, fair),
    };
    apply_raw(raw)
}

/// Query the current task's configured scheduler attributes.
///
/// # Returns
///
/// A validated owning configuration snapshot, or a scheduler-control error.
pub fn configured() -> Result<ConfiguredScheduler, SchedulerError> {
    let mut raw = RawSchedulerAttrV1::new();
    match syscall_result(syscall1(Syscall::GetSchedulerAttr, raw_ptr(&mut raw)))? {
        SchedulerResult::Ok => decode_configured(raw, None),
        SchedulerResult::BufferTooSmall => {
            let mask_nbits = raw.cpu_mask_nbits;
            let bytes = validate_mask_probe(raw.cpu_mask_bytes, mask_nbits)?;
            let mut mask = vec![0; bytes];
            raw = RawSchedulerAttrV1::new();
            raw.cpu_mask_ptr = mask.as_mut_ptr() as u64;
            raw.cpu_mask_bytes = bytes as u32;
            raw.cpu_mask_nbits = mask_nbits;
            match syscall_result(syscall1(Syscall::GetSchedulerAttr, raw_ptr(&mut raw)))? {
                SchedulerResult::Ok => decode_configured(raw, Some(mask)),
                result => Err(SchedulerError::Kernel(result)),
            }
        }
        result => Err(SchedulerError::Kernel(result)),
    }
}

/// Query the current task's runtime scheduler state.
///
/// # Returns
///
/// A runtime-only scheduler snapshot, or a scheduler-control error.
pub fn runtime_state() -> Result<RuntimeSchedulerState, SchedulerError> {
    let mut raw = RawSchedulerStateV1::new();
    match syscall_result(syscall1(Syscall::GetSchedulerState, raw_ptr(&mut raw)))? {
        SchedulerResult::Ok => decode_runtime_state(raw),
        result => Err(SchedulerError::Kernel(result)),
    }
}

fn validate_fair_values(nice: i32, util_min: u32) -> Result<(), SchedulerError> {
    if !(SCHED_NICE_MIN..=SCHED_NICE_MAX).contains(&nice) {
        return Err(SchedulerError::InvalidNice);
    }
    if util_min > SCHED_UTIL_SCALE {
        return Err(SchedulerError::InvalidUtilization);
    }
    Ok(())
}

const MAX_CPU_MASK_BYTES: usize = 1024 * 1024;

fn validate_mask_probe(bytes: u32, nbits: u32) -> Result<usize, SchedulerError> {
    let bytes = bytes as usize;
    if nbits == 0 || bytes == 0 || bytes > MAX_CPU_MASK_BYTES {
        return Err(SchedulerError::InvalidMaskProbe);
    }
    if nbits.div_ceil(8) as usize != bytes {
        return Err(SchedulerError::InvalidMaskProbe);
    }
    Ok(bytes)
}

fn duration_to_ns(duration: Duration) -> Result<u64, SchedulerError> {
    let nanoseconds = duration.as_nanos();
    if nanoseconds == 0 || nanoseconds > u64::MAX as u128 {
        return Err(SchedulerError::InvalidDuration);
    }
    Ok(nanoseconds as u64)
}

fn raw_ptr<T>(value: &mut T) -> usize {
    value as *mut T as usize
}

fn syscall_result(value: usize) -> Result<SchedulerResult, SchedulerError> {
    let Some(value) = u32::try_from(value).ok() else {
        return Err(SchedulerError::UnknownResult(value));
    };
    let Some(result) = RawSchedulerResult::from_raw(value) else {
        return Err(SchedulerError::UnknownResult(value as usize));
    };
    Ok(match result {
        RawSchedulerResult::Ok => SchedulerResult::Ok,
        RawSchedulerResult::BadAddress => SchedulerResult::BadAddress,
        RawSchedulerResult::BadSize => SchedulerResult::BadSize,
        RawSchedulerResult::UnsupportedVersion => SchedulerResult::UnsupportedVersion,
        RawSchedulerResult::InvalidFlags => SchedulerResult::InvalidFlags,
        RawSchedulerResult::InvalidPolicy => SchedulerResult::InvalidPolicy,
        RawSchedulerResult::InvalidArgument => SchedulerResult::InvalidArgument,
        RawSchedulerResult::CpuOffline => SchedulerResult::CpuOffline,
        RawSchedulerResult::EmptyCpuMask => SchedulerResult::EmptyCpuMask,
        RawSchedulerResult::AdmissionFailed => SchedulerResult::AdmissionFailed,
        RawSchedulerResult::Busy => SchedulerResult::Busy,
        RawSchedulerResult::BufferTooSmall => SchedulerResult::BufferTooSmall,
    })
}

fn encode_fair(config: FairConfig<'_>) -> RawSchedulerAttrV1 {
    let mut raw = RawSchedulerAttrV1::new();
    raw.nice = config.nice;
    raw.util_min = config.util_min;
    encode_fair_affinity(&mut raw, config.affinity);
    raw
}

fn encode_deadline(config: DeadlineConfig, fair: FairConfig<'_>) -> RawSchedulerAttrV1 {
    let mut raw = encode_fair(fair);
    raw.policy = SCHED_POLICY_DEADLINE;
    raw.runtime_ns = config.runtime_ns;
    raw.deadline_ns = config.period_ns;
    raw.period_ns = raw.deadline_ns;
    raw.deadline_cpu_id = config.cpu_id;
    raw
}

fn encode_fair_affinity(raw: &mut RawSchedulerAttrV1, affinity: FairAffinity<'_>) {
    match affinity {
        FairAffinity::Any => {}
        FairAffinity::Single(cpu_id) => {
            raw.affinity_kind = SCHED_AFFINITY_SINGLE;
            raw.cpu_id = cpu_id;
        }
        FairAffinity::Mask(mask) => {
            raw.affinity_kind = SCHED_AFFINITY_MASK;
            raw.cpu_mask_ptr = mask.bytes.as_ptr() as u64;
            raw.cpu_mask_bytes = mask.bytes.len() as u32;
            raw.cpu_mask_nbits = mask.nbits;
        }
    }
}

fn encode_configured(config: &ConfiguredScheduler) -> Result<RawSchedulerAttrV1, SchedulerError> {
    let affinity = match &config.fair_affinity {
        OwnedFairAffinity::Any => FairAffinity::Any,
        OwnedFairAffinity::Single(cpu_id) => FairAffinity::Single(*cpu_id),
        OwnedFairAffinity::Mask(mask) => FairAffinity::Mask(mask.as_borrowed()),
    };
    let fair = FairConfig {
        nice: config.fair_nice,
        util_min: config.fair_util_min,
        affinity,
    };
    match config.policy {
        SchedulerPolicy::Fair => Ok(encode_fair(fair)),
        SchedulerPolicy::Deadline => {
            let deadline = config
                .deadline
                .ok_or(SchedulerError::InvalidFieldsForPolicy)?;
            Ok(encode_deadline(deadline, fair))
        }
    }
}

fn apply_raw(mut raw: RawSchedulerAttrV1) -> Result<(), SchedulerError> {
    match syscall_result(syscall1(Syscall::SetSchedulerAttr, raw_ptr(&mut raw)))? {
        SchedulerResult::Ok => Ok(()),
        result => Err(SchedulerError::Kernel(result)),
    }
}

fn decode_policy(value: u32) -> Result<SchedulerPolicy, SchedulerError> {
    match value {
        SCHED_POLICY_FAIR => Ok(SchedulerPolicy::Fair),
        SCHED_POLICY_DEADLINE => Ok(SchedulerPolicy::Deadline),
        _ => Err(SchedulerError::UnknownPolicy(value)),
    }
}

fn decode_status(value: u32) -> Result<SchedulerStatus, SchedulerError> {
    match RawSchedulerStatus::from_raw(value) {
        Some(RawSchedulerStatus::Unknown) => Ok(SchedulerStatus::Unknown),
        Some(RawSchedulerStatus::Running) => Ok(SchedulerStatus::Running),
        Some(RawSchedulerStatus::Queued) => Ok(SchedulerStatus::Queued),
        Some(RawSchedulerStatus::Blocked) => Ok(SchedulerStatus::Blocked),
        Some(RawSchedulerStatus::Throttled) => Ok(SchedulerStatus::Throttled),
        None => Err(SchedulerError::UnknownStatus(value)),
    }
}

fn decode_owned_affinity(
    raw: &RawSchedulerAttrV1,
    mask: Option<Vec<u8>>,
) -> Result<OwnedFairAffinity, SchedulerError> {
    match raw.affinity_kind {
        SCHED_AFFINITY_ANY
            if raw.cpu_id == SCHED_CPU_ID_NONE
                && raw.cpu_mask_bytes == 0
                && raw.cpu_mask_nbits == 0 =>
        {
            Ok(OwnedFairAffinity::Any)
        }
        SCHED_AFFINITY_SINGLE
            if raw.cpu_id != SCHED_CPU_ID_NONE
                && raw.cpu_mask_ptr == 0
                && raw.cpu_mask_bytes == 0
                && raw.cpu_mask_nbits == 0 =>
        {
            Ok(OwnedFairAffinity::Single(raw.cpu_id))
        }
        SCHED_AFFINITY_MASK if raw.cpu_id == SCHED_CPU_ID_NONE && raw.cpu_mask_ptr != 0 => {
            let bytes = mask.ok_or(SchedulerError::InvalidAffinityForPolicy)?;
            if bytes.len() != raw.cpu_mask_bytes as usize {
                return Err(SchedulerError::InvalidAffinityForPolicy);
            }
            Ok(OwnedFairAffinity::Mask(OwnedCpuMask::new(
                bytes,
                raw.cpu_mask_nbits,
            )?))
        }
        SCHED_AFFINITY_ANY | SCHED_AFFINITY_SINGLE | SCHED_AFFINITY_MASK => {
            Err(SchedulerError::InvalidAffinityForPolicy)
        }
        value => Err(SchedulerError::UnknownAffinity(value)),
    }
}

fn decode_configured(
    raw: RawSchedulerAttrV1,
    mask: Option<Vec<u8>>,
) -> Result<ConfiguredScheduler, SchedulerError> {
    if raw.size != core::mem::size_of::<RawSchedulerAttrV1>() as u32 || raw.version != 1 {
        return Err(SchedulerError::InvalidResponseHeader);
    }
    if raw.flags != SCHED_ATTR_FLAGS_NONE {
        return Err(SchedulerError::InvalidResponseFlags(raw.flags));
    }
    if raw.reserved0 != 0 || raw.reserved != [0; 6] {
        return Err(SchedulerError::InvalidResponseReserved);
    }
    validate_fair_values(raw.nice, raw.util_min)?;
    let policy = decode_policy(raw.policy)?;
    let fair_affinity = decode_owned_affinity(&raw, mask)?;
    let deadline = match policy {
        SchedulerPolicy::Fair => {
            if raw.runtime_ns != 0
                || raw.deadline_ns != 0
                || raw.period_ns != 0
                || raw.deadline_cpu_id != SCHED_CPU_ID_NONE
            {
                return Err(SchedulerError::InvalidFieldsForPolicy);
            }
            None
        }
        SchedulerPolicy::Deadline => {
            if raw.deadline_ns != raw.period_ns {
                return Err(SchedulerError::InvalidFieldsForPolicy);
            }
            Some(DeadlineConfig::new(
                Duration::from_nanos(raw.runtime_ns),
                Duration::from_nanos(raw.period_ns),
                raw.deadline_cpu_id,
            )?)
        }
    };
    Ok(ConfiguredScheduler {
        policy,
        fair_nice: raw.nice,
        fair_util_min: raw.util_min,
        fair_affinity,
        deadline,
    })
}

fn decode_runtime_state(raw: RawSchedulerStateV1) -> Result<RuntimeSchedulerState, SchedulerError> {
    if raw.size != core::mem::size_of::<RawSchedulerStateV1>() as u32 || raw.version != 1 {
        return Err(SchedulerError::InvalidResponseHeader);
    }
    if raw.flags != SCHED_ATTR_FLAGS_NONE {
        return Err(SchedulerError::InvalidResponseFlags(raw.flags));
    }
    if raw.reserved0 != 0 || raw.reserved1 != 0 || raw.reserved != [0; 5] {
        return Err(SchedulerError::InvalidResponseReserved);
    }
    validate_fair_values(raw.nice, raw.util_min)?;
    let policy = decode_policy(raw.policy)?;
    match raw.affinity_kind {
        SCHED_AFFINITY_ANY | SCHED_AFFINITY_MASK if raw.configured_cpu_id == SCHED_CPU_ID_NONE => {}
        SCHED_AFFINITY_SINGLE if raw.configured_cpu_id != SCHED_CPU_ID_NONE => {}
        SCHED_AFFINITY_ANY | SCHED_AFFINITY_SINGLE | SCHED_AFFINITY_MASK => {
            return Err(SchedulerError::InvalidAffinityForPolicy);
        }
        value => return Err(SchedulerError::UnknownAffinity(value)),
    }
    if policy == SchedulerPolicy::Deadline
        && (raw.affinity_kind != SCHED_AFFINITY_SINGLE
            || raw.configured_cpu_id == SCHED_CPU_ID_NONE)
    {
        return Err(SchedulerError::InvalidAffinityForPolicy);
    }
    Ok(RuntimeSchedulerState {
        status: decode_status(raw.status)?,
        policy,
        current_cpu_id: (raw.current_cpu_id != SCHED_CPU_ID_NONE).then_some(raw.current_cpu_id),
        queued_cpu_id: (raw.queued_cpu_id != SCHED_CPU_ID_NONE).then_some(raw.queued_cpu_id),
        fair_vruntime: Duration::from_nanos(raw.fair_vruntime_ns),
        fair_vdeadline: Duration::from_nanos(raw.fair_vdeadline_ns),
        fair_slice_remaining: Duration::from_nanos(raw.fair_slice_remaining_ns),
        deadline_runtime_remaining: Duration::from_nanos(raw.deadline_runtime_remaining_ns),
        deadline_absolute: Duration::from_nanos(raw.deadline_absolute_ns),
        deadline_replenishment: Duration::from_nanos(raw.deadline_replenishment_ns),
        deadline_admission_units: raw.deadline_admission_units,
        deadline_miss_count: raw.deadline_miss_count,
        deadline_overrun_count: raw.deadline_overrun_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_mask_rejects_empty_and_non_canonical_masks() {
        assert_eq!(CpuMask::new(&[0], 8), Err(SchedulerError::EmptyCpuMask));
        assert_eq!(
            CpuMask::new(&[0b1000_0001], 1),
            Err(SchedulerError::NonCanonicalCpuMask)
        );
        assert_eq!(
            CpuMask::new(&[1, 0], 1),
            Err(SchedulerError::NonCanonicalCpuMask)
        );
    }

    #[test]
    fn deadline_config_validates_duration_and_encodes_fair_fallback() {
        assert_eq!(
            DeadlineConfig::new(Duration::ZERO, Duration::from_nanos(1), 0),
            Err(SchedulerError::InvalidDuration)
        );
        assert_eq!(
            DeadlineConfig::new(Duration::from_nanos(2), Duration::from_nanos(1), 0),
            Err(SchedulerError::RuntimeExceedsPeriod)
        );
        let config = DeadlineConfig::new(Duration::from_nanos(1), Duration::from_nanos(2), 3)
            .expect("valid deadline configuration");
        let mask_bytes = [0b0000_0011];
        let fair = FairConfig::new(
            -5,
            512,
            FairAffinity::Mask(CpuMask::new(&mask_bytes, 2).expect("valid mask")),
        )
        .expect("valid fair configuration");
        let raw = encode_deadline(config, fair);
        assert_eq!(raw.policy, SCHED_POLICY_DEADLINE);
        assert_eq!(raw.affinity_kind, SCHED_AFFINITY_MASK);
        assert_eq!(raw.cpu_id, SCHED_CPU_ID_NONE);
        assert_eq!(raw.nice, -5);
        assert_eq!(raw.util_min, 512);
        assert_eq!(raw.cpu_mask_ptr, mask_bytes.as_ptr() as u64);
        assert_eq!(raw.cpu_mask_bytes, 1);
        assert_eq!(raw.cpu_mask_nbits, 2);
        assert_eq!(raw.runtime_ns, 1);
        assert_eq!(raw.deadline_ns, 2);
        assert_eq!(raw.period_ns, 2);
        assert_eq!(raw.deadline_cpu_id, 3);
    }

    #[test]
    fn fair_config_encodes_mask_without_dangling_pointer() {
        let bytes = [0b0000_0011];
        let mask = CpuMask::new(&bytes, 2).expect("valid mask");
        let fair = FairConfig::new(1, 512, FairAffinity::Mask(mask)).expect("valid fair config");
        let raw = encode_fair(fair);
        assert_eq!(raw.policy, SCHED_POLICY_FAIR);
        assert_eq!(raw.affinity_kind, SCHED_AFFINITY_MASK);
        assert_eq!(raw.cpu_mask_bytes, 1);
        assert_eq!(raw.cpu_mask_nbits, 2);
        assert_eq!(raw.cpu_mask_ptr, bytes.as_ptr() as u64);
    }

    #[test]
    fn configured_decode_rejects_unknown_policy_and_decodes_fair() {
        let mut raw = RawSchedulerAttrV1::new();
        raw.policy = 99;
        assert_eq!(
            decode_configured(raw, None),
            Err(SchedulerError::UnknownPolicy(99))
        );

        let config = decode_configured(RawSchedulerAttrV1::new(), None)
            .expect("default fair attributes decode");
        assert_eq!(config.policy(), SchedulerPolicy::Fair);
        assert_eq!(config.fair_nice(), 0);
        assert_eq!(config.fair_util_min(), 0);
    }

    #[test]
    fn configured_deadline_round_trips_masked_fair_fallback() {
        let mut raw = RawSchedulerAttrV1::new();
        raw.policy = SCHED_POLICY_DEADLINE;
        raw.affinity_kind = SCHED_AFFINITY_MASK;
        raw.nice = -10;
        raw.util_min = 256;
        raw.cpu_mask_ptr = 1;
        raw.cpu_mask_bytes = 1;
        raw.cpu_mask_nbits = 2;
        raw.runtime_ns = 10;
        raw.deadline_ns = 20;
        raw.period_ns = 20;
        raw.deadline_cpu_id = 7;

        let configured = decode_configured(raw, Some(vec![0b0000_0011]))
            .expect("deadline configuration decodes");
        assert_eq!(configured.policy(), SchedulerPolicy::Deadline);
        assert_eq!(configured.fair_nice(), -10);
        assert_eq!(configured.fair_util_min(), 256);
        assert!(matches!(
            configured.fair_affinity(),
            OwnedFairAffinity::Mask(mask) if mask.bytes() == [0b0000_0011] && mask.nbits() == 2
        ));
        assert_eq!(
            configured.deadline().map(|deadline| deadline.cpu_id()),
            Some(7)
        );

        let encoded = encode_configured(&configured).expect("decoded configuration encodes");
        assert_eq!(encoded.affinity_kind, SCHED_AFFINITY_MASK);
        assert_eq!(encoded.nice, -10);
        assert_eq!(encoded.util_min, 256);
        assert_ne!(encoded.cpu_mask_ptr, 0);
        assert_eq!(encoded.cpu_mask_bytes, 1);
        assert_eq!(encoded.cpu_mask_nbits, 2);
        assert_eq!(encoded.runtime_ns, 10);
        assert_eq!(encoded.period_ns, 20);
        assert_eq!(encoded.deadline_cpu_id, 7);
    }

    #[test]
    fn mask_probe_rejects_zero_and_absurd_requirements() {
        assert_eq!(
            validate_mask_probe(0, 1),
            Err(SchedulerError::InvalidMaskProbe)
        );
        assert_eq!(
            validate_mask_probe(1, 0),
            Err(SchedulerError::InvalidMaskProbe)
        );
        assert_eq!(
            validate_mask_probe(2, 1),
            Err(SchedulerError::InvalidMaskProbe)
        );
        assert_eq!(
            validate_mask_probe((MAX_CPU_MASK_BYTES + 1) as u32, 8),
            Err(SchedulerError::InvalidMaskProbe)
        );
        assert_eq!(validate_mask_probe(1, 8), Ok(1));
    }

    #[test]
    fn runtime_decode_rejects_unknown_status() {
        let mut raw = RawSchedulerStateV1::new();
        raw.status = 99;
        assert_eq!(
            decode_runtime_state(raw),
            Err(SchedulerError::UnknownStatus(99))
        );
    }

    #[test]
    fn raw_result_codes_have_stable_safe_mappings() {
        let expected = [
            SchedulerResult::Ok,
            SchedulerResult::BadAddress,
            SchedulerResult::BadSize,
            SchedulerResult::UnsupportedVersion,
            SchedulerResult::InvalidFlags,
            SchedulerResult::InvalidPolicy,
            SchedulerResult::InvalidArgument,
            SchedulerResult::CpuOffline,
            SchedulerResult::EmptyCpuMask,
            SchedulerResult::AdmissionFailed,
            SchedulerResult::Busy,
            SchedulerResult::BufferTooSmall,
        ];
        for (raw, result) in expected.into_iter().enumerate() {
            assert_eq!(syscall_result(raw), Ok(result));
        }
        assert_eq!(syscall_result(12), Err(SchedulerError::UnknownResult(12)));
    }
}
