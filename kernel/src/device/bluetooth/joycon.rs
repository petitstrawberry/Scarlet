//! Bluetooth HID profile for the first-generation Nintendo Joy-Con (L/R).
//! Each ready HID link owns one profile. Reports start at the HID report ID.
//! HIDP framing, transport queues and link teardown belong to the caller.
//!
//! Wire layouts: <https://github.com/dekuNukem/Nintendo_Switch_Reverse_Engineering/blob/master/bluetooth_hid_notes.md>
//! and <https://github.com/dekuNukem/Nintendo_Switch_Reverse_Engineering/blob/master/bluetooth_hid_subcommands_notes.md>.

extern crate alloc;
use alloc::{vec, vec::Vec};

const DEVICE_INFO: u8 = 0x02;
const SET_MODE: u8 = 0x03;
const FULL_REPORT: u8 = 0x30;
const SUBCMD_REPLY: u8 = 0x21;
const ACK_WAIT_MS: u64 = 2000;
const MAX_SENDS: u8 = 2;

/// Physical half reported by the first-generation controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left = 1,
    Right = 2,
}

impl Side {
    /// Match the exact Bluetooth name, accepting HCI NUL padding.
    pub fn from_name(name: &[u8]) -> Option<Self> {
        // HCI Remote Name Request Complete can supply a NUL-padded field.
        let last = name.iter().rposition(|&byte| byte != 0)?;
        match &name[..=last] {
            b"Joy-Con (L)" => Some(Self::Left),
            b"Joy-Con (R)" => Some(Self::Right),
            _ => None,
        }
    }
}

enum Phase {
    Setup { subcmd: u8, sends: u8, due_ms: u64 },
    Playing(Gamepad),
    Failed(&'static str),
}

impl Phase {
    fn request(subcmd: u8) -> Self {
        Self::Setup {
            subcmd,
            sends: 0,
            due_ms: 0,
        }
    }
}

/// Session for a single resolved peer. Serialize calls in the HID channel worker.
pub struct JoyCon {
    side: Side,
    packet_number: u8,
    phase: Phase,
}

impl JoyCon {
    /// Start identification after HID channel setup. Supply the resolved remote
    /// name for this peer. An unrecognized name creates no session.
    pub fn connect(peer_name: &[u8]) -> Option<Self> {
        Some(Self {
            side: Side::from_name(peer_name)?,
            packet_number: 0,
            phase: Phase::request(DEVICE_INFO),
        })
    }

    /// True once the peer has acknowledged full reports and its gamepad is registered.
    pub fn is_ready(&self) -> bool {
        matches!(self.phase, Phase::Playing(_))
    }

    /// Monotonic milliseconds. Zero means poll immediately. Poll again after
    /// input since a matching reply may make a new command ready.
    pub fn next_deadline_ms(&self) -> Option<u64> {
        match self.phase {
            Phase::Setup { due_ms, .. } => Some(due_ms),
            _ => None,
        }
    }

    /// Return at most one output report to enqueue on the HID interrupt channel.
    /// Keep the returned report until sent, or close the session on send failure.
    /// An error terminates negotiation.
    pub fn poll_output(&mut self, now_ms: u64) -> Result<Option<Vec<u8>>, &'static str> {
        let (subcmd, sends, due_ms) = match &mut self.phase {
            Phase::Setup {
                subcmd,
                sends,
                due_ms,
            } => (*subcmd, sends, due_ms),
            Phase::Playing(_) => return Ok(None),
            Phase::Failed(reason) => return Err(reason),
        };
        if now_ms < *due_ms {
            return Ok(None);
        }
        if *sends == MAX_SENDS {
            let reason = "Joy-Con subcommand timed out";
            self.phase = Phase::Failed(reason);
            return Err(reason);
        }

        // Output 0x01 includes neutral rumble for both actuators.
        let mut report = vec![
            0x01,
            self.packet_number,
            0x00,
            0x01,
            0x40,
            0x40,
            0x00,
            0x01,
            0x40,
            0x40,
            subcmd,
        ];
        if subcmd == SET_MODE {
            report.push(FULL_REPORT);
        }
        self.packet_number = (self.packet_number + 1) & 0x0f;
        *sends += 1;
        *due_ms = now_ms.saturating_add(ACK_WAIT_MS);
        Ok(Some(report))
    }

    /// Consume one HID input report. Unrelated or incomplete reports do not
    /// acknowledge a request or extend its deadline. A terminal error tells
    /// the caller to close this session.
    pub fn input_report(&mut self, hid: &[u8]) -> Result<(), &'static str> {
        let subcmd = match &mut self.phase {
            Phase::Playing(gamepad) => {
                if let Some(sample) = Report::decode(self.side, hid) {
                    gamepad.update(sample);
                }
                return Ok(());
            }
            Phase::Failed(reason) => return Err(reason),
            Phase::Setup { sends: 0, .. } => return Ok(()),
            Phase::Setup { subcmd, .. } => *subcmd,
        };

        if hid.first() != Some(&SUBCMD_REPLY) || hid.get(14) != Some(&subcmd) {
            return Ok(());
        }

        match (subcmd, hid[13]) {
            // The 12-byte device-info body starts with two firmware bytes,
            // then controller type (1 = L, 2 = R, 3 = Pro).
            (DEVICE_INFO, 0x82) if hid.len() >= 27 => {
                if hid[17] != self.side as u8 {
                    let reason = "Joy-Con name and controller type disagree";
                    self.phase = Phase::Failed(reason);
                    return Err(reason);
                }
                self.phase = Phase::request(SET_MODE);
            }
            (SET_MODE, 0x80) => {
                // The mode ACK itself carries the first authoritative sample.
                let sample = Report::from_common_header(self.side, hid);
                match Gamepad::register(self.side, sample) {
                    Ok(gamepad) => self.phase = Phase::Playing(gamepad),
                    Err(reason) => {
                        self.phase = Phase::Failed(reason);
                        return Err(reason);
                    }
                }
            }
            // NACK and malformed replies leave the bounded retry armed.
            _ => (),
        }
        Ok(())
    }

    /// Release buttons, center axes and unregister this peer. Dropping the
    /// session on channel closure or controller removal does the same.
    pub fn disconnect(self) {
        drop(self);
    }
}

/// Side-local controls in the standard Joy-Con HID header, shared by the
/// rail and Bluetooth wire formats. Axes have nominal range 0..=4095.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    /// Nintendo button bits in wire order, masked to the selected half.
    pub buttons: u32,
    /// Horizontal stick position, increasing rightwards.
    pub x: i32,
    /// Vertical stick position, increasing downwards.
    pub y: i32,
}

impl Report {
    const NEUTRAL: Self = Self {
        buttons: 0,
        x: 2048,
        y: 2048,
    };

    /// Decode a 0x21, 0x30 or 0x31 report starting at its report ID.
    pub fn decode(side: Side, hid: &[u8]) -> Option<Self> {
        match hid.first()? {
            0x30 | 0x31 if hid.len() >= 12 => Some(Self::from_common_header(side, hid)),
            0x21 if hid.len() >= 15 => Some(Self::from_common_header(side, hid)),
            _ => None,
        }
    }

    fn from_common_header(side: Side, hid: &[u8]) -> Self {
        let (start, mask) = match side {
            Side::Left => (6, 0xff2900),
            Side::Right => (9, 0x0016ff),
        };
        let packed = u32::from_le_bytes([hid[start], hid[start + 1], hid[start + 2], 0]);
        Self {
            buttons: u32::from_le_bytes([hid[3], hid[4], hid[5], 0]) & mask,
            x: (packed & 0xfff) as i32,
            y: 4095 - (packed >> 12) as i32,
        }
    }
}

// Wire bit to native BTN_* code. Keep Nintendo button identities in the
// upright orientation. Each independent half exposes its one stick as ABS_X/Y.
const LEFT_BUTTONS: &[(u8, u16)] = &[
    (22, 0x136),
    (23, 0x138),
    (8, 0x13a),
    (11, 0x13d),
    (13, 0x2c0),
    (20, 0x2c1),
    (21, 0x2c2),
];
const RIGHT_BUTTONS: &[(u8, u16)] = &[
    (2, 0x130),
    (3, 0x131),
    (1, 0x133),
    (0, 0x134),
    (6, 0x137),
    (7, 0x139),
    (9, 0x13b),
    (12, 0x13c),
    (10, 0x13e),
    (4, 0x2c3),
    (5, 0x2c4),
];

struct Gamepad {
    endpoint: Endpoint,
    side: Side,
    published: Report,
}

impl Gamepad {
    fn register(side: Side, sample: Report) -> Result<Self, &'static str> {
        let gamepad = Self {
            endpoint: Endpoint::register(side)?,
            side,
            published: sample,
        };
        gamepad.emit(sample);
        Ok(gamepad)
    }

    fn update(&mut self, sample: Report) {
        // Compare with the last publication so sub-threshold motion accumulates.
        if self.published.buttons != sample.buttons
            || self.published.x.abs_diff(sample.x) > 8
            || self.published.y.abs_diff(sample.y) > 8
        {
            self.emit(sample);
            self.published = sample;
        }
    }

    fn emit(&self, sample: Report) {
        let buttons = match self.side {
            Side::Left => LEFT_BUTTONS,
            Side::Right => RIGHT_BUTTONS,
        };
        let bit = |index: u8| ((sample.buttons >> index) & 1) as i32;
        // One complete snapshot in one queue operation, including SYN_REPORT.
        let mut frame = [(0, 0, 0); RIGHT_BUTTONS.len() + 5];
        for (event, &(index, code)) in frame.iter_mut().zip(buttons) {
            *event = (1, code, bit(index));
        }
        let n = buttons.len();
        frame[n] = (3, 0, sample.x);
        frame[n + 1] = (3, 1, sample.y);
        let end = if self.side == Side::Left {
            frame[n + 2] = (3, 0x10, bit(18) - bit(19));
            frame[n + 3] = (3, 0x11, bit(16) - bit(17));
            n + 4
        } else {
            n + 2
        };
        self.endpoint.push(&frame[..=end]);
    }
}

impl Drop for Gamepad {
    fn drop(&mut self) {
        self.emit(Report::NEUTRAL);
        self.endpoint.unregister();
    }
}

#[cfg(any(not(test), target_os = "none"))]
use native::Endpoint;
#[cfg(any(not(test), target_os = "none"))]
mod native {
    use super::Side;
    use crate::device::{
        input::event_device::{EventDevice, INPUT_CAP_KEY, InputDeviceKind, InputDeviceMetadata},
        manager::DeviceManager,
    };
    use alloc::{string::ToString, sync::Arc};

    pub(super) struct Endpoint {
        event: Arc<EventDevice>,
        id: usize,
    }

    impl Endpoint {
        pub(super) fn register(side: Side) -> Result<Self, &'static str> {
            let mut metadata = InputDeviceMetadata::new(InputDeviceKind::Gamepad, INPUT_CAP_KEY)
                .with_absolute_axis(0, 0, 4095)?
                .with_absolute_axis(1, 0, 4095)?;
            if side == Side::Left {
                metadata = metadata
                    .with_absolute_axis(0x10, -1, 1)?
                    .with_absolute_axis(0x11, -1, 1)?;
            }
            let event = Arc::new(EventDevice::new_removable_gamepad(metadata)?);
            let id = DeviceManager::get_manager()
                .register_device_with_name(event.get_name().to_string(), event.clone());
            Ok(Self { event, id })
        }

        pub(super) fn push(&self, frame: &[(u16, u16, i32)]) {
            self.event.push_events(frame);
        }

        pub(super) fn unregister(&self) {
            DeviceManager::get_manager().unregister_device(self.id);
            self.event.detach_gamepad();
        }
    }
}

#[cfg(all(test, not(target_os = "none")))]
#[path = "joycon_tests.rs"]
mod tests;
#[cfg(all(test, not(target_os = "none")))]
use tests::Endpoint;
