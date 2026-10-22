//! Clock utilities measured against caller-supplied monotonic instants.

use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimerPhase {
    #[default]
    Ready,
    Running,
    Paused,
    Finished,
}

#[derive(Clone, Debug)]
pub struct Timer {
    duration: Duration,
    elapsed: Duration,
    started: Option<Duration>,
    phase: TimerPhase,
}

impl Default for Timer {
    fn default() -> Self {
        Self::new(Duration::from_secs(300))
    }
}

impl Timer {
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            elapsed: Duration::ZERO,
            started: None,
            phase: TimerPhase::Ready,
        }
    }

    pub fn phase(&self, now: Duration) -> TimerPhase {
        if self.phase == TimerPhase::Running && self.remaining(now).is_zero() {
            TimerPhase::Finished
        } else {
            self.phase
        }
    }

    pub fn remaining(&self, now: Duration) -> Duration {
        let live = self
            .started
            .map_or(Duration::ZERO, |start| now.saturating_sub(start));
        self.duration
            .saturating_sub(self.elapsed.saturating_add(live))
    }

    pub fn start(&mut self, now: Duration) {
        match self.phase(now) {
            TimerPhase::Running => return,
            TimerPhase::Finished | TimerPhase::Ready => self.elapsed = Duration::ZERO,
            TimerPhase::Paused => {}
        }
        self.started = Some(now);
        self.phase = TimerPhase::Running;
    }

    pub fn pause(&mut self, now: Duration) {
        match self.phase(now) {
            TimerPhase::Running => {
                self.elapsed = self.duration - self.remaining(now);
                self.phase = TimerPhase::Paused;
                self.started = None;
            }
            TimerPhase::Finished => {
                self.elapsed = self.duration;
                self.phase = TimerPhase::Finished;
                self.started = None;
            }
            _ => {}
        }
    }

    pub fn reset(&mut self) {
        self.elapsed = Duration::ZERO;
        self.started = None;
        self.phase = TimerPhase::Ready;
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stopwatch {
    elapsed: Duration,
    started: Option<Duration>,
}

impl Stopwatch {
    pub fn running(&self) -> bool {
        self.started.is_some()
    }

    pub fn elapsed(&self, now: Duration) -> Duration {
        self.elapsed.saturating_add(
            self.started
                .map_or(Duration::ZERO, |start| now.saturating_sub(start)),
        )
    }

    pub fn start(&mut self, now: Duration) {
        if self.started.is_none() {
            self.started = Some(now);
        }
    }

    pub fn pause(&mut self, now: Duration) {
        self.elapsed = self.elapsed(now);
        self.started = None;
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Parse an explicitly labelled HH:MM:SS input, up to 99:59:59.
pub fn parse_duration(input: &str) -> Result<Duration, &'static str> {
    let parts: Vec<&str> = input.trim().split(':').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || p.len() > 2 || !p.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err("Use hh:mm:ss, for example 00:05:00.");
    }
    let hours = parts[0].parse::<u64>().unwrap();
    let minutes = parts[1].parse::<u64>().unwrap();
    let seconds = parts[2].parse::<u64>().unwrap();
    if minutes > 59 || seconds > 59 {
        return Err("Minutes and seconds must be 00 to 59.");
    }
    let total = hours * 3600 + minutes * 60 + seconds;
    if total == 0 {
        return Err("Enter a duration greater than zero.");
    }
    Ok(Duration::from_secs(total))
}

pub fn ceil_seconds(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() != 0))
}

pub fn format_seconds(seconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    #[test]
    fn timer_pause_resume_excludes_pause_and_finishes_at_deadline() {
        let mut timer = Timer::new(s(10));
        timer.start(s(100));
        timer.pause(s(103));
        assert_eq!(timer.phase(s(1000)), TimerPhase::Paused);
        assert_eq!(timer.remaining(s(1000)), s(7));
        timer.start(s(1000));
        assert_eq!(timer.phase(s(1006)), TimerPhase::Running);
        assert_eq!(timer.phase(s(1007)), TimerPhase::Finished);
        assert_eq!(timer.remaining(s(2000)), Duration::ZERO);
    }

    #[test]
    fn reset_and_restart_never_reuse_elapsed_time() {
        let mut timer = Timer::new(s(5));
        timer.start(s(1));
        timer.start(s(2)); // repeated Start must not shift its deadline
        assert_eq!(timer.remaining(s(3)), s(3));
        timer.pause(s(7));
        assert_eq!(timer.phase(s(7)), TimerPhase::Finished);
        timer.start(s(10));
        assert_eq!(timer.remaining(s(10)), s(5));
        timer.reset();
        assert_eq!(timer.phase(s(99)), TimerPhase::Ready);
        assert_eq!(timer.remaining(s(99)), s(5));
    }

    #[test]
    fn stopwatch_accumulates_intervals_without_tick_drift() {
        let mut watch = Stopwatch::default();
        watch.start(s(100));
        watch.start(s(101));
        watch.pause(s(103));
        assert_eq!(watch.elapsed(s(999)), s(3));
        watch.start(s(1000));
        assert_eq!(watch.elapsed(s(1234)), s(237));
        watch.reset();
        assert!(!watch.running());
        assert_eq!(watch.elapsed(s(2000)), Duration::ZERO);
    }

    #[test]
    fn countdown_rounds_up_until_exact_deadline() {
        let mut timer = Timer::new(s(1));
        timer.start(Duration::ZERO);
        assert_eq!(ceil_seconds(timer.remaining(Duration::from_millis(999))), 1);
        assert_eq!(ceil_seconds(timer.remaining(s(1))), 0);
        assert_eq!(format_seconds(3661), "01:01:01");
    }

    #[test]
    fn duration_input_rejects_zero_overflow_and_malformed_values() {
        for invalid in [
            "",
            "5",
            "00:00:00",
            "00:60:00",
            "00:00:60",
            "100:00:00",
            "-1:00:00",
            "aa:00:00",
            "1:2:3:4",
            " :00:01",
        ] {
            assert!(parse_duration(invalid).is_err(), "{invalid}");
        }
        assert_eq!(parse_duration(" 01:02:03 "), Ok(s(3723)));
        assert_eq!(parse_duration("99:59:59"), Ok(s(359999)));
    }

    #[test]
    fn an_earlier_sample_cannot_underflow_elapsed_time() {
        let mut timer = Timer::new(s(5));
        timer.start(s(100));
        assert_eq!(timer.remaining(s(90)), s(5));
        let mut watch = Stopwatch::default();
        watch.start(s(100));
        assert_eq!(watch.elapsed(s(90)), Duration::ZERO);
    }
}
