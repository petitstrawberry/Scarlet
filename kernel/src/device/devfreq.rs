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
    /// Minimum safe rail voltage in microvolts for this policy's implementation.
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
    down_samples: u8,
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

fn apply(policy: &Policy, state: &mut PolicyState, next: PolicyState) -> Result<(), &'static str> {
    if policy.driver.current_frequency_khz()? != state.target_khz {
        return Err("devfreq: hardware rate differs from policy target");
    }
    let frequency = target(policy, &next);
    if frequency != state.target_khz {
        policy.driver.set_frequency_khz(frequency)?;
    }
    *state = PolicyState {
        target_khz: frequency,
        ..next
    };
    Ok(())
}

/// Register one clock domain after the driver has completed hardware bring-up.
/// OPPs are ascending and the current rate must be one of them.
pub fn register(
    name: &'static str,
    opps: &[DeviceFrequencyOpp],
    driver: Arc<dyn DeviceFrequencyDriver>,
) -> Result<(), &'static str> {
    if name.is_empty()
        || opps.is_empty()
        || opps.iter().any(|opp| opp.freq_khz == 0 || opp.min_uv == 0)
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
            down_samples: 0,
        }),
        worker_started: AtomicBool::new(false),
    }));
    Ok(())
}

const MAX_POLICIES: usize = 8;
const POLL_NS: u64 = 25_000_000;

fn poll(policy: &Policy) {
    if policy.state.lock().governor != DeviceFrequencyGovernor::SimpleOndemand {
        return;
    }
    let measurement = policy.driver.sample_utilization();
    let mut state = policy.state.lock();
    if state.governor != DeviceFrequencyGovernor::SimpleOndemand {
        return;
    }
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

    // Linux devfreq simple_ondemand: 90% jumps to the top OPP, 85-90%
    // retains the current OPP, otherwise aim at busy/total * current / 87.5%.
    // Round the result upward to a supported OPP so demand is not clipped.
    let current = state.target_khz;
    let requested = if busy * 100 > total * 90 {
        policy.opps.last().unwrap().freq_khz
    } else if busy * 100 > total * 85 {
        current
    } else {
        let raw = busy.saturating_mul(current).saturating_mul(100) / total / 88;
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
            let state = policy.state.lock();
            DeviceFrequencySnapshot {
                name: policy.name,
                governor: state.governor,
                min_khz: policy.opps[0].freq_khz,
                max_khz: policy.opps.last().unwrap().freq_khz,
                thermal_max_khz: state.thermal_max_khz,
                requested_khz: state.requested_khz,
                target_khz: state.target_khz,
                current_khz: policy.driver.current_frequency_khz().ok(),
                utilization_pct: state.utilization_pct,
                sample_count: state.sample_count,
                failed_samples: state.failed_samples,
            }
        })
        .collect()
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
