//! Multitouch frame assembly and compositor-thread gesture policy.

use std::vec::Vec;

use super::{CompositorInputEvent, PointerSource, key_codes};

/// Coordinate extent used for device-independent touch positions.
pub const TOUCH_COORD_MAX: i32 = 10_000;

/// Whether contacts describe a direct display surface or an indirect pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TouchSurface {
    Touchscreen,
    Touchpad { internal: bool },
}

/// One active type-B multitouch contact in normalized device coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TouchContact {
    pub tracking_id: i32,
    pub x: i32,
    pub y: i32,
    pub pressure: Option<i32>,
    pub touch_major: Option<i32>,
}

/// Complete per-device contact snapshot committed at `SYN_REPORT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TouchFrame {
    pub source: PointerSource,
    pub time_ns: u64,
    pub surface: TouchSurface,
    pub contacts: Vec<TouchContact>,
    pub cancelled: bool,
}

impl TouchFrame {
    pub(crate) fn cancel(source: PointerSource, surface: TouchSurface) -> Self {
        Self {
            source,
            time_ns: 0,
            surface,
            contacts: Vec::new(),
            cancelled: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AxisRange {
    minimum: i32,
    maximum: i32,
}

impl AxisRange {
    fn normalize(self, value: i32) -> i32 {
        if self.maximum <= self.minimum {
            return 0;
        }
        let value = value.clamp(self.minimum, self.maximum);
        let numerator =
            (i64::from(value) - i64::from(self.minimum)).saturating_mul(i64::from(TOUCH_COORD_MAX));
        let denominator = i64::from(self.maximum) - i64::from(self.minimum);
        (numerator / denominator) as i32
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    tracking_id: Option<i32>,
    x: Option<i32>,
    y: Option<i32>,
    pressure: Option<i32>,
    touch_major: Option<i32>,
}

/// Linux type-B slot assembler. It intentionally has no gesture policy.
#[derive(Debug)]
pub(crate) struct MtFrameAssembler {
    source: PointerSource,
    surface: TouchSurface,
    x_axis: AxisRange,
    y_axis: AxisRange,
    slots: Vec<Slot>,
    current_slot: usize,
}

impl MtFrameAssembler {
    pub(crate) fn new(
        source: PointerSource,
        surface: TouchSurface,
        x_minimum: i32,
        x_maximum: i32,
        y_minimum: i32,
        y_maximum: i32,
        slot_count: u16,
    ) -> Self {
        let count = usize::from(slot_count.max(1));
        Self {
            source,
            surface,
            x_axis: AxisRange {
                minimum: x_minimum,
                maximum: x_maximum,
            },
            y_axis: AxisRange {
                minimum: y_minimum,
                maximum: y_maximum,
            },
            slots: std::vec![Slot::default(); count],
            current_slot: 0,
        }
    }

    pub(crate) fn select_slot(&mut self, slot: i32) {
        if let Ok(slot) = usize::try_from(slot)
            && slot < self.slots.len()
        {
            self.current_slot = slot;
        }
    }

    pub(crate) fn tracking_id(&mut self, tracking_id: i32) {
        let slot = &mut self.slots[self.current_slot];
        if tracking_id < 0 {
            *slot = Slot::default();
        } else {
            // Slot reuse starts a fresh lifecycle; stale coordinates must not leak.
            *slot = Slot {
                tracking_id: Some(tracking_id),
                x: None,
                y: None,
                pressure: None,
                touch_major: None,
            };
        }
    }

    pub(crate) fn position_x(&mut self, value: i32) {
        self.slots[self.current_slot].x = Some(value);
    }

    pub(crate) fn position_y(&mut self, value: i32) {
        self.slots[self.current_slot].y = Some(value);
    }

    pub(crate) fn pressure(&mut self, value: i32) {
        self.slots[self.current_slot].pressure = Some(value);
    }

    pub(crate) fn touch_major(&mut self, value: i32) {
        self.slots[self.current_slot].touch_major = Some(value);
    }

    pub(crate) fn commit(&self, time_ns: u64) -> TouchFrame {
        let contacts = self
            .slots
            .iter()
            .filter_map(|slot| {
                Some(TouchContact {
                    tracking_id: slot.tracking_id?,
                    x: self.x_axis.normalize(slot.x?),
                    y: self.y_axis.normalize(slot.y?),
                    pressure: slot.pressure,
                    touch_major: slot.touch_major,
                })
            })
            .collect();
        TouchFrame {
            source: self.source,
            time_ns,
            surface: self.surface,
            contacts,
            cancelled: false,
        }
    }

    pub(crate) fn cancel(&mut self) -> TouchFrame {
        self.slots.fill(Slot::default());
        TouchFrame::cancel(self.source, self.surface)
    }
}

/// Lifecycle shared by gestures which are not represented in the legacy pointer ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GesturePhase {
    Begin,
    Update,
    End,
    Cancel,
}

/// Internal gesture signal. Pinch/swipe remain compositor-local until SWS gains an IPC ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GestureEvent {
    Tap {
        x: i32,
        y: i32,
    },
    Scroll {
        phase: GesturePhase,
        dx: i32,
        dy: i32,
    },
    Pinch {
        phase: GesturePhase,
        scale_milli: i32,
    },
    Swipe {
        phase: GesturePhase,
        dx: i32,
        dy: i32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TouchPolicyEvent {
    Pointer(CompositorInputEvent),
    Gesture(GestureEvent),
    DirectTouch(TouchFrame),
    SourceButton {
        source: PointerSource,
        button: u16,
        pressed: bool,
    },
    /// Drop every logical pointer button owned by this physical source.
    ///
    /// Cancellation can follow queue overflow, disconnect, or tablet-mode
    /// entry.  Releasing only the recognizer's synthetic tap button would
    /// leave a physical clickpad button stuck in the logical seat.
    ReleaseSource {
        source: PointerSource,
    },
}

/// Tunable thresholds in normalized coordinates and nanoseconds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GestureConfig {
    pub tap_move: i32,
    pub tap_timeout_ns: u64,
    pub double_tap_timeout_ns: u64,
    pub scroll_step: i32,
    pub pinch_threshold: i32,
    pub swipe_threshold: i32,
}

impl Default for GestureConfig {
    fn default() -> Self {
        Self {
            tap_move: 180,
            tap_timeout_ns: 250_000_000,
            double_tap_timeout_ns: 400_000_000,
            scroll_step: 35,
            pinch_threshold: 140,
            swipe_threshold: 180,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveGesture {
    None,
    Scroll,
    Pinch,
    Swipe,
}

#[derive(Debug)]
struct SourceState {
    source: PointerSource,
    surface: TouchSurface,
    previous: Vec<TouchContact>,
    primary_id: Option<i32>,
    start_time_ns: u64,
    start_centroid: (i32, i32),
    last_centroid: (i32, i32),
    start_distance: i32,
    last_distance: i32,
    moved: bool,
    active: ActiveGesture,
    last_tap: Option<(u64, i32, i32)>,
    tap_drag_down: bool,
}

impl SourceState {
    fn new(frame: &TouchFrame) -> Self {
        Self {
            source: frame.source,
            surface: frame.surface,
            previous: Vec::new(),
            primary_id: None,
            start_time_ns: 0,
            start_centroid: (0, 0),
            last_centroid: (0, 0),
            start_distance: 0,
            last_distance: 0,
            moved: false,
            active: ActiveGesture::None,
            last_tap: None,
            tap_drag_down: false,
        }
    }
}

/// Pure compositor-thread recognizer with isolated state for every device source.
#[derive(Debug)]
pub(crate) struct GestureRecognizer {
    config: GestureConfig,
    screen_width: u32,
    screen_height: u32,
    tablet_mode: bool,
    sources: Vec<SourceState>,
}

impl GestureRecognizer {
    pub(crate) fn new(screen_width: u32, screen_height: u32) -> Self {
        Self::with_config(screen_width, screen_height, GestureConfig::default())
    }

    pub(crate) fn with_config(
        screen_width: u32,
        screen_height: u32,
        config: GestureConfig,
    ) -> Self {
        Self {
            config,
            screen_width: screen_width.max(1),
            screen_height: screen_height.max(1),
            tablet_mode: false,
            sources: Vec::new(),
        }
    }

    pub(crate) fn set_tablet_mode(&mut self, enabled: bool) -> Vec<TouchPolicyEvent> {
        if self.tablet_mode == enabled {
            return Vec::new();
        }
        self.tablet_mode = enabled;
        if !enabled {
            return Vec::new();
        }
        let mut events = Vec::new();
        let mut retained = Vec::new();
        for mut state in self.sources.drain(..) {
            if matches!(state.surface, TouchSurface::Touchpad { internal: true }) {
                cancel_state(&mut state, &mut events);
            } else {
                retained.push(state);
            }
        }
        self.sources = retained;
        events
    }

    pub(crate) fn process(&mut self, frame: TouchFrame) -> Vec<TouchPolicyEvent> {
        if self.tablet_mode && matches!(frame.surface, TouchSurface::Touchpad { internal: true }) {
            return Vec::new();
        }
        let index = self
            .sources
            .iter()
            .position(|state| state.source == frame.source)
            .unwrap_or_else(|| {
                self.sources.push(SourceState::new(&frame));
                self.sources.len() - 1
            });
        let state = &mut self.sources[index];
        state.surface = frame.surface;
        let mut events = Vec::new();
        if frame.cancelled {
            if matches!(state.surface, TouchSurface::Touchscreen) {
                events.push(TouchPolicyEvent::DirectTouch(frame));
                state.previous.clear();
            } else {
                cancel_state(state, &mut events);
            }
            self.sources.remove(index);
            return events;
        }
        match frame.surface {
            TouchSurface::Touchscreen => {
                events.push(TouchPolicyEvent::DirectTouch(frame.clone()));
            }
            TouchSurface::Touchpad { .. } => indirect_frame(
                state,
                &frame,
                self.config,
                self.screen_width,
                self.screen_height,
                &mut events,
            ),
        }
        state.previous = frame.contacts;
        events
    }
}

fn pointer(event: CompositorInputEvent) -> TouchPolicyEvent {
    TouchPolicyEvent::Pointer(event)
}

fn gesture(event: GestureEvent) -> TouchPolicyEvent {
    TouchPolicyEvent::Gesture(event)
}

fn indirect_frame(
    state: &mut SourceState,
    frame: &TouchFrame,
    config: GestureConfig,
    screen_width: u32,
    screen_height: u32,
    events: &mut Vec<TouchPolicyEvent>,
) {
    let previous_count = state.previous.len();
    let count = frame.contacts.len();
    if previous_count == 0 && count > 0 {
        let centroid = centroid(&frame.contacts);
        state.start_time_ns = frame.time_ns;
        state.start_centroid = centroid;
        state.last_centroid = centroid;
        state.start_distance = contact_distance(&frame.contacts);
        state.last_distance = state.start_distance;
        state.moved = false;
        state.active = ActiveGesture::None;
        let is_double_tap = state.last_tap.is_some_and(|(time, x, y)| {
            frame.time_ns.saturating_sub(time) <= config.double_tap_timeout_ns
                && distance((x, y), centroid) <= config.tap_move
        });
        if count == 1 && is_double_tap {
            state.tap_drag_down = true;
            state.last_tap = None;
            events.push(pointer(CompositorInputEvent::MouseButton {
                button: key_codes::BTN_LEFT,
                pressed: true,
            }));
        }
        return;
    }

    if count == 0 {
        end_indirect(state, frame.time_ns, config, events);
        return;
    }

    let current = centroid(&frame.contacts);
    let delta = (
        current.0 - state.last_centroid.0,
        current.1 - state.last_centroid.1,
    );
    let total = distance(state.start_centroid, current);
    state.moved |= total > config.tap_move;

    if count == 1 {
        if previous_count != 1 {
            state.start_centroid = current;
            state.last_centroid = current;
        } else if delta != (0, 0) {
            events.push(pointer(CompositorInputEvent::MouseMove {
                dx: scale_delta(delta.0, screen_width),
                dy: scale_delta(delta.1, screen_height),
            }));
        }
        state.last_centroid = current;
        return;
    }

    let current_distance = contact_distance(&frame.contacts);
    if previous_count != count {
        end_active_gesture(state, GesturePhase::End, events);
        state.start_centroid = current;
        state.last_centroid = current;
        state.start_distance = current_distance;
        state.last_distance = current_distance;
        state.active = ActiveGesture::None;
        return;
    }

    if state.active == ActiveGesture::None {
        let pinch_delta = (current_distance - state.start_distance).abs();
        if count >= 3 && distance(state.start_centroid, current) >= config.swipe_threshold {
            state.active = ActiveGesture::Swipe;
            events.push(gesture(GestureEvent::Swipe {
                phase: GesturePhase::Begin,
                dx: 0,
                dy: 0,
            }));
        } else if pinch_delta >= config.pinch_threshold {
            state.active = ActiveGesture::Pinch;
            events.push(gesture(GestureEvent::Pinch {
                phase: GesturePhase::Begin,
                scale_milli: 1000,
            }));
        } else if delta != (0, 0) {
            state.active = ActiveGesture::Scroll;
            events.push(gesture(GestureEvent::Scroll {
                phase: GesturePhase::Begin,
                dx: 0,
                dy: 0,
            }));
        }
    }

    match state.active {
        ActiveGesture::Scroll => {
            let dx = delta.0 / config.scroll_step.max(1);
            let dy = delta.1 / config.scroll_step.max(1);
            if dx != 0 || dy != 0 {
                events.push(pointer(CompositorInputEvent::MouseWheel { dx, dy }));
                events.push(gesture(GestureEvent::Scroll {
                    phase: GesturePhase::Update,
                    dx,
                    dy,
                }));
            }
        }
        ActiveGesture::Pinch => {
            let scale_milli = if state.start_distance > 0 {
                current_distance.saturating_mul(1000) / state.start_distance
            } else {
                1000
            };
            events.push(gesture(GestureEvent::Pinch {
                phase: GesturePhase::Update,
                scale_milli,
            }));
        }
        ActiveGesture::Swipe => events.push(gesture(GestureEvent::Swipe {
            phase: GesturePhase::Update,
            dx: current.0 - state.start_centroid.0,
            dy: current.1 - state.start_centroid.1,
        })),
        ActiveGesture::None => {}
    }
    state.last_centroid = current;
    state.last_distance = current_distance;
}

fn end_indirect(
    state: &mut SourceState,
    time_ns: u64,
    config: GestureConfig,
    events: &mut Vec<TouchPolicyEvent>,
) {
    end_active_gesture(state, GesturePhase::End, events);
    if state.tap_drag_down {
        state.tap_drag_down = false;
        events.push(pointer(CompositorInputEvent::MouseButton {
            button: key_codes::BTN_LEFT,
            pressed: false,
        }));
    } else if state.previous.len() == 1
        && !state.moved
        && time_ns.saturating_sub(state.start_time_ns) <= config.tap_timeout_ns
    {
        events.push(pointer(CompositorInputEvent::MouseButton {
            button: key_codes::BTN_LEFT,
            pressed: true,
        }));
        events.push(pointer(CompositorInputEvent::MouseButton {
            button: key_codes::BTN_LEFT,
            pressed: false,
        }));
        events.push(gesture(GestureEvent::Tap {
            x: state.last_centroid.0,
            y: state.last_centroid.1,
        }));
        state.last_tap = Some((time_ns, state.last_centroid.0, state.last_centroid.1));
    }
    state.primary_id = None;
    state.active = ActiveGesture::None;
}

fn cancel_state(state: &mut SourceState, events: &mut Vec<TouchPolicyEvent>) {
    end_active_gesture(state, GesturePhase::Cancel, events);
    if state.tap_drag_down {
        events.push(TouchPolicyEvent::SourceButton {
            source: state.source,
            button: key_codes::BTN_LEFT,
            pressed: false,
        });
    }
    state.tap_drag_down = false;
    state.previous.clear();
    events.push(TouchPolicyEvent::ReleaseSource {
        source: state.source,
    });
}

fn end_active_gesture(
    state: &mut SourceState,
    phase: GesturePhase,
    events: &mut Vec<TouchPolicyEvent>,
) {
    let event = match state.active {
        ActiveGesture::Scroll => Some(GestureEvent::Scroll {
            phase,
            dx: 0,
            dy: 0,
        }),
        ActiveGesture::Pinch => Some(GestureEvent::Pinch {
            phase,
            scale_milli: 1000,
        }),
        ActiveGesture::Swipe => Some(GestureEvent::Swipe {
            phase,
            dx: 0,
            dy: 0,
        }),
        ActiveGesture::None => None,
    };
    if let Some(event) = event {
        events.push(gesture(event));
    }
    state.active = ActiveGesture::None;
}

fn centroid(contacts: &[TouchContact]) -> (i32, i32) {
    let count = i64::try_from(contacts.len()).unwrap_or(1).max(1);
    let x = contacts
        .iter()
        .map(|contact| i64::from(contact.x))
        .sum::<i64>()
        / count;
    let y = contacts
        .iter()
        .map(|contact| i64::from(contact.y))
        .sum::<i64>()
        / count;
    (x as i32, y as i32)
}

fn scale_delta(delta: i32, dimension: u32) -> i32 {
    (i64::from(delta) * i64::from(dimension.max(1)) / i64::from(TOUCH_COORD_MAX)) as i32
}

fn contact_distance(contacts: &[TouchContact]) -> i32 {
    if contacts.len() < 2 {
        return 0;
    }
    distance(
        (contacts[0].x, contacts[0].y),
        (contacts[1].x, contacts[1].y),
    )
}

fn distance(a: (i32, i32), b: (i32, i32)) -> i32 {
    let dx = i64::from(a.0) - i64::from(b.0);
    let dy = i64::from(a.1) - i64::from(b.1);
    integer_sqrt((dx * dx + dy * dy) as u64) as i32
}

fn integer_sqrt(value: u64) -> u64 {
    if value < 2 {
        return value;
    }
    let mut x = value;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + value / x) / 2;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(id: i32, x: i32, y: i32) -> TouchContact {
        TouchContact {
            tracking_id: id,
            x,
            y,
            pressure: None,
            touch_major: None,
        }
    }

    fn frame(time_ns: u64, contacts: Vec<TouchContact>) -> TouchFrame {
        TouchFrame {
            source: PointerSource::Local(8),
            time_ns,
            surface: TouchSurface::Touchpad { internal: true },
            contacts,
            cancelled: false,
        }
    }

    fn pointers(events: &[TouchPolicyEvent]) -> Vec<CompositorInputEvent> {
        events
            .iter()
            .filter_map(|event| match event {
                TouchPolicyEvent::Pointer(event) => Some(event.clone()),
                TouchPolicyEvent::Gesture(_)
                | TouchPolicyEvent::DirectTouch(_)
                | TouchPolicyEvent::SourceButton { .. }
                | TouchPolicyEvent::ReleaseSource { .. } => None,
            })
            .collect()
    }

    #[test]
    fn type_b_two_contacts_slot_reuse_and_lift() {
        let mut mt = MtFrameAssembler::new(
            PointerSource::Local(1),
            TouchSurface::Touchscreen,
            100,
            1100,
            -500,
            1500,
            2,
        );
        mt.tracking_id(7);
        mt.position_x(600);
        mt.position_y(500);
        mt.select_slot(1);
        mt.tracking_id(9);
        mt.position_x(1100);
        mt.position_y(1500);
        assert_eq!(
            mt.commit(1).contacts,
            std::vec![contact(7, 5000, 5000), contact(9, 10_000, 10_000)]
        );
        mt.select_slot(0);
        mt.tracking_id(-1);
        assert_eq!(mt.commit(2).contacts, std::vec![contact(9, 10_000, 10_000)]);
        mt.select_slot(0);
        mt.tracking_id(11);
        assert!(
            mt.commit(3)
                .contacts
                .iter()
                .all(|contact| contact.tracking_id != 11)
        );
        mt.position_x(100);
        mt.position_y(-500);
        assert_eq!(mt.commit(4).contacts[0], contact(11, 0, 0));
    }

    #[test]
    fn disconnect_cancels_tracking_lifecycle() {
        let mut mt = MtFrameAssembler::new(
            PointerSource::Local(1),
            TouchSurface::Touchscreen,
            0,
            100,
            0,
            100,
            1,
        );
        mt.tracking_id(5);
        mt.position_x(50);
        mt.position_y(50);
        assert!(mt.cancel().cancelled);
        assert!(mt.commit(2).contacts.is_empty());
    }

    #[test]
    fn one_finger_motion_and_tap_are_distinct() {
        let mut recognizer = GestureRecognizer::new(1000, 1000);
        assert!(
            recognizer
                .process(frame(1, std::vec![contact(1, 1000, 1000)]))
                .is_empty()
        );
        let moved = recognizer.process(frame(2, std::vec![contact(1, 1300, 1100)]));
        assert_eq!(
            pointers(&moved),
            std::vec![CompositorInputEvent::MouseMove { dx: 30, dy: 10 }]
        );
        let lifted = recognizer.process(frame(3, std::vec![]));
        assert!(
            !pointers(&lifted)
                .iter()
                .any(|event| matches!(event, CompositorInputEvent::MouseButton { .. }))
        );

        recognizer.process(frame(10, std::vec![contact(2, 2000, 2000)]));
        let tap = recognizer.process(frame(100_000_000, std::vec![]));
        assert_eq!(pointers(&tap).len(), 2);
    }

    #[test]
    fn second_tap_holds_button_for_tap_drag() {
        let mut recognizer = GestureRecognizer::new(1000, 1000);
        recognizer.process(frame(1, std::vec![contact(1, 1000, 1000)]));
        recognizer.process(frame(10, std::vec![]));
        let down = recognizer.process(frame(20, std::vec![contact(2, 1000, 1000)]));
        assert_eq!(
            pointers(&down),
            std::vec![CompositorInputEvent::MouseButton {
                button: key_codes::BTN_LEFT,
                pressed: true
            }]
        );
        let drag = recognizer.process(frame(30, std::vec![contact(2, 1400, 1000)]));
        assert!(matches!(
            pointers(&drag).as_slice(),
            [CompositorInputEvent::MouseMove { .. }]
        ));
        let up = recognizer.process(frame(40, std::vec![]));
        assert_eq!(
            pointers(&up),
            std::vec![CompositorInputEvent::MouseButton {
                button: key_codes::BTN_LEFT,
                pressed: false
            }]
        );
    }

    #[test]
    fn scroll_pinch_and_swipe_have_lifecycles() {
        let mut scroll = GestureRecognizer::new(1000, 1000);
        scroll.process(frame(
            1,
            std::vec![contact(1, 1000, 1000), contact(2, 2000, 1000)],
        ));
        let update = scroll.process(frame(
            2,
            std::vec![contact(1, 1000, 1200), contact(2, 2000, 1200)],
        ));
        assert!(update.iter().any(|event| matches!(
            event,
            TouchPolicyEvent::Gesture(GestureEvent::Scroll {
                phase: GesturePhase::Begin,
                ..
            })
        )));
        let end = scroll.process(frame(3, std::vec![]));
        assert!(end.iter().any(|event| matches!(
            event,
            TouchPolicyEvent::Gesture(GestureEvent::Scroll {
                phase: GesturePhase::End,
                ..
            })
        )));

        let mut pinch = GestureRecognizer::new(1000, 1000);
        pinch.process(frame(
            1,
            std::vec![contact(1, 2000, 2000), contact(2, 4000, 2000)],
        ));
        let update = pinch.process(frame(
            2,
            std::vec![contact(1, 1800, 2000), contact(2, 4200, 2000)],
        ));
        assert!(update.iter().any(|event| matches!(
            event,
            TouchPolicyEvent::Gesture(GestureEvent::Pinch {
                phase: GesturePhase::Begin,
                ..
            })
        )));
        let cancel = pinch.process(TouchFrame::cancel(
            PointerSource::Local(8),
            TouchSurface::Touchpad { internal: true },
        ));
        assert!(cancel.iter().any(|event| matches!(
            event,
            TouchPolicyEvent::Gesture(GestureEvent::Pinch {
                phase: GesturePhase::Cancel,
                ..
            })
        )));

        let mut swipe = GestureRecognizer::new(1000, 1000);
        swipe.process(frame(
            1,
            std::vec![
                contact(1, 1000, 1000),
                contact(2, 2000, 1000),
                contact(3, 3000, 1000)
            ],
        ));
        let update = swipe.process(frame(
            2,
            std::vec![
                contact(1, 1300, 1000),
                contact(2, 2300, 1000),
                contact(3, 3300, 1000)
            ],
        ));
        assert!(update.iter().any(|event| matches!(
            event,
            TouchPolicyEvent::Gesture(GestureEvent::Swipe {
                phase: GesturePhase::Begin,
                ..
            })
        )));
    }

    #[test]
    fn tablet_mode_cancels_only_internal_touchpad() {
        let mut recognizer = GestureRecognizer::new(1000, 1000);
        recognizer.process(frame(1, std::vec![contact(1, 1000, 1000)]));
        let mut external = frame(1, std::vec![contact(2, 2000, 2000)]);
        external.source = PointerSource::Local(9);
        external.surface = TouchSurface::Touchpad { internal: false };
        recognizer.process(external);
        let cancelled = recognizer.set_tablet_mode(true);
        assert!(cancelled.iter().any(|event| matches!(
            event,
            TouchPolicyEvent::ReleaseSource {
                source: PointerSource::Local(8)
            }
        )));
        assert!(!cancelled.iter().any(|event| matches!(
            event,
            TouchPolicyEvent::ReleaseSource {
                source: PointerSource::Local(9)
            }
        )));
        let mut external_update = frame(2, std::vec![contact(2, 2200, 2000)]);
        external_update.source = PointerSource::Local(9);
        external_update.surface = TouchSurface::Touchpad { internal: false };
        assert!(!recognizer.process(external_update).is_empty());
        assert!(
            recognizer
                .process(frame(2, std::vec![contact(1, 1200, 1000)]))
                .is_empty()
        );
    }

    #[test]
    fn touchscreen_uses_separate_direct_path() {
        let mut recognizer = GestureRecognizer::new(1001, 1001);
        let mut direct = frame(1, std::vec![contact(1, 5000, 5000)]);
        direct.surface = TouchSurface::Touchscreen;
        let first = recognizer.process(direct.clone());
        assert!(matches!(
            first.as_slice(),
            [TouchPolicyEvent::DirectTouch(_)]
        ));
        assert!(pointers(&first).is_empty());
        direct.contacts.push(contact(2, 8000, 8000));
        let second = recognizer.process(direct);
        assert!(matches!(
            second.as_slice(),
            [TouchPolicyEvent::DirectTouch(_)]
        ));
        assert!(pointers(&second).is_empty());
    }

    #[test]
    fn sources_do_not_share_centroid_or_tap_state() {
        let mut recognizer = GestureRecognizer::new(1000, 1000);
        recognizer.process(frame(1, std::vec![contact(1, 1000, 1000)]));
        let mut other = frame(1, std::vec![contact(2, 9000, 9000)]);
        other.source = PointerSource::Local(9);
        assert!(recognizer.process(other.clone()).is_empty());
        other.contacts[0].x = 9100;
        assert_eq!(
            pointers(&recognizer.process(other)),
            std::vec![CompositorInputEvent::MouseMove { dx: 10, dy: 0 }]
        );
    }
}
