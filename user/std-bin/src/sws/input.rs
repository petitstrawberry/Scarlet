//! Input event handling module

#[path = "key_repeat.rs"]
mod key_repeat;

pub(crate) use key_repeat::{
    ConsumedKeys, HeldKeys, KeyRepeatState, KeyboardSource, forward_to_binary_key_protocol,
    is_initial_press, is_physical_key_value, should_retry_keyboard_read,
};

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::println;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::vec::Vec;

use scarlet_os::handle::capability::StreamError;
use scarlet_os::input::{INPUT_CAP_DIRECT_TOUCH, InputDevice, InputDeviceKind};

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
}

/// Global input event queue
static INPUT_EVENT_QUEUE: Mutex<Vec<CompositorInputEvent>> = Mutex::new(Vec::new());
static SCREEN_WIDTH: AtomicU32 = AtomicU32::new(1);
static SCREEN_HEIGHT: AtomicU32 = AtomicU32::new(1);
static INPUT_STARTED: AtomicBool = AtomicBool::new(false);

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
}

/// Synchronization event codes
pub mod syn_codes {
    pub const SYN_REPORT: u16 = 0;
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
}

impl PointerMetadata {
    fn query(device: &InputDevice, expected_kind: InputDeviceKind) -> Self {
        let kind = match device.kind() {
            Ok(InputDeviceKind::Unknown) | Err(_) => expected_kind,
            Ok(kind) => kind,
        };
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
}

impl PointerFrame {
    fn consume(
        &mut self,
        metadata: PointerMetadata,
        event: InputEvent,
    ) -> Option<Vec<CompositorInputEvent>> {
        match event.type_ {
            event_types::EV_REL => match event.code {
                rel_codes::REL_X => self.rel_x = self.rel_x.saturating_add(event.value),
                rel_codes::REL_Y => self.rel_y = self.rel_y.saturating_add(event.value),
                rel_codes::REL_WHEEL => self.wheel_dy = self.wheel_dy.saturating_add(event.value),
                rel_codes::REL_HWHEEL => self.wheel_dx = self.wheel_dx.saturating_add(event.value),
                _ => {}
            },
            event_types::EV_ABS => match event.code {
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
                let code = if metadata.direct_touch && event.code == key_codes::BTN_TOUCH {
                    key_codes::BTN_LEFT
                } else {
                    event.code
                };
                if event.value == 0 || event.value == 1 {
                    self.buttons.push((code, event.value == 1));
                }
            }
            event_types::EV_SYN if event.code == syn_codes::SYN_REPORT => {
                return Some(self.commit(metadata));
            }
            _ => {}
        }
        None
    }

    fn commit(&mut self, metadata: PointerMetadata) -> Vec<CompositorInputEvent> {
        let mut events = Vec::new();
        if self.rel_x != 0 || self.rel_y != 0 {
            events.push(CompositorInputEvent::MouseMove {
                dx: self.rel_x,
                dy: self.rel_y,
            });
        }
        if self.abs_dirty
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
    let mut frame = PointerFrame::default();
    loop {
        super::trace::input_loop();
        match read_input_event(&device) {
            Ok(Some(event)) => {
                super::trace::input_event();
                if let Some(events) = frame.consume(metadata, event) {
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
    release_pointer_source(source);
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
    loop {
        super::trace::keyboard_loop();
        match read_input_event(&device) {
            Ok(Some(event)) => {
                super::trace::keyboard_event();
                if event.type_ == event_types::EV_KEY {
                    push_input_event(CompositorInputEvent::Keyboard {
                        code: event.code,
                        value: event.value,
                        source,
                        synthetic: false,
                    });
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
        }
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
                    event(event_types::EV_ABS, abs_codes::ABS_X, 600)
                )
                .is_none()
        );
        assert!(
            touchpad
                .consume(
                    metadata(InputDeviceKind::Touchpad),
                    event(event_types::EV_REL, rel_codes::REL_X, 7)
                )
                .is_none()
        );
        let touchpad_events = touchpad
            .consume(
                metadata(InputDeviceKind::Touchpad),
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
                    event(event_types::EV_ABS, abs_codes::ABS_Y, 500)
                )
                .is_none()
        );
        let tablet_events = tablet
            .consume(
                metadata(InputDeviceKind::Tablet),
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
    fn touchscreen_motion_precedes_touch_as_left_button() {
        set_screen_size(1001, 2001);
        let mut frame = PointerFrame::default();
        frame.consume(
            metadata(InputDeviceKind::Touchscreen),
            event(event_types::EV_ABS, abs_codes::ABS_X, 600),
        );
        frame.consume(
            metadata(InputDeviceKind::Touchscreen),
            event(event_types::EV_ABS, abs_codes::ABS_Y, 500),
        );
        frame.consume(
            metadata(InputDeviceKind::Touchscreen),
            event(event_types::EV_KEY, key_codes::BTN_TOUCH, 1),
        );
        let events = frame
            .consume(
                metadata(InputDeviceKind::Touchscreen),
                event(event_types::EV_SYN, syn_codes::SYN_REPORT, 0),
            )
            .unwrap();
        assert_eq!(
            events,
            std::vec![
                CompositorInputEvent::MouseAbsolute { x: 500, y: 1000 },
                CompositorInputEvent::MouseButton {
                    button: key_codes::BTN_LEFT,
                    pressed: true
                }
            ]
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
}
