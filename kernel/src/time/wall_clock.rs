//! Adjustable wall time anchored to an independent monotonic clock.

#[derive(Clone, Copy)]
struct Anchor {
    unix_ns: u64,
    monotonic_ns: u64,
}

pub(super) struct WallClock {
    anchor: Option<Anchor>,
}

impl WallClock {
    pub const fn new() -> Self {
        Self { anchor: None }
    }

    pub fn read(&self, now_ns: u64) -> Option<u64> {
        let anchor = self.anchor?;
        // u64::MAX is the Native ABI's unavailable sentinel, never a date.
        Some(
            anchor
                .unix_ns
                .saturating_add(now_ns.saturating_sub(anchor.monotonic_ns))
                .min(u64::MAX - 1),
        )
    }

    pub fn set(
        &mut self,
        unix_ns: u64,
        reference_ns: u64,
        now_ns: u64,
    ) -> Result<(), &'static str> {
        let elapsed = now_ns
            .checked_sub(reference_ns)
            .ok_or("future monotonic reference")?;
        let current = unix_ns
            .checked_add(elapsed)
            .filter(|ns| *ns != u64::MAX)
            .ok_or("wall time out of range")?;
        self.anchor = Some(Anchor {
            unix_ns: current,
            monotonic_ns: now_ns,
        });
        Ok(())
    }

    pub fn initialize(
        &mut self,
        unix_ns: u64,
        before_ns: u64,
        after_ns: u64,
    ) -> Result<(), &'static str> {
        if self.anchor.is_some() {
            return Err("wall clock already initialized");
        }
        let duration = after_ns
            .checked_sub(before_ns)
            .ok_or("reversed RTC sampling interval")?;
        self.set(unix_ns, before_ns + duration / 2, after_ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn adjustments_preserve_elapsed_time_and_allow_backward_steps() {
        let mut clock = WallClock::new();
        assert_eq!(clock.read(100), None);
        clock.initialize(1000, 100, 120).unwrap();
        assert_eq!(clock.read(150), Some(1040));
        clock.set(1_000_000, 160, 170).unwrap();
        assert_eq!(clock.read(200), Some(1_000_040));
        // Epoch zero can be set even when uptime is nonzero.
        clock.set(0, 200, 210).unwrap();
        assert_eq!(clock.read(220), Some(20));
        assert!(clock.initialize(9000, 220, 230).is_err());
        assert_eq!(clock.read(230), Some(30));
    }

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn invalid_samples_leave_clock_unchanged() {
        let mut clock = WallClock::new();
        assert!(clock.initialize(100, 2, 1).is_err());
        assert_eq!(clock.read(10), None);
        clock.set(100, 10, 10).unwrap();
        assert!(clock.set(500, 12, 11).is_err());
        assert!(clock.set(u64::MAX, 10, 10).is_err());
        assert!(clock.set(u64::MAX - 1, 10, 12).is_err());
        assert_eq!(clock.read(15), Some(105));
        clock.set(u64::MAX - 1, 15, 15).unwrap();
        assert_eq!(clock.read(20), Some(u64::MAX - 1));
    }
}
