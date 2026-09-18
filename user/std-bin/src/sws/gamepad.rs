//! Generic gamepad menu policy for native input devices.
//!
//! Drivers retain their gamepad class and standard button/axis codes. This
//! userspace policy maps navigation to the existing UI key interface, with
//! source-scoped held keys and compositor-owned repeat. It is independent of
//! the board and can be disabled with `[gamepad].navigation = false`.

use std::vec::Vec;
use sws_protocol::gamepad::{State, button_bit};

/// Recover a stalled client with one reset and the latest snapshot per device.
/// Every state is authoritative; unrelated devices must survive compaction.
pub(crate) fn compact_backlog(states: impl Iterator<Item = State>, latest: State) -> Vec<State> {
    let mut devices = std::collections::BTreeMap::new();
    for state in states {
        devices.insert(state.device_id, state);
    }
    devices.insert(latest.device_id, latest);
    let mut compacted = Vec::new();
    for (device_id, state) in devices {
        compacted.push(State {
            device_id,
            flags: sws_protocol::gamepad::RESET,
            time_ns: state.time_ns,
            ..State::default()
        });
        if state.flags & sws_protocol::gamepad::RESET == 0 {
            compacted.push(state);
        }
    }
    compacted
}

/// Remember which window received each device's state so focus changes cannot
/// leave a previously focused application's buttons held indefinitely.
#[derive(Default)]
pub(crate) struct Routing {
    owners: Vec<(u32, u32)>,
}

impl Routing {
    pub(crate) fn reset_except(&mut self, window: Option<u32>, time_ns: u64) -> Vec<(u32, State)> {
        let mut resets = Vec::new();
        self.owners.retain(|&(device_id, owner)| {
            if window == Some(owner) {
                return true;
            }
            resets.push((
                owner,
                State {
                    device_id,
                    flags: sws_protocol::gamepad::RESET,
                    time_ns,
                    ..State::default()
                },
            ));
            false
        });
        resets
    }

    pub(crate) fn route(&mut self, window: Option<u32>, state: State) -> Vec<(u32, State)> {
        let mut deliveries = Vec::new();
        if let Some(index) = self
            .owners
            .iter()
            .position(|&(device, _)| device == state.device_id)
        {
            let (_, owner) = self.owners[index];
            if Some(owner) != window {
                deliveries.push((
                    owner,
                    State {
                        device_id: state.device_id,
                        flags: sws_protocol::gamepad::RESET,
                        time_ns: state.time_ns,
                        ..State::default()
                    },
                ));
                self.owners.remove(index);
            }
        }
        if let Some(window) = window {
            deliveries.push((window, state));
            self.owners.retain(|&(device, _)| device != state.device_id);
            if state.flags & sws_protocol::gamepad::RESET == 0 {
                self.owners.push((state.device_id, window));
            }
        }
        deliveries
    }
}

pub(super) struct Snapshot {
    pub state: State,
    ranges: [Option<(i32, i32)>; 6],
}
impl Snapshot {
    pub(super) fn new(device_id: u32) -> Self {
        Self {
            state: State {
                device_id,
                ..State::default()
            },
            ranges: [None; 6],
        }
    }
    pub(super) fn set_axis_range(&mut self, code: u16, min: i32, max: i32) {
        if code < 6 && min < max {
            self.ranges[code as usize] = Some((min, max));
        }
    }
    pub(super) fn update(&mut self, type_: u16, code: u16, value: i32, time_ns: u64) {
        self.state.time_ns = time_ns;
        self.state.flags = 0;
        if type_ == 1 && (value == 0 || value == 1) {
            if let Some(bit) = button_bit(code) {
                if value == 1 {
                    self.state.buttons |= bit;
                } else {
                    self.state.buttons &= !bit;
                }
                if code == 0x138 && self.ranges[2].is_none() {
                    self.state.left_trigger = if value == 1 { 32767 } else { 0 };
                }
                if code == 0x139 && self.ranges[5].is_none() {
                    self.state.right_trigger = if value == 1 { 32767 } else { 0 };
                }
            }
        }
        if type_ != 3 {
            return;
        }
        if code == 0x10 {
            self.state.hat_x = value.clamp(-1, 1) as i8;
            return;
        }
        if code == 0x11 {
            self.state.hat_y = value.clamp(-1, 1) as i8;
            return;
        }
        let Some((min, max)) = self.ranges.get(code as usize).copied().flatten() else {
            return;
        };
        let range = max as i64 - min as i64;
        let unsigned = (value.clamp(min, max) as i64 - min as i64) * 32767 / range;
        let signed = ((value.clamp(min, max) as i64 - min as i64) * 65534 / range - 32767) as i16;
        match code {
            0 => self.state.left_x = signed,
            1 => self.state.left_y = signed,
            2 => self.state.left_trigger = unsigned as u16,
            3 => self.state.right_x = signed,
            4 => self.state.right_y = signed,
            5 => self.state.right_trigger = unsigned as u16,
            _ => (),
        }
    }
    pub(super) fn reset(&mut self, time_ns: u64) -> State {
        self.state = State {
            device_id: self.state.device_id,
            flags: sws_protocol::gamepad::RESET,
            time_ns,
            ..State::default()
        };
        self.state
    }
}

const SOUTH: u16 = 0x130;
const EAST: u16 = 0x131;
const NORTH: u16 = 0x133;
const WEST: u16 = 0x134;
const MODE: u16 = 0x13c;
// Left, right, up, down, confirm, cancel, home modifier, home key.
const KEYS: [u16; 8] = [105, 106, 103, 108, 28, 1, 125, 57];

#[derive(Clone, Copy)]
pub(super) struct Config {
    enabled: bool,
    confirm: u16,
    cancel: u16,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            confirm: SOUTH,
            cancel: EAST,
        }
    }
}
impl Config {
    pub(super) fn parse(content: &str) -> Self {
        let mut config = Self::default();
        let mut section = false;
        for raw in content.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.starts_with('[') {
                section = line == "[gamepad]";
                continue;
            }
            if !section {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches(['\'', '"']);
            match key.trim() {
                "navigation" => match value {
                    "false" => config.enabled = false,
                    "true" => config.enabled = true,
                    _ => (),
                },
                "confirm_button" => {
                    if let Some(button) = button(value) {
                        config.confirm = button;
                    }
                }
                "cancel_button" => {
                    if let Some(button) = button(value) {
                        config.cancel = button;
                    }
                }
                _ => (),
            }
        }
        if config.confirm == config.cancel {
            config.confirm = SOUTH;
            config.cancel = EAST;
        }
        config
    }
}
fn button(name: &str) -> Option<u16> {
    match name {
        "south" => Some(SOUTH),
        "east" => Some(EAST),
        "north" => Some(NORTH),
        "west" => Some(WEST),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct Axis {
    min: i32,
    max: i32,
    direction: i32,
}
impl Default for Axis {
    fn default() -> Self {
        Self {
            min: -32768,
            max: 32767,
            direction: 0,
        }
    }
}
impl Axis {
    fn update(&mut self, value: i32) {
        let range = self.max as i64 - self.min as i64;
        let center = self.min as i64 + range / 2;
        let offset = value.clamp(self.min, self.max) as i64 - center;
        let press = (range / 8).max(1);
        let release = (range / 16).max(1);
        self.direction = match self.direction {
            -1 if offset < -release => -1,
            1 if offset > release => 1,
            _ if offset <= -press => -1,
            _ if offset >= press => 1,
            _ => 0,
        };
    }
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Frame {
    None,
    Discard,
    Reset,
    Keys(Vec<(u16, i32)>),
}
pub(super) struct Navigation {
    config: Config,
    axes: [Axis; 2],
    hat: [i32; 2],
    buttons: [bool; 3],
    held: [bool; 8],
    desynced: bool,
}
impl Navigation {
    pub(super) fn new(config: Config) -> Self {
        Self {
            config,
            axes: [Axis::default(); 2],
            hat: [0; 2],
            buttons: [false; 3],
            held: [false; 8],
            desynced: false,
        }
    }
    pub(super) fn set_axis_range(&mut self, code: u16, min: i32, max: i32) {
        if code < 2 && min < max {
            self.axes[code as usize] = Axis {
                min,
                max,
                direction: 0,
            };
        }
    }
    pub(super) fn consume(&mut self, type_: u16, code: u16, value: i32) -> Frame {
        if type_ == 0 && code == 3 {
            self.axes.iter_mut().for_each(|a| a.direction = 0);
            self.hat = [0; 2];
            self.buttons = [false; 3];
            self.held = [false; 8];
            self.desynced = true;
            return Frame::Reset;
        }
        if self.desynced {
            if type_ == 0 && code == 0 {
                self.desynced = false;
            }
            return Frame::Discard;
        }
        if !self.config.enabled {
            return Frame::None;
        }
        match (type_, code) {
            (3, 0 | 1) => self.axes[code as usize].update(value),
            (3, 0x10 | 0x11) => self.hat[(code - 0x10) as usize] = value.clamp(-1, 1),
            (1, _) if value == 0 || value == 1 => {
                for (n, button) in [self.config.confirm, self.config.cancel, MODE]
                    .iter()
                    .enumerate()
                {
                    if code == *button {
                        self.buttons[n] = value == 1;
                    }
                }
            }
            (0, 0) => return self.frame(),
            _ => (),
        }
        Frame::None
    }
    fn frame(&mut self) -> Frame {
        let current = [
            self.hat[0] < 0 || self.axes[0].direction < 0,
            self.hat[0] > 0 || self.axes[0].direction > 0,
            self.hat[1] < 0 || self.axes[1].direction < 0,
            self.hat[1] > 0 || self.axes[1].direction > 0,
            self.buttons[0],
            self.buttons[1],
            self.buttons[2],
            self.buttons[2],
        ];
        let mut keys = Vec::new();
        // Release home chord in reverse order so its modifier cannot form a
        // second modifier-tap action. Establish modifiers before their keys.
        for n in (0..KEYS.len()).rev() {
            if self.held[n] && !current[n] {
                keys.push((KEYS[n], 0));
            }
        }
        for n in 0..KEYS.len() {
            if !self.held[n] && current[n] {
                keys.push((KEYS[n], 1));
            }
        }
        self.held = current;
        if keys.is_empty() {
            Frame::None
        } else {
            Frame::Keys(keys)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn finish(n: &mut Navigation) -> Frame {
        n.consume(0, 0, 0)
    }
    #[test]
    fn backlog_recovery_preserves_latest_state_of_each_device() {
        let a = State {
            device_id: 0,
            buttons: 1,
            ..State::default()
        };
        let b = State {
            device_id: 1,
            buttons: 2,
            ..State::default()
        };
        let released = State { buttons: 0, ..a };
        let result = compact_backlog([a, b].into_iter(), released);
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].flags, sws_protocol::gamepad::RESET);
        assert_eq!(result[1], released);
        assert_eq!(result[2].device_id, 1);
        assert_eq!(result[2].flags, sws_protocol::gamepad::RESET);
        assert_eq!(result[3], b);
    }
    #[test]
    fn focus_change_and_unsubscription_reset_previous_consumers() {
        let mut routing = Routing::default();
        let held = State {
            device_id: 2,
            buttons: 1,
            time_ns: 10,
            ..State::default()
        };
        assert_eq!(routing.route(Some(7), held), std::vec![(7, held)]);
        let delivered = routing.route(Some(8), held);
        assert_eq!(delivered.len(), 2);
        assert_eq!(delivered[0].0, 7);
        assert_eq!(delivered[0].1.buttons, 0);
        assert_eq!(delivered[0].1.flags, sws_protocol::gamepad::RESET);
        assert_eq!(delivered[1], (8, held));
        assert!(routing.reset_except(Some(8), 11).is_empty());
        assert_eq!(routing.reset_except(None, 12).len(), 1);
        assert!(routing.reset_except(None, 13).is_empty());
        assert!(routing.route(None, held).is_empty());
    }
    #[test]
    fn raw_snapshot_normalizes_axes_and_keeps_button_identity() {
        let mut s = Snapshot::new(3);
        s.set_axis_range(0, 0, 4095);
        s.set_axis_range(4, i32::MIN, i32::MAX);
        s.update(3, 0, 0, 1);
        s.update(3, 4, i32::MAX, 2);
        s.update(1, EAST, 1, 3);
        s.update(1, 0x138, 1, 4);
        assert_eq!(s.state.left_x, -32767);
        assert_eq!(s.state.right_y, 32767);
        assert_ne!(s.state.buttons & button_bit(EAST).unwrap(), 0);
        assert_eq!(s.state.left_trigger, 32767);
        let reset = s.reset(5);
        assert_eq!(reset.buttons, 0);
        assert_eq!(reset.device_id, 3);
        assert_eq!(reset.flags, sws_protocol::gamepad::RESET);
    }
    #[test]
    fn nintendo_button_layout_is_configuration_not_driver_translation() {
        let mut n = Navigation::new(Config::parse(
            "[gamepad]\nconfirm_button = \"east\"\ncancel_button = \"south\"",
        ));
        n.consume(1, EAST, 1);
        assert_eq!(finish(&mut n), Frame::Keys(std::vec![(28, 1)]));
        n.consume(1, EAST, 1);
        assert_eq!(finish(&mut n), Frame::None);
        n.consume(1, EAST, 0);
        n.consume(1, SOUTH, 1);
        assert_eq!(finish(&mut n), Frame::Keys(std::vec![(28, 0), (1, 1)]));
    }
    #[test]
    fn stick_deadzone_hysteresis_and_hat_share_navigation_ownership() {
        let mut n = Navigation::new(Config::default());
        n.set_axis_range(0, 0, 4095);
        n.consume(3, 0, 2400);
        assert_eq!(finish(&mut n), Frame::None);
        n.consume(3, 0, 2800);
        assert_eq!(finish(&mut n), Frame::Keys(std::vec![(106, 1)]));
        n.consume(3, 0, 2400);
        assert_eq!(finish(&mut n), Frame::None);
        n.consume(3, 0x10, 1);
        n.consume(3, 0, 2048);
        assert_eq!(finish(&mut n), Frame::None);
        n.consume(3, 0x10, 0);
        assert_eq!(finish(&mut n), Frame::Keys(std::vec![(106, 0)]));
        n.consume(3, 0, 1200);
        assert_eq!(finish(&mut n), Frame::Keys(std::vec![(105, 1)]));
    }
    #[test]
    fn dropped_frame_releases_state_and_recovers_from_next_snapshot() {
        let mut n = Navigation::new(Config::default());
        n.consume(1, SOUTH, 1);
        finish(&mut n);
        assert_eq!(n.consume(0, 3, 0), Frame::Reset);
        n.consume(1, SOUTH, 1);
        assert_eq!(finish(&mut n), Frame::Discard);
        n.consume(1, SOUTH, 1);
        assert_eq!(finish(&mut n), Frame::Keys(std::vec![(28, 1)]));
    }
    #[test]
    fn home_is_one_chord_and_policy_can_be_disabled() {
        let mut n = Navigation::new(Config::default());
        n.consume(1, MODE, 1);
        assert_eq!(finish(&mut n), Frame::Keys(std::vec![(125, 1), (57, 1)]));
        n.consume(1, MODE, 0);
        assert_eq!(finish(&mut n), Frame::Keys(std::vec![(57, 0), (125, 0)]));
        let mut n = Navigation::new(Config::parse("[gamepad]\nnavigation = false"));
        n.consume(1, SOUTH, 1);
        n.consume(3, 0x10, 1);
        assert_eq!(finish(&mut n), Frame::None);
    }
}
