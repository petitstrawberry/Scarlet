//! Kernel policy for non-CPU device frequency domains.
//!
//! The device driver owns clock, voltage and serialization with its work.
//! This layer owns the OPP ladder, governor request and thermal maximum. A
//! failed hardware transition never changes the advertised policy state.

use alloc::{format, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::sync::{IrqSpinLock, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceFrequencyOpp {
    /// Configured frequency of the operating point.
    pub freq_khz: u64,
    /// Minimum safe rail voltage in microvolts; zero for firmware-managed OPPs.
    pub min_uv: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceFrequencyGovernor {
    Performance,
    Powersave,
    Userspace,
    SimpleOndemand,
}

impl DeviceFrequencyGovernor {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Performance => "performance",
            Self::Powersave => "powersave",
            Self::Userspace => "userspace",
            Self::SimpleOndemand => "simple_ondemand",
        }
    }
}

/// Device busy and total cycles measured over one polling interval.
#[derive(Clone, Copy, Debug)]
pub struct DeviceFrequencyUtilization {
    pub busy: u32,
    pub total: u32,
}

/// Linux simple_ondemand's per-domain tuning. Interactive devices can leave
/// more execution headroom without changing another domain's policy.
#[derive(Clone, Copy, Debug)]
pub struct SimpleOndemandConfig {
    pub upthreshold_pct: u32,
    pub downdifferential_pct: u32,
}

impl Default for SimpleOndemandConfig {
    fn default() -> Self {
        Self {
            upthreshold_pct: 90,
            downdifferential_pct: 5,
        }
    }
}

pub trait DeviceFrequencyDriver: Send + Sync {
    /// Read the active hardware rate. The driver must serialize this with work.
    fn current_frequency_khz(&self) -> Result<u64, &'static str>;

    /// Set an exact registered OPP. A failure must leave the old rate active
    /// or mark the device lost so later requests cannot use unknown hardware.
    fn set_frequency_khz(&self, freq_khz: u64) -> Result<(), &'static str>;

    /// Read a fresh hardware activity interval. A driver without counters
    /// cannot use a utilization governor.
    fn sample_utilization(&self) -> Result<DeviceFrequencyUtilization, &'static str> {
        Err("devfreq: utilization counter unavailable")
    }
}

/// Elapsed time of actual driver frequency-change calls, including any driver
/// serialization and firmware ACK wait. This is not GPU execution time or an
/// isolated PLL/firmware latency measurement. Failed calls are counted too.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeviceFrequencyTransitions {
    pub attempts: u64,
    pub failures: u64,
    pub last_call_ns: u64,
    pub max_call_ns: u64,
    pub total_call_ns: u64,
    pub last_completed_ns: u64,
    pub last_from_khz: u64,
    pub last_to_khz: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct DeviceFrequencySnapshot {
    pub name: &'static str,
    pub governor: DeviceFrequencyGovernor,
    pub min_khz: u64,
    pub max_khz: u64,
    pub thermal_max_khz: u64,
    pub requested_khz: u64,
    pub target_khz: u64,
    pub current_khz: Option<u64>,
    pub utilization_pct: Option<u32>,
    pub sample_count: u64,
    pub failed_samples: u64,
    pub transitions: DeviceFrequencyTransitions,
    pub ondemand: SimpleOndemandConfig,
}

#[derive(Clone, Copy)]
struct PolicyState {
    governor: DeviceFrequencyGovernor,
    requested_khz: u64,
    target_khz: u64,
    thermal_max_khz: u64,
    utilization_pct: Option<u32>,
    sample_count: u64,
    failed_samples: u64,
    transitions: DeviceFrequencyTransitions,
    down_samples: u8,
    ondemand: SimpleOndemandConfig,
    active_jobs: u32,
    activity_idle_until_ns: u64,
    activity_floor_khz: u64,
    legacy_jobs: u32,
    work: WorkDemand,
}

const FRAME_NS: u64 = 16_666_667;
// Reserve 40% of the frame for CPU preparation/composition and scheduling.
// This is GPU execution headroom, not a claim to measure the whole UI deadline.
const WORK_BUDGET_NS: u64 = FRAME_NS * 3 / 5;
const WORK_DOWN_NS: u64 = 100_000_000;

#[derive(Clone, Copy, Default)]
struct WorkDemand {
    valid: bool,
    window_start_ns: u64,
    window_work: u64,
    window_jobs: u32,
    estimated_work: u64,
    job_work: u64,
    floor_khz: u64,
    low_windows: u8,
    last_change_ns: u64,
}

impl WorkDemand {
    fn required(opps: &[DeviceFrequencyOpp], work: u64) -> u64 {
        let rate = work.div_ceil(WORK_BUDGET_NS);
        opps.iter()
            .find(|opp| opp.freq_khz >= rate)
            .unwrap_or_else(|| opps.last().unwrap())
            .freq_khz
    }

    fn roll(&mut self, now: u64, opps: &[DeviceFrequencyOpp]) {
        if self.window_start_ns == 0 {
            self.window_start_ns = now;
        }
        if now.saturating_sub(self.window_start_ns) < FRAME_NS {
            return;
        }
        if self.window_jobs != 0 {
            self.estimated_work = if self.window_work >= self.estimated_work {
                self.window_work
            } else {
                ((u128::from(self.estimated_work) * 7 + u128::from(self.window_work)) / 8) as u64
            };
            let required = Self::required(opps, self.estimated_work);
            if required >= self.floor_khz {
                if required > self.floor_khz {
                    self.last_change_ns = now;
                }
                self.floor_khz = required;
                self.low_windows = 0;
            } else {
                self.low_windows = self.low_windows.saturating_add(1);
                if self.low_windows >= 4 && now.saturating_sub(self.last_change_ns) >= WORK_DOWN_NS
                {
                    let lower = opps
                        .iter()
                        .rev()
                        .find(|opp| opp.freq_khz < self.floor_khz)
                        .unwrap_or(&opps[0])
                        .freq_khz;
                    self.floor_khz = required.max(lower);
                    self.low_windows = 0;
                    self.last_change_ns = now;
                }
            }
        }
        self.window_start_ns = now;
        self.window_work = 0;
        self.window_jobs = 0;
    }

    fn next(&mut self, now: u64, queued_jobs: usize, opps: &[DeviceFrequencyOpp]) -> u64 {
        self.roll(now, opps);
        if !self.valid {
            self.floor_khz = opps.last().unwrap().freq_khz;
            return self.floor_khz;
        }
        // Sum all passes in the frame, rather than scaling for one tiny submit.
        // Pending work predicts drain demand without inventing busy cycles.
        let pending = self
            .job_work
            .saturating_mul(queued_jobs.saturating_add(1) as u64);
        let required = Self::required(
            opps,
            self.estimated_work
                .max(self.window_work.saturating_add(pending)),
        );
        if required > self.floor_khz {
            self.floor_khz = required;
            self.low_windows = 0;
            self.last_change_ns = now;
        }
        self.floor_khz
    }

    fn record(&mut self, sample: DeviceFrequencyWork, opps: &[DeviceFrequencyOpp]) {
        self.roll(sample.completed_ns, opps);
        let work = sample.busy_ns.saturating_mul(sample.frequency_khz);
        if !self.valid {
            self.valid = true;
            self.estimated_work = work;
            self.job_work = work;
            self.last_change_ns = sample.completed_ns;
        } else {
            self.job_work = if work > self.job_work {
                work
            } else {
                ((u128::from(self.job_work) * 3 + u128::from(work)) / 4) as u64
            };
        }
        self.window_work = self.window_work.saturating_add(work);
        self.window_jobs = self.window_jobs.saturating_add(1);
        let required = Self::required(opps, self.window_work);
        if required > self.floor_khz {
            self.floor_khz = required;
            self.low_windows = 0;
            self.last_change_ns = sample.completed_ns;
        }
    }
}

/// Actual GPU execution, excluding CPU command preparation and queue wait.
/// The driver must measure busy counter deltas under a stable acknowledged OPP.
#[derive(Clone, Copy)]
pub struct DeviceFrequencyWork {
    pub busy_ns: u64,
    pub frequency_khz: u64,
    pub completed_ns: u64,
}

struct Policy {
    index: usize,
    name: &'static str,
    opps: Vec<DeviceFrequencyOpp>,
    driver: Arc<dyn DeviceFrequencyDriver>,
    state: Mutex<PolicyState>,
    worker_started: AtomicBool,
}

static POLICIES: IrqSpinLock<Vec<Arc<Policy>>> = IrqSpinLock::new(Vec::new());

fn find(name: &str) -> Result<Arc<Policy>, &'static str> {
    POLICIES
        .lock()
        .iter()
        .find(|policy| policy.name == name)
        .cloned()
        .ok_or("devfreq: unknown device")
}

fn target(policy: &Policy, state: &PolicyState) -> u64 {
    let requested = match state.governor {
        DeviceFrequencyGovernor::Performance => state.thermal_max_khz,
        DeviceFrequencyGovernor::Powersave => policy.opps[0].freq_khz,
        DeviceFrequencyGovernor::Userspace => state.requested_khz,
        DeviceFrequencyGovernor::SimpleOndemand
            if state.active_jobs != 0
                || crate::time::current_time_ns() < state.activity_idle_until_ns =>
        {
            if state.legacy_jobs != 0 {
                state.thermal_max_khz
            } else {
                state.activity_floor_khz
            }
        }
        DeviceFrequencyGovernor::SimpleOndemand => state.requested_khz,
    };
    policy
        .opps
        .iter()
        .rev()
        .find(|opp| opp.freq_khz <= requested.min(state.thermal_max_khz))
        .unwrap_or(&policy.opps[0])
        .freq_khz
}

fn apply(policy: &Policy, state: &mut PolicyState, mut next: PolicyState) -> Result<(), &'static str> {
    if policy.driver.current_frequency_khz()? != state.target_khz {
        return Err("devfreq: hardware rate differs from policy target");
    }
    let frequency = target(policy, &next);
    if frequency != state.target_khz {
        let started = crate::time::current_time_ns();
        let result = policy.driver.set_frequency_khz(frequency);
        let completed = crate::time::current_time_ns();
        let elapsed = completed.saturating_sub(started);
        let transitions = &mut state.transitions;
        transitions.attempts = transitions.attempts.saturating_add(1);
        transitions.failures = transitions.failures.saturating_add(u64::from(result.is_err()));
        transitions.last_call_ns = elapsed;
        transitions.max_call_ns = transitions.max_call_ns.max(elapsed);
        transitions.total_call_ns = transitions.total_call_ns.saturating_add(elapsed);
        transitions.last_completed_ns = completed;
        transitions.last_from_khz = state.target_khz;
        transitions.last_to_khz = frequency;
        // Preserve diagnostic evidence even when the requested policy cannot
        // be committed. A failed call still leaves governor/target unchanged.
        next.transitions = *transitions;
        result?;
    }
    *state = PolicyState {
        target_khz: frequency,
        ..next
    };
    Ok(())
}

/// An actual device-work lifetime, separate from measured utilization.
///
/// Acquire before any driver execution/mapping/hardware locks. The automatic
/// governor raises a thermal-capped floor before the first command executes.
/// Drop only after DMA retirement is proven and those locks are released. A
/// driver that cannot prove retirement must retain the guard with its DMA pins.
pub struct DeviceFrequencyActivityGuard {
    policy: Arc<Policy>,
    adaptive: bool,
    work: Option<DeviceFrequencyWork>,
}

const ACTIVITY_IDLE_GRACE_NS: u64 = 100_000_000;

pub fn acquire_activity(name: &str) -> Result<DeviceFrequencyActivityGuard, &'static str> {
    acquire_work(name, None)
}

/// Learn an execution floor from real GPU work, frame cadence and queued jobs.
/// Missing measurements conservatively preserve the original cold-start boost.
pub fn acquire_adaptive_activity(
    name: &str,
    queued_jobs: usize,
) -> Result<DeviceFrequencyActivityGuard, &'static str> {
    acquire_work(name, Some(queued_jobs))
}

fn acquire_work(
    name: &str,
    queued_jobs: Option<usize>,
) -> Result<DeviceFrequencyActivityGuard, &'static str> {
    let policy = find(name)?;
    {
        let mut state = policy.state.lock();
        let mut work = state.work;
        let floor = match queued_jobs {
            Some(queued) => work.next(crate::time::current_time_ns(), queued, &policy.opps),
            None => policy.opps.last().unwrap().freq_khz,
        };
        let next = PolicyState {
            active_jobs: state
                .active_jobs
                .checked_add(1)
                .ok_or("devfreq: activity count overflow")?,
            activity_idle_until_ns: 0,
            down_samples: 0,
            activity_floor_khz: if state.active_jobs != 0 {
                state.activity_floor_khz.max(floor)
            } else {
                floor
            },
            legacy_jobs: state
                .legacy_jobs
                .checked_add(u32::from(queued_jobs.is_none()))
                .ok_or("devfreq: activity count overflow")?,
            work,
            ..*state
        };
        if target(&policy, &next) != state.target_khz {
            apply(&policy, &mut state, next)?;
        } else {
            *state = next;
        }
    }
    Ok(DeviceFrequencyActivityGuard {
        policy,
        adaptive: queued_jobs.is_some(),
        work: None,
    })
}

impl DeviceFrequencyActivityGuard {
    /// Record only successful execution after hardware locks have been released.
    /// Does not touch utilization telemetry or change clocks on the drop path.
    pub fn record_work(&mut self, work: DeviceFrequencyWork) {
        if self.adaptive
            && self
                .policy
                .opps
                .iter()
                .any(|opp| opp.freq_khz == work.frequency_khz)
        {
            self.work = Some(work);
        }
    }
}

impl Drop for DeviceFrequencyActivityGuard {
    fn drop(&mut self) {
        let mut state = self.policy.state.lock();
        debug_assert!(state.active_jobs != 0);
        if let Some(work) = self.work {
            state.work.record(work, &self.policy.opps);
        } else if self.adaptive {
            // A lost/reset/unsupported counter cannot justify a learned low
            // rate for the next job. Retry with the proven cold-start guard.
            state.work = WorkDemand::default();
        }
        if !self.adaptive {
            state.legacy_jobs -= 1;
        }
        state.active_jobs -= 1;
        if state.active_jobs == 0 {
            // Avoid clock changes between adjacent frames or queued jobs. The
            // existing worker releases the floor and downscales normally once
            // the grace expires; this path never takes a driver lock.
            state.activity_idle_until_ns =
                crate::time::current_time_ns().saturating_add(ACTIVITY_IDLE_GRACE_NS);
            state.down_samples = 0;
        }
    }
}

/// Register one clock domain after the driver has completed hardware bring-up.
/// OPPs are ascending and the current rate must be one of them.
pub fn register(
    name: &'static str,
    opps: &[DeviceFrequencyOpp],
    driver: Arc<dyn DeviceFrequencyDriver>,
) -> Result<(), &'static str> {
    if opps.iter().any(|opp| opp.min_uv == 0) {
        return Err("devfreq: invalid OPP voltage");
    }
    register_policy(name, opps, driver)
}

/// Register firmware-owned voltage/frequency pairs without inventing voltages.
/// The provider must select only validated firmware OPPs and wait for its ACK.
pub fn register_firmware_managed(
    name: &'static str,
    frequencies_khz: &[u64],
    driver: Arc<dyn DeviceFrequencyDriver>,
) -> Result<(), &'static str> {
    let opps: Vec<_> = frequencies_khz
        .iter()
        .map(|&freq_khz| DeviceFrequencyOpp {
            freq_khz,
            min_uv: 0,
        })
        .collect();
    register_policy(name, &opps, driver)
}

fn register_policy(
    name: &'static str,
    opps: &[DeviceFrequencyOpp],
    driver: Arc<dyn DeviceFrequencyDriver>,
) -> Result<(), &'static str> {
    if name.is_empty()
        || opps.is_empty()
        || opps.iter().any(|opp| opp.freq_khz == 0)
        || opps
            .windows(2)
            .any(|pair| pair[0].freq_khz >= pair[1].freq_khz)
    {
        return Err("devfreq: invalid OPP table");
    }
    let current = driver.current_frequency_khz()?;
    if !opps.iter().any(|opp| opp.freq_khz == current) {
        return Err("devfreq: current rate is outside OPP table");
    }
    let mut policies = POLICIES.lock();
    if policies.len() >= MAX_POLICIES {
        return Err("devfreq: policy worker limit reached");
    }
    if policies.iter().any(|policy| policy.name == name) {
        return Err("devfreq: device already registered");
    }
    let index = policies.len();
    policies.push(Arc::new(Policy {
        index,
        name,
        opps: opps.to_vec(),
        driver,
        state: Mutex::new(PolicyState {
            governor: DeviceFrequencyGovernor::Userspace,
            requested_khz: current,
            target_khz: current,
            thermal_max_khz: opps.last().unwrap().freq_khz,
            utilization_pct: None,
            sample_count: 0,
            failed_samples: 0,
            transitions: DeviceFrequencyTransitions::default(),
            down_samples: 0,
            ondemand: SimpleOndemandConfig::default(),
            active_jobs: 0,
            activity_idle_until_ns: 0,
            activity_floor_khz: current,
            legacy_jobs: 0,
            work: WorkDemand::default(),
        }),
        worker_started: AtomicBool::new(true),
    }));
    // Telemetry is useful under every governor. Previously the sampler only
    // started after selecting simple_ondemand, leaving manual/performance
    // policies permanently "unavailable" even when hardware counters exist.
    drop(policies);
    start_worker(index);
    Ok(())
}

const MAX_POLICIES: usize = 8;
const POLL_NS: u64 = 25_000_000;

fn poll(policy: &Policy) {
    // Keep the activity window fresh even while a manual/performance policy
    // is selected. Besides useful telemetry, this prevents finite-width
    // hardware counters from spanning minutes when automatic control resumes.
    let measurement = policy.driver.sample_utilization();
    let mut state = policy.state.lock();
    let measurement = match measurement {
        Ok(value) if value.total != 0 && value.busy <= value.total => value,
        _ => {
            state.failed_samples = state.failed_samples.saturating_add(1);
            state.down_samples = 0;
            return;
        }
    };
    let busy = u64::from(measurement.busy);
    let total = u64::from(measurement.total);
    state.utilization_pct = Some((busy * 100 / total) as u32);
    state.sample_count = state.sample_count.saturating_add(1);
    if state.governor != DeviceFrequencyGovernor::SimpleOndemand {
        return;
    }

    // Linux devfreq simple_ondemand, including per-domain thresholds.
    // Round the result upward to a supported OPP so demand is not clipped.
    let up = u64::from(state.ondemand.upthreshold_pct);
    let differential = u64::from(state.ondemand.downdifferential_pct);
    let current = state.target_khz;
    let requested = if busy * 100 > total * up {
        policy.opps.last().unwrap().freq_khz
    } else if busy * 100 > total * (up - differential) {
        current
    } else {
        let raw =
            busy.saturating_mul(current).saturating_mul(100) / total / (up - differential / 2);
        policy
            .opps
            .iter()
            .find(|opp| opp.freq_khz >= raw)
            .unwrap_or_else(|| policy.opps.last().unwrap())
            .freq_khz
    };
    let next = PolicyState {
        requested_khz: requested,
        ..*state
    };
    let next_target = target(policy, &next);
    if next_target < current {
        // A few low samples prevent a single quiet compositor frame from
        // causing an immediate down/up PLL transition and log storm.
        state.down_samples = state.down_samples.saturating_add(1);
        if state.down_samples < 4 {
            return;
        }
    } else {
        state.down_samples = 0;
    }
    if next_target != current && apply(policy, &mut state, next).is_err() {
        state.failed_samples = state.failed_samples.saturating_add(1);
    } else if next_target == current {
        state.requested_khz = requested;
    }
    state.down_samples = 0;
}

fn worker_entry<const INDEX: usize>() {
    loop {
        let policy = {
            let policies = POLICIES.lock();
            policies.get(INDEX).cloned()
        };
        if let Some(policy) = policy {
            poll(&policy);
        }
        if let Some(task) = crate::task::mytask() {
            task.sleep(task.get_trapframe(), POLL_NS);
        } else {
            crate::arch::instruction::idle();
        }
    }
}

fn start_worker(index: usize) {
    let entries: [fn(); MAX_POLICIES] = [
        worker_entry::<0>,
        worker_entry::<1>,
        worker_entry::<2>,
        worker_entry::<3>,
        worker_entry::<4>,
        worker_entry::<5>,
        worker_entry::<6>,
        worker_entry::<7>,
    ];
    let task = crate::task::new_kernel_task(format!("devfreq-{index}"), 1, entries[index]);
    task.init();
    crate::sched::scheduler::add_task(task, 0);
}

pub fn operating_points(name: &str) -> Result<Vec<DeviceFrequencyOpp>, &'static str> {
    Ok(find(name)?.opps.clone())
}

pub fn snapshots() -> Vec<DeviceFrequencySnapshot> {
    let policies = POLICIES.lock().clone();
    policies
        .iter()
        .map(|policy| {
            // Drivers may serialize live queries with device execution. Never
            // hold the policy mutex across that query: work retirement and new
            // demand must not queue behind a telemetry reader waiting for DMA.
            let state = *policy.state.lock();
            let current_khz = policy.driver.current_frequency_khz().ok();
            DeviceFrequencySnapshot {
                name: policy.name,
                governor: state.governor,
                min_khz: policy.opps[0].freq_khz,
                max_khz: policy.opps.last().unwrap().freq_khz,
                thermal_max_khz: state.thermal_max_khz,
                requested_khz: state.requested_khz,
                target_khz: state.target_khz,
                current_khz,
                utilization_pct: state.utilization_pct,
                sample_count: state.sample_count,
                failed_samples: state.failed_samples,
                transitions: state.transitions,
                ondemand: state.ondemand,
            }
        })
        .collect()
}

pub fn configure_simple_ondemand(
    name: &str,
    config: SimpleOndemandConfig,
) -> Result<(), &'static str> {
    if !(1..=100).contains(&config.upthreshold_pct)
        || config.downdifferential_pct >= config.upthreshold_pct
    {
        return Err("devfreq: invalid ondemand thresholds");
    }
    let policy = find(name)?;
    let mut state = policy.state.lock();
    state.ondemand = config;
    state.down_samples = 0;
    Ok(())
}

pub fn set_userspace_frequency(name: &str, freq_khz: u64) -> Result<(), &'static str> {
    let policy = find(name)?;
    if !policy.opps.iter().any(|opp| opp.freq_khz == freq_khz) {
        return Err("devfreq: rate is not an available OPP");
    }
    let mut state = policy.state.lock();
    let next = PolicyState {
        governor: DeviceFrequencyGovernor::Userspace,
        requested_khz: freq_khz,
        ..*state
    };
    apply(&policy, &mut state, next)
}

pub fn set_governor(name: &str, governor: DeviceFrequencyGovernor) -> Result<(), &'static str> {
    let policy = find(name)?;
    if governor == DeviceFrequencyGovernor::SimpleOndemand {
        let sample = policy.driver.sample_utilization()?;
        if sample.total == 0 || sample.busy > sample.total {
            return Err("devfreq: invalid utilization sample");
        }
    }
    let mut state = policy.state.lock();
    let requested = if governor == DeviceFrequencyGovernor::Userspace {
        state.target_khz
    } else {
        state.requested_khz
    };
    let next = PolicyState {
        governor,
        requested_khz: requested,
        down_samples: 0,
        ..*state
    };
    apply(&policy, &mut state, next)?;
    drop(state);
    if governor == DeviceFrequencyGovernor::SimpleOndemand
        && policy
            .worker_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        start_worker(policy.index);
    }
    Ok(())
}

/// Apply a kernel-owned thermal cap. The requested rate is retained and
/// restored when the cap is lifted. The cap rounds down to a supported OPP.
pub fn set_thermal_max_frequency(name: &str, max_khz: u64) -> Result<(), &'static str> {
    let policy = find(name)?;
    let Some(cap) = policy.opps.iter().rev().find(|opp| opp.freq_khz <= max_khz) else {
        return Err("devfreq: thermal cap is below minimum OPP");
    };
    let mut state = policy.state.lock();
    let next = PolicyState {
        thermal_max_khz: cap.freq_khz,
        ..*state
    };
    apply(&policy, &mut state, next)
}

#[cfg(test)]
mod activity_tests {
    extern crate std;
    use super::*;
    use core::sync::atomic::{AtomicU32, AtomicU64};

    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Driver {
        frequency: AtomicU64,
        busy: AtomicU32,
        reject: AtomicBool,
        check_policy_unlocked: AtomicBool,
    }

    impl DeviceFrequencyDriver for Driver {
        fn current_frequency_khz(&self) -> Result<u64, &'static str> {
            if self.check_policy_unlocked.load(Ordering::Relaxed) {
                assert!(
                    find("activity-test").unwrap().state.try_lock().is_some(),
                    "telemetry holds policy lock during driver query"
                );
            }
            Ok(self.frequency.load(Ordering::Relaxed))
        }
        fn set_frequency_khz(&self, frequency: u64) -> Result<(), &'static str> {
            if self.reject.load(Ordering::Relaxed) {
                return Err("test: rejected vote");
            }
            self.frequency.store(frequency, Ordering::Relaxed);
            Ok(())
        }
        fn sample_utilization(&self) -> Result<DeviceFrequencyUtilization, &'static str> {
            if self.check_policy_unlocked.load(Ordering::Relaxed) {
                assert!(
                    find("activity-test").unwrap().state.try_lock().is_some(),
                    "poll holds policy lock while sampling driver"
                );
            }
            Ok(DeviceFrequencyUtilization {
                busy: self.busy.load(Ordering::Relaxed),
                total: 100,
            })
        }
    }

    fn setup() -> (Arc<Policy>, Arc<Driver>) {
        let driver = Arc::new(Driver {
            frequency: AtomicU64::new(180_000),
            busy: AtomicU32::new(45),
            reject: AtomicBool::new(false),
            check_policy_unlocked: AtomicBool::new(false),
        });
        let policy = Arc::new(Policy {
            index: 0,
            name: "activity-test",
            opps: [180_000, 430_000, 800_000]
                .map(|freq_khz| DeviceFrequencyOpp {
                    freq_khz,
                    min_uv: 0,
                })
                .to_vec(),
            driver: driver.clone(),
            state: Mutex::new(PolicyState {
                governor: DeviceFrequencyGovernor::SimpleOndemand,
                requested_khz: 180_000,
                target_khz: 180_000,
                thermal_max_khz: 800_000,
                utilization_pct: None,
                sample_count: 0,
                failed_samples: 0,
                transitions: DeviceFrequencyTransitions::default(),
                down_samples: 0,
                ondemand: SimpleOndemandConfig {
                    upthreshold_pct: 50,
                    downdifferential_pct: 10,
                },
                active_jobs: 0,
                activity_idle_until_ns: 0,
                activity_floor_khz: 180_000,
                legacy_jobs: 0,
                work: WorkDemand::default(),
            }),
            worker_started: AtomicBool::new(true),
        });
        let mut policies = POLICIES.lock();
        policies.clear();
        policies.push(policy.clone());
        (policy, driver)
    }

    #[test]
    fn cold_work_boosts_before_sampling_without_faking_busy() {
        let _serial = SERIAL.lock().unwrap();
        let (policy, driver) = setup();
        poll(&policy);
        assert_eq!(
            driver.frequency.load(Ordering::Relaxed),
            180_000,
            "45% alone is sticky"
        );
        let guard = acquire_activity("activity-test").unwrap();
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 800_000);
        poll(&policy);
        assert_eq!(policy.state.lock().utilization_pct, Some(45));
        drop(guard);
    }

    #[test]
    fn retirement_grace_then_normal_four_low_samples_restore_idle_clock() {
        let _serial = SERIAL.lock().unwrap();
        let (policy, driver) = setup();
        let guard = acquire_activity("activity-test").unwrap();
        driver.busy.store(0, Ordering::Relaxed);
        drop(guard);
        let deadline = policy.state.lock().activity_idle_until_ns;
        assert!(deadline > crate::time::current_time_ns());
        assert!(deadline <= crate::time::current_time_ns() + ACTIVITY_IDLE_GRACE_NS);
        for _ in 0..4 {
            poll(&policy);
        }
        assert_eq!(
            driver.frequency.load(Ordering::Relaxed),
            800_000,
            "idle grace holds the floor"
        );
        policy.state.lock().activity_idle_until_ns = 0;
        for _ in 0..3 {
            poll(&policy);
        }
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 800_000);
        poll(&policy);
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 180_000);
        assert_eq!(policy.state.lock().utilization_pct, Some(0));
    }

    #[test]
    fn explicit_governors_and_thermal_cap_override_activity() {
        let _serial = SERIAL.lock().unwrap();
        let (_policy, driver) = setup();
        let guard = acquire_activity("activity-test").unwrap();
        set_thermal_max_frequency("activity-test", 430_000).unwrap();
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 430_000);
        set_governor("activity-test", DeviceFrequencyGovernor::Powersave).unwrap();
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 180_000);
        set_userspace_frequency("activity-test", 430_000).unwrap();
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 430_000);
        set_governor("activity-test", DeviceFrequencyGovernor::Performance).unwrap();
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 430_000);
        set_governor("activity-test", DeviceFrequencyGovernor::SimpleOndemand).unwrap();
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 430_000);
        drop(guard);
    }

    #[test]
    fn nested_jobs_start_grace_only_after_last_retirement() {
        let _serial = SERIAL.lock().unwrap();
        let (policy, _driver) = setup();
        let first = acquire_activity("activity-test").unwrap();
        let second = acquire_activity("activity-test").unwrap();
        drop(first);
        assert_eq!(policy.state.lock().active_jobs, 1);
        assert_eq!(policy.state.lock().activity_idle_until_ns, 0);
        drop(second);
        assert_eq!(policy.state.lock().active_jobs, 0);
        assert!(policy.state.lock().activity_idle_until_ns > 0);
    }

    #[test]
    fn rejected_boost_does_not_leak_activity_or_publish_a_rate() {
        let _serial = SERIAL.lock().unwrap();
        let (policy, driver) = setup();
        driver.reject.store(true, Ordering::Relaxed);
        assert!(acquire_activity("activity-test").is_err());
        assert_eq!(policy.state.lock().active_jobs, 0);
        assert_eq!(policy.state.lock().target_khz, 180_000);
        let failed = snapshots()[0].transitions;
        assert_eq!((failed.attempts, failed.failures), (1, 1));
        assert_eq!((failed.last_from_khz, failed.last_to_khz), (180_000, 800_000));
        driver.reject.store(false, Ordering::Relaxed);
        drop(acquire_activity("activity-test").unwrap());
        assert_eq!(policy.state.lock().active_jobs, 0);
        let mut state = policy.state.lock();
        assert_eq!((state.transitions.attempts, state.transitions.failures), (2, 1));
        let unchanged = *state;
        apply(&policy, &mut state, unchanged).unwrap();
        assert_eq!(state.transitions.attempts, 2, "unchanged rates make no driver call");
    }

    #[test]
    fn adaptive_light_frames_learn_supported_lower_opps() {
        let _serial = SERIAL.lock().unwrap();
        let (policy, _) = setup();
        let mut work = WorkDemand::default();
        let mut final_rate = 0;
        for frame in 0..60 {
            let start = 1_000_000_000 + frame * FRAME_NS;
            let rate = work.next(start, 0, &policy.opps);
            if frame == 0 {
                assert_eq!(rate, 800_000, "cold start remains protected");
            }
            // The same physical work: 0.9ms at800, 4ms at180.
            let busy_ns = 720_000_000_000u64.div_ceil(rate);
            work.record(
                DeviceFrequencyWork {
                    busy_ns,
                    frequency_khz: rate,
                    completed_ns: start + busy_ns,
                },
                &policy.opps,
            );
            final_rate = rate;
        }
        assert_eq!(
            final_rate, 180_000,
            "ordinary work must not remain permanently boosted"
        );
    }

    #[test]
    fn adaptive_adds_all_frame_passes_and_responds_to_backlog() {
        let _serial = SERIAL.lock().unwrap();
        let (policy, _) = setup();
        let mut work = WorkDemand {
            valid: true,
            floor_khz: 180_000,
            estimated_work: 800_000_000_000,
            job_work: 800_000_000_000,
            ..WorkDemand::default()
        };
        let start = 1_000_000_000;
        assert_eq!(work.next(start, 0, &policy.opps), 180_000);
        assert_eq!(
            work.next(start, 7, &policy.opps),
            800_000,
            "real queued GPU work must raise drain capacity"
        );
        let mut passes = WorkDemand {
            floor_khz: 180_000,
            ..WorkDemand::default()
        };
        for pass in 0..4 {
            passes.record(
                DeviceFrequencyWork {
                    busy_ns: 2_000_000,
                    frequency_khz: 800_000,
                    completed_ns: start + pass * 2_000_000,
                },
                &policy.opps,
            );
        }
        assert_eq!(
            passes.floor_khz, 800_000,
            "four2ms passes share one10ms GPU budget"
        );
        let mut tiny = WorkDemand {
            valid: true,
            floor_khz: 180_000,
            estimated_work: 10_000_000_000,
            job_work: 10_000_000_000,
            ..WorkDemand::default()
        };
        assert_eq!(
            tiny.next(start, 7, &policy.opps),
            180_000,
            "queue depth alone is not fake utilization"
        );
    }

    #[test]
    fn adaptive_work_is_opp_normalized_and_cpu_delay_is_not_gpu_cost() {
        let _serial = SERIAL.lock().unwrap();
        let (policy, _) = setup();
        let mut fast = WorkDemand::default();
        let mut slow = WorkDemand::default();
        fast.record(
            DeviceFrequencyWork {
                busy_ns: 900_000,
                frequency_khz: 800_000,
                completed_ns: 1_001_000_000,
            },
            &policy.opps,
        );
        slow.record(
            DeviceFrequencyWork {
                busy_ns: 4_000_000,
                frequency_khz: 180_000,
                completed_ns: 1_200_000_000,
            },
            &policy.opps,
        );
        assert_eq!(fast.estimated_work, slow.estimated_work);
        let mut cpu_only = WorkDemand::default();
        for frame in 0..30 {
            let now = 1_000_000_000 + frame * 100_000_000;
            cpu_only.next(now, 0, &policy.opps);
            cpu_only.record(
                DeviceFrequencyWork {
                    busy_ns: 0,
                    frequency_khz: 800_000,
                    completed_ns: now + 90_000_000,
                },
                &policy.opps,
            );
        }
        assert_eq!(cpu_only.estimated_work, 0);
        assert_eq!(
            cpu_only.floor_khz, 180_000,
            "90ms CPU delay must not demand max GPU clock"
        );
    }

    #[test]
    fn adaptive_heavy_work_raises_immediately_but_down_requires_sustained_windows() {
        let _serial = SERIAL.lock().unwrap();
        let (policy, _) = setup();
        let mut work = WorkDemand {
            valid: true,
            floor_khz: 180_000,
            ..WorkDemand::default()
        };
        let now = 1_000_000_000;
        work.record(
            DeviceFrequencyWork {
                busy_ns: 30_000_000,
                frequency_khz: 180_000,
                completed_ns: now,
            },
            &policy.opps,
        );
        assert_eq!(work.floor_khz, 800_000);
        work.record(
            DeviceFrequencyWork {
                busy_ns: 1_000,
                frequency_khz: 800_000,
                completed_ns: now + FRAME_NS,
            },
            &policy.opps,
        );
        assert_eq!(
            work.next(now + FRAME_NS * 2, 0, &policy.opps),
            800_000,
            "one light frame cannot downclock a burst"
        );
    }

    #[test]
    fn adaptive_guard_preserves_manual_thermal_and_true_sampler_semantics() {
        let _serial = SERIAL.lock().unwrap();
        let (policy, driver) = setup();
        policy.state.lock().work = WorkDemand {
            valid: true,
            floor_khz: 430_000,
            estimated_work: 3_000_000_000_000,
            job_work: 300_000_000_000,
            ..WorkDemand::default()
        };
        let mut guard = acquire_adaptive_activity("activity-test", 0).unwrap();
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 430_000);
        poll(&policy);
        assert_eq!(policy.state.lock().utilization_pct, Some(45));
        set_thermal_max_frequency("activity-test", 180_000).unwrap();
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 180_000);
        set_governor("activity-test", DeviceFrequencyGovernor::Powersave).unwrap();
        guard.record_work(DeviceFrequencyWork {
            busy_ns: 1_000_000,
            frequency_khz: 180_000,
            completed_ns: crate::time::current_time_ns(),
        });
        drop(guard);
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 180_000);
    }

    #[test]
    fn adaptive_missing_or_invalid_work_preserves_the_cold_guard_fallback() {
        let _serial = SERIAL.lock().unwrap();
        let (policy, driver) = setup();
        let mut guard = acquire_adaptive_activity("activity-test", 0).unwrap();
        guard.record_work(DeviceFrequencyWork {
            busy_ns: 1_000_000,
            frequency_khz: 123_456,
            completed_ns: crate::time::current_time_ns(),
        });
        drop(guard);
        assert!(!policy.state.lock().work.valid);
        drop(acquire_adaptive_activity("activity-test", 0).unwrap());
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 800_000);
    }

    #[test]
    fn adaptive_counter_loss_after_learning_restores_cold_start_protection() {
        let _serial = SERIAL.lock().unwrap();
        let (policy, driver) = setup();
        policy.state.lock().work = WorkDemand {
            valid: true,
            floor_khz: 180_000,
            estimated_work: 100_000_000_000,
            job_work: 100_000_000_000,
            ..WorkDemand::default()
        };
        let guard = acquire_adaptive_activity("activity-test", 0).unwrap();
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 180_000);
        drop(guard); // No valid execution measurement survived this job.
        assert!(!policy.state.lock().work.valid);
        drop(acquire_adaptive_activity("activity-test", 0).unwrap());
        assert_eq!(driver.frequency.load(Ordering::Relaxed), 800_000);
    }

    #[test]
    fn telemetry_and_sampling_release_policy_before_driver_queries() {
        let _serial = SERIAL.lock().unwrap();
        let (policy, driver) = setup();
        {
            let mut state = policy.state.lock();
            state.requested_khz = 800_000;
            state.thermal_max_khz = 180_000;
        }
        driver.check_policy_unlocked.store(true, Ordering::Relaxed);
        let snapshot = snapshots()[0];
        assert_eq!(snapshot.requested_khz, 800_000);
        assert_eq!(snapshot.thermal_max_khz, 180_000);
        assert_eq!(
            snapshot.current_khz,
            Some(180_000),
            "publish the ACKed rate, not the request"
        );
        // At45% the governor retains the capped rate; sampling itself must not
        // take the policy lock before calling the hardware counter provider.
        poll(&policy);
        assert_eq!(policy.state.lock().utilization_pct, Some(45));
    }

    #[test]
    fn stable_adaptive_floor_does_not_oscillate_with_legacy_high_busy_request() {
        let _serial = SERIAL.lock().unwrap();
        let (policy, driver) = setup();
        policy.state.lock().work = WorkDemand {
            valid: true,
            floor_khz: 430_000,
            estimated_work: 3_000_000_000_000,
            job_work: 300_000_000_000,
            ..WorkDemand::default()
        };
        driver.busy.store(60, Ordering::Relaxed);
        let mut guard = acquire_adaptive_activity("activity-test", 0).unwrap();
        for _ in 0..8 {
            poll(&policy);
            assert_eq!(policy.state.lock().requested_khz, 800_000);
            assert_eq!(driver.frequency.load(Ordering::Relaxed), 430_000);
            assert_eq!(policy.state.lock().utilization_pct, Some(60));
        }
        guard.record_work(DeviceFrequencyWork {
            busy_ns: 5_000_000,
            frequency_khz: 430_000,
            completed_ns: crate::time::current_time_ns(),
        });
        drop(guard);
        for _ in 0..4 {
            poll(&policy);
            assert_eq!(
                driver.frequency.load(Ordering::Relaxed),
                430_000,
                "grace also uses adaptive floor exclusively"
            );
        }
    }
}
