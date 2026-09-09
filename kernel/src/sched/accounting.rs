//! CPU-owned busy/idle accounting, independent of task lookup and migration.

use crate::sync::snapshot::{SnapshotCell, SnapshotValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Activity {
    Busy,
    Idle,
}

#[derive(Clone, Copy)]
struct CpuTime {
    busy_ns: u64,
    idle_ns: u64,
    active: Option<Activity>,
    since_ns: u64,
}

impl CpuTime {
    const EMPTY: Self = Self {
        busy_ns: 0,
        idle_ns: 0,
        active: None,
        since_ns: 0,
    };

    fn finish(&mut self, now_ns: u64) {
        let delta = now_ns.saturating_sub(self.since_ns);
        match self.active {
            Some(Activity::Busy) => self.busy_ns = self.busy_ns.saturating_add(delta),
            Some(Activity::Idle) => self.idle_ns = self.idle_ns.saturating_add(delta),
            None => {}
        }
        self.since_ns = now_ns.max(self.since_ns);
    }
}

impl SnapshotValue<4> for CpuTime {
    fn encode(self) -> [u64; 4] {
        [
            self.busy_ns,
            self.idle_ns,
            self.since_ns,
            match self.active {
                None => 0,
                Some(Activity::Busy) => 1,
                Some(Activity::Idle) => 2,
            },
        ]
    }
    fn decode(words: [u64; 4]) -> Self {
        Self {
            busy_ns: words[0],
            idle_ns: words[1],
            since_ns: words[2],
            active: match words[3] {
                1 => Some(Activity::Busy),
                2 => Some(Activity::Idle),
                _ => None,
            },
        }
    }
}

pub(super) struct CpuClock(SnapshotCell<CpuTime, 4>);

impl CpuClock {
    pub fn new() -> Self {
        Self(SnapshotCell::new(CpuTime::EMPTY))
    }

    /// Commit the preceding interval and start its successor at the same time.
    pub fn switch(&self, now_ns: u64, next: Option<Activity>) {
        self.0.update(|state| {
            state.finish(now_ns);
            state.active = next;
        });
    }

    /// Includes the currently active interval from the same coherent state.
    pub fn snapshot(&self, now_ns: u64) -> (u64, u64) {
        let mut state = self.0.read();
        state.finish(now_ns);
        (state.busy_ns, state.idle_ns)
    }

    #[cfg(test)]
    pub fn reset_for_test(&self) {
        self.0.update(|state| *state = CpuTime::EMPTY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn cpu_clock_transfers_active_time_without_double_counting_switches() {
        let clock = CpuClock::new();
        clock.switch(0, Some(Activity::Busy));
        let boundary = 0x1_0000_0001;
        assert_eq!(clock.snapshot(boundary), (boundary, 0));
        clock.switch(boundary, Some(Activity::Idle));
        assert_eq!(clock.snapshot(boundary), (boundary, 0));
        assert_eq!(clock.snapshot(boundary + 100), (boundary, 100));
        clock.switch(boundary + 100, Some(Activity::Busy));
        assert_eq!(clock.snapshot(boundary + 200), (boundary + 100, 100));
        clock.switch(boundary + 200, None);
        clock.switch(boundary + 200, None);
        assert_eq!(clock.snapshot(u64::MAX), (boundary + 100, 100));
    }
}
