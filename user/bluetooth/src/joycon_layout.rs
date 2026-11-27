//! Joy-Con layout policy operates on complete, side-local evdev snapshots.
use std::collections::BTreeMap;

pub const KEY: u16 = 1;
pub const ABS: u16 = 3;
pub const X: u16 = 0;
pub const Y: u16 = 1;
pub const RX: u16 = 3;
pub const RY: u16 = 4;
pub const HAT_X: u16 = 0x10;
pub const HAT_Y: u16 = 0x11;
pub const CENTER: i32 = 2048;
pub const MAX: i32 = 4095;

const SOUTH: u16 = 0x130;
const EAST: u16 = 0x131;
const NORTH: u16 = 0x133;
const WEST: u16 = 0x134;
const L: u16 = 0x136;
const R: u16 = 0x137;
const ZL: u16 = 0x138;
const ZR: u16 = 0x139;
const MINUS: u16 = 0x13a;
const PLUS: u16 = 0x13b;
const HOME: u16 = 0x13c;
const THUMBL: u16 = 0x13d;
const THUMBR: u16 = 0x13e;
const CAPTURE: u16 = 0x2c0;
const LEFT_SR: u16 = 0x2c1;
const LEFT_SL: u16 = 0x2c2;
const RIGHT_SR: u16 = 0x2c3;
const RIGHT_SL: u16 = 0x2c4;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum Id {
    Left(u8),
    Right(u8),
}

impl Id {
    pub fn path(self) -> String {
        match self {
            Self::Left(n) => format!("/dev/joycon-left{n}"),
            Self::Right(n) => format!("/dev/joycon-right{n}"),
        }
    }

    fn buttons(self) -> &'static [u16] {
        match self {
            Self::Left(_) => &[L, ZL, MINUS, THUMBL, CAPTURE, LEFT_SR, LEFT_SL],
            Self::Right(_) => &[
                SOUTH, EAST, NORTH, WEST, R, ZR, PLUS, HOME, THUMBR, RIGHT_SR, RIGHT_SL,
            ],
        }
    }

    fn axes(self) -> &'static [u16] {
        match self {
            Self::Left(_) => &[X, Y, HAT_X, HAT_Y],
            Self::Right(_) => &[RX, RY],
        }
    }

    fn rails(self) -> u32 {
        match self {
            Self::Left(_) => bit(LEFT_SR) | bit(LEFT_SL),
            Self::Right(_) => bit(RIGHT_SR) | bit(RIGHT_SL),
        }
    }

    fn shoulders(self) -> u32 {
        match self {
            Self::Left(_) => bit(L) | bit(ZL),
            Self::Right(_) => bit(R) | bit(ZR),
        }
    }
}

fn bit(key: u16) -> u32 {
    match key {
        SOUTH..=THUMBR => 1 << (key - SOUTH),
        CAPTURE..=RIGHT_SL => 1 << (16 + key - CAPTURE),
        _ => 0,
    }
}

pub type Event = (u16, u16, i32);
pub const SYN: Event = (0, 0, 0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    keys: u32,
    stick: [i32; 2],
    hat: [i32; 2],
}

impl Default for Sample {
    fn default() -> Self {
        Self {
            keys: 0,
            stick: [CENTER; 2],
            hat: [0; 2],
        }
    }
}

/// The raw endpoint supplies every control in each report, even on open and
/// after overflow. Never act on a partial frame or the SYN_DROPPED boundary.
#[derive(Default)]
pub struct Decoder {
    sample: Sample,
    keys_seen: u32,
    axes_seen: u8,
    discard: bool,
}

impl Decoder {
    pub fn event(&mut self, id: Id, event: Event) -> Option<Sample> {
        match event {
            (0, 3, _) => self.discard = true,
            SYN => {
                let frame = std::mem::take(self);
                let complete = !frame.discard
                    && frame.keys_seen.count_ones() as usize == id.buttons().len()
                    && frame.axes_seen.count_ones() as usize == id.axes().len();
                return complete.then_some(frame.sample);
            }
            (KEY, key, value @ (0 | 1)) if id.buttons().contains(&key) => {
                let mask = bit(key);
                self.keys_seen |= mask;
                if value == 1 {
                    self.sample.keys |= mask
                } else {
                    self.sample.keys &= !mask
                }
            }
            (ABS, code, value) => {
                if let Some(i) = id.axes().iter().position(|&c| c == code) {
                    let valid = if i < 2 {
                        (0..=MAX).contains(&value)
                    } else {
                        (-1..=1).contains(&value)
                    };
                    self.discard |= !valid;
                    self.axes_seen |= 1 << i;
                    if i < 2 {
                        self.sample.stick[i] = value
                    } else {
                        self.sample.hat[i - 2] = value
                    }
                }
            }
            _ => {}
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layout {
    Upright(Id),
    Mini(Id),
    Pair(Id, Id), // left, right
}

impl Layout {
    pub fn contains(self, id: Id) -> bool {
        match self {
            Self::Upright(a) | Self::Mini(a) => a == id,
            Self::Pair(a, b) => a == id || b == id,
        }
    }

    fn partner(self, id: Id) -> Option<Id> {
        match self {
            Self::Pair(a, b) if a == id => Some(b),
            Self::Pair(a, b) if b == id => Some(a),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Half {
    raw: Sample,
    // A layout gesture must be released before its constituent buttons can
    // become gameplay input in the newly created pad.
    consumed: u32,
}

#[derive(Default)]
pub struct Seat {
    halves: BTreeMap<Id, Half>,
    pub pads: BTreeMap<Layout, u8>,
}

impl Seat {
    fn assign(&mut self, layout: Layout) {
        let mut counts = [0; 4];
        for &player in self.pads.values() {
            counts[usize::from(player - 1)] += 1;
        }
        let index = (0..4).min_by_key(|&i| counts[i]).unwrap();
        self.pads.insert(layout, index as u8 + 1);
    }

    fn layout(&self, id: Id) -> Option<Layout> {
        self.pads.keys().copied().find(|layout| layout.contains(id))
    }

    pub fn remove(&mut self, id: Id) {
        self.halves.remove(&id);
        if let Some(layout) = self.layout(id) {
            let player = self.pads.remove(&layout).unwrap();
            if let Some(partner) = layout.partner(id) {
                self.pads.insert(Layout::Upright(partner), player);
            }
        }
    }

    pub fn update(&mut self, id: Id, raw: Sample) {
        let layout = self.layout(id).unwrap_or_else(|| {
            let single = Layout::Upright(id);
            self.assign(single);
            single
        });
        let half = self.halves.entry(id).or_default();
        let prev = half.raw.keys;
        half.raw = raw;
        half.consumed &= raw.keys;
        let rails = id.rails();
        if raw.keys & rails == rails && prev & rails != rails && layout != Layout::Mini(id) {
            half.consumed |= rails;
            let player = self.pads.remove(&layout).unwrap();
            self.pads.insert(Layout::Mini(id), player);
            if let Some(partner) = layout.partner(id) {
                self.assign(Layout::Upright(partner));
            }
            return;
        }

        let shoulders = id.shoulders();
        if layout.partner(id).is_some() || raw.keys & shoulders == 0 || prev & shoulders != 0 {
            return;
        }
        let partner = self.pads.keys().find_map(|&other| {
            let (Layout::Upright(peer) | Layout::Mini(peer)) = other else {
                return None;
            };
            if matches!(
                (id, peer),
                (Id::Left(_), Id::Right(_)) | (Id::Right(_), Id::Left(_))
            ) {
                let half = &self.halves[&peer];
                if half.raw.keys & !half.consumed & peer.shoulders() != 0 {
                    return Some((other, peer));
                }
            }
            None
        });
        if let Some((other, peer)) = partner {
            let player = self
                .pads
                .remove(&layout)
                .unwrap()
                .min(self.pads.remove(&other).unwrap());
            for member in [id, peer] {
                let half = self.halves.get_mut(&member).unwrap();
                half.consumed |= half.raw.keys & member.shoulders();
            }
            let pair = match id {
                Id::Left(_) => Layout::Pair(id, peer),
                Id::Right(_) => Layout::Pair(peer, id),
            };
            self.pads.insert(pair, player);
        }
    }

    pub fn snapshot(&self, layout: Layout) -> Vec<Event> {
        let mut out = Vec::with_capacity(25);
        let mut append = |id, mini| {
            let half = &self.halves[&id];
            let mut state = half.raw;
            state.keys &= !half.consumed;
            if mini {
                sideways(&mut out, id, state)
            } else {
                upright(&mut out, id, state)
            }
        };
        match layout {
            Layout::Upright(id) => append(id, false),
            Layout::Mini(id) => append(id, true),
            Layout::Pair(left, right) => {
                append(left, false);
                append(right, false);
            }
        }
        out.push(SYN);
        out
    }
}

fn upright(out: &mut Vec<Event>, id: Id, state: Sample) {
    for &key in id.buttons() {
        out.push((KEY, key, i32::from(state.keys & bit(key) != 0)));
    }
    for (&code, value) in id
        .axes()
        .iter()
        .zip(state.stick.into_iter().chain(state.hat))
    {
        out.push((ABS, code, value));
    }
}

fn reverse(value: i32) -> i32 {
    if value == CENTER { CENTER } else { MAX - value }
}

// SDL hidapi Switch HandleMiniControllerStateL/R (standard full report).
// Scarlet's upright Y already increases downwards. Both mini sticks are XY.
fn sideways(out: &mut Vec<Event>, id: Id, state: Sample) {
    let [x, y] = state.stick;
    let (stick, mappings): (_, &[(u16, u16)]) = match id {
        Id::Left(_) => {
            let [hx, hy] = state.hat;
            for (key, pressed) in [
                (SOUTH, hx < 0),
                (EAST, hy > 0),
                (NORTH, hx > 0),
                (WEST, hy < 0),
            ] {
                out.push((KEY, key, i32::from(pressed)));
            }
            (
                [y, reverse(x)],
                &[
                    (MINUS, PLUS),
                    (CAPTURE, HOME),
                    (THUMBL, THUMBL),
                    (LEFT_SL, L),
                    (LEFT_SR, R),
                    (L, LEFT_SL),
                    (ZL, LEFT_SR),
                ],
            )
        }
        Id::Right(_) => (
            [reverse(y), x],
            &[
                (EAST, SOUTH),
                (NORTH, EAST),
                (WEST, NORTH),
                (SOUTH, WEST),
                (PLUS, PLUS),
                (HOME, HOME),
                (THUMBR, THUMBL),
                (RIGHT_SL, L),
                (RIGHT_SR, R),
                (R, RIGHT_SR),
                (ZR, RIGHT_SL),
            ],
        ),
    };
    for &(src, dst) in mappings {
        out.push((KEY, dst, i32::from(state.keys & bit(src) != 0)));
    }
    out.extend([(ABS, X, stick[0]), (ABS, Y, stick[1])]);
}

#[cfg(test)]
#[path = "joycon_layout_tests.rs"]
mod tests;
