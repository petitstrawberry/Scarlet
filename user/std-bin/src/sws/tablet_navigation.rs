//! Bottom-edge navigation for one device's complete contact snapshots.
//!
//! Coordinates and dimensions are output pixels; `scale_milli` is pixels per
//! logical pixel times 1000. Call even for unclaimed frames and the final empty
//! frame: contact history prevents an existing finger from entering the edge.
//! `consumed` covers tentative gestures and the final release/drain frame.
//! Only `Begin` should start changing compositor focus or presentation.
//!
//! Release commits Home above 0.22 progress and at least 32 logical pixels of
//! upward travel within 600 ms. A stationary hold of at least 180 ms above 0.22
//! instead commits Overview (evaluated on release, with two logical pixels of
//! jitter tolerance). Horizontal travel must exceed 60 logical pixels and 1.5
//! times vertical travel. A retreat exceeding both 25% of peak travel and the
//! axis's activation threshold cancels permanently until release.

const HOLD_NS: u64 = 180_000_000;
const QUICK_NS: u64 = 600_000_000;
const COMMIT_PROGRESS: f64 = 0.22;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Destination {
    Home,
    Overview,
    Previous,
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Update {
    Begin,
    Drag { progress: f32 },
    End { destination: Destination },
    Cancel,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(super) struct Step {
    pub consumed: bool,
    pub update: Option<Update>,
}

impl Step {
    fn owned(update: Option<Update>) -> Self {
        Self {
            consumed: true,
            update,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Geometry {
    width: u32,
    height: u32,
    scale_milli: u32,
}

impl Geometry {
    fn valid(self) -> bool {
        self.width > 0 && self.height > 0 && self.scale_milli > 0
    }

    fn logical(self, value: f64) -> f64 {
        value * f64::from(self.scale_milli) / 1000.0
    }

    fn eligible(self, x: i32, y: i32) -> bool {
        let (x, y) = (f64::from(x), f64::from(y));
        let (width, height) = (f64::from(self.width), f64::from(self.height));
        self.valid()
            && x >= width * 0.2
            && x <= width * 0.8
            && y >= 0.0
            && y <= height
            && height - y <= self.logical(12.0)
    }
}

#[derive(Debug, Clone, Copy)]
struct Gesture {
    id: i32,
    geometry: Geometry,
    start_x: f64,
    start_y: f64,
    dx: f64,
    up: f64,
    peak_up: f64,
    peak_horizontal: f64,
    motion_x: f64,
    motion_y: f64,
    started_ns: u64,
    last_motion_ns: u64,
    last_frame_ns: u64,
    begun: bool,
}

impl Gesture {
    fn progress(self) -> f64 {
        (self.up / (f64::from(self.geometry.height) * 0.4)).clamp(0.0, 1.0)
    }

    fn moved(&mut self, x: i32, y: i32, now_ns: u64) {
        let (x, y) = (f64::from(x), f64::from(y));
        self.dx = x - self.start_x;
        self.up = self.start_y - y;
        self.peak_up = self.peak_up.max(self.up);
        if self.dx * self.peak_horizontal >= 0.0 && self.dx.abs() > self.peak_horizontal.abs() {
            self.peak_horizontal = self.dx;
        }
        // Ignore at most two logical pixels of sensor jitter. The anchor only
        // moves with significant motion, so repeated small steps accumulate.
        let (mx, my) = (x - self.motion_x, y - self.motion_y);
        let jitter = self.geometry.logical(2.0);
        if mx * mx + my * my > jitter * jitter {
            self.motion_x = x;
            self.motion_y = y;
            self.last_motion_ns = now_ns;
        }
        self.last_frame_ns = now_ns;
    }

    fn reversed(self) -> bool {
        if self.peak_up >= self.peak_horizontal.abs() {
            let retreat = self.peak_up - self.up;
            retreat > self.geometry.logical(12.0) && retreat > self.peak_up * 0.25
        } else {
            let retreat = self.peak_horizontal.abs() - self.dx * self.peak_horizontal.signum();
            retreat > self.geometry.logical(20.0) && retreat > self.peak_horizontal.abs() * 0.25
        }
    }

    fn destination(self, now_ns: u64) -> Option<Destination> {
        if !self.begun {
            return None;
        }
        if self.dx.abs() > self.geometry.logical(60.0) && self.dx.abs() > self.up.abs() * 1.5 {
            return Some(if self.dx > 0.0 {
                Destination::Previous
            } else {
                Destination::Next
            });
        }
        // An ambiguous diagonal must not accidentally become a Home swipe.
        if self.up <= self.dx.abs() || self.progress() <= COMMIT_PROGRESS {
            return None;
        }
        if now_ns.saturating_sub(self.last_motion_ns) >= HOLD_NS {
            Some(Destination::Overview)
        } else if now_ns.saturating_sub(self.started_ns) <= QUICK_NS
            && self.up >= self.geometry.logical(32.0)
        {
            Some(Destination::Home)
        } else {
            None
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
enum State {
    #[default]
    Idle,
    Tracking(Gesture),
    Draining,
}

#[derive(Debug, Default)]
pub(super) struct Navigation {
    state: State,
    prior_ids: std::vec::Vec<i32>,
}

impl Navigation {
    /// Whether this recognizer owns the stream, including cancellation draining.
    pub(super) fn is_captured(&self) -> bool {
        matches!(self.state, State::Tracking(_) | State::Draining)
    }

    /// Cancel any captured gesture, retaining ownership until all contacts lift.
    /// Return `Cancel` once, including for tentative capture before `Begin`.
    /// Keep observing the same source after reset so its drain can complete.
    pub(super) fn reset(&mut self) -> Option<Update> {
        if matches!(self.state, State::Tracking(_)) {
            self.state = State::Draining;
            Some(Update::Cancel)
        } else {
            None
        }
    }

    /// Process a complete snapshot for a single, caller-selected device stream.
    /// Empty snapshots end gestures; cancellation never commits a destination.
    /// A geometry change or backwards timestamp cancels an owned gesture.
    pub(super) fn process(
        &mut self,
        contacts: &[(i32, i32, i32)],
        cancelled: bool,
        now_ns: u64,
        width: u32,
        height: u32,
        scale_milli: u32,
    ) -> Step {
        let fresh = self.prior_ids.is_empty();
        self.prior_ids.clear();
        self.prior_ids.extend(contacts.iter().map(|&(id, _, _)| id));
        let geometry = Geometry {
            width,
            height,
            scale_milli,
        };
        match self.state {
            State::Idle => {
                if !cancelled && fresh && contacts.len() == 1 {
                    let (id, x, y) = contacts[0];
                    if geometry.eligible(x, y) {
                        self.state = State::Tracking(Gesture {
                            id,
                            geometry,
                            start_x: f64::from(x),
                            start_y: f64::from(y),
                            dx: 0.0,
                            up: 0.0,
                            peak_up: 0.0,
                            peak_horizontal: 0.0,
                            motion_x: f64::from(x),
                            motion_y: f64::from(y),
                            started_ns: now_ns,
                            last_motion_ns: now_ns,
                            last_frame_ns: now_ns,
                            begun: false,
                        });
                        return Step::owned(None);
                    }
                }
                Step::default()
            }
            State::Draining => {
                if contacts.is_empty() {
                    self.state = State::Idle;
                }
                Step::owned(None)
            }
            State::Tracking(mut gesture) => {
                if cancelled
                    || geometry != gesture.geometry
                    || now_ns < gesture.last_frame_ns
                    || contacts.len() > 1
                    || contacts.first().is_some_and(|c| c.0 != gesture.id)
                {
                    return self.cancel(contacts.is_empty());
                }
                if contacts.is_empty() {
                    self.state = State::Idle;
                    return Step::owned(Some(match gesture.destination(now_ns) {
                        Some(destination) => Update::End { destination },
                        None => Update::Cancel,
                    }));
                }
                let (_, x, y) = contacts[0];
                gesture.moved(x, y, now_ns);
                if gesture.reversed() {
                    return self.cancel(false);
                }
                let update = if !gesture.begun {
                    if gesture.up > geometry.logical(12.0)
                        || gesture.dx.abs() > geometry.logical(20.0)
                    {
                        gesture.begun = true;
                        Some(Update::Begin)
                    } else {
                        None
                    }
                } else {
                    Some(Update::Drag {
                        progress: gesture.progress() as f32,
                    })
                };
                self.state = State::Tracking(gesture);
                Step::owned(update)
            }
        }
    }

    fn cancel(&mut self, all_up: bool) -> Step {
        self.state = if all_up { State::Idle } else { State::Draining };
        Step::owned(Some(Update::Cancel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn capture_query_includes_tentative_capture_and_drain_until_release() {
        let mut n = Navigation::default();
        assert!(!n.is_captured());
        finger(&mut n, 500, 995, 0);
        assert!(n.is_captured());
        n.reset();
        assert!(n.is_captured());
        finger(&mut n, 500, 795, 10);
        assert!(n.is_captured());
        frame(&mut n, &[], 20);
        assert!(!n.is_captured());
        finger(&mut n, 500, 995, 30);
        finger(&mut n, 500, 795, 40);
        assert!(n.is_captured());
        frame(&mut n, &[], 50);
        assert!(!n.is_captured());
    }

    fn frame(n: &mut Navigation, contacts: &[(i32, i32, i32)], ms: u64) -> Step {
        n.process(contacts, false, ms * MS, 1000, 1000, 1000)
    }

    fn finger(n: &mut Navigation, x: i32, y: i32, ms: u64) -> Step {
        frame(n, &[(7, x, y)], ms)
    }

    fn end(destination: Destination) -> Step {
        Step::owned(Some(Update::End { destination }))
    }

    #[test]
    fn upward_swipe_begins_once_drags_and_goes_home() {
        let mut n = Navigation::default();
        assert_eq!(finger(&mut n, 500, 995, 0), Step::owned(None));
        assert_eq!(finger(&mut n, 500, 983, 10), Step::owned(None));
        assert_eq!(finger(&mut n, 500, 982, 20).update, Some(Update::Begin));
        assert_eq!(
            finger(&mut n, 500, 795, 100).update,
            Some(Update::Drag { progress: 0.5 })
        );
        assert_eq!(frame(&mut n, &[], 120), end(Destination::Home));
        assert_eq!(frame(&mut n, &[], 130), Step::default());
    }

    #[test]
    fn stationary_release_selects_overview_without_timer_events() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 895, 100);
        assert_eq!(frame(&mut n, &[], 280), end(Destination::Overview));
    }

    #[test]
    fn hold_boundary_and_small_sensor_jitter() {
        for (release, destination) in [(279, Destination::Home), (280, Destination::Overview)] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, 500, 895, 100);
            finger(&mut n, 501, 895, 260);
            assert_eq!(frame(&mut n, &[], release), end(destination));
        }
    }

    #[test]
    fn slow_continuous_motion_is_not_a_hold() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        for ms in (100..=1000).step_by(100) {
            finger(&mut n, 500, 995 - (ms / 5) as i32, ms);
        }
        assert_eq!(frame(&mut n, &[], 1010).update, Some(Update::Cancel));
    }

    #[test]
    fn small_steps_accumulate_and_reset_stationary_clock() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 895, 100);
        for i in 1..=10 {
            finger(&mut n, 500, 895 - i as i32, 100 + i * 40);
        }
        assert_eq!(frame(&mut n, &[], 510), end(Destination::Home));
    }

    #[test]
    fn horizontal_swipes_map_direction_and_require_strict_threshold() {
        for (dx, destination) in [(61_i32, Destination::Previous), (-61, Destination::Next)] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            assert_eq!(finger(&mut n, 500 + dx.signum() * 20, 995, 10).update, None);
            assert_eq!(
                finger(&mut n, 500 + dx, 995, 50).update,
                Some(Update::Begin)
            );
            assert_eq!(frame(&mut n, &[], 80), end(destination));
        }
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 560, 995, 50);
        assert_eq!(frame(&mut n, &[], 80).update, Some(Update::Cancel));
    }

    #[test]
    fn horizontal_dominance_is_strict_and_ambiguous_diagonal_cancels() {
        for (dx, expected) in [
            (150, Step::owned(Some(Update::Cancel))),
            (151, end(Destination::Previous)),
        ] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, 500 + dx, 895, 100);
            assert_eq!(frame(&mut n, &[], 120), expected);
        }
    }

    #[test]
    fn keyboard_edge_controls_and_outside_bottom_strip_are_unclaimed() {
        for (x, y) in [
            (199, 999),
            (801, 999),
            (0, 999),
            (999, 999),
            (500, 987),
            (500, 1001),
            (500, -1),
        ] {
            let mut n = Navigation::default();
            assert_eq!(finger(&mut n, x, y, 0), Step::default());
            assert_eq!(finger(&mut n, 500, 995, 10), Step::default());
            assert_eq!(finger(&mut n, 500, 700, 20), Step::default());
            assert_eq!(frame(&mut n, &[], 30), Step::default());
        }
        for x in [200, 800] {
            let mut n = Navigation::default();
            assert_eq!(finger(&mut n, x, 988, 0), Step::owned(None));
        }
    }

    #[test]
    fn tap_and_short_swipes_cancel_even_after_begin() {
        for y in [995, 990, 980, 920, 907] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, 500, y, 50);
            assert_eq!(frame(&mut n, &[], 100), Step::owned(Some(Update::Cancel)));
        }
    }

    #[test]
    fn reversal_cancels_and_drains_even_if_finger_swipes_again() {
        for (far_x, far_y, back_x, back_y) in [
            (500, 795, 500, 900),
            (600, 995, 550, 995),
            (400, 995, 450, 995),
            (600, 995, 350, 995),
            (400, 995, 650, 995),
        ] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, far_x, far_y, 100);
            assert_eq!(
                finger(&mut n, back_x, back_y, 150).update,
                Some(Update::Cancel)
            );
            assert_eq!(finger(&mut n, far_x, far_y, 200), Step::owned(None));
            assert_eq!(frame(&mut n, &[], 220), Step::owned(None));
            assert_eq!(frame(&mut n, &[], 230), Step::default());
        }
    }

    #[test]
    fn small_reversal_tolerance_does_not_cancel() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 795, 100);
        assert!(matches!(
            finger(&mut n, 500, 800, 110).update,
            Some(Update::Drag { .. })
        ));
        assert_eq!(frame(&mut n, &[], 120), end(Destination::Home));
    }

    #[test]
    fn multitouch_cancels_once_and_never_leaks_remaining_fingers() {
        for begun in [false, true] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            if begun {
                finger(&mut n, 500, 795, 10);
            }
            assert_eq!(
                frame(&mut n, &[(7, 500, 795), (8, 600, 995)], 20).update,
                Some(Update::Cancel)
            );
            assert_eq!(frame(&mut n, &[(8, 600, 995)], 30), Step::owned(None));
            assert_eq!(frame(&mut n, &[(9, 500, 995)], 40), Step::owned(None));
            assert_eq!(frame(&mut n, &[], 50), Step::owned(None));
            assert_eq!(finger(&mut n, 500, 995, 60), Step::owned(None));
            assert_eq!(finger(&mut n, 500, 795, 70).update, Some(Update::Begin));
        }
    }

    #[test]
    fn initially_multiple_contacts_and_replacement_ids_do_not_start() {
        let mut n = Navigation::default();
        assert_eq!(
            frame(&mut n, &[(1, 500, 995), (2, 600, 995)], 0),
            Step::default()
        );
        assert_eq!(frame(&mut n, &[(2, 600, 995)], 10), Step::default());
        assert_eq!(finger(&mut n, 500, 995, 20), Step::default());
        frame(&mut n, &[], 30);
        assert_eq!(finger(&mut n, 500, 995, 40), Step::owned(None));
    }

    #[test]
    fn reused_id_is_fresh_after_release_but_replacement_cancels_capture() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        frame(&mut n, &[], 10);
        finger(&mut n, 500, 995, 20);
        assert_eq!(finger(&mut n, 500, 795, 30).update, Some(Update::Begin));
        assert_eq!(
            frame(&mut n, &[(8, 500, 795)], 40).update,
            Some(Update::Cancel)
        );
        assert_eq!(frame(&mut n, &[], 50), Step::owned(None));
    }

    #[test]
    fn explicit_cancel_with_or_without_contacts_never_commits() {
        for remaining in [false, true] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, 500, 795, 50);
            let contacts = if remaining {
                &[(7, 500, 795)][..]
            } else {
                &[][..]
            };
            assert_eq!(
                n.process(contacts, true, 100 * MS, 1000, 1000, 1000),
                Step::owned(Some(Update::Cancel))
            );
            if remaining {
                assert_eq!(finger(&mut n, 500, 995, 110), Step::owned(None));
                assert_eq!(frame(&mut n, &[], 120), Step::owned(None));
            }
            assert_eq!(frame(&mut n, &[], 130), Step::default());
        }
    }

    #[test]
    fn cancelled_initial_contact_does_not_start_later() {
        let mut n = Navigation::default();
        assert_eq!(
            n.process(&[(7, 500, 995)], true, 0, 1000, 1000, 1000),
            Step::default()
        );
        assert_eq!(finger(&mut n, 500, 995, 10), Step::default());
        assert_eq!(n.reset(), None);
        assert_eq!(finger(&mut n, 500, 995, 20), Step::default());
    }

    #[test]
    fn reset_returns_cancel_once_and_retains_drain_and_contact_history() {
        for begun in [false, true] {
            let mut n = Navigation::default();
            assert_eq!(n.reset(), None);
            finger(&mut n, 500, 995, 0);
            if begun {
                finger(&mut n, 500, 795, 10);
            }
            assert_eq!(n.reset(), Some(Update::Cancel));
            assert_eq!(n.reset(), None);
            assert_eq!(finger(&mut n, 500, 795, 20), Step::owned(None));
            assert_eq!(frame(&mut n, &[], 30), Step::owned(None));
            assert_eq!(finger(&mut n, 500, 995, 40), Step::owned(None));
            assert_eq!(finger(&mut n, 500, 795, 50).update, Some(Update::Begin));
        }
    }

    #[test]
    fn scale_affects_edge_and_thresholds_but_not_normalized_progress() {
        for scale in [1000, 1500, 2000] {
            let mut n = Navigation::default();
            let px = |logical: i32| logical * scale / 1000;
            let mut send = |x, y, ms| {
                n.process(
                    &[(7, px(x), px(y))],
                    false,
                    ms * MS,
                    px(1000) as u32,
                    px(1000) as u32,
                    scale as u32,
                )
            };
            assert_eq!(send(500, 988, 0), Step::owned(None));
            assert_eq!(send(500, 976, 10).update, None);
            assert_eq!(send(500, 974, 20).update, Some(Update::Begin));
            assert_eq!(
                send(500, 788, 30).update,
                Some(Update::Drag { progress: 0.5 })
            );
            assert_eq!(
                n.process(
                    &[],
                    false,
                    40 * MS,
                    px(1000) as u32,
                    px(1000) as u32,
                    scale as u32
                ),
                end(Destination::Home)
            );
        }
    }

    #[test]
    fn progress_clamps_and_extreme_coordinates_do_not_overflow() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 900, 10);
        assert_eq!(
            finger(&mut n, 500, i32::MIN, 20).update,
            Some(Update::Drag { progress: 1.0 })
        );
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 530, 995, 10);
        assert_eq!(
            finger(&mut n, 540, 1000, 20).update,
            Some(Update::Drag { progress: 0.0 })
        );
    }

    #[test]
    fn invalid_geometry_does_not_capture_and_changes_cancel() {
        for (w, h, scale) in [(0, 1000, 1000), (1000, 0, 1000), (1000, 1000, 0)] {
            let mut n = Navigation::default();
            assert_eq!(
                n.process(&[(7, 500, 995)], false, 0, w, h, scale),
                Step::default()
            );
            assert_eq!(finger(&mut n, 500, 995, 10), Step::default());
        }
        for (w, h, scale) in [(1001, 1000, 1000), (1000, 1001, 1000), (1000, 1000, 2000)] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            assert_eq!(
                n.process(&[(7, 500, 795)], false, MS, w, h, scale).update,
                Some(Update::Cancel)
            );
            assert_eq!(frame(&mut n, &[], 10), Step::owned(None));
        }
    }

    #[test]
    fn backwards_time_cancels_without_underflow() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 100);
        assert_eq!(finger(&mut n, 500, 795, 99).update, Some(Update::Cancel));
        assert_eq!(frame(&mut n, &[], 101), Step::owned(None));
    }
}
