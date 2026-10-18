//! Count completed application frames independently of retained HUD painting.

#[derive(Debug, Default)]
pub(crate) struct FrameRate {
    start_ns: Option<u64>,
    frames: u64,
}

impl FrameRate {
    pub(crate) fn record(&mut self, now_ns: u64) -> Option<(u64, u64)> {
        let start = self.start_ns.get_or_insert(now_ns);
        if now_ns == *start {
            return None;
        }
        self.frames = self.frames.saturating_add(1);
        let elapsed = now_ns.saturating_sub(*start);
        if elapsed < 500_000_000 {
            return None;
        }
        let reading = (
            self.frames.saturating_mul(1_000_000_000_000) / elapsed,
            elapsed / self.frames / 1_000,
        );
        *start = now_ns;
        self.frames = 0;
        Some(reading)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_completed_intervals_without_hud_paints() {
        let mut rate = FrameRate::default();
        assert_eq!(rate.record(10_000_000_000), None);
        for frame in 1..5 {
            assert_eq!(rate.record(10_000_000_000 + frame * 100_000_000), None);
        }
        assert_eq!(rate.record(10_500_000_000), Some((10_000, 100_000)));
        assert_eq!(rate.record(11_000_000_000), Some((2_000, 500_000)));
    }

    #[test]
    fn slow_frames_do_not_reset_the_meter_to_zero() {
        let mut rate = FrameRate::default();
        assert_eq!(rate.record(0), None);
        assert_eq!(rate.record(2_000_000_000), Some((500, 2_000_000)));
        assert_eq!(rate.record(4_000_000_000), Some((500, 2_000_000)));
    }
}
