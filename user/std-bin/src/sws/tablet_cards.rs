//! Pure interaction state for one overview card contact.
//!
//! Coordinates, deltas and card height are output pixels. Times are monotonic
//! nanoseconds; `scale` is pixels per logical pixel times 1000 (1000 = 1x).
//! Feed positions to `update`, and call `tick` while stationary and at release
//! before querying `tap`/`dismiss_committed`/`release_velocity`. Do not synthesize
//! a duplicate position update at release: it would report a stationary sample.
//! The caller owns contact identity, multitouch cancellation and removal after
//! release. Cancelled contacts stay inert until discarded.
//! `next_deadline_ns` supplies a single hold wakeup while Pending and within
//! slop; movement, cancellation or promotion removes that timer. Ticks use
//! wall time from the same monotonic clock as touch events, but never advance
//! the event-ordering timestamp. Queued events may legitimately predate ticks.

const LONG_PRESS_NS: u64 = 400_000_000;
const VELOCITY_STALE_NS: u64 = 100_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Pending,
    Scroll,
    Dismiss,
    Move,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Contact {
    mode: Mode,
    cancelled: bool,
    left_slop: bool,
    scale: u32,
    start_x: i32,
    start_y: i32,
    x: i32,
    y: i32,
    started_ns: u64,
    last_seen_ns: u64, // Latest wall/event time, used only for velocity freshness.
    sample_ns: u64,    // Last position event: ordering and velocity sampling only.
    last_motion_ns: u64,
    velocity: f32,
}

impl Contact {
    /// Zero scale creates an inert, cancelled contact.
    pub(super) fn new(x: i32, y: i32, now: u64, scale: u32) -> Self {
        Self {
            mode: Mode::Pending,
            cancelled: scale == 0,
            left_slop: false,
            scale,
            start_x: x,
            start_y: y,
            x,
            y,
            started_ns: now,
            last_seen_ns: now,
            sample_ns: now,
            last_motion_ns: now,
            velocity: 0.0,
        }
    }

    pub(super) fn update(&mut self, x: i32, y: i32, now: u64) {
        if !self.observe_event_time(now) {
            return;
        }

        let dt = now - self.sample_ns;
        let motion_x = f64::from(x) - f64::from(self.x);
        // Use the most recent positional segment, never lifetime average speed.
        // Equal timestamps rebase without dividing by zero or retaining a fling.
        self.velocity = if dt > 0 && dt <= VELOCITY_STALE_NS {
            (motion_x * 1_000_000_000.0 / dt as f64) as f32
        } else {
            0.0
        };
        if x != self.x {
            self.last_motion_ns = now;
        }
        self.sample_ns = now;
        self.x = x;
        self.y = y;

        if self.mode == Mode::Pending {
            let (dx, dy) = self.displacement();
            let slop = self.logical(8.0);
            self.left_slop |= dx * dx + dy * dy > slop * slop;
            // Inspect the new position before considering a long press. A late
            // movement event is not proof the finger was still at the deadline.
            if self.left_slop && dx * dx + dy * dy > slop * slop {
                if dx.abs() * 5.0 >= dy.abs() * 6.0 {
                    self.mode = Mode::Scroll;
                } else if dy.abs() * 5.0 >= dx.abs() * 6.0 {
                    if dy < 0.0 {
                        self.mode = Mode::Dismiss;
                    } else {
                        self.cancel();
                    }
                }
            }
        }
        self.promote_hold(now);
    }

    /// Promote a stationary hold without advancing touch-event ordering.
    /// Ticks older than the latest position event are ignored, not cancelled.
    pub(super) fn tick(&mut self, now: u64) {
        if self.cancelled || now < self.sample_ns {
            return;
        }
        self.last_seen_ns = self.last_seen_ns.max(now);
        self.promote_hold(now);
    }

    /// Absolute hold wakeup in monotonic nanoseconds; re-query after input.
    /// None after leaving slop (even if still Pending), promotion or cancel,
    /// or if the deadline is beyond the representable clock range.
    pub(super) fn next_deadline_ns(&self) -> Option<u64> {
        if self.cancelled || self.left_slop || self.mode != Mode::Pending {
            return None;
        }
        self.started_ns.checked_add(LONG_PRESS_NS)
    }

    /// Terminal cancellation (including when the caller detects multitouch).
    pub(super) fn cancel(&mut self) {
        self.cancelled = true;
        self.mode = Mode::Pending;
        self.velocity = 0.0;
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Cancelled contacts report Pending; inspect `is_cancelled` for that case.
    pub(super) fn mode(&self) -> Mode {
        self.mode
    }

    /// Total signed displacement from touch-down, in output pixels.
    pub(super) fn dx(&self) -> f32 {
        self.displacement().0 as f32
    }

    /// Positive is downward. Deltas remain unfiltered in every mode.
    pub(super) fn dy(&self) -> f32 {
        self.displacement().1 as f32
    }

    /// Release eligibility, with slop retained even after returning to origin.
    pub(super) fn tap(&self) -> bool {
        !self.cancelled && !self.left_slop && self.mode == Mode::Pending
    }

    /// Distance alone commits: fast short swipes cannot dismiss a card.
    /// Retreat below the threshold before release withdraws the commitment.
    pub(super) fn dismiss_committed(&self, card_height: u32) -> bool {
        !self.cancelled
            && self.mode == Mode::Dismiss
            && -self.displacement().1 >= self.logical(64.0).max(f64::from(card_height) * 0.25)
    }

    /// Latest horizontal segment speed in output px/s, only for Scroll.
    /// Freshness is evaluated at the latest update/tick.
    pub(super) fn velocity_x(&self) -> f32 {
        self.release_velocity(self.last_seen_ns)
    }

    /// Query speed at release without refreshing the last movement timestamp.
    /// Stale (>=100 ms) or releases before the last position event yield zero.
    /// A queued release may predate the latest wall-clock tick.
    pub(super) fn release_velocity(&self, now: u64) -> f32 {
        if self.cancelled
            || self.mode != Mode::Scroll
            || now < self.sample_ns
            || now - self.last_motion_ns >= VELOCITY_STALE_NS
        {
            0.0
        } else {
            self.velocity
        }
    }

    fn logical(&self, distance: f64) -> f64 {
        distance * f64::from(self.scale) / 1000.0
    }

    fn displacement(&self) -> (f64, f64) {
        // Convert before subtracting, including for opposite i32 extremes.
        (
            f64::from(self.x) - f64::from(self.start_x),
            f64::from(self.y) - f64::from(self.start_y),
        )
    }

    fn observe_event_time(&mut self, now: u64) -> bool {
        if now < self.sample_ns {
            self.cancel();
        }
        if self.cancelled {
            return false;
        }
        self.last_seen_ns = self.last_seen_ns.max(now);
        true
    }

    fn promote_hold(&mut self, now: u64) {
        if self
            .next_deadline_ns()
            .is_some_and(|deadline| now >= deadline)
        {
            self.mode = Mode::Move;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn jitter_taps_but_exact_long_press_deadline_moves() {
        let mut c = Contact::new(100, 100, 50 * MS, 1000);
        c.update(103, 96, 100 * MS);
        c.update(108, 100, 200 * MS); // Exactly eight logical pixels is allowed.
        c.tick(449 * MS);
        assert!(c.tap());
        c.tick(450 * MS);
        assert_eq!(c.mode(), Mode::Move);
        assert!(!c.tap());
        c.update(400, -200, 500 * MS);
        assert_eq!(c.mode(), Mode::Move);
        assert_eq!((c.dx(), c.dy()), (300.0, -300.0));
        assert!(!c.dismiss_committed(200));
        assert_eq!(c.velocity_x(), 0.0);
    }

    #[test]
    fn scaled_slop_is_radial_and_retains_any_excursion() {
        for scale in [500, 1000, 1500, 2000] {
            let slop = (8 * scale / 1000) as i32;
            let mut c = Contact::new(0, 0, 0, scale);
            c.update(slop, 0, 10 * MS);
            assert!(c.tap());
            c.update(slop, slop, 20 * MS); // Diagonal exceeds radial slop.
            assert_eq!(c.mode(), Mode::Pending);
            assert!(!c.tap());
            c.update(0, 0, 30 * MS);
            c.tick(1000 * MS);
            assert!(!c.tap());
            assert_eq!(c.mode(), Mode::Pending);
        }
    }

    #[test]
    fn diagonal_wait_never_becomes_move_then_can_lock_either_axis() {
        for (x, y, expected) in [(30, -20, Mode::Scroll), (20, -30, Mode::Dismiss)] {
            let mut c = Contact::new(0, 0, 0, 1000);
            c.update(20, -20, 50 * MS);
            c.tick(1000 * MS);
            assert_eq!(c.mode(), Mode::Pending);
            assert!(!c.tap());
            c.update(x, y, 1010 * MS);
            assert_eq!(c.mode(), expected);
        }
    }

    #[test]
    fn dominance_boundary_and_axis_lock_survive_direction_changes() {
        for sign in [-1, 1] {
            let mut c = Contact::new(0, 0, 0, 1000);
            c.update(sign * 11, -10, 10 * MS);
            assert_eq!(c.mode(), Mode::Pending);
            c.update(sign * 12, -10, 20 * MS);
            assert_eq!(c.mode(), Mode::Scroll); // Exactly 1.2 dominance.
            c.update(-sign * 5, -500, 30 * MS);
            c.tick(1000 * MS);
            assert_eq!(c.mode(), Mode::Scroll);
            assert!(!c.dismiss_committed(100));
        }
        let mut c = Contact::new(0, 0, 0, 1000);
        c.update(10, -11, 10 * MS);
        assert_eq!(c.mode(), Mode::Pending);
        c.update(10, -12, 20 * MS);
        assert_eq!(c.mode(), Mode::Dismiss);
        c.update(500, 20, 30 * MS);
        assert_eq!(c.mode(), Mode::Dismiss);
        assert!(!c.dismiss_committed(100));
        assert_eq!(c.release_velocity(30 * MS), 0.0);
    }

    #[test]
    fn downward_drag_cancels_even_if_it_later_returns_or_swipes_up() {
        let mut c = Contact::new(0, 0, 0, 1000);
        c.update(0, 9, 10 * MS);
        assert!(c.is_cancelled());
        c.update(0, 0, 20 * MS);
        c.tick(400 * MS);
        c.update(100, -300, 450 * MS);
        assert!(!c.tap());
        assert!(!c.dismiss_committed(100));
        assert_eq!(c.release_velocity(450 * MS), 0.0);
        assert_eq!(c.mode(), Mode::Pending);
    }

    #[test]
    fn late_position_update_does_not_invent_a_hold_before_movement() {
        let mut c = Contact::new(0, 0, 0, 1000);
        c.update(9, 0, 400 * MS);
        c.tick(800 * MS);
        assert_eq!(c.mode(), Mode::Scroll);
        let mut still = Contact::new(0, 0, 0, 1000);
        still.update(4, 0, 400 * MS);
        assert_eq!(still.mode(), Mode::Move);
    }

    #[test]
    fn dismiss_requires_scaled_distance_or_quarter_height_and_can_retreat() {
        for (scale, height, threshold) in [(1000, 100, 64), (2000, 100, 128), (1000, 800, 200)] {
            let mut c = Contact::new(50, 500, 0, scale);
            c.update(50, 500 - threshold + 1, MS);
            assert_eq!(c.mode(), Mode::Dismiss);
            assert!(!c.dismiss_committed(height)); // Even a very fast swipe.
            c.update(50, 500 - threshold, 20 * MS);
            assert!(c.dismiss_committed(height));
            c.update(50, 500 - threshold + 1, 30 * MS);
            assert!(!c.dismiss_committed(height));
            c.update(50, 501, 40 * MS);
            assert!(!c.dismiss_committed(height));
            assert!(!c.tap());
        }
        let mut c = Contact::new(0, 0, 0, 1000);
        c.update(0, -64, MS);
        assert!(!c.dismiss_committed(257)); // Fractional quarter-height threshold.
        c.update(0, -65, 2 * MS);
        assert!(c.dismiss_committed(257));
    }

    #[test]
    fn velocity_tracks_recent_reversal_in_output_pixels() {
        let mut c = Contact::new(0, 0, 0, 2000);
        c.update(20, 0, 20 * MS);
        assert_eq!(c.velocity_x(), 1000.0);
        c.update(60, 0, 40 * MS);
        assert_eq!(c.velocity_x(), 2000.0);
        c.update(50, 0, 50 * MS);
        assert_eq!(c.release_velocity(60 * MS), -1000.0);
        c.tick(149 * MS);
        assert_eq!(c.velocity_x(), -1000.0);
        assert_eq!(c.release_velocity(150 * MS), 0.0);
        c.tick(150 * MS);
        assert_eq!(c.velocity_x(), 0.0);
    }

    #[test]
    fn stationary_duplicate_and_sparse_samples_do_not_retain_a_fling() {
        let mut c = Contact::new(0, 0, 0, 1000);
        c.update(20, 0, 20 * MS);
        c.update(20, 0, 30 * MS);
        assert_eq!(c.release_velocity(30 * MS), 0.0);
        c.update(40, 0, 40 * MS);
        assert_eq!(c.velocity_x(), 2000.0);
        c.update(50, 0, 40 * MS); // Changed coordinate with zero elapsed time.
        assert_eq!(c.velocity_x(), 0.0);
        c.update(60, 0, 50 * MS);
        assert_eq!(c.velocity_x(), 1000.0);
        c.update(100, 0, 151 * MS);
        assert_eq!(c.velocity_x(), 0.0);
        c.update(110, 0, 161 * MS);
        assert_eq!(c.velocity_x(), 1000.0);
    }

    #[test]
    fn ticks_do_not_shorten_velocity_sampling_interval() {
        let mut c = Contact::new(0, 0, 0, 1000);
        c.update(20, 0, 20 * MS);
        c.tick(39 * MS);
        c.update(40, 0, 40 * MS);
        assert_eq!(c.velocity_x(), 1000.0);
        c.update(40, 100, 50 * MS);
        assert_eq!(c.velocity_x(), 0.0); // Vertical motion cannot fling horizontally.
    }

    #[test]
    fn queued_motion_and_release_behind_wall_ticks_preserve_scroll_velocity() {
        let mut c = Contact::new(0, 0, 0, 1000);
        c.update(20, 0, 20 * MS);
        c.tick(32 * MS); // A compositor tick overtakes a queued event by 2 ms.
        c.update(40, 0, 30 * MS);
        assert!(!c.is_cancelled());
        assert_eq!(c.mode(), Mode::Scroll);
        assert_eq!(c.dx(), 40.0);
        assert_eq!(c.velocity_x(), 2000.0); // 20 pixels / 10 event milliseconds.
        c.tick(48 * MS);
        c.tick(45 * MS); // Queued release, also behind wall time.
        assert!(!c.is_cancelled());
        assert_eq!(c.release_velocity(45 * MS), 2000.0);
        c.tick(130 * MS);
        assert_eq!(c.velocity_x(), 0.0);
        assert_eq!(c.release_velocity(45 * MS), 2000.0);
    }

    #[test]
    fn queued_hold_events_behind_ticks_do_not_cancel_or_reschedule() {
        let mut c = Contact::new(100, 100, 50 * MS, 1000);
        c.tick(448 * MS);
        c.update(102, 100, 440 * MS);
        assert!(c.tap());
        assert_eq!(c.next_deadline_ns(), Some(450 * MS));
        c.tick(460 * MS);
        assert_eq!(c.mode(), Mode::Move);
        c.update(103, 100, 455 * MS);
        assert!(!c.is_cancelled());
        assert_eq!(c.mode(), Mode::Move);
        assert_eq!(c.next_deadline_ns(), None);
    }

    #[test]
    fn deadline_exists_only_for_a_pending_hold_inside_slop() {
        let mut c = Contact::new(0, 0, 50 * MS, 1000);
        assert_eq!(c.next_deadline_ns(), Some(450 * MS));
        c.update(8, 0, 100 * MS);
        assert_eq!(c.next_deadline_ns(), Some(450 * MS));
        c.tick(449 * MS);
        assert_eq!(c.next_deadline_ns(), Some(450 * MS));
        c.tick(450 * MS);
        assert_eq!(c.next_deadline_ns(), None);
        for (x, y, mode) in [
            (9, 0, Mode::Scroll),
            (0, -9, Mode::Dismiss),
            (9, 9, Mode::Pending),
        ] {
            let mut c = Contact::new(0, 0, 0, 1000);
            c.update(x, y, 20 * MS);
            assert_eq!(c.mode(), mode);
            assert_eq!(c.next_deadline_ns(), None);
            c.update(0, 0, 30 * MS);
            assert_eq!(c.next_deadline_ns(), None);
        }
        let mut c = Contact::new(0, 0, 0, 1000);
        c.cancel();
        assert_eq!(c.next_deadline_ns(), None);
        assert_eq!(Contact::new(0, 0, 0, 0).next_deadline_ns(), None);
        assert_eq!(Contact::new(0, 0, u64::MAX, 1000).next_deadline_ns(), None);
        assert_eq!(
            Contact::new(0, 0, u64::MAX - LONG_PRESS_NS, 1000).next_deadline_ns(),
            Some(u64::MAX)
        );
    }

    #[test]
    fn cancellation_is_terminal_in_every_mode() {
        for mode in [Mode::Pending, Mode::Scroll, Mode::Dismiss, Mode::Move] {
            let mut c = Contact::new(0, 0, 0, 1000);
            match mode {
                Mode::Pending => (),
                Mode::Scroll => c.update(100, 0, 20 * MS),
                Mode::Dismiss => c.update(0, -100, 20 * MS),
                Mode::Move => c.tick(400 * MS),
            }
            assert_eq!(c.mode(), mode);
            c.cancel();
            c.cancel();
            c.update(500, -500, 500 * MS);
            c.tick(1000 * MS);
            assert!(c.is_cancelled());
            assert!(!c.tap());
            assert!(!c.dismiss_committed(100));
            assert_eq!(c.release_velocity(1000 * MS), 0.0);
        }
    }

    #[test]
    fn backwards_events_cancel_but_old_ticks_are_ignored_without_underflow() {
        let mut c = Contact::new(0, 0, 100 * MS, 1000);
        c.update(30, 0, 120 * MS);
        assert_eq!(c.release_velocity(119 * MS), 0.0);
        c.tick(119 * MS);
        assert!(!c.is_cancelled());
        assert_eq!(c.velocity_x(), 1500.0);
        c.tick(150 * MS);
        c.update(60, 0, 140 * MS); // Ordered event behind the tick is accepted.
        assert!(!c.is_cancelled());
        c.update(90, 0, 139 * MS); // A true event-order violation still cancels.
        c.tick(1000 * MS);
        assert!(c.is_cancelled());
        assert!(!c.tap());
        assert_eq!(c.velocity_x(), 0.0);
    }

    #[test]
    fn extreme_coordinates_times_and_invalid_scale_are_safe() {
        let mut c = Contact::new(i32::MIN, i32::MAX, 0, 1000);
        c.update(i32::MAX, i32::MAX, 1);
        assert_eq!(c.mode(), Mode::Scroll);
        assert!(c.dx().is_finite() && c.velocity_x().is_finite());
        assert!(c.dx() > i32::MAX as f32);
        c.update(i32::MIN, i32::MIN, 2);
        assert!(c.dy() < i32::MIN as f32);
        let mut c = Contact::new(0, 0, u64::MAX - LONG_PRESS_NS, u32::MAX);
        c.tick(u64::MAX);
        assert_eq!(c.mode(), Mode::Move);
        let mut c = Contact::new(0, 0, u64::MAX, 1000);
        c.tick(u64::MAX);
        assert!(c.tap());
        c.tick(0); // Older ticks cannot invalidate an event stream.
        assert!(!c.is_cancelled());
        c.update(0, 0, 0); // A wrapped event clock still cancels.
        assert!(c.is_cancelled());
        let mut c = Contact::new(0, 0, 0, 0);
        c.update(100, -100, MS);
        c.tick(400 * MS);
        assert!(c.is_cancelled());
        assert!(!c.tap());
        assert!(!c.dismiss_committed(0));
    }
}
