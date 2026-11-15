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
    down_samples: u8,
    ondemand: SimpleOndemandConfig,
    active_jobs: u32,
    activity_idle_until_ns: u64,
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
            state.thermal_max_khz
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

/// An actual device-work lifetime, separate from measured utilization.
///
/// Acquire before any driver execution/mapping/hardware locks. The automatic
/// governor raises a thermal-capped floor before the first command executes.
/// Drop only after DMA retirement is proven and those locks are released. A
/// driver that cannot prove retirement must retain the guard with its DMA pins.
pub struct DeviceFrequencyActivityGuard {
    policy: Arc<Policy>,
}

const ACTIVITY_IDLE_GRACE_NS: u64 = 100_000_000;

pub fn acquire_activity(name: &str) -> Result<DeviceFrequencyActivityGuard, &'static str> {
    let policy = find(name)?;
    {
        let mut state = policy.state.lock();
        let next = PolicyState {
            active_jobs: state
                .active_jobs
                .checked_add(1)
                .ok_or("devfreq: activity count overflow")?,
            activity_idle_until_ns: 0,
            down_samples: 0,
            ..*state
        };
        if target(&policy, &next) != state.target_khz {
            apply(&policy, &mut state, next)?;
        } else {
            *state = next;
        }
    }
    Ok(DeviceFrequencyActivityGuard { policy })
}

impl Drop for DeviceFrequencyActivityGuard {
    fn drop(&mut self) {
        let mut state = self.policy.state.lock();
        debug_assert!(state.active_jobs != 0);
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
            down_samples: 0,
            ondemand: SimpleOndemandConfig::default(),
            active_jobs: 0,
            activity_idle_until_ns: 0,
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
    }

    impl DeviceFrequencyDriver for Driver {
        fn current_frequency_khz(&self) -> Result<u64, &'static str> {
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
                down_samples: 0,
                ondemand: SimpleOndemandConfig {
                    upthreshold_pct: 50,
                    downdifferential_pct: 10,
                },
                active_jobs: 0,
                activity_idle_until_ns: 0,
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
        driver.reject.store(false, Ordering::Relaxed);
        drop(acquire_activity("activity-test").unwrap());
        assert_eq!(policy.state.lock().active_jobs, 0);
    }
}
