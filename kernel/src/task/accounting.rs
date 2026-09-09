//! Task runtime accounting independent of the native register width.

use crate::sync::diagnostic::TryDiagnosticState;
use crate::sync::snapshot::{SnapshotCell, SnapshotValue};
use core::sync::atomic::{AtomicU32, Ordering};

use super::{
    SCHED_UTIL_DECAY_DEN, SCHED_UTIL_DECAY_INTERVAL_NS, SCHED_UTIL_DECAY_NUM, SCHED_UTIL_SCALE,
    TASK_CPU_HOG_THRESHOLD_PER_MILLE, TASK_CPU_HOG_WINDOW_NS,
};

#[derive(Clone, Copy, Debug)]
struct Runtime {
    completed_ns: u64,
    started_at_ns: Option<u64>,
}

impl Runtime {
    fn active_ns(self, now_ns: u64) -> u64 {
        self.started_at_ns
            .map_or(0, |start| now_ns.saturating_sub(start))
    }

    fn total_ns(self, now_ns: u64) -> u64 {
        self.completed_ns.saturating_add(self.active_ns(now_ns))
    }
}

impl SnapshotValue<3> for Runtime {
    fn encode(self) -> [u64; 3] {
        [
            self.completed_ns,
            self.started_at_ns.unwrap_or(0),
            u64::from(self.started_at_ns.is_some()),
        ]
    }

    fn decode(words: [u64; 3]) -> Self {
        Self {
            completed_ns: words[0],
            started_at_ns: (words[2] != 0).then_some(words[1]),
        }
    }
}

/// Owns the transition between active time and committed time. Both are
/// observed together, so a concurrent stop cannot double-count an interval.
#[derive(Debug)]
pub(super) struct CpuAccounting(SnapshotCell<Runtime, 3>);

impl CpuAccounting {
    pub fn new() -> Self {
        Self(SnapshotCell::new(Runtime {
            completed_ns: 0,
            started_at_ns: None,
        }))
    }

    pub fn begin(&self, now_ns: u64) -> bool {
        self.0.update(|state| {
            // Marking an already active interval must not discard its runtime.
            if state.started_at_ns.is_some() {
                return false;
            }
            state.started_at_ns = Some(now_ns);
            true
        })
    }

    pub fn finish(&self, now_ns: u64) -> u64 {
        self.0.update(|state| {
            let delta = state.active_ns(now_ns);
            state.completed_ns = state.completed_ns.saturating_add(delta);
            state.started_at_ns = None;
            delta
        })
    }

    pub fn total_ns(&self, now_ns: u64) -> u64 {
        self.0.read().total_ns(now_ns)
    }

    pub fn try_total_ns(&self, now_ns: u64) -> Option<u64> {
        self.0.try_read().map(|state| state.total_ns(now_ns))
    }

    pub fn active_ns(&self, now_ns: u64) -> u64 {
        self.0.read().active_ns(now_ns)
    }
}

#[derive(Clone, Copy)]
struct UtilWindow {
    average: u32,
    updated_at: Option<u64>,
    runtime_ns: u64,
    accounted_until: Option<u64>,
}

impl SnapshotValue<6> for UtilWindow {
    fn encode(self) -> [u64; 6] {
        [
            u64::from(self.average),
            self.updated_at.unwrap_or(0),
            u64::from(self.updated_at.is_some()),
            self.runtime_ns,
            self.accounted_until.unwrap_or(0),
            u64::from(self.accounted_until.is_some()),
        ]
    }
    fn decode(words: [u64; 6]) -> Self {
        Self {
            average: words[0] as u32,
            updated_at: (words[2] != 0).then_some(words[1]),
            runtime_ns: words[3],
            accounted_until: (words[5] != 0).then_some(words[4]),
        }
    }
}

fn decay_average(average: u32, elapsed_ns: u64) -> u32 {
    let periods = (elapsed_ns / SCHED_UTIL_DECAY_INTERVAL_NS).min(64);
    let mut next = u64::from(average);
    for _ in 0..periods {
        next =
            next.saturating_mul(u64::from(SCHED_UTIL_DECAY_NUM)) / u64::from(SCHED_UTIL_DECAY_DEN);
        if next == 0 {
            break;
        }
    }
    next.min(u64::from(SCHED_UTIL_SCALE)) as u32
}

impl UtilWindow {
    fn decayed_average(self, now_ns: u64) -> u32 {
        self.updated_at.map_or(self.average, |updated| {
            decay_average(self.average, now_ns.saturating_sub(updated))
        })
    }

    fn account(&mut self, now_ns: u64) -> u32 {
        let Some(last_accounted) = self.accounted_until else {
            return self.average;
        };
        self.runtime_ns = self
            .runtime_ns
            .saturating_add(now_ns.saturating_sub(last_accounted));
        self.accounted_until = Some(now_ns.max(last_accounted));
        let Some(last_update) = self.updated_at else {
            self.updated_at = Some(now_ns);
            return self.average;
        };
        let elapsed = now_ns.saturating_sub(last_update);
        if elapsed < SCHED_UTIL_DECAY_INTERVAL_NS {
            return self.average;
        }
        let sample = ((u128::from(self.runtime_ns) * u128::from(SCHED_UTIL_SCALE))
            / u128::from(elapsed))
        .min(u128::from(SCHED_UTIL_SCALE)) as u32;
        let average = self.decayed_average(now_ns);
        self.average = if sample > average {
            average.saturating_add(sample.saturating_sub(average).saturating_add(1) / 2)
        } else {
            average.saturating_mul(7).saturating_add(sample) / 8
        }
        .min(SCHED_UTIL_SCALE);
        self.updated_at = Some(now_ns);
        self.runtime_ns = 0;
        self.average
    }
}

/// Runtime-window timestamps and their utilization estimate share an owner.
/// The scalar average remains a native word atomic on every target.
pub(super) struct SchedUtil {
    window: SnapshotCell<UtilWindow, 6>,
    average: AtomicU32,
}

impl SchedUtil {
    pub fn new() -> Self {
        Self {
            window: SnapshotCell::new(UtilWindow {
                average: 0,
                updated_at: None,
                runtime_ns: 0,
                accounted_until: None,
            }),
            average: AtomicU32::new(0),
        }
    }
    pub fn average(&self) -> u32 {
        self.average.load(Ordering::Relaxed)
    }
    pub fn decayed_average(&self, now_ns: u64) -> u32 {
        self.window.read().decayed_average(now_ns)
    }
    pub fn begin(&self, now_ns: u64) {
        self.window
            .update(|window| window.accounted_until = Some(now_ns));
    }
    pub fn account(&self, now_ns: u64) -> u32 {
        self.window.update(|window| {
            let average = window.account(now_ns);
            self.average.store(average, Ordering::Relaxed);
            average
        })
    }
    pub fn finish(&self, now_ns: u64) {
        self.window.update(|window| {
            let average = window.account(now_ns);
            window.accounted_until = None;
            self.average.store(average, Ordering::Relaxed);
        });
    }
}

#[derive(Clone, Copy)]
struct HogWindow {
    at_ns: u64,
    runtime_ns: u64,
    pc: usize,
    privileged: bool,
}

pub(super) struct HogSample {
    pub usage_per_mille: u32,
    pub window_ns: u64,
    pub runtime_ns: u64,
    pub start_pc: usize,
    pub start_privileged: bool,
}

/// Diagnostic sampling may skip a contended window and never affects accounting.
pub(super) struct CpuHog(TryDiagnosticState<Option<HogWindow>>);

impl CpuHog {
    pub const fn new() -> Self {
        Self(TryDiagnosticState::new(None))
    }

    pub fn sample(
        &self,
        now_ns: u64,
        runtime_ns: u64,
        pc: usize,
        privileged: bool,
    ) -> Option<HogSample> {
        self.0.try_update(|window| {
            let current = HogWindow {
                at_ns: now_ns,
                runtime_ns,
                pc,
                privileged,
            };
            let Some(previous) = *window else {
                *window = Some(current);
                return None;
            };
            let window_ns = now_ns.saturating_sub(previous.at_ns);
            if window_ns < TASK_CPU_HOG_WINDOW_NS {
                return None;
            }
            let consumed = runtime_ns.saturating_sub(previous.runtime_ns);
            let usage_per_mille =
                ((u128::from(consumed) * 1_000) / u128::from(window_ns)).min(1_000) as u32;
            *window = Some(current);
            (usage_per_mille >= TASK_CPU_HOG_THRESHOLD_PER_MILLE).then_some(HogSample {
                usage_per_mille,
                window_ns,
                runtime_ns: consumed,
                start_pc: previous.pc,
                start_privileged: previous.privileged,
            })
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn runtime_keeps_zero_start_and_wide_intervals_and_commits_once() {
        let accounting = CpuAccounting::new();
        accounting.begin(0);
        accounting.begin(1);
        assert_eq!(accounting.total_ns(10), 10);
        assert_eq!(accounting.finish(10), 10);
        assert_eq!(accounting.finish(20), 0);
        assert_eq!(accounting.total_ns(20), 10);
        let start = 0x1_0000_0000;
        let elapsed = 0x2_0000_0001;
        accounting.begin(start);
        assert_eq!(accounting.active_ns(start + elapsed), elapsed);
        assert_eq!(accounting.finish(start + elapsed), elapsed);
        assert_eq!(accounting.total_ns(start + elapsed + 1), elapsed + 10);
        assert_eq!(accounting.active_ns(u64::MAX), 0);
    }
}
