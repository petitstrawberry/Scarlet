use crate::mouse_wire::{Event as MouseWireEvent, Modifiers, MouseProducer, PushError};
use scarlet_os::Handle;
use sws_client_c::scene::SwsRect;

pub const MOUSE_LEFT: u16 = 0x110;
pub const MOUSE_RIGHT: u16 = 0x111;
pub const MOUSE_MIDDLE: u16 = 0x112;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const EV_ABS: u16 = 3;
const EV_KEY: u16 = 1;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const BTN_TOUCH: u16 = 0x14a;

type WireEvent = (u16, u16, i32);

#[derive(Default)]
struct Routed {
    keyboard: Vec<WireEvent>,
    touch: Vec<WireEvent>,
    mouse_cancel: Option<(i32, i32)>,
}

#[derive(Default)]
struct MouseState {
    buttons: [bool; 3],
    last: Option<(i32, i32)>,
}

#[derive(Default)]
struct ModifierLocks {
    caps: bool,
    num: bool,
    scroll: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Down,
    Move,
    Up,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TouchChange {
    pub id: u64,
    pub phase: TouchPhase,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy)]
struct ActiveTouch {
    id: u64,
    forwarded: bool,
}

#[derive(Default)]
struct TouchState {
    active: Option<ActiveTouch>,
    last: (i32, i32),
}

struct Core {
    keys: [bool; 256],
    mouse: MouseState,
    locks: ModifierLocks,
    touch: TouchState,
    width: u32,
    height: u32,
}

impl Default for Core {
    fn default() -> Self {
        Self {
            keys: [false; 256],
            mouse: MouseState::default(),
            locks: ModifierLocks::default(),
            touch: TouchState::default(),
            width: 0,
            height: 0,
        }
    }
}

impl Core {
    fn resize(&mut self, width: u32, height: u32) -> Routed {
        let routed = Routed {
            keyboard: Vec::new(),
            touch: self.release_touch(),
            mouse_cancel: self.mouse_cancel().and_then(|event| match event {
                MouseWireEvent::Cancel { x, y } => Some((x, y)),
                _ => None,
            }),
        };
        self.width = width;
        self.height = height;
        self.touch = TouchState::default();
        self.mouse = MouseState::default();
        routed
    }

    fn key(&mut self, code: u16, value: i32) -> Vec<WireEvent> {
        if !(1..=255).contains(&code) || !(0..=2).contains(&value) {
            return Vec::new();
        }
        self.keys[code as usize] = value != 0;
        if value == 1 {
            match code {
                58 => self.locks.caps = !self.locks.caps,
                69 => self.locks.num = !self.locks.num,
                70 => self.locks.scroll = !self.locks.scroll,
                _ => {}
            }
        }
        vec![(EV_KEY, code, value)]
    }

    fn modifiers(&self) -> Modifiers {
        let mut bits = 0;
        if self.keys[42] || self.keys[54] {
            bits |= Modifiers::SHIFT;
        }
        if self.keys[29] || self.keys[97] {
            bits |= Modifiers::CONTROL;
        }
        if self.keys[56] || self.keys[100] {
            bits |= Modifiers::ALT;
        }
        if self.keys[125] || self.keys[126] {
            bits |= Modifiers::META;
        }
        if self.locks.caps {
            bits |= Modifiers::CAPS_LOCK;
        }
        if self.locks.num {
            bits |= Modifiers::NUM_LOCK;
        }
        if self.locks.scroll {
            bits |= Modifiers::SCROLL_LOCK;
        }
        Modifiers::new(bits)
    }

    fn mouse_motion(&mut self, r: SwsRect, x: i32, y: i32, inside: bool) -> Vec<MouseWireEvent> {
        let captured = self.mouse.buttons.iter().any(|down| *down);
        if (!inside || !contains(r, x, y)) && !captured {
            return self
                .mouse
                .last
                .take()
                .map(|(x, y)| MouseWireEvent::Cancel { x, y })
                .into_iter()
                .collect();
        }
        let Some(position) = map(r, self.width, self.height, x, y) else {
            self.mouse.last = None;
            return Vec::new();
        };
        if self.mouse.last == Some(position) {
            return Vec::new();
        }
        self.mouse.last = Some(position);
        vec![MouseWireEvent::Position {
            x: position.0,
            y: position.1,
        }]
    }

    fn mouse_button(
        &mut self,
        r: SwsRect,
        x: i32,
        y: i32,
        code: u16,
        value: i32,
        inside: bool,
    ) -> Vec<MouseWireEvent> {
        let Some(index) = mouse_button_index(code) else {
            return Vec::new();
        };
        let pressed = match value {
            0 => false,
            1 | 2 => true,
            _ => return Vec::new(),
        };
        if pressed {
            if !inside || !contains(r, x, y) || self.mouse.buttons[index] {
                return Vec::new();
            }
            let Some(position) = map(r, self.width, self.height, x, y) else {
                return Vec::new();
            };
            self.mouse.last = Some(position);
            self.mouse.buttons[index] = true;
            vec![MouseWireEvent::Button {
                x: position.0,
                y: position.1,
                code,
                value: 1,
            }]
        } else if self.mouse.buttons[index] {
            self.mouse.buttons[index] = false;
            let position = map(r, self.width, self.height, x, y)
                .or(self.mouse.last)
                .unwrap_or((0, 0));
            let mut events = vec![MouseWireEvent::Button {
                x: position.0,
                y: position.1,
                code,
                value: 0,
            }];
            if !inside && !self.mouse.buttons.iter().any(|down| *down) {
                self.mouse.last = None;
                events.push(MouseWireEvent::Cancel {
                    x: position.0,
                    y: position.1,
                });
            } else {
                self.mouse.last = Some(position);
            }
            events
        } else {
            Vec::new()
        }
    }

    fn mouse_wheel(
        &mut self,
        r: SwsRect,
        x: i32,
        y: i32,
        code: u16,
        value: i32,
        inside: bool,
    ) -> Vec<MouseWireEvent> {
        if !inside || !contains(r, x, y) || !matches!(code, REL_WHEEL | REL_HWHEEL) || value == 0 {
            return Vec::new();
        }
        let Some(position) = map(r, self.width, self.height, x, y) else {
            return Vec::new();
        };
        vec![MouseWireEvent::Scroll {
            x: position.0,
            y: position.1,
            code,
            value,
        }]
    }

    fn mouse_cancel(&mut self) -> Option<MouseWireEvent> {
        let had_buttons = self.mouse.buttons.iter().any(|down| *down);
        self.mouse.buttons = [false; 3];
        let position = self.mouse.last.take();
        position
            .or(had_buttons.then_some((0, 0)))
            .map(|(x, y)| MouseWireEvent::Cancel { x, y })
    }

    fn touch(&mut self, r: SwsRect, change: TouchChange) -> Vec<WireEvent> {
        match change.phase {
            TouchPhase::Down => {
                if self.touch.active.is_some() {
                    return Vec::new();
                }
                let forwarded = contains(r, change.x, change.y);
                self.touch.active = Some(ActiveTouch {
                    id: change.id,
                    forwarded,
                });
                if !forwarded {
                    return Vec::new();
                }
                let Some(position) = map(r, self.width, self.height, change.x, change.y) else {
                    return Vec::new();
                };
                self.touch.last = position;
                vec![
                    (EV_ABS, ABS_X, position.0),
                    (EV_ABS, ABS_Y, position.1),
                    (EV_KEY, BTN_TOUCH, 1),
                ]
            }
            TouchPhase::Move => {
                let Some(active) = self.touch.active else {
                    return Vec::new();
                };
                if active.id != change.id || !active.forwarded {
                    return Vec::new();
                }
                let Some(position) = map(r, self.width, self.height, change.x, change.y) else {
                    return Vec::new();
                };
                self.touch.last = position;
                vec![
                    (EV_ABS, ABS_X, position.0),
                    (EV_ABS, ABS_Y, position.1),
                    (EV_KEY, BTN_TOUCH, 1),
                ]
            }
            TouchPhase::Up | TouchPhase::Cancel => {
                let Some(active) = self.touch.active else {
                    return Vec::new();
                };
                if active.id != change.id {
                    return Vec::new();
                }
                self.touch.active = None;
                if !active.forwarded {
                    return Vec::new();
                }
                if change.phase == TouchPhase::Up
                    && let Some(position) = map(r, self.width, self.height, change.x, change.y)
                {
                    self.touch.last = position;
                }
                vec![
                    (EV_ABS, ABS_X, self.touch.last.0),
                    (EV_ABS, ABS_Y, self.touch.last.1),
                    (EV_KEY, BTN_TOUCH, 0),
                ]
            }
        }
    }

    fn release(&mut self) -> Routed {
        Routed {
            keyboard: self.release_keys(),
            touch: self.release_touch(),
            mouse_cancel: self.mouse_cancel().and_then(|event| match event {
                MouseWireEvent::Cancel { x, y } => Some((x, y)),
                _ => None,
            }),
        }
    }

    fn release_keys(&mut self) -> Vec<WireEvent> {
        let mut keyboard = Vec::new();
        for code in 1..256 {
            if self.keys[code] {
                keyboard.push((EV_KEY, code as u16, 0));
                self.keys[code] = false;
            }
        }
        keyboard
    }

    fn release_touch(&mut self) -> Vec<WireEvent> {
        let Some(active) = self.touch.active.take() else {
            return Vec::new();
        };
        if !active.forwarded {
            return Vec::new();
        }
        vec![
            (EV_ABS, ABS_X, self.touch.last.0),
            (EV_ABS, ABS_Y, self.touch.last.1),
            (EV_KEY, BTN_TOUCH, 0),
        ]
    }
}

fn mouse_button_index(code: u16) -> Option<usize> {
    match code {
        MOUSE_LEFT => Some(0),
        MOUSE_RIGHT => Some(1),
        MOUSE_MIDDLE => Some(2),
        _ => None,
    }
}

fn encode(events: &[WireEvent]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity((events.len() + 1) * 16);
    for &(kind, code, value) in events.iter().chain(&[(0, 0, 0)]) {
        bytes.extend_from_slice(&0u64.to_ne_bytes());
        bytes.extend_from_slice(&kind.to_ne_bytes());
        bytes.extend_from_slice(&code.to_ne_bytes());
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

fn send(handle: &Handle, events: &[WireEvent]) -> Result<(), &'static str> {
    if events.is_empty() {
        return Ok(());
    }
    handle
        .as_stream()
        .map_err(|_| "input stream")?
        .write_all(&encode(events))
        .map_err(|_| "input delivery")
}

pub fn fit(content: SwsRect, width: u32, height: u32) -> SwsRect {
    let (w, h) = if u64::from(content.width) * u64::from(height)
        <= u64::from(content.height) * u64::from(width)
    {
        (
            content.width,
            (u64::from(content.width) * u64::from(height) / u64::from(width)) as u32,
        )
    } else {
        (
            (u64::from(content.height) * u64::from(width) / u64::from(height)) as u32,
            content.height,
        )
    };
    SwsRect {
        x: content.x + (content.width - w) / 2,
        y: content.y + (content.height - h) / 2,
        width: w,
        height: h,
    }
}

fn map(r: SwsRect, width: u32, height: u32, x: i32, y: i32) -> Option<(i32, i32)> {
    if r.width == 0 || r.height == 0 {
        return None;
    }
    Some((
        ((i64::from(x) - i64::from(r.x)).clamp(0, i64::from(r.width) - 1) * i64::from(width)
            / i64::from(r.width)) as i32,
        ((i64::from(y) - i64::from(r.y)).clamp(0, i64::from(r.height) - 1) * i64::from(height)
            / i64::from(r.height)) as i32,
    ))
}

pub fn contains(r: SwsRect, x: i32, y: i32) -> bool {
    i64::from(x) >= i64::from(r.x)
        && i64::from(y) >= i64::from(r.y)
        && i64::from(x) < i64::from(r.x) + i64::from(r.width)
        && i64::from(y) < i64::from(r.y) + i64::from(r.height)
}

pub struct Input {
    keyboard: Handle,
    touch: Handle,
    mouse: MouseProducer,
    core: Core,
}

impl Input {
    pub fn new(keyboard: Handle, touch: Handle, mouse: Handle, width: u32, height: u32) -> Self {
        let mouse = MouseProducer::new(mouse).unwrap_or_else(|error| {
            eprintln!("android-runtime: absolute mouse pipe disabled: {error}");
            MouseProducer::disabled()
        });
        Self {
            keyboard,
            touch,
            mouse,
            core: Core {
                width,
                height,
                ..Core::default()
            },
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), &'static str> {
        let routed = self.core.resize(width, height);
        if let Some((x, y)) = routed.mouse_cancel {
            self.enqueue_mouse(vec![MouseWireEvent::Cancel { x, y }]);
        }
        self.flush_mouse();
        send(&self.touch, &routed.touch)
    }

    fn enqueue_mouse(&mut self, events: Vec<MouseWireEvent>) {
        if !self.mouse.is_enabled() {
            return;
        }
        let modifiers = self.core.modifiers();
        for event in events {
            match self.mouse.push(event, modifiers) {
                Ok(()) => {}
                Err(PushError::Full) => {
                    eprintln!("android-runtime: absolute mouse pipe disabled: bounded queue full");
                    break;
                }
                Err(PushError::Closed) => break,
            }
        }
    }

    pub fn flush_mouse(&mut self) {
        if self.mouse.is_enabled() && self.mouse.flush().is_err() {
            eprintln!("android-runtime: absolute mouse pipe disabled: write failure");
        }
    }

    pub fn key(&mut self, code: u16, value: i32) -> Result<(), &'static str> {
        let events = self.core.key(code, value);
        send(&self.keyboard, &events)
    }

    pub fn mouse_motion(
        &mut self,
        r: SwsRect,
        x: i32,
        y: i32,
        inside: bool,
    ) -> Result<(), &'static str> {
        let events = self.core.mouse_motion(r, x, y, inside);
        self.enqueue_mouse(events);
        Ok(())
    }

    pub fn mouse_button(
        &mut self,
        r: SwsRect,
        x: i32,
        y: i32,
        code: u16,
        value: i32,
        inside: bool,
    ) -> Result<(), &'static str> {
        let events = self.core.mouse_button(r, x, y, code, value, inside);
        self.enqueue_mouse(events);
        Ok(())
    }

    pub fn mouse_wheel(
        &mut self,
        r: SwsRect,
        x: i32,
        y: i32,
        code: u16,
        value: i32,
        inside: bool,
    ) -> Result<(), &'static str> {
        let events = self.core.mouse_wheel(r, x, y, code, value, inside);
        self.enqueue_mouse(events);
        Ok(())
    }

    pub fn mouse_cancel(&mut self) {
        if let Some(event) = self.core.mouse_cancel() {
            self.enqueue_mouse(vec![event]);
        }
        self.flush_mouse();
    }

    pub fn touch(&mut self, r: SwsRect, change: TouchChange) -> Result<(), &'static str> {
        let events = self.core.touch(r, change);
        send(&self.touch, &events)
    }

    pub fn release(&mut self) -> Result<(), &'static str> {
        let routed = self.core.release();
        if let Some((x, y)) = routed.mouse_cancel {
            self.enqueue_mouse(vec![MouseWireEvent::Cancel { x, y }]);
        }
        self.flush_mouse();
        let touch = send(&self.touch, &routed.touch);
        let keyboard = send(&self.keyboard, &routed.keyboard);
        touch?;
        keyboard
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fitted() -> SwsRect {
        fit(
            SwsRect {
                x: 2,
                y: 42,
                width: 800,
                height: 480,
            },
            640,
            480,
        )
    }

    fn core() -> Core {
        Core {
            width: 640,
            height: 480,
            ..Core::default()
        }
    }

    #[test]
    fn resize_updates_mapping_and_clears_contact_state() {
        let mut core = core();
        core.mouse_motion(fitted(), 100, 100, true);
        assert!(core.touch.active.is_none());
        core.resize(480, 640);
        assert_eq!((core.width, core.height), (480, 640));
        assert_eq!(core.mouse.buttons, [false; 3]);
        assert!(core.mouse.last.is_none());
        assert!(core.touch.active.is_none());
    }

    #[test]
    fn resize_cancels_active_mouse_and_touch_before_remapping() {
        let r = fitted();
        let mut core = core();
        core.mouse_motion(r, 196, 100, true);
        core.mouse_button(r, 196, 100, MOUSE_LEFT, 1, true);
        core.touch(r, down(17));

        let routed = core.resize(480, 640);
        assert_eq!(routed.mouse_cancel, Some((114, 58)));
        assert_eq!(
            routed.touch,
            vec![
                (EV_ABS, ABS_X, 18),
                (EV_ABS, ABS_Y, 58),
                (EV_KEY, BTN_TOUCH, 0),
            ]
        );
        assert_eq!((core.width, core.height), (480, 640));
        assert_eq!(core.mouse.buttons, [false; 3]);
        assert!(core.touch.active.is_none());
    }

    fn down(id: u64) -> TouchChange {
        TouchChange {
            id,
            phase: TouchPhase::Down,
            x: 100,
            y: 100,
        }
    }

    #[test]
    fn letterbox_mapping_and_captured_drag_keep_source_clamps() {
        let r = fitted();
        assert_eq!((r.x, r.y, r.width, r.height), (82, 42, 640, 480));
        assert!(!contains(r, 81, 42));
        assert_eq!(map(r, 640, 480, 82, 42), Some((0, 0)));
        assert_eq!(map(r, 640, 480, 1000, -1), Some((639, 0)));

        let mut core = core();
        assert_eq!(
            core.mouse_motion(r, 196, 100, true),
            vec![MouseWireEvent::Position { x: 114, y: 58 }]
        );
        assert_eq!(
            core.mouse_button(r, 196, 100, MOUSE_LEFT, 1, true),
            vec![MouseWireEvent::Button {
                x: 114,
                y: 58,
                code: MOUSE_LEFT,
                value: 1,
            }]
        );
        assert_eq!(
            core.mouse_motion(r, 1000, 600, false),
            vec![MouseWireEvent::Position { x: 639, y: 479 }]
        );
        assert_eq!(
            core.mouse_button(r, 1000, 600, MOUSE_LEFT, 0, false),
            vec![
                MouseWireEvent::Button {
                    x: 639,
                    y: 479,
                    code: MOUSE_LEFT,
                    value: 0,
                },
                MouseWireEvent::Cancel { x: 639, y: 479 },
            ]
        );
    }

    #[test]
    fn initial_absolute_motion_scaling_and_mouseleave_are_explicit() {
        let r = fitted();
        let mut core = core();
        assert_eq!(
            core.mouse_motion(r, 100, 100, true),
            vec![MouseWireEvent::Position { x: 18, y: 58 }]
        );
        assert_eq!(
            core.mouse_motion(r, 132, 100, true),
            vec![MouseWireEvent::Position { x: 50, y: 58 }]
        );
        assert_eq!(
            core.mouse_motion(r, 20, 20, false),
            vec![MouseWireEvent::Cancel { x: 50, y: 58 }]
        );
        assert_eq!(
            core.mouse_motion(r, 164, 100, true),
            vec![MouseWireEvent::Position { x: 82, y: 58 }]
        );

        let scaled = fit(
            SwsRect {
                x: 20,
                y: 30,
                width: 320,
                height: 240,
            },
            640,
            480,
        );
        let mut scaled_core = Core {
            width: 640,
            height: 480,
            ..Core::default()
        };
        assert_eq!(
            scaled_core.mouse_motion(scaled, 180, 150, true),
            vec![MouseWireEvent::Position { x: 320, y: 240 }]
        );
    }

    #[test]
    fn buttons_and_wheels_carry_mapped_absolute_positions() {
        let r = fitted();
        let mut core = core();
        assert_eq!(
            core.mouse_button(r, 196, 100, MOUSE_LEFT, 1, true),
            vec![MouseWireEvent::Button {
                x: 114,
                y: 58,
                code: MOUSE_LEFT,
                value: 1,
            }]
        );
        assert_eq!(
            core.mouse_button(r, 0, 0, MOUSE_LEFT, 0, false),
            vec![
                MouseWireEvent::Button {
                    x: 0,
                    y: 0,
                    code: MOUSE_LEFT,
                    value: 0,
                },
                MouseWireEvent::Cancel { x: 0, y: 0 },
            ]
        );
        assert_eq!(
            core.mouse_wheel(r, 196, 100, REL_WHEEL, -1, true),
            vec![MouseWireEvent::Scroll {
                x: 114,
                y: 58,
                code: REL_WHEEL,
                value: -1,
            }]
        );
        assert_eq!(
            core.mouse_wheel(r, 196, 100, REL_HWHEEL, 2, true),
            vec![MouseWireEvent::Scroll {
                x: 114,
                y: 58,
                code: REL_HWHEEL,
                value: 2,
            }]
        );
        assert!(core.mouse_wheel(r, 20, 20, REL_WHEEL, 1, false).is_empty());
    }

    #[test]
    fn focus_release_cancels_mouse_but_preserves_touch_and_key_release() {
        let r = fitted();
        let mut core = core();
        core.mouse_button(r, 196, 100, MOUSE_RIGHT, 1, true);
        assert_eq!(
            core.touch(r, down(5)),
            vec![
                (EV_ABS, ABS_X, 18),
                (EV_ABS, ABS_Y, 58),
                (EV_KEY, BTN_TOUCH, 1),
            ]
        );
        core.key(30, 1);
        let released = core.release();
        assert_eq!(released.mouse_cancel, Some((114, 58)));
        assert_eq!(
            released.touch,
            vec![
                (EV_ABS, ABS_X, 18),
                (EV_ABS, ABS_Y, 58),
                (EV_KEY, BTN_TOUCH, 0),
            ]
        );
        assert_eq!(released.keyboard, vec![(EV_KEY, 30, 0)]);
    }

    #[test]
    fn mouse_and_real_touch_state_remain_independent() {
        let r = fitted();
        let mut core = core();
        core.mouse_motion(r, 100, 100, true);
        assert_eq!(core.touch(r, down(9)).len(), 3);
        assert_eq!(
            core.touch(
                r,
                TouchChange {
                    id: 9,
                    phase: TouchPhase::Move,
                    x: 110,
                    y: 100,
                },
            ),
            vec![
                (EV_ABS, ABS_X, 28),
                (EV_ABS, ABS_Y, 58),
                (EV_KEY, BTN_TOUCH, 1),
            ]
        );
        assert_eq!(
            core.mouse_motion(r, 20, 20, false),
            vec![MouseWireEvent::Cancel { x: 18, y: 58 }]
        );
        assert_eq!(
            core.touch(
                r,
                TouchChange {
                    id: 9,
                    phase: TouchPhase::Up,
                    x: 110,
                    y: 100,
                },
            ),
            vec![
                (EV_ABS, ABS_X, 28),
                (EV_ABS, ABS_Y, 58),
                (EV_KEY, BTN_TOUCH, 0),
            ]
        );
    }

    #[test]
    fn portable_modifiers_track_both_sides_and_lock_keys() {
        let mut core = core();
        for code in [42, 29, 56, 125, 58, 69, 70] {
            core.key(code, 1);
        }
        let modifiers = core.modifiers();
        assert_eq!(modifiers.bits(), 0x7f);
        core.key(42, 0);
        assert_eq!(core.modifiers().bits(), 0x7f & !Modifiers::SHIFT);
        core.key(54, 1);
        assert_eq!(core.modifiers().bits() & Modifiers::SHIFT, Modifiers::SHIFT);
    }

    #[test]
    fn touch_keeps_first_contact_and_ignores_pointer_style_replacement() {
        let r = fitted();
        let mut core = core();
        let down = |id, x| TouchChange {
            id,
            phase: TouchPhase::Down,
            x,
            y: 100,
        };
        assert_eq!(core.touch(r, down(41, 100)).len(), 3);
        assert!(core.touch(r, down(42, 200)).is_empty());
        assert!(
            core.touch(
                r,
                TouchChange {
                    id: 42,
                    phase: TouchPhase::Move,
                    x: 220,
                    y: 110,
                }
            )
            .is_empty()
        );
        assert_eq!(
            core.touch(
                r,
                TouchChange {
                    id: 41,
                    phase: TouchPhase::Move,
                    x: 110,
                    y: 100,
                }
            ),
            vec![
                (EV_ABS, ABS_X, 28),
                (EV_ABS, ABS_Y, 58),
                (EV_KEY, BTN_TOUCH, 1)
            ]
        );
        assert!(
            core.touch(
                r,
                TouchChange {
                    id: 42,
                    phase: TouchPhase::Up,
                    x: 220,
                    y: 110,
                }
            )
            .is_empty()
        );
        assert_eq!(
            core.touch(
                r,
                TouchChange {
                    id: 41,
                    phase: TouchPhase::Up,
                    x: 110,
                    y: 100,
                }
            ),
            vec![
                (EV_ABS, ABS_X, 28),
                (EV_ABS, ABS_Y, 58),
                (EV_KEY, BTN_TOUCH, 0)
            ]
        );
    }

    #[test]
    fn chrome_touch_blocks_replacement_and_captured_touch_releases_outside() {
        let r = fitted();
        let mut core = core();
        assert!(
            core.touch(
                r,
                TouchChange {
                    id: 7,
                    phase: TouchPhase::Down,
                    x: 20,
                    y: 10,
                }
            )
            .is_empty()
        );
        assert!(
            core.touch(
                r,
                TouchChange {
                    id: 8,
                    phase: TouchPhase::Down,
                    x: 100,
                    y: 100,
                }
            )
            .is_empty()
        );
        assert!(
            core.touch(
                r,
                TouchChange {
                    id: 7,
                    phase: TouchPhase::Up,
                    x: 20,
                    y: 10,
                }
            )
            .is_empty()
        );
        assert_eq!(
            core.touch(r, down(9)),
            vec![
                (EV_ABS, ABS_X, 18),
                (EV_ABS, ABS_Y, 58),
                (EV_KEY, BTN_TOUCH, 1)
            ]
        );
        assert_eq!(
            core.touch(
                r,
                TouchChange {
                    id: 9,
                    phase: TouchPhase::Cancel,
                    x: 0,
                    y: 0,
                }
            ),
            vec![
                (EV_ABS, ABS_X, 18),
                (EV_ABS, ABS_Y, 58),
                (EV_KEY, BTN_TOUCH, 0)
            ]
        );
    }
}
