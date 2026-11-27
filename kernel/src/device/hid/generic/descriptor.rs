//! Bounded HID 1.11 short-item parser. Input offsets are independent for every
//! report ID and advance across padding, unmapped usages and other collections.
use crate::drivers::usb::hid::boot::translate_boot_key;
use alloc::{vec, vec::Vec};

pub(super) const MAX_DESCRIPTOR: usize = 4096;
const MAX_BITS: usize = 8192;
const MAX_SLOTS: usize = 512;
const MAX_APPS: usize = 16;
const MAX_DEPTH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Keyboard,
    Mouse,
    Gamepad,
    Joystick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Key {
    Ignored,
    Rollover,
    Code(u16),
}

pub(super) enum Mapping {
    Key(Key),
    Array(Vec<Key>),
    Absolute(u16),
    Relative(u16),
    Hat(u16),
}

pub(super) struct Field {
    pub id: u8,
    pub bit: usize,
    pub size: usize,
    pub count: usize,
    pub min: i32,
    pub max: i32,
    pub null: bool,
    pub mapping: Mapping,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Axis {
    pub code: u16,
    pub min: i32,
    pub max: i32,
    pub neutral: i32,
}

pub(super) struct Application {
    pub kind: Kind,
    pub fields: Vec<Field>,
    pub buttons: Vec<u16>,
    pub axes: Vec<Axis>,
    pub relative: bool,
}

impl Application {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            fields: Vec::new(),
            buttons: Vec::new(),
            axes: Vec::new(),
            relative: false,
        }
    }

    fn axis(&mut self, axis: Axis) -> Option<()> {
        if let Some(old) = self.axes.iter().find(|a| a.code == axis.code) {
            return (*old == axis).then_some(());
        }
        self.axes.push(axis);
        Some(())
    }

    fn add(&mut self, field: Field) -> Option<()> {
        let mut key = |key: Key| {
            if let Key::Code(code) = key
                && !self.buttons.contains(&code)
            {
                self.buttons.push(code);
            }
        };
        match &field.mapping {
            Mapping::Array(keys) => keys.iter().copied().for_each(key),
            Mapping::Key(k) => key(*k),
            Mapping::Relative(_) => self.relative = true,
            Mapping::Absolute(code) => {
                let (min, max) = (field.min, field.max);
                let neutral = if min < 0 {
                    0i32.clamp(min, max)
                } else if matches!(code, 0 | 1 | 3 | 4) {
                    (i64::from(min) + (i64::from(max) - i64::from(min) + 1) / 2) as i32
                } else {
                    min
                };
                self.axis(Axis {
                    code: *code,
                    min,
                    max,
                    neutral,
                })?;
            }
            Mapping::Hat(x) => {
                for code in [*x, x + 1] {
                    self.axis(Axis {
                        code,
                        min: -1,
                        max: 1,
                        neutral: 0,
                    })?;
                }
            }
        }
        self.fields.push(field);
        Some(())
    }
}

pub(super) struct Descriptor {
    pub numbered: bool,
    pub input_bits: [u16; 256],
    pub apps: Vec<Application>,
}

#[derive(Clone, Copy, Default)]
struct Globals {
    page: u16,
    min: i32,
    max_raw: u32,
    max_size: usize,
    size: usize,
    count: usize,
    id: u8,
}

fn signed(raw: u32, size: usize) -> i32 {
    if size == 0 {
        return 0;
    }
    let shift = 32 - size * 8;
    ((raw << shift) as i32) >> shift
}

#[derive(Default)]
struct Locals {
    ranges: Vec<(u32, u32)>,
    minimum: Option<u32>,
}

impl Locals {
    fn range(&mut self, first: u32, last: u32) -> Option<()> {
        if first > last || first >> 16 != last >> 16 || self.ranges.len() >= MAX_SLOTS {
            return None;
        }
        self.ranges.push((first, last));
        Some(())
    }

    fn usages(&self) -> impl Iterator<Item = u32> + '_ {
        self.ranges.iter().flat_map(|&(start, end)| start..=end)
    }
}

fn kind(usage: u32) -> Option<Kind> {
    match usage {
        0x10002 => Some(Kind::Mouse),
        0x10004 => Some(Kind::Joystick),
        0x10005 => Some(Kind::Gamepad),
        0x10006 | 0x10007 => Some(Kind::Keyboard),
        _ => None,
    }
}

fn key_usage(kind: Kind, usage: u32) -> Key {
    let id = usage as u16;
    let code = match (kind, usage >> 16, id) {
        (Kind::Keyboard, 7, 1..=3) => return Key::Rollover,
        (Kind::Keyboard, 7, 0x68..=0x73) => 183 + id - 0x68, // F13..F24
        (Kind::Keyboard, 7, 0x7f..=0x81) => [113, 115, 114][usize::from(id - 0x7f)],
        (Kind::Keyboard, 7, 0..=255) => {
            return translate_boot_key(id as u8).map_or(Key::Ignored, Key::Code);
        }
        (Kind::Mouse, 9, 1..=8) => 0x110 + id - 1,
        (Kind::Gamepad, 9, 1..=16) => 0x130 + id - 1,
        (Kind::Joystick, 9, 1..=16) => 0x120 + id - 1,
        (Kind::Gamepad | Kind::Joystick, 9, 17..=56) => 0x2c0 + id - 17,
        (Kind::Gamepad | Kind::Joystick, 1, 0x90..=0x93) => {
            [0x220, 0x221, 0x223, 0x222][usize::from(id - 0x90)]
        }
        _ => return Key::Ignored,
    };
    Key::Code(code)
}

fn variable(app: &Application, usage: u32, relative: bool, min: i32, max: i32) -> Option<Mapping> {
    match (app.kind, usage, relative) {
        (Kind::Mouse, 0x10030..=0x10032, true) => Some(Mapping::Relative((usage - 0x10030) as u16)),
        (Kind::Mouse, 0x10038, true) => Some(Mapping::Relative(8)),
        (Kind::Mouse, 0xc0238, true) => Some(Mapping::Relative(6)),
        (Kind::Mouse | Kind::Joystick | Kind::Gamepad, 0x10030..=0x10038, false) if min < max => {
            Some(Mapping::Absolute((usage - 0x10030) as u16))
        }
        (Kind::Joystick | Kind::Gamepad, 0x10039, false)
            if matches!(i64::from(max) - i64::from(min) + 1, 4 | 8) =>
        {
            let hats = app
                .fields
                .iter()
                .filter(|f| matches!(f.mapping, Mapping::Hat(_)))
                .count();
            (hats < 4).then_some(Mapping::Hat(0x10 + 2 * hats as u16))
        }
        (_, _, false) => match key_usage(app.kind, usage) {
            key @ Key::Code(_) => Some(Mapping::Key(key)),
            _ => None,
        },
        _ => None,
    }
}

fn input(
    app: &mut Application,
    g: Globals,
    start: usize,
    locals: &Locals,
    flags: u32,
    budget: &mut usize,
) -> Option<()> {
    if flags & (1 | 0x100) != 0 {
        return Some(());
    } // Padding or buffered stream.
    let min = g.min;
    let max = if min < 0 {
        signed(g.max_raw, g.max_size)
    } else {
        i32::try_from(g.max_raw).ok()?
    };
    if max < min || g.size > 32 || g.count > MAX_SLOTS {
        return None;
    }
    let field = |bit, count, mapping| Field {
        id: g.id,
        bit,
        size: g.size,
        count,
        min,
        max,
        null: flags & 64 != 0,
        mapping,
    };
    let mut usages = locals.usages();
    if flags & 2 != 0 {
        let mut last = 0;
        for i in 0..g.count {
            last = usages.next().unwrap_or(last);
            if let Some(mapping) = variable(app, last, flags & 4 != 0, min, max) {
                *budget = budget.checked_sub(1)?;
                app.add(field(start + i * g.size, 1, mapping))?;
            }
        }
    } else if flags & 4 == 0 {
        let limit = (i64::from(max) - i64::from(min) + 1) as u64;
        let keys: Vec<_> = usages
            .take(limit.min((MAX_SLOTS + 1) as u64) as usize)
            .map(|u| key_usage(app.kind, u))
            .collect();
        if keys.len() > MAX_SLOTS {
            return None;
        }
        if keys.iter().any(|k| matches!(k, Key::Code(_))) {
            *budget = budget.checked_sub(g.count.checked_add(keys.len())?)?;
            app.add(field(start, g.count, Mapping::Array(keys)))?;
        }
    }
    Some(())
}

/// Validate report framing without requiring an input usage understood by this
/// driver. Dedicated kernel drivers and raw-only interfaces can use other pages.
pub(crate) fn valid_descriptor(bytes: &[u8]) -> bool {
    Descriptor::inspect(bytes, false).is_some()
}

impl Descriptor {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let desc = Self::inspect(bytes, true)?;
        (!desc.apps.is_empty()).then_some(desc)
    }

    fn inspect(bytes: &[u8], map_inputs: bool) -> Option<Self> {
        if bytes.len() > MAX_DESCRIPTOR {
            return None;
        }
        let mut desc = Self {
            numbered: false,
            input_bits: [0; 256],
            apps: Vec::new(),
        };
        let mut globals = Globals::default();
        let mut stack = Vec::new();
        let mut locals = Locals::default();
        let mut collections: Vec<Option<usize>> = Vec::new();
        let mut budget: usize = 2048;
        let mut seen_unnumbered = false;
        let mut has_data = false;
        let mut delimiter = false;
        let mut other_bits = vec![[0u16; 2]; 256];
        let mut rest = bytes;
        while let Some((&prefix, tail)) = rest.split_first() {
            if prefix == 0xfe {
                if map_inputs {
                    return None;
                }
                let length = usize::from(*tail.first()?);
                rest = tail.get(2 + length..)?;
                continue;
            }
            let size = [0, 1, 2, 4][usize::from(prefix & 3)];
            let (data, next) = tail.split_at_checked(size)?;
            rest = next;
            let mut word = [0; 4];
            word[..size].copy_from_slice(data);
            let value = u32::from_le_bytes(word);
            let usage = if size == 4 {
                value
            } else {
                (u32::from(globals.page) << 16) | value
            };
            match prefix & 0xfc {
                0x04 => globals.page = u16::try_from(value).ok()?,
                0x14 => globals.min = signed(value, size),
                0x24 => {
                    globals.max_raw = value;
                    globals.max_size = size;
                }
                0x74 => globals.size = value as usize,
                0x84 => {
                    globals.id = u8::try_from(value).ok()?;
                    if globals.id == 0 {
                        return None;
                    }
                    desc.numbered = true;
                }
                0x94 => globals.count = value as usize,
                0xa4 if stack.len() < MAX_DEPTH => stack.push(globals),
                0xb4 => globals = stack.pop()?,
                0x08 if locals.minimum.is_none() => locals.range(usage, usage)?,
                0x18 if locals.minimum.is_none() => locals.minimum = Some(usage),
                0x28 => {
                    let first = locals.minimum.take()?;
                    locals.range(first, usage)?;
                }
                0xa8 => {
                    if map_inputs {
                        return None;
                    }
                    match (delimiter, value) {
                        (false, 1) => delimiter = true,
                        (true, 0) => delimiter = false,
                        _ => return None,
                    }
                }
                0x80 | 0x90 | 0xb0 => {
                    if collections.is_empty()
                        || locals.minimum.is_some()
                        || delimiter
                        || globals.count == 0
                        || globals.size == 0
                    {
                        return None;
                    }
                    has_data = true;
                    if globals.id == 0 {
                        seen_unnumbered = true;
                    }
                    let bits = globals.size.checked_mul(globals.count)?;
                    if bits > MAX_BITS {
                        return None;
                    }
                    let id = usize::from(globals.id);
                    let used = match prefix & 0xfc {
                        0x80 => &mut desc.input_bits[id],
                        0x90 => &mut other_bits[id][0],
                        _ => &mut other_bits[id][1],
                    };
                    let start = usize::from(*used);
                    *used = u16::try_from(start.checked_add(bits)?).ok()?;
                    if usize::from(*used) > MAX_BITS {
                        return None;
                    }
                    if prefix & 0xfc == 0x80
                        && let Some(index) = *collections.last()?
                    {
                        input(
                            &mut desc.apps[index],
                            globals,
                            start,
                            &locals,
                            value,
                            &mut budget,
                        )?;
                    }
                    locals = Locals::default();
                }
                0xa0 if collections.len() < MAX_DEPTH && locals.minimum.is_none() => {
                    let app = if map_inputs && value == 1 {
                        if let Some(kind) = locals.usages().next().and_then(kind) {
                            if desc.apps.len() >= MAX_APPS {
                                return None;
                            }
                            desc.apps.push(Application::new(kind));
                            Some(desc.apps.len() - 1)
                        } else {
                            None
                        }
                    } else {
                        collections.last().copied().flatten()
                    };
                    collections.push(app);
                    locals = Locals::default();
                }
                0xc0 if size == 0 && locals.minimum.is_none() => {
                    collections.pop()?;
                    locals = Locals::default();
                }
                0x34 | 0x44 | 0x54 | 0x64 | 0x38 | 0x48 | 0x58 | 0x78 | 0x88 | 0x98 => {} // Physical units and labels.
                _ => return None,
            }
        }
        if !collections.is_empty()
            || !stack.is_empty()
            || locals.minimum.is_some()
            || delimiter
            || !has_data
            || (desc.numbered && seen_unnumbered)
        {
            return None;
        }
        desc.apps.retain(|a| !a.fields.is_empty());
        Some(desc)
    }
}
