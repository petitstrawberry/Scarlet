//! Native memory-pressure accounting for reclaim retries and scheduler state.
//!
//! [`begin_reclaim`] is called only after a typed allocation failure and before
//! bounded clean-page reclaim plus one retry. Its fixed-capacity interval store
//! allocates no heap memory. Concurrent reclaim intervals are unioned for
//! `some`, whose averages use integer basis points.
//!
//! `full` is not inferred from `some`. Linux PSI defines full as memory-stalled
//! non-idle tasks with no runnable task left to advance. Scheduler integration
//! must call [`scheduler_full_state_changed`] after every ready-queue enqueue or
//! dequeue, current-task switch, wake, and reclaim begin/end. It computes
//! `all_non_idle_stalled = active_reclaim_tasks != 0 && ready_tasks == 0 &&`
//! every non-idle current task is in the active-ID snapshot. Membership is
//! published with [`try_reclaim_tasks_snapshot`] and an epoch; the scheduler
//! rejects a computed result if that epoch changed before publication. That
//! transition opens/closes a full interval without allocation or periodic
//! sampling. PSI averages follow the primary kernel's 2-second fixed-point
//! `calc_load` recurrence. Catch-up is bounded to the retained 300-second
//! history; cumulative totals remain exact across that bound.

use alloc::sync::{Arc, Weak};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::environment::MAX_NUM_CPUS;
use crate::sync::{IrqSpinLock, Waker};

const INTERVAL_CAPACITY: usize = 1024;
const MAX_TRIGGERS: usize = 16;
const NEVER: u64 = u64::MAX;
const PSI_AVG_PERIOD_NS: u64 = 2_000_000_000;
const MAX_AVG_CATCHUP_PERIODS: u64 = 150;
const FSHIFT: u32 = 11;
const FIXED_1: u64 = 1 << FSHIFT;
const EXP_10S: u64 = 1677;
const EXP_60S: u64 = 1981;
const EXP_300S: u64 = 2034;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PressureClass {
    Some,
    Full,
}

#[derive(Clone, Copy)]
struct Interval {
    start_ns: u64,
    end_ns: u64,
}

impl Interval {
    const EMPTY: Self = Self {
        start_ns: 0,
        end_ns: 0,
    };

    fn overlap_ns(self, start_ns: u64, end_ns: u64) -> u64 {
        self.end_ns
            .min(end_ns)
            .saturating_sub(self.start_ns.max(start_ns))
    }
}

fn fixed_power_int(mut exp: u64, mut exponent: u64) -> u64 {
    let mut result = FIXED_1;
    while exponent != 0 {
        if exponent & 1 != 0 {
            result = (result.saturating_mul(exp) + FIXED_1 / 2) >> FSHIFT;
        }
        exp = (exp.saturating_mul(exp) + FIXED_1 / 2) >> FSHIFT;
        exponent >>= 1;
    }
    result
}

fn calc_load(load: u64, exp: u64, active: u64) -> u64 {
    let decayed = load.saturating_mul(exp);
    let fresh = active.saturating_mul(FIXED_1.saturating_sub(exp));
    (decayed.saturating_add(fresh).saturating_add(FIXED_1 / 2)) >> FSHIFT
}

fn calc_load_n(load: u64, exp: u64, active: u64, periods: u64) -> u64 {
    calc_load(load, fixed_power_int(exp, periods), active)
}

struct History {
    intervals: [Interval; INTERVAL_CAPACITY],
    start: usize,
    len: usize,
    total_ns: u64,
    avg_last_update_ns: u64,
    avg10_fixed: u64,
    avg60_fixed: u64,
    avg300_fixed: u64,
}

impl History {
    const fn new() -> Self {
        Self {
            intervals: [Interval::EMPTY; INTERVAL_CAPACITY],
            start: 0,
            len: 0,
            total_ns: 0,
            avg_last_update_ns: 0,
            avg10_fixed: 0,
            avg60_fixed: 0,
            avg300_fixed: 0,
        }
    }

    fn push(&mut self, interval: Interval) {
        if interval.end_ns <= interval.start_ns {
            return;
        }
        self.total_ns = self
            .total_ns
            .saturating_add(interval.end_ns - interval.start_ns);
        let slot = if self.len < INTERVAL_CAPACITY {
            let slot = (self.start + self.len) % INTERVAL_CAPACITY;
            self.len += 1;
            slot
        } else {
            let slot = self.start;
            self.start = (self.start + 1) % INTERVAL_CAPACITY;
            slot
        };
        self.intervals[slot] = interval;
    }

    fn covered_ns(&self, now_ns: u64, window_ns: u64, since_ns: u64) -> u64 {
        let window_start = now_ns.saturating_sub(window_ns).max(since_ns);
        if now_ns <= window_start {
            return 0;
        }
        let mut covered = 0u64;
        for offset in 0..self.len {
            let interval = self.intervals[(self.start + offset) % INTERVAL_CAPACITY];
            covered = covered.saturating_add(interval.overlap_ns(window_start, now_ns));
        }
        covered
    }

    fn apply_sample(&mut self, stalled_ns: u64, period_ns: u64) {
        let percent = stalled_ns.min(period_ns).saturating_mul(100) / period_ns.max(1);
        let active = percent.min(100).saturating_mul(FIXED_1);
        self.avg10_fixed = calc_load(self.avg10_fixed, EXP_10S, active);
        self.avg60_fixed = calc_load(self.avg60_fixed, EXP_60S, active);
        self.avg300_fixed = calc_load(self.avg300_fixed, EXP_300S, active);
    }

    fn apply_zero_periods(&mut self, periods: u64) {
        if periods == 0 {
            return;
        }
        self.avg10_fixed = calc_load_n(self.avg10_fixed, EXP_10S, 0, periods);
        self.avg60_fixed = calc_load_n(self.avg60_fixed, EXP_60S, 0, periods);
        self.avg300_fixed = calc_load_n(self.avg300_fixed, EXP_300S, 0, periods);
    }
}

struct PressureState {
    some: History,
    full: History,
    active_count: usize,
    union_start_ns: Option<u64>,
    active_tasks: [ActiveTask; MAX_NUM_CPUS],
    active_task_count: usize,
    active_epoch: u64,
    active_membership_unknown: bool,
    active_membership_overflow: bool,
    full_available: bool,
    last_full_revision: u64,
    full_stalled: bool,
    full_start_ns: Option<u64>,
    triggers: [Option<Weak<MemoryPressureTrigger>>; MAX_TRIGGERS],
}

#[derive(Clone, Copy)]
struct ActiveTask {
    id: usize,
    depth: usize,
}

impl ActiveTask {
    const EMPTY: Self = Self { id: 0, depth: 0 };
}

impl PressureState {
    const fn new() -> Self {
        Self {
            some: History::new(),
            full: History::new(),
            active_count: 0,
            union_start_ns: None,
            active_tasks: [ActiveTask::EMPTY; MAX_NUM_CPUS],
            active_task_count: 0,
            active_epoch: 0,
            active_membership_unknown: false,
            active_membership_overflow: false,
            full_available: false,
            last_full_revision: 0,
            full_stalled: false,
            full_start_ns: None,
            triggers: [const { None }; MAX_TRIGGERS],
        }
    }

    fn begin_some(&mut self, task_id: usize, started_ns: u64) {
        self.active_epoch = self.active_epoch.saturating_add(1);
        if self.active_count == 0 {
            self.union_start_ns = Some(started_ns);
        }
        self.active_count = self.active_count.saturating_add(1);
        if task_id == 0 {
            self.active_membership_unknown = true;
        } else if let Some(task) = self.active_tasks[..self.active_task_count]
            .iter_mut()
            .find(|task| task.id == task_id)
        {
            task.depth = task.depth.saturating_add(1);
        } else if self.active_task_count < MAX_NUM_CPUS {
            self.active_tasks[self.active_task_count] = ActiveTask {
                id: task_id,
                depth: 1,
            };
            self.active_task_count += 1;
        } else {
            self.active_membership_overflow = true;
        }
    }

    fn finish_some(&mut self, task_id: usize, finished_ns: Option<u64>) -> bool {
        if self.active_count == 0 {
            return true;
        }
        self.active_epoch = self.active_epoch.saturating_add(1);
        self.active_count -= 1;
        if let Some(index) = self.active_tasks[..self.active_task_count]
            .iter()
            .position(|active| active.id == task_id)
        {
            if self.active_tasks[index].depth > 1 {
                self.active_tasks[index].depth -= 1;
            } else {
                self.active_task_count -= 1;
                self.active_tasks[index] = self.active_tasks[self.active_task_count];
                self.active_tasks[self.active_task_count] = ActiveTask::EMPTY;
            }
        }
        if self.active_count != 0 {
            return false;
        }
        self.active_membership_unknown = false;
        self.active_membership_overflow = false;
        let Some(started_ns) = self.union_start_ns.take() else {
            return true;
        };
        if let Some(finished_ns) = finished_ns {
            self.some.push(Interval {
                start_ns: started_ns,
                end_ns: finished_ns,
            });
        }
        true
    }

    fn set_full(&mut self, all_non_idle_stalled: bool, now_ns: u64) {
        self.full_available = true;
        if all_non_idle_stalled == self.full_stalled {
            return;
        }
        self.full_stalled = all_non_idle_stalled;
        if all_non_idle_stalled {
            self.full_start_ns = Some(now_ns);
        } else if let Some(started_ns) = self.full_start_ns.take() {
            self.full.push(Interval {
                start_ns: started_ns,
                end_ns: now_ns,
            });
        }
    }

    fn accept_full_revision(
        &mut self,
        revision: u64,
        all_non_idle_stalled: bool,
        expected_epoch: Option<u64>,
        scheduler_snapshot_valid: bool,
        now_ns: u64,
    ) -> bool {
        if revision <= self.last_full_revision {
            return false;
        }
        self.last_full_revision = revision;
        let valid_snapshot = !all_non_idle_stalled
            || (expected_epoch == Some(self.active_epoch) && scheduler_snapshot_valid);
        self.set_full(all_non_idle_stalled && valid_snapshot, now_ns);
        true
    }

    fn membership_snapshot(&self) -> Option<(u64, [usize; MAX_NUM_CPUS], usize)> {
        if self.active_membership_unknown || self.active_membership_overflow {
            return None;
        }
        let mut tasks = [0usize; MAX_NUM_CPUS];
        for (index, task) in self.active_tasks[..self.active_task_count]
            .iter()
            .enumerate()
        {
            tasks[index] = task.id;
        }
        Some((self.active_epoch, tasks, self.active_task_count))
    }

    fn register(&mut self, trigger: &Arc<MemoryPressureTrigger>) -> bool {
        let mut free_slot = None;
        for (index, slot) in self.triggers.iter_mut().enumerate() {
            match slot {
                Some(existing) if existing.strong_count() != 0 => {}
                Some(_) => {
                    *slot = Some(Arc::downgrade(trigger));
                    return true;
                }
                None if free_slot.is_none() => free_slot = Some(index),
                None => {}
            }
        }
        if let Some(index) = free_slot {
            self.triggers[index] = Some(Arc::downgrade(trigger));
            true
        } else {
            false
        }
    }

    fn history(&self, class: PressureClass) -> &History {
        match class {
            PressureClass::Some => &self.some,
            PressureClass::Full => &self.full,
        }
    }

    fn history_mut(&mut self, class: PressureClass) -> &mut History {
        match class {
            PressureClass::Some => &mut self.some,
            PressureClass::Full => &mut self.full,
        }
    }

    fn advance_averages(&mut self, now_ns: u64) {
        self.advance_average_class(PressureClass::Some, now_ns);
        self.advance_average_class(PressureClass::Full, now_ns);
    }

    fn advance_average_class(&mut self, class: PressureClass, now_ns: u64) {
        let last_update_ns = self.history(class).avg_last_update_ns;
        if last_update_ns == 0 {
            self.history_mut(class).avg_last_update_ns = now_ns;
            return;
        }
        let periods = now_ns.saturating_sub(last_update_ns) / PSI_AVG_PERIOD_NS;
        if periods == 0 {
            return;
        }

        let skipped = periods.saturating_sub(MAX_AVG_CATCHUP_PERIODS);
        let mut sample_start_ns =
            last_update_ns.saturating_add(skipped.saturating_mul(PSI_AVG_PERIOD_NS));
        self.history_mut(class).apply_zero_periods(skipped);
        for _ in skipped..periods {
            let sample_end_ns = sample_start_ns.saturating_add(PSI_AVG_PERIOD_NS);
            let stalled_ns =
                self.covered_ns(class, sample_end_ns, PSI_AVG_PERIOD_NS, sample_start_ns);
            self.history_mut(class)
                .apply_sample(stalled_ns, PSI_AVG_PERIOD_NS);
            sample_start_ns = sample_end_ns;
        }
        self.history_mut(class).avg_last_update_ns = sample_start_ns;
    }

    fn covered_ns(&self, class: PressureClass, now_ns: u64, window_ns: u64, since_ns: u64) -> u64 {
        let mut covered = self.history(class).covered_ns(now_ns, window_ns, since_ns);
        let active = match class {
            PressureClass::Some if self.active_count != 0 => self.union_start_ns,
            PressureClass::Full if self.full_stalled => self.full_start_ns,
            _ => None,
        };
        if let Some(started_ns) = active {
            covered = covered.saturating_add(
                Interval {
                    start_ns: started_ns,
                    end_ns: now_ns,
                }
                .overlap_ns(now_ns.saturating_sub(window_ns).max(since_ns), now_ns),
            );
        }
        covered
    }

    fn live_total_ns(&self, class: PressureClass, now_ns: u64) -> u64 {
        let active_start = match class {
            PressureClass::Some if self.active_count != 0 => self.union_start_ns,
            PressureClass::Full if self.full_stalled => self.full_start_ns,
            _ => None,
        };
        self.history(class)
            .total_ns
            .saturating_add(active_start.map_or(0, |started_ns| now_ns.saturating_sub(started_ns)))
    }

    fn evaluate_triggers(
        &self,
        class: PressureClass,
        now_ns: u64,
        awakened: &mut [Option<Arc<MemoryPressureTrigger>>; MAX_TRIGGERS],
    ) {
        for index in 0..MAX_TRIGGERS {
            let Some(trigger) = self.triggers[index].as_ref().and_then(Weak::upgrade) else {
                continue;
            };
            if trigger.class == class && trigger.observe(self, now_ns) {
                awakened[index] = Some(trigger);
            }
        }
    }
}

static PRESSURE: IrqSpinLock<PressureState> = IrqSpinLock::new(PressureState::new());
static RECLAIM_ACTIVE: AtomicBool = AtomicBool::new(false);

#[must_use = "dropping the guard ends the measured reclaim interval"]
pub(crate) struct ReclaimStallGuard {
    started_ns: Option<u64>,
    task_id: usize,
}

pub(crate) fn begin_reclaim() -> ReclaimStallGuard {
    let scheduler_change = crate::sched::scheduler::memory_pressure_state_change_begin();
    let task_id =
        crate::sched::scheduler::current_task_id(crate::arch::get_cpu().get_cpuid()).unwrap_or(0);
    let mut pressure = PRESSURE.lock();
    let started_ns = crate::timer::diagnostic_time_ns();
    if let Some(started_ns) = started_ns {
        pressure.advance_averages(started_ns);
        pressure.begin_some(task_id, started_ns);
        RECLAIM_ACTIVE.store(true, Ordering::Release);
    }
    drop(pressure);
    drop(scheduler_change);
    ReclaimStallGuard {
        started_ns,
        task_id,
    }
}

impl Drop for ReclaimStallGuard {
    fn drop(&mut self) {
        if self.started_ns.take().is_none() {
            return;
        }
        let scheduler_change = crate::sched::scheduler::memory_pressure_state_change_begin();
        let mut awakened = [const { None }; MAX_TRIGGERS];
        {
            let mut pressure = PRESSURE.lock();
            let finished_ns = crate::timer::diagnostic_time_ns();
            if pressure.finish_some(self.task_id, finished_ns) {
                RECLAIM_ACTIVE.store(false, Ordering::Release);
            }
            if let Some(finished_ns) = finished_ns {
                pressure.advance_averages(finished_ns);
                pressure.evaluate_triggers(PressureClass::Some, finished_ns, &mut awakened);
            }
        }
        drop(scheduler_change);
        for trigger in awakened.into_iter().flatten() {
            trigger.notify();
        }
    }
}

/// Scheduler event hook. Call after releasing scheduler queue locks whenever a
/// ready/current task or reclaim membership changes. `all_non_idle_stalled`
/// must come from actual scheduler state; pass false initially to advertise the
/// producer before a stall exists.
pub(crate) fn scheduler_full_state_changed(
    revision: u64,
    all_non_idle_stalled: bool,
    expected_epoch: Option<u64>,
) {
    let mut awakened = [const { None }; MAX_TRIGGERS];
    {
        let mut pressure = PRESSURE.lock();
        let now_ns = crate::timer::diagnostic_time_ns().unwrap_or(0);
        let previously_stalled = pressure.full_stalled;
        let scheduler_snapshot_valid = !all_non_idle_stalled
            || crate::sched::scheduler::memory_pressure_snapshot_valid(revision);
        if !pressure.accept_full_revision(
            revision,
            all_non_idle_stalled,
            expected_epoch,
            scheduler_snapshot_valid,
            now_ns,
        ) {
            return;
        }
        pressure.advance_averages(now_ns);
        if previously_stalled && !pressure.full_stalled {
            pressure.evaluate_triggers(PressureClass::Full, now_ns, &mut awakened);
        }
    }
    for trigger in awakened.into_iter().flatten() {
        trigger.notify();
    }
}

/// Single fixed atomic load for the scheduler's inactive fast path.
#[inline]
pub(crate) fn reclaim_active_fast() -> bool {
    RECLAIM_ACTIVE.load(Ordering::Acquire)
}

/// Return the complete active-reclaim task set without allocating or waiting.
/// `None` means a concurrent update, an unknown task ID, or capacity overflow;
/// the scheduler must publish conservative `full = false` for that sample.
pub(crate) fn try_reclaim_tasks_snapshot() -> Option<(u64, [usize; MAX_NUM_CPUS], usize)> {
    let pressure = PRESSURE.try_lock()?;
    pressure.membership_snapshot()
}

pub(crate) fn full_accounting_available() -> bool {
    PRESSURE.lock().full_available
}

pub(crate) struct MemoryPressureTrigger {
    class: PressureClass,
    threshold_ns: u64,
    window_ns: u64,
    activated_ns: u64,
    last_notified_ns: AtomicU64,
    event_ready: AtomicBool,
    waker: Arc<Waker>,
}

impl MemoryPressureTrigger {
    pub(crate) fn arm(
        class: PressureClass,
        threshold_us: u64,
        window_us: u64,
    ) -> Option<Arc<Self>> {
        if threshold_us == 0 || window_us == 0 || threshold_us > window_us {
            return None;
        }
        if class == PressureClass::Full && !full_accounting_available() {
            return None;
        }
        let activated_ns = crate::timer::diagnostic_time_ns()?;
        let trigger = Arc::new(Self {
            class,
            threshold_ns: threshold_us.saturating_mul(1_000),
            window_ns: window_us.saturating_mul(1_000),
            activated_ns,
            last_notified_ns: AtomicU64::new(NEVER),
            event_ready: AtomicBool::new(false),
            waker: Arc::new(Waker::new_interruptible("memory-pressure-trigger")),
        });
        PRESSURE.lock().register(&trigger).then_some(trigger)
    }

    fn observe(&self, pressure: &PressureState, now_ns: u64) -> bool {
        let covered_ns = pressure.covered_ns(self.class, now_ns, self.window_ns, self.activated_ns);
        if covered_ns < self.threshold_ns {
            return false;
        }
        let last = self.last_notified_ns.load(Ordering::Acquire);
        if last != NEVER && now_ns.saturating_sub(last) < self.window_ns {
            return false;
        }
        if self
            .last_notified_ns
            .compare_exchange(last, now_ns, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        self.event_ready.store(true, Ordering::Release);
        true
    }

    pub(crate) fn take_event(&self) -> bool {
        self.event_ready.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn has_event(&self) -> bool {
        self.event_ready.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn test_trigger() -> Arc<Self> {
        Arc::new(Self {
            class: PressureClass::Some,
            threshold_ns: 70_000_000,
            window_ns: 1_000_000_000,
            activated_ns: 0,
            last_notified_ns: AtomicU64::new(NEVER),
            event_ready: AtomicBool::new(false),
            waker: Arc::new(Waker::new_interruptible("memory-pressure-test")),
        })
    }

    #[cfg(test)]
    pub(crate) fn raise_event_for_test(&self) {
        self.event_ready.store(true, Ordering::Release);
    }

    pub(crate) fn subscribe_readiness(
        &self,
        waiter: &Arc<Waker>,
    ) -> crate::sync::ReadinessRegistration {
        self.waker.subscribe_readiness(waiter)
    }

    pub(crate) fn wait_waker(&self) -> Arc<Waker> {
        self.waker.clone()
    }

    fn notify(&self) {
        // Waker owns readiness fan-out. Its current implementation allocates
        // temporary vectors; an allocation-free broadcast is still required for
        // a strict no-heap guarantee on trigger publication.
        self.waker.wake_all();
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PressureStats {
    pub(crate) avg10_basis_points: u16,
    pub(crate) avg60_basis_points: u16,
    pub(crate) avg300_basis_points: u16,
    pub(crate) total_us: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct PressureSnapshot {
    pub(crate) some: PressureStats,
    pub(crate) full: PressureStats,
}

fn fixed_to_basis_points(load: u64) -> u16 {
    load.saturating_mul(100)
        .saturating_add(FIXED_1 / 2)
        .checked_div(FIXED_1)
        .unwrap_or(0)
        .min(10_000) as u16
}

fn stats(pressure: &PressureState, class: PressureClass, now_ns: u64) -> PressureStats {
    let history = pressure.history(class);
    PressureStats {
        avg10_basis_points: fixed_to_basis_points(history.avg10_fixed),
        avg60_basis_points: fixed_to_basis_points(history.avg60_fixed),
        avg300_basis_points: fixed_to_basis_points(history.avg300_fixed),
        total_us: pressure.live_total_ns(class, now_ns) / 1_000,
    }
}

pub(crate) fn snapshot() -> PressureSnapshot {
    let now_ns = crate::timer::diagnostic_time_ns().unwrap_or(0);
    let mut pressure = PRESSURE.lock();
    pressure.advance_averages(now_ns);
    PressureSnapshot {
        some: stats(&pressure, PressureClass::Some, now_ns),
        full: stats(&pressure, PressureClass::Full, now_ns),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeMemoryTelemetry {
    pub failed_requests: u64,
    pub failed_requested_pages: u64,
    pub total_pages: usize,
    pub free_pages: usize,
}

pub fn native_telemetry() -> NativeMemoryTelemetry {
    let (failed_requests, failed_requested_pages) = crate::mem::pmm::allocation_failure_totals();
    let (total_pages, free_pages) = crate::mem::pmm::page_counts();
    NativeMemoryTelemetry {
        failed_requests,
        failed_requested_pages,
        total_pages,
        free_pages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn concurrent_some_intervals_are_unioned() {
        let mut state = PressureState::new();
        let epoch = state.active_epoch;
        state.begin_some(1, 100);
        assert!(state.active_epoch > epoch);
        let epoch = state.active_epoch;
        state.begin_some(2, 200);
        assert!(state.active_epoch > epoch);
        state.finish_some(1, Some(300));
        assert_eq!(state.some.total_ns, 0);
        let (_, tasks, count) = state.membership_snapshot().unwrap();
        assert_eq!(&tasks[..count], &[2]);
        state.finish_some(2, Some(500));
        assert_eq!(state.some.total_ns, 400);
        assert_eq!(state.covered_ns(PressureClass::Some, 1_000, 1_000, 0), 400);
    }

    #[test_case]
    fn full_intervals_require_the_scheduler_transition() {
        let mut state = PressureState::new();
        state.set_full(false, 100);
        state.set_full(true, 200);
        state.set_full(false, 350);
        assert_eq!(state.full.total_ns, 150);
        assert!(state.full_available);
    }

    #[test_case]
    fn stale_full_publication_cannot_reopen_a_newer_false_state() {
        let mut state = PressureState::new();
        state.begin_some(1, 1);
        let epoch = state.active_epoch;
        assert!(state.accept_full_revision(1, true, Some(epoch), true, 100));
        assert!(state.accept_full_revision(2, false, Some(epoch), true, 200));
        assert!(!state.accept_full_revision(1, true, Some(epoch), true, 300));
        assert!(!state.full_stalled);
        assert_eq!(state.full.total_ns, 100);
    }

    #[test_case]
    fn stale_membership_epoch_makes_true_conservative_false() {
        let mut state = PressureState::new();
        state.begin_some(1, 1);
        let stale_epoch = state.active_epoch;
        state.begin_some(2, 2);
        assert!(state.accept_full_revision(1, true, Some(stale_epoch), true, 100));
        assert!(!state.full_stalled);
    }

    #[test_case]
    fn stale_scheduler_revision_makes_true_conservative_false() {
        let mut state = PressureState::new();
        state.begin_some(1, 1);
        let epoch = state.active_epoch;
        assert!(state.accept_full_revision(1, true, Some(epoch), false, 100));
        assert!(!state.full_stalled);
    }

    #[test_case]
    fn active_membership_snapshot_is_complete_or_conservatively_absent() {
        let mut state = PressureState::new();
        state.begin_some(1, 10);
        let (epoch, tasks, count) = state.membership_snapshot().unwrap();
        assert_eq!(epoch, state.active_epoch);
        assert_eq!(&tasks[..count], &[1]);
        state.finish_some(1, Some(20));

        state.begin_some(0, 30);
        assert!(state.membership_snapshot().is_none());
        state.finish_some(0, Some(40));

        for id in 1..=MAX_NUM_CPUS {
            state.begin_some(id, 50);
        }
        state.begin_some(MAX_NUM_CPUS + 1, 50);
        assert!(state.membership_snapshot().is_none());
    }

    #[test_case]
    fn psi_ewma_uses_fixed_point_decay_constants() {
        let mut history = History::new();
        history.apply_sample(PSI_AVG_PERIOD_NS, PSI_AVG_PERIOD_NS);
        assert!((1_800..=1_820).contains(&fixed_to_basis_points(history.avg10_fixed)));
        let first = history.avg10_fixed;
        history.apply_zero_periods(1);
        assert!(history.avg10_fixed < first);
        assert_eq!(fixed_to_basis_points(0), 0);
    }

    #[test_case]
    fn empty_history_reports_zero_for_both_classes() {
        let state = PressureState::new();
        assert_eq!(stats(&state, PressureClass::Some, 100).total_us, 0);
        assert_eq!(stats(&state, PressureClass::Full, 100).total_us, 0);
    }

    #[test_case]
    fn native_telemetry_reports_consistent_page_availability() {
        let telemetry = native_telemetry();
        assert!(telemetry.free_pages <= telemetry.total_pages);
    }
}
