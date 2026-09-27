//! Volume adjustment and transient-feedback lifetime, independent of presentation.
use std::time::{Duration, Instant};

const LIFETIME: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Feedback {
    pub percent: Option<u8>,
    pub muted: bool,
}

/// One refreshed deadline prevents an earlier key's timer hiding newer feedback.
#[derive(Default)]
pub struct Visibility {
    deadline: Option<Instant>,
}
impl Visibility {
    pub fn show(&mut self, now: Instant) -> bool {
        let opened = self.deadline.is_none();
        self.deadline = Some(now + LIFETIME);
        opened
    }
    pub fn expire(&mut self, now: Instant) -> bool {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.deadline = None;
            true
        } else {
            false
        }
    }
}

pub fn step_percent(percent: u8, up: bool) -> u8 {
    if up {
        percent.saturating_add(5).min(100)
    } else {
        percent.min(100).saturating_sub(5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeats_extend_one_deadline_and_bounds_still_show_feedback() {
        let start = Instant::now();
        let mut visible = Visibility::default();
        assert!(visible.show(start));
        assert!(!visible.show(start + Duration::from_secs(1)));
        assert!(!visible.expire(start + LIFETIME));
        assert!(visible.expire(start + Duration::from_millis(2500)));
        assert!(!visible.expire(start + Duration::from_secs(3)));
        assert!(visible.show(start + Duration::from_secs(4)));
        assert_eq!(step_percent(98, true), 100);
        assert_eq!(step_percent(100, true), 100);
        assert_eq!(step_percent(2, false), 0);
        assert_eq!(step_percent(0, false), 0);
    }
}
