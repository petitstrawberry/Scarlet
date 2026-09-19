//! Kernel thermal zones and cooling-device policy.
//!
//! Hardware drivers supply temperatures and cooling states. This module owns
//! polling, hysteresis, fail-safe behavior and exclusive cooler ownership. Temperature values
//! are millidegrees Celsius, matching the device-tree thermal binding.

use alloc::{format, sync::Arc, vec::Vec};

use crate::sync::IrqSpinLock;

pub trait ThermalSensor: Send + Sync {
    fn name(&self) -> &'static str;
    fn read_millicelsius(&self) -> Result<i32, &'static str>;
}

pub trait CoolingDevice: Send + Sync {
    fn name(&self) -> &'static str;
    fn max_state(&self) -> u32;
    fn set_state(&self, state: u32) -> Result<(), &'static str>;
}

#[derive(Clone, Copy)]
pub struct ThermalTrip {
    pub temperature_mc: i32,
    pub hysteresis_mc: i32,
    pub cooling_state: u32,
}

/// A continuous cooling curve, with an optional low-pass filter on the
/// temperature passed to the curve. The sensor supplies a board-specific
/// estimate; the kernel owns filtering, hysteresis and PWM interpolation.
pub struct ContinuousCoolingCurve {
    pub points: Vec<CoolingPoint>,
    pub turn_on_mc: i32,
    pub turn_off_mc: i32,
    pub filter: ThermalFilter,
}

#[derive(Clone, Copy)]
pub struct CoolingPoint {
    pub temperature_mc: i32,
    pub cooling_state: u32,
}

#[derive(Clone, Copy)]
pub struct ThermalFilter {
    pub sta_gain: i64,
    pub sta_divisor: i64,
    pub iir_min: i64,
    pub iir_max: i64,
    pub iir_gain_divisor: i64,
    pub iir_power: u32,
    pub rising_width_mc: i64,
    pub falling_width_mc: i64,
}

pub struct CoolingPolicy {
    pub device: Arc<dyn CoolingDevice>,
    /// Hardware state already selected by the provider at registration.
    pub initial_state: u32,
    pub baseline_state: u32,
    pub fail_safe_state: u32,
    pub trips: Vec<ThermalTrip>,
    pub continuous: Option<ContinuousCoolingCurve>,
}

pub struct ThermalZoneRegistration {
    pub name: &'static str,
    pub sensors: Vec<Arc<dyn ThermalSensor>>,
    pub coolers: Vec<CoolingPolicy>,
}

#[derive(Clone, Copy)]
pub struct ThermalZoneSnapshot {
    pub name: &'static str,
    pub hottest_mc: Option<i32>,
    pub sample_count: u64,
    pub failed_samples: u64,
}

struct CoolerState {
    level: usize,
    applied_state: u32,
    filtered_mc: Option<i32>,
    sta_mc: i64,
    lta_mc: i64,
    fan_on: bool,
}

struct ZoneState {
    coolers: Vec<CoolerState>,
    hottest_mc: Option<i32>,
    sample_count: u64,
    failed_samples: u64,
}

struct ThermalZone {
    registration: ThermalZoneRegistration,
    state: IrqSpinLock<ZoneState>,
}

static ZONES: IrqSpinLock<Vec<Arc<ThermalZone>>> = IrqSpinLock::new(Vec::new());
const MAX_ZONES: usize = 8;

fn validate(registration: &ThermalZoneRegistration) -> Result<(), &'static str> {
    if registration.name.is_empty()
        || registration.sensors.is_empty()
        || registration.coolers.is_empty()
    {
        return Err("thermal zone requires sensors and cooling devices");
    }
    for policy in &registration.coolers {
        let max = policy.device.max_state();
        if policy.initial_state > max || policy.baseline_state > max || policy.fail_safe_state > max
        {
            return Err("thermal cooling state exceeds device maximum");
        }
        if let Some(curve) = &policy.continuous {
            let filter = curve.filter;
            if !policy.trips.is_empty()
                || curve.points.len() < 2
                || curve.turn_off_mc > curve.turn_on_mc
                || curve.points[0].cooling_state != policy.baseline_state
                || filter.sta_gain <= 0
                || filter.sta_gain > filter.sta_divisor
                || filter.iir_min <= 0
                || filter.iir_min > filter.iir_max
                || filter.iir_max > filter.iir_gain_divisor
                || filter.iir_power == 0
                || filter.rising_width_mc <= 0
                || filter.falling_width_mc <= 0
                || curve.points.windows(2).any(|pair| {
                    pair[0].temperature_mc >= pair[1].temperature_mc
                        || pair[0].cooling_state > pair[1].cooling_state
                })
                || curve.points.last().unwrap().cooling_state > max
                || policy.fail_safe_state < curve.points.last().unwrap().cooling_state
            {
                return Err("invalid continuous thermal cooling curve");
            }
            continue;
        }
        let mut previous_temp = i32::MIN;
        let mut previous_state = policy.baseline_state;
        for trip in &policy.trips {
            if trip.temperature_mc <= previous_temp
                || trip.hysteresis_mc < 0
                || trip.cooling_state < previous_state
                || trip.cooling_state > max
            {
                return Err("invalid thermal trip sequence");
            }
            previous_temp = trip.temperature_mc;
            previous_state = trip.cooling_state;
        }
        if policy.fail_safe_state < previous_state {
            return Err("thermal fail-safe state is weaker than hottest trip");
        }
    }
    Ok(())
}

/// Register a zone after its hardware providers are ready. The provider's
/// initial state is applied before the first synchronous sensor sample, and a
/// failed sample immediately selects the fail-safe state.
pub fn register_zone(registration: ThermalZoneRegistration) -> Result<(), &'static str> {
    validate(&registration)?;
    {
        let zones = ZONES.lock();
        if zones.len() >= MAX_ZONES {
            return Err("thermal zone worker limit reached");
        }
        if zones
            .iter()
            .any(|zone| zone.registration.name == registration.name)
        {
            return Err("thermal zone already registered");
        }
        for policy in &registration.coolers {
            if zones.iter().any(|zone| {
                zone.registration
                    .coolers
                    .iter()
                    .any(|existing| Arc::ptr_eq(&existing.device, &policy.device))
            }) {
                // Shared coolers require max-state arbitration between zones.
                return Err("thermal cooling device already owned by a zone");
            }
        }
    }
    for policy in &registration.coolers {
        policy.device.set_state(policy.initial_state)?;
    }
    let coolers = registration
        .coolers
        .iter()
        .map(|policy| CoolerState {
            level: 0,
            applied_state: policy.initial_state,
            filtered_mc: None,
            sta_mc: 0,
            lta_mc: 0,
            fan_on: false,
        })
        .collect();
    let zone = Arc::new(ThermalZone {
        registration,
        state: IrqSpinLock::new(ZoneState {
            coolers,
            hottest_mc: None,
            sample_count: 0,
            failed_samples: 0,
        }),
    });
    crate::println!(
        "thermal: zone {} registered; sampling sensors",
        zone.registration.name,
    );
    // The hardware provider is already ready. Apply the first real sample
    // before boot continues so a fail-safe fan does not run at full PWM
    // throughout unrelated GPU firmware and userspace initialization. A
    // running worker must not poll this zone concurrently with its first
    // sample, so publish it only after the synchronous poll has completed.
    poll(&zone)?;
    let index = {
        let mut zones = ZONES.lock();
        if zones.len() >= MAX_ZONES {
            return Err("thermal zone worker limit reached");
        }
        if zones
            .iter()
            .any(|existing| existing.registration.name == zone.registration.name)
        {
            return Err("thermal zone already registered");
        }
        zones.push(zone);
        zones.len() - 1
    };
    start_worker(index);
    Ok(())
}

pub fn snapshots() -> Vec<ThermalZoneSnapshot> {
    ZONES
        .lock()
        .iter()
        .map(|zone| {
            let state = zone.state.lock();
            ThermalZoneSnapshot {
                name: zone.registration.name,
                hottest_mc: state.hottest_mc,
                sample_count: state.sample_count,
                failed_samples: state.failed_samples,
            }
        })
        .collect()
}

fn target_level(policy: &CoolingPolicy, mut level: usize, temperature_mc: i32) -> usize {
    while level < policy.trips.len() && temperature_mc >= policy.trips[level].temperature_mc {
        level += 1;
    }
    while level > 0 {
        let trip = policy.trips[level - 1];
        if temperature_mc >= trip.temperature_mc.saturating_sub(trip.hysteresis_mc) {
            break;
        }
        level -= 1;
    }
    level
}

fn continuous_target(curve: &ContinuousCoolingCurve, state: &mut CoolerState, raw_mc: i32) -> u32 {
    let filter = curve.filter;
    let raw = i64::from(raw_mc);
    if state.filtered_mc.is_none() {
        state.sta_mc = raw;
        state.lta_mc = raw;
    } else {
        state.sta_mc += filter.sta_gain * (raw - state.sta_mc) / filter.sta_divisor;
        let delta = (state.sta_mc - state.lta_mc).abs();
        let width = if state.sta_mc > state.lta_mc {
            filter.rising_width_mc
        } else {
            filter.falling_width_mc
        };
        let gain = if delta >= width {
            filter.iir_max
        } else {
            let mut scaled = (filter.iir_max - filter.iir_min) * delta / width;
            for _ in 1..filter.iir_power {
                scaled = scaled * delta / width;
            }
            (filter.iir_min + scaled).min(filter.iir_max)
        };
        state.lta_mc += gain * (state.sta_mc - state.lta_mc) / filter.iir_gain_divisor;
    }
    let temperature = state.lta_mc;
    state.filtered_mc = Some(temperature as i32);
    if state.fan_on {
        if temperature < i64::from(curve.turn_off_mc) {
            state.fan_on = false;
        }
    } else if temperature > i64::from(curve.turn_on_mc) {
        state.fan_on = true;
    }
    if !state.fan_on {
        return curve.points[0].cooling_state;
    }
    for pair in curve.points.windows(2) {
        let low = pair[0];
        let high = pair[1];
        if temperature < i64::from(high.temperature_mc) {
            let numerator = (temperature - i64::from(low.temperature_mc)).max(0)
                * i64::from(high.cooling_state - low.cooling_state);
            let width = i64::from(high.temperature_mc - low.temperature_mc);
            return low.cooling_state + (numerator / width) as u32;
        }
    }
    curve.points.last().unwrap().cooling_state
}

fn poll(zone: &ThermalZone) -> Result<(), &'static str> {
    let mut hottest = i32::MIN;
    let mut failed = None;
    let mut readings = Vec::with_capacity(zone.registration.sensors.len());
    for sensor in &zone.registration.sensors {
        match sensor.read_millicelsius() {
            Ok(temperature) => {
                hottest = hottest.max(temperature);
                readings.push((sensor.name(), temperature));
            }
            Err(error) => {
                failed = Some((sensor.name(), error));
                break;
            }
        }
    }
    let (sample_count, failed_samples, requests) = {
        let mut state = zone.state.lock();
        state.sample_count += 1;
        state.hottest_mc = failed.is_none().then_some(hottest);
        if failed.is_some() {
            state.failed_samples += 1;
        }
        let requests = zone
            .registration
            .coolers
            .iter()
            .zip(&mut state.coolers)
            .map(|(policy, cooler)| {
                let level = if failed.is_some() || policy.continuous.is_some() {
                    cooler.level
                } else {
                    target_level(policy, cooler.level, hottest)
                };
                let requested = if failed.is_some() {
                    policy.fail_safe_state
                } else if let Some(curve) = &policy.continuous {
                    continuous_target(curve, cooler, hottest)
                } else if level == 0 {
                    policy.baseline_state
                } else {
                    policy.trips[level - 1].cooling_state
                };
                (level, requested, cooler.applied_state, cooler.filtered_mc)
            })
            .collect::<Vec<_>>();
        (state.sample_count, state.failed_samples, requests)
    };
    let mut apply_error = None;
    for (index, (level, requested, applied, filtered)) in requests.into_iter().enumerate() {
        let policy = &zone.registration.coolers[index];
        if requested == applied {
            zone.state.lock().coolers[index].level = level;
            continue;
        }
        // Hardware callbacks may perform I/O; never hold the zone spinlock
        // across them or across diagnostic output.
        match policy.device.set_state(requested) {
            Ok(()) => {
                let mut state = zone.state.lock();
                state.coolers[index].applied_state = requested;
                state.coolers[index].level = level;
                drop(state);
                if failed.is_some() {
                    crate::println!(
                        "thermal: {} {} state={} sensor-fault",
                        zone.registration.name,
                        policy.device.name(),
                        requested,
                    );
                } else {
                    crate::println!(
                        "thermal: {} {} state={} raw={}mC filtered={:?}mC",
                        zone.registration.name,
                        policy.device.name(),
                        requested,
                        hottest,
                        filtered,
                    );
                }
            }
            Err(error) => {
                apply_error.get_or_insert(error);
                if sample_count == 1 || sample_count % 30 == 0 {
                    crate::println!(
                        "thermal: {} {} state change failed: {}",
                        zone.registration.name,
                        policy.device.name(),
                        error
                    );
                }
            }
        }
    }
    if let Some((sensor, error)) = failed {
        if failed_samples == 1 || failed_samples % 30 == 0 {
            crate::println!(
                "thermal: {} sensor {} failed: {}",
                zone.registration.name,
                sensor,
                error
            );
        }
    } else if sample_count == 1
        || sample_count % 30 == 0
        || sample_count <= 600 && sample_count % 5 == 0
    {
        for (sensor, temperature) in readings {
            crate::println!(
                "thermal: {} {}={}mC",
                zone.registration.name,
                sensor,
                temperature,
            );
        }
    }
    apply_error.map_or(Ok(()), Err)
}

fn worker_entry<const INDEX: usize>() {
    loop {
        let zone = ZONES.lock().get(INDEX).cloned();
        if let Some(zone) = zone {
            let _ = poll(&zone);
        }
        if let Some(task) = crate::task::mytask() {
            task.sleep(task.get_trapframe(), 1_000_000_000);
        } else {
            crate::arch::instruction::idle();
        }
    }
}

fn start_worker(index: usize) {
    // A stalled GPU clock transition must never block the fan's temperature
    // sampling loop. Give each zone its own kernel task and fixed entrypoint.
    let entries: [fn(); MAX_ZONES] = [
        worker_entry::<0>,
        worker_entry::<1>,
        worker_entry::<2>,
        worker_entry::<3>,
        worker_entry::<4>,
        worker_entry::<5>,
        worker_entry::<6>,
        worker_entry::<7>,
    ];
    let task = crate::task::new_kernel_task(format!("thermal-{index}"), 1, entries[index]);
    task.init();
    crate::sched::scheduler::add_task(task, 0);
}
