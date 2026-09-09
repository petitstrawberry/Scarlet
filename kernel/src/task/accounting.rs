//! Task runtime accounting independent of the native register width.

use crate::sync::snapshot::{SnapshotCell, SnapshotValue};

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

    pub fn begin(&self, now_ns: u64) {
        self.0.update(|state| {
            // Marking an already active interval must not discard its runtime.
            state.started_at_ns.get_or_insert(now_ns);
        });
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
