//! Bottom-edge navigation for one device's complete contact snapshots.
//!
//! Coordinates and dimensions are output pixels; `scale_milli` is pixels per
//! logical pixel times 1000. Call even for unclaimed frames and the final empty
//! frame: contact history prevents an existing finger from entering the edge.
//! `consumed` covers tentative gestures and the final release/drain frame.
//! Only `Begin` should start changing compositor focus or presentation. It has
//! no coordinates; derive its initial presentation from that contact snapshot.
//! Subsequent `Drag`s report clamped 0..1 upward progress, raw horizontal output
//! pixels from the original down (right positive), and Overview readiness.
//!
//! Progress reaches one at 40% of output height. Above 0.18 progress (7.2% of
//! output height) and at least 32 logical pixels up, pausing for >=100 ms
//! marks the Overview preview ready, allowing two logical pixels of sensor
//! jitter. The hold affects preview readiness only. Readiness stays latched
//! during motion above that threshold; retreating below it clears readiness
//! and permits a new pause. Call `tick` on every compositor tick,
//! using the same monotonic clock as `process`, to reveal a pause without
//! waiting for a touch event.
//! `next_deadline_ns` supplies the pending hold's absolute wakeup time; after
//! readiness there is no further timer, so a stationary hold needs no polling.
//! Position updates inspect motion before maturing a pending hold: sparse
//! movement samples alone do not prove a pause. Ticks use the latest known
//! position; readiness already exposed by a tick remains latched above threshold.
//!
//! Release selects a workspace offset once deliberate sideways travel exceeds
//! 60 logical pixels: either a horizontal-dominant gesture or a sideways leg
//! after moving up. Incidental diagonal drift does not switch tasks. Offsets use the
//! nearest `page_stride`, with a minimum magnitude of one; finger left means
//! positive. Otherwise, full progress (at least 40% of output height up)
//! selects Home, even after a pause. Any intermediate rise beyond the same
//! 12-logical-pixel activation slop selects Overview, with or without a pause.
//! Release destinations do not depend on duration or preview readiness.
//! Below-threshold releases cancel. Returning inside the horizontal threshold
//! reselects Home or Overview according to the final upward travel.
//! Reversals never permanently cancel: only stream invalidation (cancel,
//! reset, geometry/time changes, multitouch or contact replacement) drains
//! exclusively until all contacts lift. A capture requires an initially fresh
//! single contact; fingers already observed elsewhere cannot enter the edge.

const HOLD_NS: u64 = 100_000_000;
const COMMIT_PROGRESS: f64 = 0.18;

/// Distance between workspace pages in output pixels, shared with rendering.
/// The one-pixel floor keeps division defined even for zero/tiny widths;
/// zero-width output geometry itself is never eligible for capture.
pub fn page_stride(width: u32) -> f32 {
    (width as f32 * 0.65).max(1.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Destination {
    Home,
    Overview,
    /// Relative workspace offset: leftward finger movement is positive.
    Switch(i32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Update {
    Begin,
    Drag {
        progress: f32,
        horizontal: f32,
        overview_ready: bool,
    },
    End {
        destination: Destination,
    },
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
    upward_anchor: f64,
    sideways_origin_x: f64,
    motion_x: f64,
    motion_y: f64,
    last_motion_ns: u64,
    last_frame_ns: u64, // Touch-event ordering only; ticks never advance this.
    above_since_ns: Option<u64>,
    overview_ready: bool,
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
        // Keep the horizontal leg separate from drift during an upward swipe.
        // Unlike requiring total horizontal dominance, this still allows an
        // up-then-sideways task switch without lifting or waiting for a hold.
        if self.up > self.upward_anchor + self.geometry.logical(2.0) {
            self.upward_anchor = self.up;
            self.sideways_origin_x = x;
        }
        if self.above_threshold() {
            self.above_since_ns.get_or_insert(now_ns);
        } else {
            self.above_since_ns = None;
            self.overview_ready = false;
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
        // A late movement sample does not prove the finger paused before it.
        // Inspect motion first; only stationary/jitter samples can mature a
        // pending hold here. Readiness already exposed by tick stays latched.
        self.refresh_readiness(now_ns);
    }

    fn above_threshold(self) -> bool {
        self.progress() > COMMIT_PROGRESS && self.up >= self.geometry.logical(32.0)
    }

    fn refresh_readiness(&mut self, now_ns: u64) {
        if self
            .next_deadline_ns()
            .is_some_and(|deadline| now_ns >= deadline)
        {
            self.overview_ready = true;
        }
    }

    fn next_deadline_ns(self) -> Option<u64> {
        if !self.begun || self.overview_ready {
            return None;
        }
        self.last_motion_ns
            .max(self.above_since_ns?)
            .checked_add(HOLD_NS)
    }

    fn drag(self) -> Update {
        Update::Drag {
            progress: self.progress() as f32,
            horizontal: self.dx as f32,
            overview_ready: self.overview_ready,
        }
    }

    fn destination(self) -> Option<Destination> {
        if !self.begun {
            return None;
        }
        let sideways = self.start_x + self.dx - self.sideways_origin_x;
        if self.dx.abs() > self.geometry.logical(60.0)
            && (self.dx.abs() > self.up.max(0.0)
                || (sideways.abs() > self.geometry.logical(60.0)
                    && sideways.signum() == self.dx.signum()))
        {
            let pages = (self.dx.abs() / f64::from(page_stride(self.geometry.width)))
                .round()
                .max(1.0) as i32;
            return Some(Destination::Switch(if self.dx < 0.0 {
                pages
            } else {
                -pages
            }));
        }
        if self.progress() >= 1.0 {
            Some(Destination::Home)
        } else if self.up > self.geometry.logical(12.0) {
            Some(Destination::Overview)
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
    /// Advance pause detection on each compositor tick using the touch clock.
    /// Emit a `Drag` only when readiness changes, never `Begin` or `End`.
    /// Ticks preceding the latest touch event are ignored. Ticks never advance
    /// the event-ordering timestamp: queued, ordered events may predate a tick.
    /// Only backwards touch-event timestamps cancel in `process`.
    pub(super) fn tick(&mut self, now_ns: u64) -> Option<Update> {
        let State::Tracking(gesture) = &mut self.state else {
            return None;
        };
        if !gesture.begun || now_ns < gesture.last_frame_ns {
            return None;
        }
        let was_ready = gesture.overview_ready;
        gesture.refresh_readiness(now_ns);
        (was_ready != gesture.overview_ready).then(|| gesture.drag())
    }

    /// Absolute monotonic deadline for a pending pause; re-query after input.
    /// Call `tick` at/after this time. No timer is needed while idle, draining,
    /// below threshold or already ready, or if the deadline cannot fit in u64.
    pub(super) fn next_deadline_ns(&self) -> Option<u64> {
        match self.state {
            State::Tracking(gesture) => gesture.next_deadline_ns(),
            _ => None,
        }
    }

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
                            upward_anchor: 0.0,
                            sideways_origin_x: f64::from(x),
                            motion_x: f64::from(x),
                            motion_y: f64::from(y),
                            last_motion_ns: now_ns,
                            last_frame_ns: now_ns,
                            above_since_ns: None,
                            overview_ready: false,
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
                    gesture.refresh_readiness(now_ns);
                    self.state = State::Idle;
                    return Step::owned(Some(match gesture.destination() {
                        Some(destination) => Update::End { destination },
                        None => Update::Cancel,
                    }));
                }
                let (_, x, y) = contacts[0];
                gesture.moved(x, y, now_ns);
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
                    Some(gesture.drag())
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

    fn drag(progress: f32, horizontal: f32, overview_ready: bool) -> Option<Update> {
        Some(Update::Drag {
            progress,
            horizontal,
            overview_ready,
        })
    }

    #[test]
    fn intermediate_swipe_begins_once_drags_and_stays_in_overview() {
        let mut n = Navigation::default();
        assert_eq!(finger(&mut n, 500, 995, 0), Step::owned(None));
        assert_eq!(finger(&mut n, 500, 983, 10), Step::owned(None));
        assert_eq!(finger(&mut n, 500, 982, 20).update, Some(Update::Begin));
        assert_eq!(finger(&mut n, 500, 795, 100).update, drag(0.5, 0.0, false));
        assert_eq!(frame(&mut n, &[], 120), end(Destination::Overview));
        assert_eq!(frame(&mut n, &[], 130), Step::default());
    }

    #[test]
    fn every_activated_upward_release_has_a_destination_at_all_scales() {
        // Include the old gap (13..72 logical pixels), the overview range,
        // and the exact Home boundary, after a tentative sideways start.
        for scale in [1000, 1500, 2000] {
            for height in [400, 720, 1200] {
                let px = |logical: i32| logical * scale / 1000;
                for up in 13..=height * 2 / 5 + 1 {
                    for sideways in [-50, 0, 50] {
                        let mut n = Navigation::default();
                        let mut send = |contacts: &[(i32, i32, i32)], ms| {
                            n.process(contacts, false, ms * MS, px(1000) as u32,
                                px(height) as u32, scale as u32)
                        };
                        send(&[(7, px(500), px(height - 5))], 0);
                        send(&[(7, px(500 + sideways), px(height - 5))], 10);
                        send(&[(7, px(500 + sideways), px(height - 5 - up))], 20);
                        let destination = if px(up) as f64 >= f64::from(px(height)) * 0.4 {
                            Destination::Home
                        } else {
                            Destination::Overview
                        };
                        assert_eq!(send(&[], 30), end(destination),
                            "scale={scale} height={height} up={up} sideways={sideways}");
                    }
                }
            }
        }
    }

    #[test]
    fn diagonal_upward_swipes_do_not_become_task_switches() {
        for sign in [-1, 1] {
            for up in [80, 200, 399, 400] {
                let mut n = Navigation::default();
                finger(&mut n, 500, 995, 0);
                finger(&mut n, 500 + sign * 30, 955, 20);
                finger(&mut n, 500 + sign * 70, 995 - up, 40);
                assert_eq!(frame(&mut n, &[], 50), end(if up >= 400 {
                    Destination::Home
                } else {
                    Destination::Overview
                }));
            }
        }
    }

    #[test]
    fn sideways_retreat_preserves_vertical_overview_until_return_to_edge() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 945, 20);
        finger(&mut n, 600, 945, 40);
        finger(&mut n, 540, 945, 60);
        assert_eq!(frame(&mut n, &[], 80), end(Destination::Overview));
    }

    #[test]
    fn stationary_release_selects_overview_without_timer_events() {
        for release in [101, 199, 200, 60_000] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, 500, 895, 100);
            assert_eq!(frame(&mut n, &[], release), end(Destination::Overview));
        }
    }

    #[test]
    fn hold_boundary_and_jitter_change_preview_only_not_release_destination() {
        for (release, ready) in [(199, false), (200, true)] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, 500, 895, 100);
            finger(&mut n, 501, 895, 180);
            assert_eq!(
                finger(&mut n, 501, 895, release).update,
                drag(0.25, 1.0, ready)
            );
            assert_eq!(frame(&mut n, &[], release), end(Destination::Overview));
        }
    }

    #[test]
    fn short_rise_and_brief_pause_select_overview_but_quick_full_swipe_goes_home() {
        for with_tick in [false, true] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, 500, 975, 20);
            // Eight percent of height qualifies for Overview; 100 ms readies its preview.
            assert_eq!(finger(&mut n, 500, 915, 80).update, drag(0.2, 0.0, false));
            assert_eq!(n.next_deadline_ns(), Some(180 * MS));
            assert_eq!(n.tick(179 * MS), None);
            if with_tick {
                assert_eq!(n.tick(180 * MS), drag(0.2, 0.0, true));
            }
            assert_eq!(frame(&mut n, &[], 180), end(Destination::Overview));
        }

        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 975, 20);
        assert_eq!(finger(&mut n, 500, 915, 40).update, drag(0.2, 0.0, false));
        assert_eq!(finger(&mut n, 500, 795, 80).update, drag(0.5, 0.0, false));
        assert_eq!(finger(&mut n, 500, 595, 120).update, drag(1.0, 0.0, false));
        assert_eq!(frame(&mut n, &[], 130), end(Destination::Home));
    }

    #[test]
    fn overview_pause_preview_has_32_pixel_floor_but_release_has_no_gap() {
        for scale in [1000, 1500, 2000] {
            let px = |logical: i32| logical * scale / 1000;
            for (up, ready) in [(30, false), (32, true)] {
                let mut n = Navigation::default();
                n.process(
                    &[(7, px(500), px(395))],
                    false,
                    0,
                    px(1000) as u32,
                    px(400) as u32,
                    scale as u32,
                );
                assert_eq!(
                    n.process(
                        &[(7, px(500), px(395 - up))],
                        false,
                        50 * MS,
                        px(1000) as u32,
                        px(400) as u32,
                        scale as u32
                    )
                    .update,
                    Some(Update::Begin)
                );
                // Both distances exceed 7.2% of height; only 32 meets the floor.
                assert_eq!(n.next_deadline_ns(), ready.then_some(150 * MS));
                assert_eq!(n.tick(149 * MS), None);
                assert_eq!(
                    n.tick(150 * MS),
                    ready.then(|| Update::Drag {
                        progress: 0.2,
                        horizontal: 0.0,
                        overview_ready: true,
                    })
                );
                assert_eq!(
                    n.process(
                        &[],
                        false,
                        150 * MS,
                        px(1000) as u32,
                        px(400) as u32,
                        scale as u32
                    ),
                    end(Destination::Overview)
                );
            }
        }
    }

    #[test]
    fn tick_exposes_pause_once_at_exact_boundary_without_touch_frames() {
        let mut n = Navigation::default();
        assert_eq!(n.tick(0), None);
        finger(&mut n, 500, 995, 0);
        assert_eq!(n.tick(1000 * MS), None); // A held edge tap never begins.
        assert_eq!(finger(&mut n, 500, 795, 1000).update, Some(Update::Begin));
        assert_eq!(n.tick(1099 * MS), None);
        assert_eq!(n.tick(1100 * MS), drag(0.5, 0.0, true));
        assert_eq!(n.tick(1101 * MS), None);
        assert_eq!(n.tick(2000 * MS), None);
        assert_eq!(frame(&mut n, &[], 2010), end(Destination::Overview));
        assert_eq!(n.tick(3000 * MS), None);
    }

    #[test]
    fn pause_then_browse_multiple_pages_and_reverse_while_held() {
        for with_tick in [false, true] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, 500, 795, 100);
            if with_tick {
                assert_eq!(n.tick(280 * MS), drag(0.5, 0.0, true));
            }
            // Original-down displacement, not displacement since the pause.
            assert_eq!(
                finger(&mut n, -800, 795, 300).update,
                drag(0.5, -1300.0, with_tick)
            );
            assert_eq!(
                finger(&mut n, 2450, 795, 350).update,
                drag(0.5, 1950.0, with_tick)
            );
            assert_eq!(n.tick(351 * MS), None);
            assert_eq!(frame(&mut n, &[], 360), end(Destination::Switch(-3)));
        }
    }

    #[test]
    fn browsing_back_to_origin_reselects_overview() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 795, 100);
        n.tick(280 * MS);
        finger(&mut n, -800, 795, 300);
        assert_eq!(finger(&mut n, 500, 795, 350).update, drag(0.5, 0.0, true));
        assert_eq!(frame(&mut n, &[], 360), end(Destination::Overview));
    }

    #[test]
    fn retreat_clears_readiness_and_a_new_pause_can_select_again() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 795, 100);
        assert_eq!(n.tick(280 * MS), drag(0.5, 0.0, true));
        assert_eq!(finger(&mut n, 500, 995, 300).update, drag(0.0, 0.0, false));
        assert_eq!(n.tick(1000 * MS), None);
        assert_eq!(finger(&mut n, 500, 795, 1100).update, drag(0.5, 0.0, false));
        assert_eq!(n.tick(1199 * MS), None);
        assert_eq!(n.tick(1200 * MS), drag(0.5, 0.0, true));
        assert_eq!(frame(&mut n, &[], 1300), end(Destination::Overview));
    }

    #[test]
    fn retreat_to_origin_cancels_only_on_release() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 795, 100);
        assert_eq!(finger(&mut n, 500, 995, 150).update, drag(0.0, 0.0, false));
        assert!(n.is_captured());
        assert_eq!(frame(&mut n, &[], 200), Step::owned(Some(Update::Cancel)));
        assert!(!n.is_captured());
    }

    #[test]
    fn pause_starts_above_threshold_even_when_crossing_is_only_jitter() {
        for with_preview in [false, true] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, 500, 923, 100); // Exactly 0.18 is not enough.
            assert_eq!(n.tick(500 * MS), None);
            assert_eq!(
                finger(&mut n, 500, 922, 510).update,
                drag(0.1825, 0.0, false)
            );
            let release = if with_preview {
                assert_eq!(n.tick(609 * MS), None);
                assert_eq!(n.tick(610 * MS), drag(0.1825, 0.0, true));
                610
            } else {
                510
            };
            assert_eq!(frame(&mut n, &[], release), end(Destination::Overview));
        }
    }

    #[test]
    fn horizontal_motion_before_readiness_restarts_pause_clock() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 795, 100);
        finger(&mut n, 550, 795, 190);
        assert_eq!(n.tick(200 * MS), None);
        assert_eq!(n.tick(289 * MS), None);
        assert_eq!(n.tick(290 * MS), drag(0.5, 50.0, true));
        assert_eq!(frame(&mut n, &[], 291), end(Destination::Overview));
    }

    #[test]
    fn full_swipe_boundary_ignores_diagonal_drift_pause_and_duration() {
        for paused in [false, true] {
            for time in [300, 10_000] {
                for dx in [-61, -60, 0, 60, 61] {
                    for up in [399, 400, 401] {
                        let mut n = Navigation::default();
                        finger(&mut n, 500, 995, 0);
                        finger(&mut n, 500, 795, 100);
                        if paused {
                            assert_eq!(n.tick(200 * MS), drag(0.5, 0.0, true));
                        }
                        assert_eq!(
                            finger(&mut n, 500 + dx, 995 - up, time).update,
                            drag((up as f32 / 400.0).min(1.0), dx as f32, paused)
                        );
                        let destination = if up >= 400 {
                            Destination::Home
                        } else {
                            Destination::Overview
                        };
                        assert_eq!(frame(&mut n, &[], time + 10), end(destination));
                    }
                }
            }
        }
    }

    #[test]
    fn returning_from_horizontal_motion_reselects_overview_without_a_pause() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 300, 795, 100);
        assert_eq!(finger(&mut n, 500, 795, 150).update, drag(0.5, 0.0, false));
        assert_eq!(frame(&mut n, &[], 160), end(Destination::Overview));
    }

    #[test]
    fn page_stride_and_nearest_arbitrary_offsets_have_defined_boundaries() {
        assert_eq!(page_stride(0), 1.0);
        assert_eq!(page_stride(1), 1.0);
        assert_eq!(page_stride(1000), 650.0);
        for (distance, pages) in [
            (61, 1),
            (974, 1),
            (975, 2),
            (1624, 2),
            (1625, 3),
            (6500, 10),
        ] {
            for sign in [-1, 1] {
                let mut n = Navigation::default();
                finger(&mut n, 500, 995, 0);
                finger(&mut n, 500 + sign * distance, 995, 100);
                assert_eq!(
                    frame(&mut n, &[], 120),
                    end(Destination::Switch(-sign * pages))
                );
            }
        }
    }

    #[test]
    fn tick_does_not_revive_cancelled_streams_or_rewind_touch_time() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 100);
        finger(&mut n, 500, 795, 200);
        assert_eq!(n.tick(199 * MS), None);
        assert_eq!(n.tick(380 * MS), drag(0.5, 0.0, true));
        assert_eq!(n.tick(200 * MS), None);
        assert_eq!(n.reset(), Some(Update::Cancel));
        assert_eq!(n.tick(1000 * MS), None);
        assert_eq!(frame(&mut n, &[], 1010), Step::owned(None));
        assert_eq!(n.tick(1020 * MS), None);
    }

    #[test]
    fn ordered_queued_events_behind_wall_ticks_keep_capture() {
        for (wall, event, ready) in [(150, 140, false), (300, 290, true)] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, 500, 795, 100);
            n.tick(wall * MS);
            assert_eq!(
                finger(&mut n, 490, 795, event).update,
                drag(0.5, -10.0, ready)
            );
            assert!(n.is_captured());
            assert_eq!(frame(&mut n, &[], event + 1), end(Destination::Overview));
        }
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 795, 100);
        n.tick(150 * MS);
        finger(&mut n, 490, 795, 140);
        assert_eq!(finger(&mut n, 480, 795, 139).update, Some(Update::Cancel));
    }

    #[test]
    fn hold_deadline_tracks_motion_jitter_retreat_and_readiness() {
        let mut n = Navigation::default();
        assert_eq!(n.next_deadline_ns(), None);
        finger(&mut n, 500, 995, 0);
        assert_eq!(n.next_deadline_ns(), None);
        finger(&mut n, 500, 923, 50);
        assert_eq!(n.next_deadline_ns(), None);
        finger(&mut n, 500, 922, 100); // Crossing with only one pixel of jitter.
        assert_eq!(n.next_deadline_ns(), Some(200 * MS));
        finger(&mut n, 500, 795, 120);
        assert_eq!(n.next_deadline_ns(), Some(220 * MS));
        finger(&mut n, 501, 795, 200); // Jitter does not postpone the wakeup.
        assert_eq!(n.next_deadline_ns(), Some(220 * MS));
        assert_eq!(n.tick(219 * MS), None);
        assert_eq!(n.next_deadline_ns(), Some(220 * MS));
        assert_eq!(n.tick(220 * MS), drag(0.5, 1.0, true));
        assert_eq!(n.next_deadline_ns(), None);
        finger(&mut n, 400, 795, 310);
        assert_eq!(n.next_deadline_ns(), None);
        finger(&mut n, 500, 995, 320);
        assert_eq!(n.next_deadline_ns(), None);
        finger(&mut n, 500, 795, 330);
        assert_eq!(n.next_deadline_ns(), Some(430 * MS));
        frame(&mut n, &[], 340);
        assert_eq!(n.next_deadline_ns(), None);
    }

    #[test]
    fn cancellation_and_unrepresentable_hold_time_have_no_deadline() {
        for reset in [false, true] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, 500, 795, 100);
            assert_eq!(n.next_deadline_ns(), Some(200 * MS));
            if reset {
                n.reset();
            } else {
                frame(&mut n, &[(7, 500, 795), (8, 600, 995)], 120);
            }
            assert_eq!(n.next_deadline_ns(), None);
            frame(&mut n, &[], 200);
            assert_eq!(n.next_deadline_ns(), None);
        }
        let mut n = Navigation::default();
        n.process(&[(7, 500, 995)], false, u64::MAX - 1, 1000, 1000, 1000);
        n.process(&[(7, 500, 795)], false, u64::MAX, 1000, 1000, 1000);
        assert_eq!(n.next_deadline_ns(), None);
        assert_eq!(n.tick(u64::MAX), None);
    }

    #[test]
    fn slow_continuous_intermediate_motion_releases_to_overview_without_a_hold() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        // Four pixels per sample exceed jitter tolerance throughout this slow rise.
        for ms in (100..=5000).step_by(100) {
            let step = finger(&mut n, 500, 995 - (ms / 25) as i32, ms);
            if ms > 400 {
                assert_eq!(step.update, drag(ms as f32 / 10_000.0, 0.0, false));
            }
        }
        assert_eq!(frame(&mut n, &[], 5010), end(Destination::Overview));
    }

    #[test]
    fn sparse_continuous_motion_does_not_invent_a_hold_before_each_sample() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 895, 100);
        for (ms, y) in [(300, 795), (500, 695), (700, 595), (900, 495)] {
            assert_eq!(
                finger(&mut n, 500, y, ms).update,
                drag(((995 - y) as f32 / 400.0).min(1.0), 0.0, false)
            );
            assert_eq!(n.next_deadline_ns(), Some((ms + 100) * MS));
        }
        assert_eq!(frame(&mut n, &[], 910), end(Destination::Home));
    }

    #[test]
    fn stationary_touch_sample_can_expose_hold_without_a_tick() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 795, 100);
        assert_eq!(finger(&mut n, 501, 795, 200).update, drag(0.5, 1.0, true));
        assert_eq!(n.next_deadline_ns(), None);
        assert_eq!(frame(&mut n, &[], 210), end(Destination::Overview));
    }

    #[test]
    fn small_steps_accumulate_and_reset_stationary_clock() {
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 500, 895, 100);
        for i in 1..=10 {
            assert_eq!(
                finger(&mut n, 500, 895 - i as i32, 100 + i * 40).update,
                drag((100 + i) as f32 / 400.0, 0.0, false)
            );
        }
        assert_eq!(frame(&mut n, &[], 510), end(Destination::Overview));
    }

    #[test]
    fn horizontal_swipes_map_direction_and_require_strict_threshold() {
        for (dx, destination) in [
            (61_i32, Destination::Switch(-1)),
            (-61, Destination::Switch(1)),
        ] {
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
    fn upward_then_horizontal_switches_without_horizontal_dominance() {
        for dx in [-150, -61, 61, 150] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, 500, 795, 50);
            assert_eq!(
                finger(&mut n, 500 + dx, 795, 100).update,
                drag(0.5, dx as f32, false)
            );
            assert_eq!(
                frame(&mut n, &[], 120),
                end(Destination::Switch(-dx.signum()))
            );
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
        for y in [995, 990, 983] {
            for release in [100, 10_000] {
                let mut n = Navigation::default();
                finger(&mut n, 500, 995, 0);
                finger(&mut n, 500, y, 50);
                assert_eq!(
                    frame(&mut n, &[], release),
                    Step::owned(Some(Update::Cancel))
                );
            }
        }
    }

    #[test]
    fn reversal_keeps_capture_and_can_select_again() {
        for (far_x, far_y, back_x, back_y, destination) in [
            (500, 795, 500, 990, Destination::Overview),
            (600, 995, 550, 995, Destination::Switch(-1)),
            (400, 995, 450, 995, Destination::Switch(1)),
            (600, 995, 350, 995, Destination::Switch(-1)),
            (400, 995, 650, 995, Destination::Switch(1)),
        ] {
            let mut n = Navigation::default();
            finger(&mut n, 500, 995, 0);
            finger(&mut n, far_x, far_y, 100);
            assert!(matches!(
                finger(&mut n, back_x, back_y, 150).update,
                Some(Update::Drag { .. })
            ));
            assert!(n.is_captured());
            finger(&mut n, far_x, far_y, 200);
            assert_eq!(frame(&mut n, &[], 220), end(destination));
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
        assert_eq!(frame(&mut n, &[], 120), end(Destination::Overview));
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
            assert_eq!(send(500, 788, 30).update, drag(0.5, 0.0, false));
            assert_eq!(
                n.process(
                    &[],
                    false,
                    40 * MS,
                    px(1000) as u32,
                    px(1000) as u32,
                    scale as u32
                ),
                end(Destination::Overview)
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
            drag(1.0, 0.0, false)
        );
        let mut n = Navigation::default();
        finger(&mut n, 500, 995, 0);
        finger(&mut n, 530, 995, 10);
        assert_eq!(finger(&mut n, 540, 1000, 20).update, drag(0.0, 40.0, false));
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
