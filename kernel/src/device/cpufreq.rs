//! CPU frequency policy, governor, and provider registry.
//!
//! This module keeps platform-specific frequency switching behind a common
//! driver interface. Callers operate on CPU frequency policies, which map to
//! firmware performance domains shared by one or more CPUs.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use spin::Mutex;

use crate::{environment::MAX_NUM_CPUS, task::SCHED_UTIL_SCALE};

const MAX_CPUFREQ_BACKENDS: usize = 8;
const MAX_CPUFREQ_POLICIES: usize = 8;
/// Maximum number of operating points stored for one CPU frequency policy.
pub const MAX_CPUFREQ_OPPS: usize = 32;

const INVALID_PERFORMANCE_DOMAIN: u32 = 0;
const CPUFREQ_UP_RATE_LIMIT_NS: u64 = 10_000_000;
const CPUFREQ_DOWN_RATE_LIMIT_NS: u64 = 100_000_000;
const SCHEDUTIL_HEADROOM_NUM: u64 = 5;
const SCHEDUTIL_HEADROOM_DEN: u64 = 4;

/// CPU frequency operating point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuFrequencyOpp {
    /// Platform performance-state index.
    pub pstate: u32,
    /// Frequency represented by this operating point, in kHz.
    pub freq_khz: u64,
}

impl CpuFrequencyOpp {
    const fn empty() -> Self {
        Self {
            pstate: 0,
            freq_khz: 0,
        }
    }
}

/// Generic CPU frequency governor attached to a policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuFrequencyGovernor {
    /// Keep the policy at its highest operating point.
    Performance,
    /// Keep the policy at its lowest operating point.
    Powersave,
    /// Accept explicit target requests and do not update automatically.
    Userspace,
    /// Scale from scheduler utilization.
    Schedutil,
}

impl CpuFrequencyGovernor {
    /// Return a stable lowercase governor name.
    ///
    /// # Returns
    ///
    /// A static string suitable for diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Performance => "performance",
            Self::Powersave => "powersave",
            Self::Userspace => "userspace",
            Self::Schedutil => "schedutil",
        }
    }
}

/// Architecture- or platform-provided CPU frequency diagnostic information.
///
/// This is a best-effort snapshot for debug/introspection surfaces. Not every
/// platform can report every field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuFrequencyInfo {
    /// Firmware phandle of the performance domain shared by this CPU.
    pub performance_domain: u32,
    /// Raw platform-specific frequency/status register value.
    pub raw_status: u32,
    /// Current performance state, if the platform layout is known.
    pub current_pstate: Option<u32>,
    /// Target performance state, if the platform layout is known.
    pub target_pstate: Option<u32>,
    /// Current frequency in kHz, if the current pstate is known in the OPP table.
    pub current_freq_khz: Option<u64>,
    /// Target frequency in kHz, if the target pstate is known in the OPP table.
    pub target_freq_khz: Option<u64>,
    /// Maximum frequency in kHz from the OPP table.
    pub max_freq_khz: Option<u64>,
}

/// CPU frequency policy snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuFrequencyPolicyInfo {
    /// Firmware performance-domain phandle.
    pub domain: u32,
    /// Bit mask of logical CPUs that share this policy.
    pub cpus_mask: u64,
    /// Active governor.
    pub governor: CpuFrequencyGovernor,
    /// Minimum policy frequency in kHz.
    pub min_freq_khz: u64,
    /// Maximum policy frequency in kHz.
    pub max_freq_khz: u64,
    /// Last successfully requested target frequency in kHz.
    pub target_freq_khz: u64,
    /// Last sampled scheduler utilization in [`SCHED_UTIL_SCALE`] units.
    pub last_util: u32,
    /// Number of operating points registered for this policy.
    pub opp_count: usize,
    /// Expected transition latency in nanoseconds.
    pub transition_latency_ns: u64,
}

/// CPU frequency policy registration data.
pub struct CpuFrequencyPolicyRegistration<'a> {
    /// Backend name that owns this policy.
    pub backend_name: &'static str,
    /// Firmware performance-domain phandle.
    pub domain: u32,
    /// Available operating points.
    pub opps: &'a [CpuFrequencyOpp],
    /// Default governor for this policy.
    pub governor: CpuFrequencyGovernor,
    /// Expected transition latency in nanoseconds.
    pub transition_latency_ns: u64,
}

/// CPU frequency provider.
#[derive(Clone, Copy)]
pub struct CpuFrequencyBackend {
    /// Stable backend name for diagnostics.
    pub name: &'static str,
    /// Snapshot callback for a logical CPU ID.
    pub snapshot: fn(usize) -> Option<CpuFrequencyInfo>,
    /// Set a performance-domain target pstate.
    pub set_pstate: Option<fn(u32, u32) -> Result<(), &'static str>>,
}

#[derive(Clone, Copy)]
struct CpuFrequencyPolicy {
    valid: bool,
    backend_name: Option<&'static str>,
    domain: u32,
    cpus_mask: u64,
    opp_count: usize,
    opps: [CpuFrequencyOpp; MAX_CPUFREQ_OPPS],
    min_freq_khz: u64,
    max_freq_khz: u64,
    target_freq_khz: u64,
    governor: CpuFrequencyGovernor,
    transition_latency_ns: u64,
    last_governor_update_ns: u64,
    last_target_change_ns: u64,
    last_util: u32,
    request_generation: u64,
}

impl CpuFrequencyPolicy {
    const fn empty() -> Self {
        Self {
            valid: false,
            backend_name: None,
            domain: INVALID_PERFORMANCE_DOMAIN,
            cpus_mask: 0,
            opp_count: 0,
            opps: [CpuFrequencyOpp::empty(); MAX_CPUFREQ_OPPS],
            min_freq_khz: 0,
            max_freq_khz: 0,
            target_freq_khz: 0,
            governor: CpuFrequencyGovernor::Schedutil,
            transition_latency_ns: 0,
            last_governor_update_ns: 0,
            last_target_change_ns: 0,
            last_util: 0,
            request_generation: 0,
        }
    }

    fn invalidate_deferred_requests(&mut self) -> u64 {
        self.request_generation = self.request_generation.wrapping_add(1);
        self.request_generation
    }

    fn info(&self) -> CpuFrequencyPolicyInfo {
        CpuFrequencyPolicyInfo {
            domain: self.domain,
            cpus_mask: self.cpus_mask,
            governor: self.governor,
            min_freq_khz: self.min_freq_khz,
            max_freq_khz: self.max_freq_khz,
            target_freq_khz: self.target_freq_khz,
            last_util: self.last_util,
            opp_count: self.opp_count,
            transition_latency_ns: self.transition_latency_ns,
        }
    }

    fn resolve_target(&self, target_freq_khz: u64) -> Option<CpuFrequencyOpp> {
        let mut best_above: Option<CpuFrequencyOpp> = None;
        let mut best_max: Option<CpuFrequencyOpp> = None;

        for opp in self.opps[..self.opp_count].iter().copied() {
            if best_max
                .map(|current| opp.freq_khz > current.freq_khz)
                .unwrap_or(true)
            {
                best_max = Some(opp);
            }

            if opp.freq_khz >= target_freq_khz
                && best_above
                    .map(|current| opp.freq_khz < current.freq_khz)
                    .unwrap_or(true)
            {
                best_above = Some(opp);
            }
        }

        best_above.or(best_max)
    }
}

static CPU_PERF_DOMAINS: [AtomicU32; MAX_NUM_CPUS] =
    [const { AtomicU32::new(INVALID_PERFORMANCE_DOMAIN) }; MAX_NUM_CPUS];
static CPUFREQ_BACKENDS: Mutex<[Option<CpuFrequencyBackend>; MAX_CPUFREQ_BACKENDS]> =
    Mutex::new([None; MAX_CPUFREQ_BACKENDS]);
static CPUFREQ_POLICIES: Mutex<[CpuFrequencyPolicy; MAX_CPUFREQ_POLICIES]> =
    Mutex::new([CpuFrequencyPolicy::empty(); MAX_CPUFREQ_POLICIES]);
static CPUFREQ_TRANSITION_LOCK: Mutex<()> = Mutex::new(());
static CPUFREQ_PENDING_REQUESTS: Mutex<PendingRequests> = Mutex::new(PendingRequests::empty());
static CPUFREQ_WORKER_STARTED: AtomicBool = AtomicBool::new(false);
static CPUFREQ_WORKER_WAKER: crate::sync::Waker =
    crate::sync::Waker::new_uninterruptible("cpufreq-worker");

/// Register the firmware performance domain associated with a CPU.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU ID used by the scheduler.
/// * `phandle` - Firmware phandle from a CPU node's `performance-domains`.
pub fn register_cpu_performance_domain(cpu_id: usize, phandle: u32) {
    if cpu_id >= MAX_NUM_CPUS || phandle == INVALID_PERFORMANCE_DOMAIN {
        return;
    }

    CPU_PERF_DOMAINS[cpu_id].store(phandle, Ordering::SeqCst);
    update_policy_cpu_masks();
}

/// Return the firmware performance domain associated with a CPU.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU ID used by the scheduler.
///
/// # Returns
///
/// The registered performance-domain phandle, or `None` if unavailable.
pub fn cpu_performance_domain(cpu_id: usize) -> Option<u32> {
    if cpu_id >= MAX_NUM_CPUS {
        return None;
    }

    match CPU_PERF_DOMAINS[cpu_id].load(Ordering::SeqCst) {
        INVALID_PERFORMANCE_DOMAIN => None,
        phandle => Some(phandle),
    }
}

/// Register a CPU frequency backend.
///
/// # Arguments
///
/// * `backend` - Provider callbacks for a platform-specific implementation.
///
/// # Returns
///
/// `Ok(())` on success, or an error if the registry is full.
pub fn register_backend(backend: CpuFrequencyBackend) -> Result<(), &'static str> {
    let mut backends = CPUFREQ_BACKENDS.lock();

    if backends
        .iter()
        .flatten()
        .any(|registered| registered.name == backend.name)
    {
        return Ok(());
    }

    let Some(slot) = backends.iter_mut().find(|slot| slot.is_none()) else {
        return Err("cpufreq: backend registry full");
    };
    *slot = Some(backend);
    Ok(())
}

/// Register a CPU frequency policy.
///
/// # Arguments
///
/// * `registration` - Policy metadata and operating points owned by a backend.
///
/// # Returns
///
/// `Ok(())` on success, or an error if the policy is invalid or the registry is full.
///
/// Policies must be registered from boot initcalls before late initcalls run.
pub fn register_policy(
    registration: CpuFrequencyPolicyRegistration<'_>,
) -> Result<(), &'static str> {
    if registration.domain == INVALID_PERFORMANCE_DOMAIN {
        return Err("cpufreq: invalid performance domain");
    }
    if registration.opps.is_empty() {
        return Err("cpufreq: policy has no operating points");
    }

    let _transition = CPUFREQ_TRANSITION_LOCK.lock();
    let mut policies = CPUFREQ_POLICIES.lock();
    let slot_index = policies
        .iter()
        .position(|policy| {
            policy.valid
                && policy.domain == registration.domain
                && policy.backend_name == Some(registration.backend_name)
        })
        .or_else(|| policies.iter().position(|policy| !policy.valid))
        .ok_or("cpufreq: policy registry full")?;
    let slot = &mut policies[slot_index];
    let request_generation = slot.invalidate_deferred_requests();

    let mut opps = [CpuFrequencyOpp::empty(); MAX_CPUFREQ_OPPS];
    let opp_count = core::cmp::min(registration.opps.len(), MAX_CPUFREQ_OPPS);
    let mut min_freq_khz = u64::MAX;
    let mut max_freq_khz = 0u64;

    for (index, opp) in registration
        .opps
        .iter()
        .copied()
        .take(opp_count)
        .enumerate()
    {
        opps[index] = opp;
        min_freq_khz = min_freq_khz.min(opp.freq_khz);
        max_freq_khz = max_freq_khz.max(opp.freq_khz);
    }

    let cpus_mask = cpus_mask_for_domain(registration.domain);
    *slot = CpuFrequencyPolicy {
        valid: true,
        backend_name: Some(registration.backend_name),
        domain: registration.domain,
        cpus_mask,
        opp_count,
        opps,
        min_freq_khz,
        max_freq_khz,
        target_freq_khz: max_freq_khz,
        governor: registration.governor,
        transition_latency_ns: registration.transition_latency_ns,
        last_governor_update_ns: 0,
        last_target_change_ns: 0,
        last_util: 0,
        request_generation,
    };

    Ok(())
}

/// Return a CPU frequency policy for a CPU.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU ID used by the scheduler.
///
/// # Returns
///
/// Policy snapshot for the CPU's performance domain, if one exists.
pub fn cpu_frequency_policy_info(cpu_id: usize) -> Option<CpuFrequencyPolicyInfo> {
    let domain = cpu_performance_domain(cpu_id)?;
    cpu_frequency_policy_info_by_domain(domain)
}

/// Return a CPU frequency policy by performance domain.
///
/// # Arguments
///
/// * `domain` - Firmware performance-domain phandle.
///
/// # Returns
///
/// Policy snapshot, if one exists.
pub fn cpu_frequency_policy_info_by_domain(domain: u32) -> Option<CpuFrequencyPolicyInfo> {
    let policies = CPUFREQ_POLICIES.lock();
    policies
        .iter()
        .find(|policy| policy.valid && policy.domain == domain)
        .map(CpuFrequencyPolicy::info)
}

/// Set the governor attached to a performance domain.
///
/// # Arguments
///
/// * `domain` - Firmware performance-domain phandle.
/// * `governor` - Governor to attach.
///
/// # Returns
///
/// `Ok(())` on success, or an error if the policy does not exist.
///
/// This function must be called from task context.
pub fn set_domain_governor(
    domain: u32,
    governor: CpuFrequencyGovernor,
) -> Result<(), &'static str> {
    let _transition = CPUFREQ_TRANSITION_LOCK.lock();
    let mut policies = CPUFREQ_POLICIES.lock();
    let policy = policies
        .iter_mut()
        .find(|policy| policy.valid && policy.domain == domain)
        .ok_or("cpufreq: policy not found")?;
    policy.governor = governor;
    policy.invalidate_deferred_requests();
    Ok(())
}

/// Request a target frequency for a performance domain.
///
/// # Arguments
///
/// * `domain` - Firmware performance-domain phandle.
/// * `target_freq_khz` - Requested frequency in kHz.
///
/// # Returns
///
/// The selected operating point on success, or an error from the backend.
///
/// This synchronous function may wait for platform MMIO and must be called
/// from task context, never from an IRQ or FIQ handler.
pub fn set_domain_target_frequency(
    domain: u32,
    target_freq_khz: u64,
) -> Result<CpuFrequencyOpp, &'static str> {
    let _transition = CPUFREQ_TRANSITION_LOCK.lock();
    let Some(request) = explicit_target_request_for_domain(domain, target_freq_khz) else {
        return Err("cpufreq: policy not found");
    };

    apply_target_request_locked(request)?;
    Ok(request.opp)
}

/// Return CPU frequency diagnostics for a CPU.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU ID used by the scheduler.
///
/// # Returns
///
/// A best-effort frequency snapshot, or `None` if no backend can report one.
pub fn cpu_frequency_info(cpu_id: usize) -> Option<CpuFrequencyInfo> {
    let backends = CPUFREQ_BACKENDS.lock();

    backends
        .iter()
        .flatten()
        .find_map(|backend| (backend.snapshot)(cpu_id))
}

/// Update the governor for the performance domain containing a CPU.
///
/// Scheduler code calls this from the timer tick after refreshing CPU
/// utilization. Governors are rate-limited internally, and hardware updates
/// are deferred to a kernel worker so timer IRQ/FIQ context never performs
/// slow or allocating backend operations.
///
/// # Arguments
///
/// * `cpu_id` - Logical CPU ID whose scheduler tick fired.
pub fn on_scheduler_tick(cpu_id: usize) {
    let Some(domain) = cpu_performance_domain(cpu_id) else {
        return;
    };
    let Some(request) = governor_request_for_domain(domain) else {
        return;
    };

    let _ = queue_deferred_request(request);
}

#[derive(Clone, Copy)]
struct TargetRequest {
    backend_name: &'static str,
    domain: u32,
    opp: CpuFrequencyOpp,
    generation: u64,
}

struct PendingRequests {
    requests: [Option<TargetRequest>; MAX_CPUFREQ_POLICIES],
}

impl PendingRequests {
    const fn empty() -> Self {
        Self {
            requests: [None; MAX_CPUFREQ_POLICIES],
        }
    }

    fn queue(&mut self, request: TargetRequest) -> Result<bool, &'static str> {
        let was_empty = self.requests.iter().all(Option::is_none);

        if let Some(slot) = self.requests.iter_mut().find(|slot| {
            slot.map(|pending| pending.domain == request.domain)
                .unwrap_or(false)
        }) {
            *slot = Some(request);
            return Ok(false);
        }

        let Some(slot) = self.requests.iter_mut().find(|slot| slot.is_none()) else {
            return Err("cpufreq: pending request queue full");
        };
        *slot = Some(request);
        Ok(was_empty)
    }

    fn take(&mut self) -> Option<TargetRequest> {
        self.requests.iter_mut().find_map(Option::take)
    }
}

fn take_pending_request() -> Option<TargetRequest> {
    CPUFREQ_PENDING_REQUESTS.lock().take()
}

fn queue_deferred_request(request: TargetRequest) -> Result<(), &'static str> {
    let added = CPUFREQ_PENDING_REQUESTS.lock().queue(request)?;
    if added {
        CPUFREQ_WORKER_WAKER.wake_one();
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetRequestOutcome {
    Applied,
    Superseded,
}

fn apply_deferred_target_request(
    request: TargetRequest,
) -> Result<TargetRequestOutcome, &'static str> {
    let _transition = CPUFREQ_TRANSITION_LOCK.lock();
    if !target_request_is_current(request) {
        return Ok(TargetRequestOutcome::Superseded);
    }

    apply_target_request_locked(request)?;
    Ok(TargetRequestOutcome::Applied)
}

fn process_pending_request() -> bool {
    let Some(request) = take_pending_request() else {
        return false;
    };
    let _ = apply_deferred_target_request(request);
    true
}

fn cpufreq_worker_entry() {
    loop {
        while process_pending_request() {}

        let Some(task) = crate::task::mytask() else {
            crate::arch::instruction::idle();
        };
        CPUFREQ_WORKER_WAKER.wait(task.get_id(), task.get_trapframe());
    }
}

fn start_cpufreq_worker() {
    if !CPUFREQ_POLICIES.lock().iter().any(|policy| policy.valid)
        || CPUFREQ_WORKER_STARTED
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
    {
        return;
    }

    let task = crate::task::new_kernel_task(
        alloc::string::String::from("cpufreq-worker"),
        1,
        cpufreq_worker_entry,
    );
    task.init();
    crate::sched::scheduler::add_task(task, 0);
}

crate::late_initcall!(start_cpufreq_worker);

fn target_request_for_domain(domain: u32, target_freq_khz: u64) -> Option<TargetRequest> {
    let policies = CPUFREQ_POLICIES.lock();
    let policy = policies
        .iter()
        .find(|policy| policy.valid && policy.domain == domain)?;
    let backend_name = policy.backend_name?;
    let opp = policy.resolve_target(target_freq_khz)?;

    Some(TargetRequest {
        backend_name,
        domain,
        opp,
        generation: policy.request_generation,
    })
}

fn explicit_target_request_for_domain(domain: u32, target_freq_khz: u64) -> Option<TargetRequest> {
    let mut policies = CPUFREQ_POLICIES.lock();
    let policy = policies
        .iter_mut()
        .find(|policy| policy.valid && policy.domain == domain)?;
    let backend_name = policy.backend_name?;
    let opp = policy.resolve_target(target_freq_khz)?;
    let generation = policy.invalidate_deferred_requests();

    Some(TargetRequest {
        backend_name,
        domain,
        opp,
        generation,
    })
}

fn governor_request_for_domain(domain: u32) -> Option<TargetRequest> {
    let now_ns = crate::timer::get_time_ns();
    let mut policies = CPUFREQ_POLICIES.lock();
    let policy = policies
        .iter_mut()
        .find(|policy| policy.valid && policy.domain == domain)?;

    update_policy_cpu_mask(policy);

    if now_ns.saturating_sub(policy.last_governor_update_ns) < CPUFREQ_UP_RATE_LIMIT_NS {
        return None;
    }

    let util = policy_util(policy.cpus_mask);
    policy.last_util = util;
    policy.last_governor_update_ns = now_ns;

    let target_freq_khz = match policy.governor {
        CpuFrequencyGovernor::Performance => policy.max_freq_khz,
        CpuFrequencyGovernor::Powersave => policy.min_freq_khz,
        CpuFrequencyGovernor::Userspace => return None,
        CpuFrequencyGovernor::Schedutil => schedutil_target(policy, util),
    };

    if target_freq_khz < policy.target_freq_khz
        && now_ns.saturating_sub(policy.last_target_change_ns) < CPUFREQ_DOWN_RATE_LIMIT_NS
    {
        return None;
    }

    let opp = policy.resolve_target(target_freq_khz)?;
    if policy.target_freq_khz == opp.freq_khz {
        return None;
    }

    let backend_name = policy.backend_name?;
    Some(TargetRequest {
        backend_name,
        domain: policy.domain,
        opp,
        generation: policy.request_generation,
    })
}

fn target_request_is_current(request: TargetRequest) -> bool {
    let policies = CPUFREQ_POLICIES.lock();
    policies.iter().any(|policy| {
        policy.valid
            && policy.domain == request.domain
            && policy.backend_name == Some(request.backend_name)
            && policy.request_generation == request.generation
    })
}

fn apply_target_request_locked(request: TargetRequest) -> Result<(), &'static str> {
    set_backend_pstate(request.backend_name, request.domain, request.opp.pstate)?;

    let now_ns = crate::timer::get_time_ns();
    let mut policies = CPUFREQ_POLICIES.lock();
    if let Some(policy) = policies.iter_mut().find(|policy| {
        policy.valid
            && policy.domain == request.domain
            && policy.backend_name == Some(request.backend_name)
            && policy.request_generation == request.generation
    }) {
        policy.target_freq_khz = request.opp.freq_khz;
        policy.last_target_change_ns = now_ns;
    }

    Ok(())
}

fn set_backend_pstate(
    backend_name: &'static str,
    domain: u32,
    pstate: u32,
) -> Result<(), &'static str> {
    let backends = CPUFREQ_BACKENDS.lock();
    let backend = backends
        .iter()
        .flatten()
        .find(|backend| backend.name == backend_name)
        .ok_or("cpufreq: backend not found")?;
    let set_pstate = backend
        .set_pstate
        .ok_or("cpufreq: backend cannot set pstate")?;
    set_pstate(domain, pstate)
}

fn schedutil_target(policy: &CpuFrequencyPolicy, util: u32) -> u64 {
    if util == 0 {
        return policy.min_freq_khz;
    }

    let boosted_util = ((util as u64) * SCHEDUTIL_HEADROOM_NUM)
        .div_ceil(SCHEDUTIL_HEADROOM_DEN)
        .min(SCHED_UTIL_SCALE as u64);
    let range = policy.max_freq_khz.saturating_sub(policy.min_freq_khz);
    policy
        .min_freq_khz
        .saturating_add(range.saturating_mul(boosted_util) / SCHED_UTIL_SCALE as u64)
}

fn policy_util(cpus_mask: u64) -> u32 {
    let mut util = 0u32;
    for cpu_id in 0..MAX_NUM_CPUS {
        if cpus_mask & cpu_bit(cpu_id) == 0 {
            continue;
        }
        if let Some(snapshot) = crate::sched::scheduler::cpu_util_snapshot(cpu_id) {
            let cpu_util = core::cmp::max(snapshot.util_avg, snapshot.util_min);
            util = util.max(cpu_util);
        }
    }
    util.min(SCHED_UTIL_SCALE)
}

fn update_policy_cpu_masks() {
    let mut policies = CPUFREQ_POLICIES.lock();
    for policy in policies.iter_mut().filter(|policy| policy.valid) {
        update_policy_cpu_mask(policy);
    }
}

fn update_policy_cpu_mask(policy: &mut CpuFrequencyPolicy) {
    policy.cpus_mask = cpus_mask_for_domain(policy.domain);
}

fn cpus_mask_for_domain(domain: u32) -> u64 {
    let mut mask = 0u64;
    for cpu_id in 0..MAX_NUM_CPUS {
        if CPU_PERF_DOMAINS[cpu_id].load(Ordering::SeqCst) == domain {
            mask |= cpu_bit(cpu_id);
        }
    }
    mask
}

fn cpu_bit(cpu_id: usize) -> u64 {
    if cpu_id < u64::BITS as usize {
        1u64 << cpu_id
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_SET_PSTATE_CALLS: AtomicU32 = AtomicU32::new(0);

    fn test_snapshot(_cpu_id: usize) -> Option<CpuFrequencyInfo> {
        None
    }

    fn test_set_pstate(_domain: u32, _pstate: u32) -> Result<(), &'static str> {
        TEST_SET_PSTATE_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn request(domain: u32, pstate: u32, freq_khz: u64) -> TargetRequest {
        TargetRequest {
            backend_name: "test-cpufreq",
            domain,
            opp: CpuFrequencyOpp { pstate, freq_khz },
            generation: 0,
        }
    }

    #[test_case]
    fn pending_requests_coalesce_latest_target_per_domain() {
        let mut pending = PendingRequests::empty();

        assert!(
            pending
                .queue(request(1, 3, 1_200_000))
                .expect("first domain request should queue")
        );
        assert!(
            !pending
                .queue(request(2, 4, 1_800_000))
                .expect("second domain request should queue without another wake")
        );
        assert!(
            !pending
                .queue(request(1, 5, 2_100_000))
                .expect("newer request for the same domain should replace it")
        );

        let first = pending.take().expect("first request should remain queued");
        let second = pending.take().expect("second request should remain queued");

        assert_eq!(first.domain, 1);
        assert_eq!(first.opp.pstate, 5);
        assert_eq!(first.opp.freq_khz, 2_100_000);
        assert_eq!(second.domain, 2);
        assert_eq!(second.opp.pstate, 4);
        assert!(pending.take().is_none());
    }

    #[test_case]
    fn deferred_request_is_superseded_by_explicit_policy_changes() {
        const DOMAIN: u32 = u32::MAX - 1;
        const OPPS: [CpuFrequencyOpp; 3] = [
            CpuFrequencyOpp {
                pstate: 1,
                freq_khz: 100_000,
            },
            CpuFrequencyOpp {
                pstate: 2,
                freq_khz: 200_000,
            },
            CpuFrequencyOpp {
                pstate: 3,
                freq_khz: 300_000,
            },
        ];

        register_backend(CpuFrequencyBackend {
            name: "test-deferred-cpufreq",
            snapshot: test_snapshot,
            set_pstate: Some(test_set_pstate),
        })
        .expect("test backend should register");
        register_policy(CpuFrequencyPolicyRegistration {
            backend_name: "test-deferred-cpufreq",
            domain: DOMAIN,
            opps: &OPPS,
            governor: CpuFrequencyGovernor::Schedutil,
            transition_latency_ns: 0,
        })
        .expect("test policy should register");
        TEST_SET_PSTATE_CALLS.store(0, Ordering::SeqCst);

        let deferred_target =
            target_request_for_domain(DOMAIN, 100_000).expect("test target request should resolve");
        assert!(queue_deferred_request(deferred_target).is_ok());
        assert_eq!(TEST_SET_PSTATE_CALLS.load(Ordering::SeqCst), 0);
        assert!(process_pending_request());
        assert_eq!(TEST_SET_PSTATE_CALLS.load(Ordering::SeqCst), 1);

        let stale_target = target_request_for_domain(DOMAIN, 300_000)
            .expect("stale test target request should resolve");
        assert!(queue_deferred_request(stale_target).is_ok());

        set_domain_target_frequency(DOMAIN, 200_000)
            .expect("explicit target should be applied synchronously");
        assert_eq!(TEST_SET_PSTATE_CALLS.load(Ordering::SeqCst), 2);
        assert!(process_pending_request());
        assert_eq!(TEST_SET_PSTATE_CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(
            cpu_frequency_policy_info_by_domain(DOMAIN)
                .expect("test policy should remain registered")
                .target_freq_khz,
            200_000
        );

        let stale_governor = target_request_for_domain(DOMAIN, 100_000)
            .expect("second test target request should resolve");
        assert!(queue_deferred_request(stale_governor).is_ok());
        set_domain_governor(DOMAIN, CpuFrequencyGovernor::Userspace)
            .expect("governor change should succeed");
        assert!(process_pending_request());
        assert_eq!(TEST_SET_PSTATE_CALLS.load(Ordering::SeqCst), 2);
    }
}
