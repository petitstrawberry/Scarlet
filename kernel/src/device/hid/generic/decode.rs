//! Report decoding is transactional. Each application retains its key state per
//! report ID, since an input packet updates only the fields in that report.
use super::descriptor::{Application, Key, Kind, Mapping};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};

pub(super) type Event = (u16, u16, i32);
const SYN: Event = (0, 0, 0);

#[derive(Default)]
struct Values {
    keys: BTreeSet<u16>,
    absolute: BTreeMap<u16, i32>,
    relative: BTreeMap<u16, i32>,
}

enum Invalid {
    Rollover,
    Malformed,
}

fn extract(data: &[u8], bit: usize, width: usize, signed: bool) -> Option<i64> {
    let shift = bit % 8;
    let mut word = 0u64;
    for (i, &b) in data
        .get(bit / 8..(bit + width).div_ceil(8))?
        .iter()
        .enumerate()
    {
        word |= u64::from(b) << (i * 8);
    }
    let value = ((word >> shift) & ((1u64 << width) - 1)) as i64;
    Some(if signed && value & (1i64 << (width - 1)) != 0 {
        value - (1i64 << width)
    } else {
        value
    })
}

fn decode(app: &Application, id: u8, data: &[u8]) -> Result<Values, Invalid> {
    let mut values = Values::default();
    for field in app.fields.iter().filter(|f| f.id == id) {
        for index in 0..field.count {
            let v = extract(
                data,
                field.bit + index * field.size,
                field.size,
                field.min < 0,
            )
            .ok_or(Invalid::Malformed)?;
            let within = (i64::from(field.min)..=i64::from(field.max)).contains(&v);
            match &field.mapping {
                Mapping::Array(keys) => {
                    if within {
                        match keys.get((v - i64::from(field.min)) as usize) {
                            Some(Key::Code(code)) => {
                                values.keys.insert(*code);
                            }
                            Some(Key::Rollover) => return Err(Invalid::Rollover),
                            _ => {}
                        }
                    }
                }
                Mapping::Hat(x) => {
                    let (dx, dy) = if within {
                        const HAT: [(i32, i32); 8] = [
                            (0, -1),
                            (1, -1),
                            (1, 0),
                            (1, 1),
                            (0, 1),
                            (-1, 1),
                            (-1, 0),
                            (-1, -1),
                        ];
                        let directions = i64::from(field.max) - i64::from(field.min) + 1;
                        HAT[((v - i64::from(field.min)) * 8 / directions) as usize]
                    } else if field.null {
                        (0, 0)
                    } else {
                        return Err(Invalid::Malformed);
                    };
                    values.absolute.insert(*x, dx);
                    values.absolute.insert(x + 1, dy);
                }
                _ if !within && field.null => {}
                _ if !within => return Err(Invalid::Malformed),
                Mapping::Key(Key::Code(code)) if v != 0 => {
                    values.keys.insert(*code);
                }
                Mapping::Key(_) => {}
                Mapping::Absolute(code) => {
                    values.absolute.insert(*code, v as i32);
                }
                Mapping::Relative(code) => {
                    let delta = values.relative.entry(*code).or_default();
                    *delta = delta.saturating_add(v as i32);
                }
            }
        }
    }
    Ok(values)
}

pub(super) struct Input {
    pub app: Application,
    keys_by_id: BTreeMap<u8, BTreeSet<u16>>,
    axes: BTreeMap<u16, i32>,
}

impl Input {
    pub fn new(app: Application) -> Self {
        let axes = app.axes.iter().map(|a| (a.code, a.neutral)).collect();
        Self {
            app,
            keys_by_id: BTreeMap::new(),
            axes,
        }
    }

    fn gamepad(&self) -> bool {
        matches!(self.app.kind, Kind::Gamepad | Kind::Joystick)
    }

    fn held(&self, code: u16) -> bool {
        self.keys_by_id.values().any(|keys| keys.contains(&code))
    }

    pub fn snapshot(&self) -> Vec<Event> {
        let mut frame = Vec::new();
        for &key in &self.app.buttons {
            frame.push((1, key, i32::from(self.held(key))));
        }
        for (&axis, &value) in &self.axes {
            frame.push((3, axis, value));
        }
        frame.push(SYN);
        frame
    }

    fn apply(&mut self, id: u8, values: Values) -> Vec<Event> {
        let previous = self.keys_by_id.insert(id, values.keys).unwrap_or_default();
        let mut frame = Vec::new();
        if !self.gamepad() {
            for &code in previous.symmetric_difference(&self.keys_by_id[&id]) {
                if !self
                    .keys_by_id
                    .iter()
                    .any(|(&other, keys)| other != id && keys.contains(&code))
                {
                    frame.push((1, code, i32::from(self.keys_by_id[&id].contains(&code))));
                }
            }
        }
        for (code, value) in values.absolute {
            if self.axes.insert(code, value) != Some(value) {
                frame.push((3, code, value));
            }
        }
        if self.gamepad() {
            return self.snapshot();
        }
        for (code, delta) in values.relative {
            if delta != 0 {
                frame.push((2, code, delta));
            }
        }
        if !frame.is_empty() {
            frame.push(SYN);
        }
        frame
    }

    /// A lost/invalid packet releases held input. Snapshot streams always
    /// publish all controls; delta streams additionally carry SYN_DROPPED.
    pub fn reset(&mut self, loss: bool) -> Vec<Event> {
        let mut frame = Vec::new();
        if loss && !self.gamepad() {
            frame.extend([(0, 3, 0), SYN]);
        }
        for &code in &self.app.buttons {
            if self.held(code) {
                frame.push((1, code, 0));
            }
        }
        self.keys_by_id.clear();
        for a in &self.app.axes {
            if self.axes.insert(a.code, a.neutral) != Some(a.neutral) {
                frame.push((3, a.code, a.neutral));
            }
        }
        if self.gamepad() {
            return self.snapshot();
        }
        frame.push(SYN);
        frame
    }

    pub fn report(&mut self, id: u8, data: Option<&[u8]>) -> Vec<Event> {
        if !self.app.fields.iter().any(|f| f.id == id) {
            return Vec::new();
        }
        let Some(bytes) = data else {
            return self.reset(true);
        };
        match decode(&self.app, id, bytes) {
            Ok(values) => self.apply(id, values),
            Err(Invalid::Rollover) => Vec::new(),
            Err(Invalid::Malformed) => self.reset(true),
        }
    }
}
