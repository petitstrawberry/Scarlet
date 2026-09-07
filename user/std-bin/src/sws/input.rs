//! Input event handling module

#[path = "key_repeat.rs"]
mod key_repeat;
#[path = "touch.rs"]
mod touch;

pub(crate) use key_repeat::{
    ConsumedKeys, HeldKeys, KeyRepeatState, KeyboardSource, ModifierTapState,
    forward_to_binary_key_protocol, is_initial_press, is_physical_key_value,
    should_retry_keyboard_read,
};
pub(crate) use touch::{
    GestureEvent, GestureRecognizer, TOUCH_COORD_MAX, TouchContact, TouchFrame, TouchPolicyEvent,
    TouchSurface,
};

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::println;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::vec::Vec;

use scarlet_os::handle::capability::StreamError;
use scarlet_os::input::{
    INPUT_CAP_DIRECT_TOUCH, INPUT_CAP_INTERNAL, INPUT_CAP_MT, InputDevice, InputDeviceKind,
};
use sws_protocol::input_environment_capability_flags as environment_capabilities;

const DEVICE_INDEX_LIMIT: u8 = 8;
const DEVICE_SCAN_INTERVAL: Duration = Duration::from_secs(1);
const SHORT_READ_DELAY: Duration = Duration::from_millis(10);

/// Input event structure (16 bytes, matches kernel InputEvent)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct InputEvent {
    pub time: u64,  // 8 bytes - timestamp in nanoseconds
    pub type_: u16, // 2 bytes - event type
    pub code: u16,  // 2 bytes - event code
    pub value: i32, // 4 bytes - event value
}

impl InputEvent {
    pub const SIZE: usize = core::mem::size_of::<Self>();
}

/// Processed input events for compositor
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositorInputEvent {
    MouseMove {
        dx: i32,
        dy: i32,
    },
    MouseButton {
        button: u16,
        pressed: bool,
    },
    MouseAbsolute {
        x: i32,
        y: i32,
    },
    /// Mouse wheel/scroll event
    MouseWheel {
        dx: i32,
        dy: i32,
    },
    Keyboard {
        code: u16,
        /// Linux evdev key value: 0 release, 1 press, 2 repeat.
        value: i32,
        /// Producer identity used to scope held-key state and disconnects.
        source: KeyboardSource,
        /// True when value 2 came from SWS rather than the input device.
        synthetic: bool,
    },
    /// The keyboard stream disconnected; release keys owned by that source.
    KeyboardReset {
        /// Only state owned by this producer is discarded.
        source: KeyboardSource,
    },
    /// A normalized per-device multitouch snapshot for compositor-thread policy.
    TouchFrame(TouchFrame),
    /// Aggregated convertible/lid posture reported by live switch readers.
    PostureChanged {
        /// `None` is unchanged; `Some(None)` became unknown; `Some(Some(v))`
        /// became known with value `v`.
        tablet_mode: Option<Option<bool>>,
        /// `None` is unchanged; `Some(None)` became unknown; `Some(Some(v))`
        /// became known with value `v`.
        lid_closed: Option<Option<bool>>,
    },
}

/// Global input event queue
static INPUT_EVENT_QUEUE: Mutex<Vec<CompositorInputEvent>> = Mutex::new(Vec::new());
static SCREEN_WIDTH: AtomicU32 = AtomicU32::new(1);
static SCREEN_HEIGHT: AtomicU32 = AtomicU32::new(1);
static INPUT_STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PostureReport {
    tablet_mode: Option<bool>,
    lid_closed: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PostureDelta {
    tablet_mode: Option<Option<bool>>,
    lid_closed: Option<Option<bool>>,
}

impl PostureDelta {
    fn between(previous: PostureReport, current: PostureReport) -> Self {
        Self {
            tablet_mode: (previous.tablet_mode != current.tablet_mode)
                .then_some(current.tablet_mode),
            lid_closed: (previous.lid_closed != current.lid_closed).then_some(current.lid_closed),
        }
    }

    fn is_empty(self) -> bool {
        self.tablet_mode.is_none() && self.lid_closed.is_none()
    }
}

#[derive(Debug, Default)]
struct PostureRegistry {
    sources: Vec<(u8, PostureReport)>,
}

impl PostureRegistry {
    fn aggregate(&self) -> PostureReport {
        fn aggregate_field(values: impl Iterator<Item = Option<bool>>) -> Option<bool> {
            let mut reporters = values.flatten().peekable();
            reporters.peek()?;
            Some(reporters.any(|value| value))
        }

        PostureReport {
            tablet_mode: aggregate_field(self.sources.iter().map(|(_, report)| report.tablet_mode)),
            lid_closed: aggregate_field(self.sources.iter().map(|(_, report)| report.lid_closed)),
        }
    }

    fn update(&mut self, source: u8, report: PostureReport) -> PostureDelta {
        let previous = self.aggregate();
        if let Some((_, current)) = self
            .sources
            .iter_mut()
            .find(|(existing, _)| *existing == source)
        {
            *current = report;
        } else {
            self.sources.push((source, report));
        }
        PostureDelta::between(previous, self.aggregate())
    }

    fn remove(&mut self, source: u8) -> PostureDelta {
        let previous = self.aggregate();
        self.sources.retain(|(existing, _)| *existing != source);
        PostureDelta::between(previous, self.aggregate())
    }
}

static SWITCH_POSTURE: Mutex<PostureRegistry> = Mutex::new(PostureRegistry {
    sources: Vec::new(),
});

fn publish_posture_delta(delta: PostureDelta) {
    if !delta.is_empty() {
        push_input_event(CompositorInputEvent::PostureChanged {
            tablet_mode: delta.tablet_mode,
            lid_closed: delta.lid_closed,
        });
    }
}

struct PostureRegistration {
    source: u8,
}

impl PostureRegistration {
    fn new(source: u8, report: PostureReport) -> Self {
        let mut registry = SWITCH_POSTURE
            .lock()
            .expect("SWS posture registry mutex poisoned");
        publish_posture_delta(registry.update(source, report));
        Self { source }
    }

    fn update(&self, report: PostureReport) {
        let mut registry = SWITCH_POSTURE
            .lock()
            .expect("SWS posture registry mutex poisoned");
        publish_posture_delta(registry.update(self.source, report));
    }
}

impl Drop for PostureRegistration {
    fn drop(&mut self) {
        let mut registry = SWITCH_POSTURE
            .lock()
            .expect("SWS posture registry mutex poisoned");
        publish_posture_delta(registry.remove(self.source));
    }
}

#[derive(Debug, Default)]
struct CapabilityRegistry {
    direct_touch: u32,
    fine_pointer: u32,
    keyboard: u32,
}

impl CapabilityRegistry {
    fn flags(&self) -> u32 {
        let mut flags = 0;
        if self.direct_touch != 0 {
            flags |= environment_capabilities::DIRECT_TOUCH;
        }
        if self.fine_pointer != 0 {
            flags |= environment_capabilities::FINE_POINTER;
        }
        if self.keyboard != 0 {
            flags |= environment_capabilities::KEYBOARD;
        }
        flags
    }

    fn add(&mut self, flags: u32) -> Option<u32> {
        let previous = self.flags();
        if flags & environment_capabilities::DIRECT_TOUCH != 0 {
            self.direct_touch = self.direct_touch.saturating_add(1);
        }
        if flags & environment_capabilities::FINE_POINTER != 0 {
            self.fine_pointer = self.fine_pointer.saturating_add(1);
        }
        if flags & environment_capabilities::KEYBOARD != 0 {
            self.keyboard = self.keyboard.saturating_add(1);
        }
        let current = self.flags();
        (current != previous).then_some(current)
    }

    fn remove(&mut self, flags: u32) -> Option<u32> {
        let previous = self.flags();
        if flags & environment_capabilities::DIRECT_TOUCH != 0 {
            self.direct_touch = self.direct_touch.saturating_sub(1);
        }
        if flags & environment_capabilities::FINE_POINTER != 0 {
            self.fine_pointer = self.fine_pointer.saturating_sub(1);
        }
        if flags & environment_capabilities::KEYBOARD != 0 {
            self.keyboard = self.keyboard.saturating_sub(1);
        }
        let current = self.flags();
        (current != previous).then_some(current)
    }
}

static LIVE_CAPABILITIES: Mutex<CapabilityRegistry> = Mutex::new(CapabilityRegistry {
    direct_touch: 0,
    fine_pointer: 0,
    keyboard: 0,
});

struct CapabilityRegistration {
    flags: u32,
}

impl Drop for CapabilityRegistration {
    fn drop(&mut self) {
        update_live_capabilities(self.flags, false);
    }
}

fn publish_capabilities(flags: u32) {
    if let Some(snapshot) = super::input_environment::update_capabilities(flags) {
        super::ipc::broadcast_input_environment_changed(snapshot);
        super::input_environment_sbus::queue_state_changed(snapshot);
    }
}

fn update_live_capabilities(flags: u32, add: bool) {
    let mut registry = LIVE_CAPABILITIES
        .lock()
        .expect("SWS input capability mutex poisoned");
    let changed = if add {
        registry.add(flags)
    } else {
        registry.remove(flags)
    };
    if let Some(flags) = changed {
        // Publish while the registry is locked so concurrent reader starts and
        // disconnects cannot reorder authoritative snapshots.
        publish_capabilities(flags);
    }
}

fn register_live_capabilities(flags: u32) -> CapabilityRegistration {
    update_live_capabilities(flags, true);
    CapabilityRegistration { flags }
}

/// Update the screen size used to scale absolute input devices.
pub fn set_screen_size(width: u32, height: u32) {
    SCREEN_WIDTH.store(width.max(1), Ordering::Relaxed);
    SCREEN_HEIGHT.store(height.max(1), Ordering::Relaxed);
}

/// Add an input event to the global queue
pub fn push_input_event(event: CompositorInputEvent) {
    let mut queue = INPUT_EVENT_QUEUE.lock().expect("SWS mutex poisoned");
    let should_wake = queue.is_empty();
    queue.push(event);
    drop(queue);
    if should_wake {
        super::ipc::wake_compositor();
    }
}

/// Get all pending input events from the queue
pub fn pop_all_input_events() -> Vec<CompositorInputEvent> {
    let mut queue = INPUT_EVENT_QUEUE.lock().expect("SWS mutex poisoned");
    core::mem::take(&mut *queue)
}

/// Return whether the compositor input queue has pending events.
///
/// # Returns
///
/// `true` if input events are queued for the compositor.
pub fn has_pending_input_events() -> bool {
    !INPUT_EVENT_QUEUE
        .lock()
        .expect("SWS mutex poisoned")
        .is_empty()
}

/// Event types
pub mod event_types {
    pub const EV_SYN: u16 = 0x00;
    pub const EV_KEY: u16 = 0x01;
    pub const EV_REL: u16 = 0x02;
    pub const EV_ABS: u16 = 0x03;
    pub const EV_SW: u16 = 0x05;
}

/// Synchronization event codes
pub mod syn_codes {
    pub const SYN_REPORT: u16 = 0;
    pub const SYN_DROPPED: u16 = 3;
}

/// Relative axis codes
pub mod rel_codes {
    pub const REL_X: u16 = 0x00;
    pub const REL_Y: u16 = 0x01;
    pub const REL_HWHEEL: u16 = 0x06;
    pub const REL_WHEEL: u16 = 0x08;
    pub const REL_HWHEEL_HI_RES: u16 = 0x0c;
    pub const REL_WHEEL_HI_RES: u16 = 0x0b;
}

/// Surface pixels per wheel notch (compositor-side scaling, matches Mutter).
pub const WHEEL_PIXELS_PER_NOTCH: i32 = 10;

/// Absolute axis codes
pub mod abs_codes {
    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
    pub const ABS_MT_SLOT: u16 = 0x2f;
    pub const ABS_MT_TOUCH_MAJOR: u16 = 0x30;
    pub const ABS_MT_POSITION_X: u16 = 0x35;
    pub const ABS_MT_POSITION_Y: u16 = 0x36;
    pub const ABS_MT_TRACKING_ID: u16 = 0x39;
    pub const ABS_MT_PRESSURE: u16 = 0x3a;
}

/// Switch event codes.
pub mod switch_codes {
    pub const SW_LID: u16 = 0x00;
    pub const SW_TABLET_MODE: u16 = 0x01;
}

/// Key codes
#[allow(dead_code)]
pub mod key_codes {
    pub const BTN_LEFT: u16 = 0x110;
    pub const BTN_TOUCH: u16 = 0x14a;
    #[allow(dead_code)]
    pub const BTN_RIGHT: u16 = 0x111;
    #[allow(dead_code)]
    pub const BTN_MIDDLE: u16 = 0x112;
    pub const KEY_ESC: u16 = 0x01;
    pub const KEY_1: u16 = 0x02;
    pub const KEY_2: u16 = 0x03;
    pub const KEY_3: u16 = 0x04;
    pub const KEY_4: u16 = 0x05;
    pub const KEY_5: u16 = 0x06;
    pub const KEY_6: u16 = 0x07;
    pub const KEY_7: u16 = 0x08;
    pub const KEY_8: u16 = 0x09;
    pub const KEY_9: u16 = 0x0a;
    pub const KEY_0: u16 = 0x0b;
    pub const KEY_MINUS: u16 = 0x0c;
    pub const KEY_EQUAL: u16 = 0x0d;
    pub const KEY_BACKSPACE: u16 = 0x0e;
    pub const KEY_TAB: u16 = 0x0f;
    pub const KEY_H: u16 = 0x23;
    pub const KEY_N: u16 = 0x31;
    pub const KEY_LEFTBRACE: u16 = 0x1a;
    pub const KEY_RIGHTBRACE: u16 = 0x1b;
    pub const KEY_ENTER: u16 = 0x1c;
    pub const KEY_SPACE: u16 = 0x39;
    pub const KEY_LEFTCTRL: u16 = 0x1d;
    pub const KEY_RIGHTCTRL: u16 = 0x61;
    pub const KEY_LEFTSHIFT: u16 = 0x2a;
    pub const KEY_RIGHTSHIFT: u16 = 0x36;
    pub const KEY_LEFTALT: u16 = 0x38;
    pub const KEY_RIGHTALT: u16 = 0x64;
    pub const KEY_LEFT: u16 = 0x69;
    pub const KEY_RIGHT: u16 = 0x6a;
    pub const KEY_DELETE: u16 = 0x6f;
    pub const KEY_ZENKAKUHANKAKU: u16 = 0x55;
    pub const KEY_HENKAN: u16 = 0x5c;
    pub const KEY_MUHENKAN: u16 = 0x5e;
    pub const KEY_HANGUEL: u16 = 0x7a;
    pub const KEY_LEFTMETA: u16 = 0x7d;
    pub const KEY_RIGHTMETA: u16 = 0x7e;
    pub const KEY_BACKSLASH: u16 = 0x2b;
    pub const KEY_SEMICOLON: u16 = 0x27;
    pub const KEY_APOSTROPHE: u16 = 0x28;
    pub const KEY_GRAVE: u16 = 0x29;
    pub const KEY_COMMA: u16 = 0x33;
    pub const KEY_DOT: u16 = 0x34;
    pub const KEY_SLASH: u16 = 0x35;
    pub const KEY_CAPSLOCK: u16 = 0x3a;
    pub const KEY_NUMLOCK: u16 = 0x45;
    pub const KEY_SCROLLLOCK: u16 = 0x46;
    pub const KEY_FN: u16 = 0x1d0;
    pub const KEY_VOLUMEDOWN: u16 = 0x72;
    pub const KEY_VOLUMEUP: u16 = 0x73;
    pub const KEY_POWER: u16 = 0x74;
}

/// Input manager - starts one independent reader for every logical-seat device.
pub struct InputManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AxisRange {
    minimum: i32,
    maximum: i32,
}

impl AxisRange {
    fn normalize(self, value: i32, dimension: u32) -> i32 {
        if dimension <= 1 || self.maximum <= self.minimum {
            return 0;
        }
        let value = value.clamp(self.minimum, self.maximum);
        let numerator =
            (i64::from(value) - i64::from(self.minimum)).saturating_mul(i64::from(dimension - 1));
        let denominator = i64::from(self.maximum) - i64::from(self.minimum);
        (numerator / denominator) as i32
    }
}

#[derive(Debug, Clone, Copy)]
struct PointerMetadata {
    kind: InputDeviceKind,
    direct_touch: bool,
    x_axis: Option<AxisRange>,
    y_axis: Option<AxisRange>,
    mt_x_axis: Option<AxisRange>,
    mt_y_axis: Option<AxisRange>,
    mt_slot_count: u16,
    internal: bool,
}

impl PointerMetadata {
    fn query(device: &InputDevice, expected_kind: InputDeviceKind) -> Self {
        let kind = match device.kind() {
            Ok(InputDeviceKind::Unknown) | Err(_) => expected_kind,
            Ok(kind) => kind,
        };
        let capabilities = device.capabilities().unwrap_or(0);
        let x_axis = query_axis(device, abs_codes::ABS_X);
        let y_axis = query_axis(device, abs_codes::ABS_Y);

        // Old virtio-tablet kernels predate the metadata controls but have a
        // stable 0..32767 ABI. No other device class receives guessed ranges.
        let legacy_tablet =
            kind == InputDeviceKind::Tablet && (x_axis.is_none() || y_axis.is_none());
        let fallback = legacy_tablet.then_some(AxisRange {
            minimum: 0,
            maximum: 32767,
        });
        Self {
            kind,
            direct_touch: kind == InputDeviceKind::Touchscreen
                || device
                    .capabilities()
                    .is_ok_and(|caps| caps & INPUT_CAP_DIRECT_TOUCH != 0),
            x_axis: x_axis.or(fallback),
            y_axis: y_axis.or(fallback),
            mt_x_axis: query_axis(device, abs_codes::ABS_MT_POSITION_X),
            mt_y_axis: query_axis(device, abs_codes::ABS_MT_POSITION_Y),
            mt_slot_count: if capabilities & INPUT_CAP_MT != 0 {
                device.multitouch_slot_count().unwrap_or(0)
            } else {
                0
            },
            internal: capabilities & INPUT_CAP_INTERNAL != 0,
        }
    }

    fn multitouch(self) -> bool {
        self.mt_slot_count > 0 && self.mt_x_axis.is_some() && self.mt_y_axis.is_some()
    }

    fn touch_surface(self) -> TouchSurface {
        if self.direct_touch {
            TouchSurface::Touchscreen
        } else {
            TouchSurface::Touchpad {
                internal: self.internal,
            }
        }
    }

    fn environment_capabilities(self) -> u32 {
        if self.direct_touch {
            environment_capabilities::DIRECT_TOUCH
        } else if matches!(
            self.kind,
            InputDeviceKind::Mouse | InputDeviceKind::Touchpad | InputDeviceKind::Tablet
        ) {
            // Tablet is the legacy absolute tablet pointer class here. There
            // is no reliable pen metadata in the current input ABI.
            environment_capabilities::FINE_POINTER
        } else {
            0
        }
    }
}

fn query_axis(device: &InputDevice, code: u16) -> Option<AxisRange> {
    let axis = device.absolute_axis(code).ok()?;
    let minimum = axis.minimum;
    let maximum = axis.maximum;
    (minimum < maximum).then_some(AxisRange { minimum, maximum })
}

#[derive(Debug, Default)]
struct PointerFrame {
    rel_x: i32,
    rel_y: i32,
    wheel_dx: i32,
    wheel_dy: i32,
    abs_x: Option<i32>,
    abs_y: Option<i32>,
    abs_dirty: bool,
    buttons: Vec<(u16, bool)>,
    legacy_touch: Option<bool>,
}

impl PointerFrame {
    fn consume(
        &mut self,
        metadata: PointerMetadata,
        source: PointerSource,
        event: InputEvent,
    ) -> Option<Vec<CompositorInputEvent>> {
        match event.type_ {
            event_types::EV_REL if !metadata.multitouch() => match event.code {
                rel_codes::REL_X => self.rel_x = self.rel_x.saturating_add(event.value),
                rel_codes::REL_Y => self.rel_y = self.rel_y.saturating_add(event.value),
                rel_codes::REL_WHEEL => self.wheel_dy = self.wheel_dy.saturating_add(event.value),
                rel_codes::REL_HWHEEL => self.wheel_dx = self.wheel_dx.saturating_add(event.value),
                _ => {}
            },
            event_types::EV_ABS if !metadata.multitouch() => match event.code {
                abs_codes::ABS_X => {
                    self.abs_x = Some(event.value);
                    self.abs_dirty = true;
                }
                abs_codes::ABS_Y => {
                    self.abs_y = Some(event.value);
                    self.abs_dirty = true;
                }
                _ => {}
            },
            event_types::EV_KEY => {
                if metadata.multitouch() {
                    // A multitouch contact lifecycle is owned exclusively by
                    // MtFrameAssembler. Direct-touch devices commonly mirror
                    // it through BTN_TOUCH and may also expose BTN_LEFT for
                    // legacy consumers; forwarding either mirror would turn
                    // one physical contact into both TouchFrame and mouse
                    // button streams.
                    if event.code == key_codes::BTN_TOUCH
                        || metadata.direct_touch && event.code == key_codes::BTN_LEFT
                    {
                        return None;
                    }
                }
                if metadata.direct_touch && event.code == key_codes::BTN_TOUCH {
                    if event.value == 0 || event.value == 1 {
                        self.legacy_touch = Some(event.value == 1);
                    }
                    return None;
                }
                if event.value == 0 || event.value == 1 {
                    self.buttons.push((event.code, event.value == 1));
                }
            }
            event_types::EV_SYN if event.code == syn_codes::SYN_REPORT => {
                return Some(self.commit(metadata, source));
            }
            _ => {}
        }
        None
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    fn commit(
        &mut self,
        metadata: PointerMetadata,
        source: PointerSource,
    ) -> Vec<CompositorInputEvent> {
        let mut events = Vec::new();
        if self.rel_x != 0 || self.rel_y != 0 {
            events.push(CompositorInputEvent::MouseMove {
                dx: self.rel_x,
                dy: self.rel_y,
            });
        }
        if metadata.direct_touch && !metadata.multitouch() {
            if let (Some(x), Some(y), Some(x_axis), Some(y_axis), Some(pressed)) = (
                self.abs_x,
                self.abs_y,
                metadata.x_axis,
                metadata.y_axis,
                self.legacy_touch,
            ) {
                events.push(CompositorInputEvent::TouchFrame(TouchFrame {
                    source,
                    time_ns: 0,
                    surface: TouchSurface::Touchscreen,
                    contacts: if pressed {
                        std::vec![TouchContact {
                            tracking_id: 0,
                            x: x_axis.normalize(x, (touch::TOUCH_COORD_MAX + 1) as u32),
                            y: y_axis.normalize(y, (touch::TOUCH_COORD_MAX + 1) as u32),
                            pressure: None,
                            touch_major: None,
                        }]
                    } else {
                        Vec::new()
                    },
                    cancelled: false,
                }));
            }
        } else if self.abs_dirty
            && let (Some(x), Some(y), Some(x_axis), Some(y_axis)) =
                (self.abs_x, self.abs_y, metadata.x_axis, metadata.y_axis)
        {
            events.push(CompositorInputEvent::MouseAbsolute {
                x: x_axis.normalize(x, SCREEN_WIDTH.load(Ordering::Relaxed)),
                y: y_axis.normalize(y, SCREEN_HEIGHT.load(Ordering::Relaxed)),
            });
        }
        if self.wheel_dx != 0 || self.wheel_dy != 0 {
            events.push(CompositorInputEvent::MouseWheel {
                dx: self.wheel_dx,
                dy: self.wheel_dy,
            });
        }
        events.extend(
            self.buttons
                .drain(..)
                .map(|(button, pressed)| CompositorInputEvent::MouseButton { button, pressed }),
        );
        self.rel_x = 0;
        self.rel_y = 0;
        self.wheel_dx = 0;
        self.wheel_dy = 0;
        self.abs_dirty = false;
        events
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointerSource {
    Local(u8),
    Remote(usize),
}

#[derive(Debug, Default)]
struct LogicalPointerButtons {
    held: Vec<(PointerSource, u16)>,
}

impl LogicalPointerButtons {
    fn update(&mut self, source: PointerSource, button: u16, pressed: bool) -> Option<bool> {
        let was_held = self.held.iter().any(|(_, held)| *held == button);
        if pressed {
            if !self.held.contains(&(source, button)) {
                self.held.push((source, button));
            }
        } else {
            self.held.retain(|entry| *entry != (source, button));
        }
        let is_held = self.held.iter().any(|(_, held)| *held == button);
        (was_held != is_held).then_some(is_held)
    }

    fn drain_source(&mut self, source: PointerSource) -> Vec<(u16, bool)> {
        let mut affected = Vec::new();
        for (_, button) in self
            .held
            .iter()
            .filter(|(held_source, _)| *held_source == source)
        {
            if !affected.contains(button) {
                affected.push(*button);
            }
        }
        self.held.retain(|(held_source, _)| *held_source != source);
        affected
            .into_iter()
            .filter(|button| !self.held.iter().any(|(_, held)| held == button))
            .map(|button| (button, false))
            .collect()
    }
}

static POINTER_BUTTONS: Mutex<LogicalPointerButtons> =
    Mutex::new(LogicalPointerButtons { held: Vec::new() });

fn push_pointer_frame(source: PointerSource, events: Vec<CompositorInputEvent>) {
    for event in events {
        match event {
            CompositorInputEvent::MouseButton { button, pressed } => {
                push_pointer_button(source, button, pressed);
            }
            event => push_input_event(event),
        }
    }
}

pub(crate) fn push_pointer_button(source: PointerSource, button: u16, pressed: bool) {
    let transition = POINTER_BUTTONS
        .lock()
        .expect("SWS pointer mutex poisoned")
        .update(source, button, pressed);
    if let Some(pressed) = transition {
        push_input_event(CompositorInputEvent::MouseButton { button, pressed });
    }
}

pub(crate) fn release_pointer_source(source: PointerSource) {
    let releases = POINTER_BUTTONS
        .lock()
        .expect("SWS pointer mutex poisoned")
        .drain_source(source);
    for (button, pressed) in releases {
        push_input_event(CompositorInputEvent::MouseButton { button, pressed });
    }
}

impl InputManager {
    /// Start readers for all bounded native input-device classes.
    pub fn start_input_thread(screen_width: u32, screen_height: u32) -> Result<(), &'static str> {
        set_screen_size(screen_width, screen_height);
        if !begin_input_start(&INPUT_STARTED) {
            return Ok(());
        }
        println!("[InputManager] Starting logical-seat input readers...");

        if thread::Builder::new()
            .spawn(input_discovery_supervisor)
            .is_err()
        {
            INPUT_STARTED.store(false, Ordering::Release);
            return Err("Failed to start input discovery supervisor");
        }
        println!("[InputManager] Logical-seat input readers started");
        Ok(())
    }
}

fn begin_input_start(started: &AtomicBool) -> bool {
    started
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn input_discovery_supervisor() {
    let active_paths = Arc::new(Mutex::new(Vec::<std::string::String>::new()));
    loop {
        for (class_index, (prefix, kind)) in [
            ("mouse", InputDeviceKind::Mouse),
            ("touchpad", InputDeviceKind::Touchpad),
            ("trackpad", InputDeviceKind::Touchpad),
            ("touchscreen", InputDeviceKind::Touchscreen),
            ("tablet", InputDeviceKind::Tablet),
        ]
        .into_iter()
        .enumerate()
        {
            for index in 0..DEVICE_INDEX_LIMIT {
                let path = std::format!("/dev/{prefix}{index}");
                let source = PointerSource::Local(class_index as u8 * DEVICE_INDEX_LIMIT + index);
                try_spawn_pointer_reader(&active_paths, path, kind, source);
            }
        }
        for index in 0..DEVICE_INDEX_LIMIT {
            let path = std::format!("/dev/keyboard{index}");
            try_spawn_keyboard_reader(&active_paths, path, index);
        }
        for index in 0..DEVICE_INDEX_LIMIT {
            try_spawn_switch_reader(&active_paths, std::format!("/dev/switch{index}"), index);
        }
        thread::sleep(DEVICE_SCAN_INTERVAL);
    }
}

fn claim_device(
    active_paths: &Arc<Mutex<Vec<std::string::String>>>,
    path: &str,
) -> Option<InputDevice> {
    let mut active = active_paths.lock().expect("SWS input mutex poisoned");
    if active.iter().any(|active_path| active_path == path) {
        return None;
    }
    let device = InputDevice::open(path).ok()?;
    active.push(path.into());
    Some(device)
}

fn release_device(active_paths: &Mutex<Vec<std::string::String>>, path: &str) {
    active_paths
        .lock()
        .expect("SWS input mutex poisoned")
        .retain(|active_path| active_path != path);
}

fn try_spawn_pointer_reader(
    active_paths: &Arc<Mutex<Vec<std::string::String>>>,
    path: std::string::String,
    expected_kind: InputDeviceKind,
    source: PointerSource,
) {
    let Some(device) = claim_device(active_paths, &path) else {
        return;
    };
    let reader_paths = Arc::clone(active_paths);
    let cleanup_path = path.clone();
    let failure_path = cleanup_path.clone();
    if thread::Builder::new()
        .spawn(move || {
            pointer_device_reader(device, path, expected_kind, source);
            release_device(&reader_paths, &cleanup_path);
        })
        .is_err()
    {
        release_device(active_paths, &failure_path);
    }
}

fn pointer_device_reader(
    device: InputDevice,
    path: std::string::String,
    expected_kind: InputDeviceKind,
    source: PointerSource,
) {
    let metadata = PointerMetadata::query(&device, expected_kind);
    println!("[InputThread] Opened {} as {:?}", path, metadata.kind);
    let _capability_registration = register_live_capabilities(metadata.environment_capabilities());
    let mut frame = PointerFrame::default();
    let mut mt = match (metadata.mt_x_axis, metadata.mt_y_axis) {
        (Some(x), Some(y)) if metadata.multitouch() => Some(touch::MtFrameAssembler::new(
            source,
            metadata.touch_surface(),
            x.minimum,
            x.maximum,
            y.minimum,
            y.maximum,
            metadata.mt_slot_count,
        )),
        _ => None,
    };
    let mut mt_desynced = false;
    let mut legacy_desynced = false;
    loop {
        super::trace::input_loop();
        match read_input_event(&device) {
            Ok(Some(event)) => {
                super::trace::input_event();
                if mt.is_none()
                    && event.type_ == event_types::EV_SYN
                    && event.code == syn_codes::SYN_DROPPED
                {
                    frame.reset();
                    release_pointer_source(source);
                    legacy_desynced = true;
                    continue;
                }
                if legacy_desynced {
                    if event.type_ == event_types::EV_SYN && event.code == syn_codes::SYN_REPORT {
                        legacy_desynced = false;
                    }
                    continue;
                }
                if let Some(mt) = mt.as_mut() {
                    if let Some(touch_frame) = consume_mt_event(mt, &mut mt_desynced, event) {
                        if touch_frame.cancelled {
                            frame.reset();
                            release_pointer_source(source);
                        }
                        push_input_event(CompositorInputEvent::TouchFrame(touch_frame));
                    }
                    if mt_desynced
                        || event.type_ == event_types::EV_SYN
                            && event.code == syn_codes::SYN_DROPPED
                    {
                        continue;
                    }
                }
                if let Some(events) = frame.consume(metadata, source, event) {
                    push_pointer_frame(source, events);
                }
            }
            Ok(None) => {
                super::trace::input_empty();
                thread::sleep(SHORT_READ_DELAY);
            }
            Err(error) => {
                println!("[InputThread] {} disconnected: {:?}", path, error);
                break;
            }
        }
    }
    if let Some(mt) = mt.as_mut() {
        push_input_event(CompositorInputEvent::TouchFrame(mt.cancel()));
    }
    release_pointer_source(source);
}

fn consume_mt_event(
    mt: &mut touch::MtFrameAssembler,
    desynced: &mut bool,
    event: InputEvent,
) -> Option<TouchFrame> {
    if event.type_ == event_types::EV_SYN && event.code == syn_codes::SYN_DROPPED {
        *desynced = true;
        return Some(mt.cancel());
    }
    if *desynced {
        if event.type_ == event_types::EV_SYN && event.code == syn_codes::SYN_REPORT {
            *desynced = false;
        }
        return None;
    }
    if event.type_ == event_types::EV_ABS {
        match event.code {
            abs_codes::ABS_MT_SLOT => mt.select_slot(event.value),
            abs_codes::ABS_MT_TRACKING_ID => mt.tracking_id(event.value),
            abs_codes::ABS_MT_POSITION_X => mt.position_x(event.value),
            abs_codes::ABS_MT_POSITION_Y => mt.position_y(event.value),
            abs_codes::ABS_MT_PRESSURE => mt.pressure(event.value),
            abs_codes::ABS_MT_TOUCH_MAJOR => mt.touch_major(event.value),
            _ => {}
        }
        None
    } else if event.type_ == event_types::EV_SYN && event.code == syn_codes::SYN_REPORT {
        Some(mt.commit(event.time))
    } else {
        None
    }
}

fn try_spawn_switch_reader(
    active_paths: &Arc<Mutex<Vec<std::string::String>>>,
    path: std::string::String,
    source: u8,
) {
    let Some(device) = claim_device(active_paths, &path) else {
        return;
    };
    let reader_paths = Arc::clone(active_paths);
    let cleanup_path = path.clone();
    let failure_path = cleanup_path.clone();
    if thread::Builder::new()
        .spawn(move || {
            switch_device_reader(device, path, source);
            release_device(&reader_paths, &cleanup_path);
        })
        .is_err()
    {
        release_device(active_paths, &failure_path);
    }
}

fn switch_device_reader(device: InputDevice, path: std::string::String, source: u8) {
    let mut tablet_mode = device.switch_state(switch_codes::SW_TABLET_MODE).ok();
    let mut lid_closed = device.switch_state(switch_codes::SW_LID).ok();
    let registration = PostureRegistration::new(
        source,
        PostureReport {
            tablet_mode,
            lid_closed,
        },
    );
    let mut dirty = false;
    let mut desynced = false;
    loop {
        match read_input_event(&device) {
            Ok(Some(event))
                if event.type_ == event_types::EV_SYN && event.code == syn_codes::SYN_DROPPED =>
            {
                // EventDevice switch state is authoritative even if its event
                // queue overflowed.  Resample it, then ignore the interrupted
                // frame through the recovery SYN_REPORT boundary.
                tablet_mode = device.switch_state(switch_codes::SW_TABLET_MODE).ok();
                lid_closed = device.switch_state(switch_codes::SW_LID).ok();
                registration.update(PostureReport {
                    tablet_mode,
                    lid_closed,
                });
                dirty = false;
                desynced = true;
            }
            Ok(Some(event)) if desynced => {
                if event.type_ == event_types::EV_SYN && event.code == syn_codes::SYN_REPORT {
                    desynced = false;
                }
            }
            Ok(Some(event)) if event.type_ == event_types::EV_SW => {
                match event.code {
                    switch_codes::SW_TABLET_MODE => tablet_mode = Some(event.value != 0),
                    switch_codes::SW_LID => lid_closed = Some(event.value != 0),
                    _ => continue,
                }
                dirty = true;
            }
            Ok(Some(event))
                if event.type_ == event_types::EV_SYN
                    && event.code == syn_codes::SYN_REPORT
                    && dirty =>
            {
                registration.update(PostureReport {
                    tablet_mode,
                    lid_closed,
                });
                dirty = false;
            }
            Ok(Some(_)) => {}
            Ok(None) => thread::sleep(SHORT_READ_DELAY),
            Err(error) => {
                println!("[InputThread] {} disconnected: {:?}", path, error);
                break;
            }
        }
    }
}

fn try_spawn_keyboard_reader(
    active_paths: &Arc<Mutex<Vec<std::string::String>>>,
    path: std::string::String,
    index: u8,
) {
    let Some(device) = claim_device(active_paths, &path) else {
        return;
    };
    let reader_paths = Arc::clone(active_paths);
    let cleanup_path = path.clone();
    let failure_path = cleanup_path.clone();
    if thread::Builder::new()
        .spawn(move || {
            keyboard_device_reader(device, path, index);
            release_device(&reader_paths, &cleanup_path);
        })
        .is_err()
    {
        release_device(active_paths, &failure_path);
    }
}

fn keyboard_device_reader(device: InputDevice, path: std::string::String, index: u8) {
    let source = KeyboardSource::Local(index);
    println!("[KeyboardThread] Opened {}", path);
    let _capability_registration = register_live_capabilities(environment_capabilities::KEYBOARD);
    let mut desynced = false;
    loop {
        super::trace::keyboard_loop();
        match read_input_event(&device) {
            Ok(Some(event)) => {
                super::trace::keyboard_event();
                if let Some(event) = consume_keyboard_event(source, &mut desynced, event) {
                    push_input_event(event);
                }
            }
            Ok(None) => {
                super::trace::keyboard_short_read();
                thread::sleep(SHORT_READ_DELAY);
            }
            Err(error) => {
                println!("[KeyboardThread] {} disconnected: {:?}", path, error);
                break;
            }
        }
    }
    push_input_event(CompositorInputEvent::KeyboardReset { source });
}

fn consume_keyboard_event(
    source: KeyboardSource,
    desynced: &mut bool,
    event: InputEvent,
) -> Option<CompositorInputEvent> {
    if event.type_ == event_types::EV_SYN && event.code == syn_codes::SYN_DROPPED {
        *desynced = true;
        return Some(CompositorInputEvent::KeyboardReset { source });
    }
    if *desynced {
        if event.type_ == event_types::EV_SYN && event.code == syn_codes::SYN_REPORT {
            *desynced = false;
        }
        return None;
    }
    (event.type_ == event_types::EV_KEY).then_some(CompositorInputEvent::Keyboard {
        code: event.code,
        value: event.value,
        source,
        synthetic: false,
    })
}

fn read_input_event(device: &InputDevice) -> Result<Option<InputEvent>, StreamError> {
    let mut buffer = [0u8; InputEvent::SIZE];
    let bytes_read = device.read(&mut buffer)?;
    if should_retry_keyboard_read(bytes_read, InputEvent::SIZE) {
        return Ok(None);
    }
    // SAFETY: the kernel filled one complete integer-only record. Unaligned
    // reads are valid for the byte-aligned buffer.
    Ok(Some(unsafe {
        core::ptr::read_unaligned(buffer.as_ptr() as *const InputEvent)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(type_: u16, code: u16, value: i32) -> InputEvent {
        InputEvent {
            time: 0,
            type_,
            code,
            value,
        }
    }

    fn metadata(kind: InputDeviceKind) -> PointerMetadata {
        PointerMetadata {
            kind,
            direct_touch: kind == InputDeviceKind::Touchscreen,
            x_axis: Some(AxisRange {
                minimum: 100,
                maximum: 1100,
            }),
            y_axis: Some(AxisRange {
                minimum: -500,
                maximum: 1500,
            }),
            mt_x_axis: None,
            mt_y_axis: None,
            mt_slot_count: 0,
            internal: false,
        }
    }

    fn multitouch_metadata(kind: InputDeviceKind) -> PointerMetadata {
        PointerMetadata {
            mt_x_axis: Some(AxisRange {
                minimum: 100,
                maximum: 1100,
            }),
            mt_y_axis: Some(AxisRange {
                minimum: -500,
                maximum: 1500,
            }),
            mt_slot_count: 5,
            ..metadata(kind)
        }
    }

    #[test]
    fn sole_posture_source_disconnect_becomes_unknown() {
        let mut registry = PostureRegistry::default();
        assert_eq!(
            registry.update(
                0,
                PostureReport {
                    tablet_mode: Some(false),
                    lid_closed: None,
                }
            ),
            PostureDelta {
                tablet_mode: Some(Some(false)),
                lid_closed: None,
            }
        );
        assert_eq!(
            registry.remove(0),
            PostureDelta {
                tablet_mode: Some(None),
                lid_closed: None,
            }
        );
    }

    #[test]
    fn posture_registry_recomputes_disagreeing_sources_on_removal() {
        let mut registry = PostureRegistry::default();
        registry.update(
            0,
            PostureReport {
                tablet_mode: Some(false),
                lid_closed: None,
            },
        );
        assert_eq!(
            registry.update(
                1,
                PostureReport {
                    tablet_mode: Some(true),
                    lid_closed: None,
                }
            ),
            PostureDelta {
                tablet_mode: Some(Some(true)),
                lid_closed: None,
            }
        );
        assert_eq!(
            registry.remove(1),
            PostureDelta {
                tablet_mode: Some(Some(false)),
                lid_closed: None,
            }
        );
        assert_eq!(registry.aggregate().tablet_mode, Some(false));
    }

    #[test]
    fn posture_registry_aggregates_split_field_sources_independently() {
        let mut registry = PostureRegistry::default();
        registry.update(
            0,
            PostureReport {
                tablet_mode: Some(true),
                lid_closed: None,
            },
        );
        assert_eq!(
            registry.update(
                1,
                PostureReport {
                    tablet_mode: None,
                    lid_closed: Some(false),
                }
            ),
            PostureDelta {
                tablet_mode: None,
                lid_closed: Some(Some(false)),
            }
        );
        assert_eq!(
            registry.remove(1),
            PostureDelta {
                tablet_mode: None,
                lid_closed: Some(None),
            }
        );
        assert_eq!(registry.aggregate().tablet_mode, Some(true));
    }

    #[test]
    fn capability_registry_keeps_shared_flags_until_last_disconnect() {
        let mut registry = CapabilityRegistry::default();
        let direct_touch = environment_capabilities::DIRECT_TOUCH;

        assert_eq!(registry.add(direct_touch), Some(direct_touch));
        assert_eq!(registry.add(direct_touch), None);
        assert_eq!(registry.remove(direct_touch), None);
        assert_eq!(registry.flags(), direct_touch);
        assert_eq!(registry.remove(direct_touch), Some(0));
        assert_eq!(registry.flags(), 0);
    }

    #[test]
    fn capability_registry_aggregates_independent_device_classes() {
        let mut registry = CapabilityRegistry::default();
        let direct_touch = environment_capabilities::DIRECT_TOUCH;
        let fine_pointer = environment_capabilities::FINE_POINTER;
        let keyboard = environment_capabilities::KEYBOARD;

        assert_eq!(registry.add(direct_touch), Some(direct_touch));
        assert_eq!(
            registry.add(fine_pointer),
            Some(direct_touch | fine_pointer)
        );
        assert_eq!(
            registry.add(keyboard),
            Some(direct_touch | fine_pointer | keyboard)
        );
        assert_eq!(registry.remove(fine_pointer), Some(direct_touch | keyboard));
        assert_eq!(registry.remove(direct_touch), Some(keyboard));
        assert_eq!(registry.remove(keyboard), Some(0));
    }

    #[test]
    fn pointer_metadata_maps_only_reliable_environment_capabilities() {
        assert_eq!(
            metadata(InputDeviceKind::Touchscreen).environment_capabilities(),
            environment_capabilities::DIRECT_TOUCH
        );
        for kind in [
            InputDeviceKind::Mouse,
            InputDeviceKind::Touchpad,
            InputDeviceKind::Tablet,
        ] {
            assert_eq!(
                metadata(kind).environment_capabilities(),
                environment_capabilities::FINE_POINTER
            );
        }
        assert_eq!(
            metadata(InputDeviceKind::Tablet).environment_capabilities()
                & environment_capabilities::PEN,
            0
        );
    }

    #[test]
    fn tablet_and_touchpad_keep_independent_frames() {
        set_screen_size(1001, 2001);
        let mut tablet = PointerFrame::default();
        let mut touchpad = PointerFrame::default();
        assert!(
            tablet
                .consume(
                    metadata(InputDeviceKind::Tablet),
                    PointerSource::Local(0),
                    event(event_types::EV_ABS, abs_codes::ABS_X, 600)
                )
                .is_none()
        );
        assert!(
            touchpad
                .consume(
                    metadata(InputDeviceKind::Touchpad),
                    PointerSource::Local(1),
                    event(event_types::EV_REL, rel_codes::REL_X, 7)
                )
                .is_none()
        );
        let touchpad_events = touchpad
            .consume(
                metadata(InputDeviceKind::Touchpad),
                PointerSource::Local(1),
                event(event_types::EV_SYN, syn_codes::SYN_REPORT, 0),
            )
            .unwrap();
        assert_eq!(
            touchpad_events,
            std::vec![CompositorInputEvent::MouseMove { dx: 7, dy: 0 }]
        );
        assert!(
            tablet
                .consume(
                    metadata(InputDeviceKind::Tablet),
                    PointerSource::Local(0),
                    event(event_types::EV_ABS, abs_codes::ABS_Y, 500)
                )
                .is_none()
        );
        let tablet_events = tablet
            .consume(
                metadata(InputDeviceKind::Tablet),
                PointerSource::Local(0),
                event(event_types::EV_SYN, syn_codes::SYN_REPORT, 0),
            )
            .unwrap();
        assert_eq!(
            tablet_events,
            std::vec![CompositorInputEvent::MouseAbsolute { x: 500, y: 1000 }]
        );
    }

    #[test]
    fn absolute_axes_scale_independently_with_nonzero_minimum() {
        assert_eq!(
            AxisRange {
                minimum: 100,
                maximum: 1100
            }
            .normalize(600, 1001),
            500
        );
        assert_eq!(
            AxisRange {
                minimum: -500,
                maximum: 1500
            }
            .normalize(500, 2001),
            1000
        );
        assert_eq!(
            AxisRange {
                minimum: 100,
                maximum: 1100
            }
            .normalize(-1, 1001),
            0
        );
        assert_eq!(
            AxisRange {
                minimum: i32::MIN,
                maximum: i32::MAX,
            }
            .normalize(0, 1001),
            500
        );
    }

    #[test]
    fn legacy_touchscreen_uses_direct_touch_frame() {
        set_screen_size(1001, 2001);
        let mut frame = PointerFrame::default();
        frame.consume(
            metadata(InputDeviceKind::Touchscreen),
            PointerSource::Local(24),
            event(event_types::EV_ABS, abs_codes::ABS_X, 600),
        );
        frame.consume(
            metadata(InputDeviceKind::Touchscreen),
            PointerSource::Local(24),
            event(event_types::EV_ABS, abs_codes::ABS_Y, 500),
        );
        frame.consume(
            metadata(InputDeviceKind::Touchscreen),
            PointerSource::Local(24),
            event(event_types::EV_KEY, key_codes::BTN_TOUCH, 1),
        );
        let events = frame
            .consume(
                metadata(InputDeviceKind::Touchscreen),
                PointerSource::Local(24),
                event(event_types::EV_SYN, syn_codes::SYN_REPORT, 0),
            )
            .unwrap();
        assert_eq!(
            events,
            std::vec![CompositorInputEvent::TouchFrame(TouchFrame {
                source: PointerSource::Local(24),
                time_ns: 0,
                surface: TouchSurface::Touchscreen,
                contacts: std::vec![TouchContact {
                    tracking_id: 0,
                    x: 5000,
                    y: 5000,
                    pressure: None,
                    touch_major: None,
                }],
                cancelled: false,
            })]
        );
    }

    #[test]
    fn multitouch_touchscreen_does_not_duplicate_legacy_button_mirrors() {
        let metadata = multitouch_metadata(InputDeviceKind::Touchscreen);
        let source = PointerSource::Local(24);
        let mut frame = PointerFrame::default();

        assert!(
            frame
                .consume(
                    metadata,
                    source,
                    event(event_types::EV_KEY, key_codes::BTN_TOUCH, 1),
                )
                .is_none()
        );
        assert!(
            frame
                .consume(
                    metadata,
                    source,
                    event(event_types::EV_KEY, key_codes::BTN_LEFT, 1),
                )
                .is_none()
        );
        assert_eq!(
            frame.consume(
                metadata,
                source,
                event(event_types::EV_SYN, syn_codes::SYN_REPORT, 0),
            ),
            Some(std::vec![])
        );
    }

    #[test]
    fn multitouch_touchpad_keeps_its_physical_click_button() {
        let metadata = multitouch_metadata(InputDeviceKind::Touchpad);
        let source = PointerSource::Local(8);
        let mut frame = PointerFrame::default();

        frame.consume(
            metadata,
            source,
            event(event_types::EV_KEY, key_codes::BTN_LEFT, 1),
        );
        assert_eq!(
            frame.consume(
                metadata,
                source,
                event(event_types::EV_SYN, syn_codes::SYN_REPORT, 0),
            ),
            Some(std::vec![CompositorInputEvent::MouseButton {
                button: key_codes::BTN_LEFT,
                pressed: true,
            }])
        );
    }

    #[test]
    fn pointer_buttons_are_or_aggregated_across_devices() {
        let mouse = PointerSource::Local(0);
        let touch = PointerSource::Local(24);
        let mut buttons = LogicalPointerButtons::default();
        assert_eq!(buttons.update(mouse, key_codes::BTN_LEFT, true), Some(true));
        assert_eq!(buttons.update(touch, key_codes::BTN_LEFT, true), None);
        assert_eq!(buttons.update(mouse, key_codes::BTN_LEFT, false), None);
        assert_eq!(
            buttons.update(touch, key_codes::BTN_LEFT, false),
            Some(false)
        );
    }

    #[test]
    fn local_keyboard_sources_include_device_identity() {
        assert_ne!(KeyboardSource::Local(0), KeyboardSource::Local(1));
        let mut held = HeldKeys::default();
        held.update(KeyboardSource::Local(0), key_codes::KEY_SPACE, 1);
        held.update(KeyboardSource::Local(1), key_codes::KEY_SPACE, 1);
        assert_eq!(
            held.codes_for_source(KeyboardSource::Local(0)),
            std::vec![key_codes::KEY_SPACE]
        );
        assert!(!held.update(KeyboardSource::Local(0), key_codes::KEY_SPACE, 0));
        assert!(held.has_any(&[key_codes::KEY_SPACE]));
    }

    #[test]
    fn retry_delays_are_nonzero_and_device_scan_is_bounded() {
        assert!(DEVICE_SCAN_INTERVAL >= Duration::from_secs(1));
        assert!(SHORT_READ_DELAY > Duration::ZERO);
        assert_eq!((0..DEVICE_INDEX_LIMIT).count(), 8);
    }

    #[test]
    fn local_and_remote_buttons_share_one_logical_or_state() {
        let local = PointerSource::Local(0);
        let remote = PointerSource::Remote(7);
        let mut buttons = LogicalPointerButtons::default();

        assert_eq!(buttons.update(local, key_codes::BTN_LEFT, true), Some(true));
        assert_eq!(buttons.update(remote, key_codes::BTN_LEFT, false), None);
        assert_eq!(buttons.update(remote, key_codes::BTN_LEFT, true), None);
        assert_eq!(buttons.update(local, key_codes::BTN_LEFT, false), None);
        assert_eq!(
            buttons.update(remote, key_codes::BTN_LEFT, false),
            Some(false)
        );
    }

    #[test]
    fn remote_disconnect_releases_only_its_logical_buttons() {
        let local = PointerSource::Local(0);
        let remote = PointerSource::Remote(7);
        let mut buttons = LogicalPointerButtons::default();
        buttons.update(remote, key_codes::BTN_LEFT, true);
        assert_eq!(buttons.drain_source(PointerSource::Remote(99)), std::vec![]);
        assert_eq!(
            buttons.drain_source(remote),
            std::vec![(key_codes::BTN_LEFT, false)]
        );

        buttons.update(local, key_codes::BTN_LEFT, true);
        buttons.update(remote, key_codes::BTN_LEFT, true);
        assert_eq!(buttons.drain_source(remote), std::vec![]);
        assert_eq!(
            buttons.update(local, key_codes::BTN_LEFT, false),
            Some(false)
        );
    }

    #[test]
    fn non_owner_remote_disconnect_has_no_effect() {
        let owner = PointerSource::Remote(7);
        let mut buttons = LogicalPointerButtons::default();
        assert_eq!(buttons.update(owner, key_codes::BTN_LEFT, true), Some(true));
        assert_eq!(buttons.drain_source(PointerSource::Remote(99)), std::vec![]);
        assert_eq!(
            buttons.update(owner, key_codes::BTN_LEFT, false),
            Some(false)
        );
    }

    #[test]
    fn input_start_claim_is_idempotent_and_resettable_after_failure() {
        let started = AtomicBool::new(false);
        assert!(begin_input_start(&started));
        assert!(!begin_input_start(&started));
        started.store(false, Ordering::Release);
        assert!(begin_input_start(&started));
    }

    #[test]
    fn syn_dropped_cancels_mt_and_waits_for_clean_report() {
        let mut mt = touch::MtFrameAssembler::new(
            PointerSource::Local(8),
            TouchSurface::Touchpad { internal: true },
            0,
            1000,
            0,
            1000,
            2,
        );
        let mut desynced = false;
        consume_mt_event(
            &mut mt,
            &mut desynced,
            event(event_types::EV_ABS, abs_codes::ABS_MT_TRACKING_ID, 7),
        );
        consume_mt_event(
            &mut mt,
            &mut desynced,
            event(event_types::EV_ABS, abs_codes::ABS_MT_POSITION_X, 500),
        );
        consume_mt_event(
            &mut mt,
            &mut desynced,
            event(event_types::EV_ABS, abs_codes::ABS_MT_POSITION_Y, 500),
        );
        let cancel = consume_mt_event(
            &mut mt,
            &mut desynced,
            event(event_types::EV_SYN, syn_codes::SYN_DROPPED, 0),
        )
        .expect("overflow must emit cancellation");
        assert!(cancel.cancelled);
        assert!(desynced);

        assert!(
            consume_mt_event(
                &mut mt,
                &mut desynced,
                event(event_types::EV_ABS, abs_codes::ABS_MT_TRACKING_ID, 8),
            )
            .is_none()
        );
        assert!(
            consume_mt_event(
                &mut mt,
                &mut desynced,
                event(event_types::EV_SYN, syn_codes::SYN_REPORT, 0),
            )
            .is_none()
        );
        assert!(!desynced);
        let clean = consume_mt_event(
            &mut mt,
            &mut desynced,
            event(event_types::EV_SYN, syn_codes::SYN_REPORT, 0),
        )
        .expect("next complete frame must be accepted");
        assert!(clean.contacts.is_empty());
    }

    #[test]
    fn syn_dropped_resets_keyboard_and_rejects_interrupted_frame() {
        let source = KeyboardSource::Local(3);
        let mut desynced = false;
        assert_eq!(
            consume_keyboard_event(
                source,
                &mut desynced,
                event(event_types::EV_KEY, key_codes::KEY_SPACE, 1),
            ),
            Some(CompositorInputEvent::Keyboard {
                code: key_codes::KEY_SPACE,
                value: 1,
                source,
                synthetic: false,
            })
        );
        assert_eq!(
            consume_keyboard_event(
                source,
                &mut desynced,
                event(event_types::EV_SYN, syn_codes::SYN_DROPPED, 0),
            ),
            Some(CompositorInputEvent::KeyboardReset { source })
        );
        assert!(desynced);
        assert_eq!(
            consume_keyboard_event(
                source,
                &mut desynced,
                event(event_types::EV_KEY, key_codes::KEY_SPACE, 0),
            ),
            None
        );
        assert_eq!(
            consume_keyboard_event(
                source,
                &mut desynced,
                event(event_types::EV_SYN, syn_codes::SYN_REPORT, 0),
            ),
            None
        );
        assert!(!desynced);
    }
}
