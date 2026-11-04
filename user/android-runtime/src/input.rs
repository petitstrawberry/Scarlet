use scarlet_os::Handle;
use sws_client_c::scene::SwsRect;

pub const MOUSE_LEFT: u16 = 0x110;
pub const MOUSE_RIGHT: u16 = 0x111;
pub const MOUSE_MIDDLE: u16 = 0x112;
const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const EV_REL: u16 = 2;
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
    mouse: Vec<WireEvent>,
}

#[derive(Default)]
struct MouseState {
    buttons: [bool; 3],
    last: Option<(i32, i32)>,
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
    touch: TouchState,
    width: u32,
    height: u32,
}

impl Default for Core {
    fn default() -> Self {
        Self {
            keys: [false; 256],
            mouse: MouseState::default(),
            touch: TouchState::default(),
            width: 0,
            height: 0,
        }
    }
}

impl Core {
    fn key(&mut self, code: u16, value: i32) -> Vec<WireEvent> {
        if !(1..=255).contains(&code) || !(0..=2).contains(&value) {
            return Vec::new();
        }
        self.keys[code as usize] = value != 0;
        vec![(EV_KEY, code, value)]
    }

    fn mouse_motion(&mut self, r: SwsRect, x: i32, y: i32, inside: bool) -> Vec<WireEvent> {
        let captured = self.mouse.buttons.iter().any(|down| *down);
        if (!inside || !contains(r, x, y)) && !captured {
            self.mouse.last = None;
            return Vec::new();
        }
        let Some(position) = map(r, self.width, self.height, x, y) else {
            self.mouse.last = None;
            return Vec::new();
        };
        let Some(previous) = self.mouse.last.replace(position) else {
            return Vec::new();
        };
        let (dx, dy) = (position.0 - previous.0, position.1 - previous.1);
        if dx == 0 && dy == 0 {
            Vec::new()
        } else {
            vec![(EV_REL, REL_X, dx), (EV_REL, REL_Y, dy)]
        }
    }

    fn mouse_button(
        &mut self,
        r: SwsRect,
        x: i32,
        y: i32,
        code: u16,
        value: i32,
        inside: bool,
    ) -> Vec<WireEvent> {
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
            self.mouse.last = map(r, self.width, self.height, x, y);
            self.mouse.buttons[index] = true;
            vec![(EV_KEY, code, 1)]
        } else if self.mouse.buttons[index] {
            self.mouse.buttons[index] = false;
            vec![(EV_KEY, code, 0)]
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
    ) -> Vec<WireEvent> {
        if !inside || !contains(r, x, y) || !matches!(code, REL_WHEEL | REL_HWHEEL) || value == 0 {
            return Vec::new();
        }
        vec![(EV_REL, code, value)]
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
        let mut routed = Routed::default();
        for (index, code) in [MOUSE_LEFT, MOUSE_RIGHT, MOUSE_MIDDLE]
            .into_iter()
            .enumerate()
        {
            if self.mouse.buttons[index] {
                self.mouse.buttons[index] = false;
                routed.mouse.push((EV_KEY, code, 0));
            }
        }
        self.mouse.last = None;
        if let Some(active) = self.touch.active.take()
            && active.forwarded
        {
            routed.touch.extend_from_slice(&[
                (EV_ABS, ABS_X, self.touch.last.0),
                (EV_ABS, ABS_Y, self.touch.last.1),
                (EV_KEY, BTN_TOUCH, 0),
            ]);
        }
        for code in 1..256 {
            if self.keys[code] {
                routed.keyboard.push((EV_KEY, code as u16, 0));
                self.keys[code] = false;
            }
        }
        routed
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
    mouse: Handle,
    core: Core,
}

impl Input {
    pub fn new(keyboard: Handle, touch: Handle, mouse: Handle, width: u32, height: u32) -> Self {
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
        send(&self.mouse, &events)
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
        send(&self.mouse, &events)
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
        send(&self.mouse, &events)
    }

    pub fn touch(&mut self, r: SwsRect, change: TouchChange) -> Result<(), &'static str> {
        let events = self.core.touch(r, change);
        send(&self.touch, &events)
    }

    pub fn release(&mut self) -> Result<(), &'static str> {
        let routed = self.core.release();
        send(&self.mouse, &routed.mouse)?;
        send(&self.touch, &routed.touch)?;
        send(&self.keyboard, &routed.keyboard)
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

    fn down(id: u64) -> TouchChange {
        TouchChange {
            id,
            phase: TouchPhase::Down,
            x: 100,
            y: 100,
        }
    }

    #[test]
    fn letterbox_mapping_matches_physical_content_and_clamps_captured_drag() {
        let r = fitted();
        assert_eq!((r.x, r.y, r.width, r.height), (82, 42, 640, 480));
        assert!(!contains(r, 81, 42));
        assert_eq!(map(r, 640, 480, 82, 42), Some((0, 0)));
        assert_eq!(map(r, 640, 480, 1000, -1), Some((639, 0)));

        let mut core = core();
        core.touch(
            r,
            TouchChange {
                id: 1,
                phase: TouchPhase::Down,
                x: 82,
                y: 42,
            },
        );
        assert_eq!(
            core.touch(
                r,
                TouchChange {
                    id: 1,
                    phase: TouchPhase::Up,
                    x: 1000,
                    y: -1,
                },
            ),
            vec![
                (EV_ABS, ABS_X, 639),
                (EV_ABS, ABS_Y, 0),
                (EV_KEY, BTN_TOUCH, 0)
            ]
        );
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

    #[test]
    fn mouse_motion_buttons_and_wheels_use_only_the_mouse_producer() {
        let r = fitted();
        let mut core = core();
        assert!(core.mouse_motion(r, 100, 100, true).is_empty());
        assert_eq!(
            core.mouse_motion(r, 132, 100, true),
            vec![(EV_REL, REL_X, 32), (EV_REL, REL_Y, 0)]
        );
        assert!(core.mouse_motion(r, 20, 20, false).is_empty());
        assert!(core.mouse_motion(r, 164, 100, true).is_empty());
        assert_eq!(
            core.mouse_motion(r, 196, 100, true),
            vec![(EV_REL, REL_X, 32), (EV_REL, REL_Y, 0)]
        );

        let mut press = |code| core.mouse_button(r, 196, 100, code, 1, true);
        assert_eq!(press(MOUSE_LEFT), vec![(EV_KEY, MOUSE_LEFT, 1)]);
        assert_eq!(press(MOUSE_RIGHT), vec![(EV_KEY, MOUSE_RIGHT, 1)]);
        assert_eq!(press(MOUSE_MIDDLE), vec![(EV_KEY, MOUSE_MIDDLE, 1)]);
        assert_eq!(
            core.mouse_button(r, 0, 0, MOUSE_LEFT, 0, false),
            vec![(EV_KEY, MOUSE_LEFT, 0)]
        );
        assert!(core.mouse_button(r, 0, 0, MOUSE_LEFT, 0, false).is_empty());
        assert_eq!(
            core.mouse_wheel(r, 196, 100, REL_WHEEL, -1, true),
            vec![(EV_REL, REL_WHEEL, -1)]
        );
        assert_eq!(
            core.mouse_wheel(r, 196, 100, REL_HWHEEL, 2, true),
            vec![(EV_REL, REL_HWHEEL, 2)]
        );
        assert!(core.mouse_wheel(r, 20, 20, REL_WHEEL, 1, false).is_empty());
    }

    #[test]
    fn captured_mouse_drag_clamps_outside_but_new_outside_presses_stay_rejected() {
        let r = fitted();
        let mut core = core();
        core.mouse_motion(r, 196, 100, true);
        assert_eq!(
            core.mouse_button(r, 196, 100, MOUSE_LEFT, 1, true),
            vec![(EV_KEY, MOUSE_LEFT, 1)]
        );
        assert_eq!(
            core.mouse_motion(r, 1000, 600, false),
            vec![(EV_REL, REL_X, 525), (EV_REL, REL_Y, 421)]
        );
        assert_eq!(
            core.mouse_button(r, 1000, 600, MOUSE_LEFT, 0, false),
            vec![(EV_KEY, MOUSE_LEFT, 0)]
        );
        assert!(core.mouse_motion(r, 1100, 700, false).is_empty());
        assert!(
            core.mouse_button(r, 196, 100, MOUSE_RIGHT, 1, false)
                .is_empty()
        );
        assert!(core.mouse_motion(r, 220, 100, false).is_empty());
    }

    #[test]
    fn titlebar_press_is_not_injected_and_focus_release_is_complete() {
        let r = fitted();
        let mut core = core();
        assert!(
            core.mouse_button(r, 20, 10, MOUSE_LEFT, 1, false)
                .is_empty()
        );
        assert_eq!(
            core.mouse_button(r, 196, 100, MOUSE_RIGHT, 1, true),
            vec![(EV_KEY, MOUSE_RIGHT, 1)]
        );
        assert_eq!(
            core.touch(
                r,
                TouchChange {
                    id: 5,
                    phase: TouchPhase::Down,
                    x: 196,
                    y: 100,
                }
            )
            .len(),
            3
        );
        core.key(30, 1);
        let released = core.release();
        assert_eq!(released.mouse, vec![(EV_KEY, MOUSE_RIGHT, 0)]);
        assert_eq!(released.touch.last().copied(), Some((EV_KEY, BTN_TOUCH, 0)));
        assert_eq!(released.keyboard, vec![(EV_KEY, 30, 0)]);
    }

    #[test]
    fn native_records_have_stable_layout_and_append_syn() {
        let bytes = encode(&[(EV_REL, REL_X, 7), (EV_KEY, MOUSE_LEFT, 1)]);
        assert_eq!(bytes.len(), 48);
        assert_eq!(u16::from_ne_bytes([bytes[8], bytes[9]]), EV_REL);
        assert_eq!(u16::from_ne_bytes([bytes[10], bytes[11]]), REL_X);
        assert_eq!(i32::from_ne_bytes(bytes[12..16].try_into().unwrap()), 7);
        assert_eq!(u16::from_ne_bytes([bytes[24], bytes[25]]), EV_KEY);
        assert_eq!(u16::from_ne_bytes([bytes[26], bytes[27]]), MOUSE_LEFT);
        assert_eq!(i32::from_ne_bytes(bytes[28..32].try_into().unwrap()), 1);
        assert_eq!(&bytes[32..], &[0; 16]);
    }
}
