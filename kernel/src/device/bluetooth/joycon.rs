//! Bluetooth HID profile for the first-generation Nintendo Joy-Con (L/R).
//! Each ready HID link owns one profile. Reports start at the HID report ID.
//! HIDP framing, transport queues and link teardown belong to the caller.
//!
//! Wire layouts: <https://github.com/dekuNukem/Nintendo_Switch_Reverse_Engineering/blob/master/bluetooth_hid_notes.md>
//! and <https://github.com/dekuNukem/Nintendo_Switch_Reverse_Engineering/blob/master/bluetooth_hid_subcommands_notes.md>.

extern crate alloc;
use alloc::{sync::Arc, vec, vec::Vec};

type OutputNotify = Arc<dyn Fn() + Send + Sync>;

const DEVICE_INFO: u8 = 0x02;
const SET_MODE: u8 = 0x03;
const SET_PLAYER_LIGHTS: u8 = 0x30;
const PLAYER_ONE_STEADY: u8 = 0x01;
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
    fn stick_axes(self) -> (u16, u16) {
        match self {
            Self::Left => (0, 1),  // ABS_X, ABS_Y
            Self::Right => (3, 4), // ABS_RX, ABS_RY
        }
    }

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

struct Pending {
    subcmd: u8,
    sends: u8,
    due_ms: u64,
}

impl Pending {
    fn new(subcmd: u8) -> Self {
        Self {
            subcmd,
            sends: 0,
            due_ms: 0,
        }
    }
}

enum Phase {
    Setup,
    Playing(Gamepad),
    Failed(&'static str),
}

#[derive(PartialEq, Eq)]
enum LightAck {
    Waiting,
    ResendBeforeAccepting,
    Confirmed,
}

/// Session for a single resolved peer. Serialize calls in the HID channel worker.
pub struct JoyCon {
    side: Side,
    packet_number: u8,
    phase: Phase,
    pending: Option<Pending>,
    light_mask: u8,
    light_ack: LightAck,
    outstanding_light_replies: u32,
    notify: OutputNotify,
}

impl JoyCon {
    /// Start identification after HID channel setup. Supply the resolved remote
    /// name and a callback that marks work pending and wakes this adapter's
    /// worker when userspace changes the player number.
    pub fn connect(peer_name: &[u8], notify: impl Fn() + Send + Sync + 'static) -> Option<Self> {
        Some(Self {
            side: Side::from_name(peer_name)?,
            packet_number: 0,
            phase: Phase::Setup,
            pending: Some(Pending::new(DEVICE_INFO)),
            light_mask: PLAYER_ONE_STEADY,
            light_ack: LightAck::Waiting,
            outstanding_light_replies: 0,
            notify: Arc::new(notify),
        })
    }

    /// True once the peer has acknowledged full reports and its gamepad is registered.
    pub fn is_ready(&self) -> bool {
        matches!(self.phase, Phase::Playing(_))
    }

    /// Monotonic milliseconds. Zero means poll immediately. Poll again after
    /// input since a matching reply may make a new command ready.
    pub fn next_deadline_ms(&self) -> Option<u64> {
        self.pending.as_ref().map(|request| request.due_ms)
    }

    /// Return at most one output report to enqueue on the HID interrupt channel.
    /// Keep the returned report until sent, or close the session on send failure.
    /// An error terminates negotiation.
    pub fn poll_output(&mut self, now_ms: u64) -> Result<Option<Vec<u8>>, &'static str> {
        if let Phase::Failed(reason) = self.phase {
            return Err(reason);
        }
        // A 0x30 ACK carries no mask. Keep a newer assignment in the endpoint
        // until the current command is acknowledged or reaches its retry time.
        if let Phase::Playing(half) = &self.phase
            && self
                .pending
                .as_ref()
                .is_none_or(|p| p.sends == 0 || now_ms >= p.due_ms)
            && let Some(player @ 1..=4) = half.endpoint.take_player()
        {
            // Players 1 through 4 light the first one through four LEDs.
            let mask = (1u8 << player) - 1;
            if mask != self.light_mask
                || (self.pending.is_none() && self.light_ack != LightAck::Confirmed)
            {
                // A previous send, including a retry or a timed-out command,
                // may still ACK 0x30. Reserve both sends for a different mask
                // in that case, regardless of which ACK/NACK arrives first.
                self.light_ack = if mask != self.light_mask && self.outstanding_light_replies != 0 {
                    LightAck::ResendBeforeAccepting
                } else {
                    LightAck::Waiting
                };
                self.light_mask = mask;
                self.pending = Some(Pending::new(SET_PLAYER_LIGHTS));
            }
        }
        let Some(request) = &mut self.pending else {
            return Ok(None);
        };
        if now_ms < request.due_ms {
            return Ok(None);
        }
        let subcmd = request.subcmd;
        if request.sends == MAX_SENDS {
            self.pending = None;
            if subcmd == SET_PLAYER_LIGHTS {
                // LED feedback is optional. The registered gamepad stays live.
                return Ok(None);
            }
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
        match subcmd {
            SET_MODE => report.push(FULL_REPORT),
            SET_PLAYER_LIGHTS => {
                report.push(self.light_mask);
                self.outstanding_light_replies = self.outstanding_light_replies.saturating_add(1);
                if request.sends != 0 {
                    self.light_ack = LightAck::Waiting;
                }
            }
            _ => (),
        }
        self.packet_number = (self.packet_number + 1) & 0x0f;
        request.sends += 1;
        request.due_ms = now_ms.saturating_add(ACK_WAIT_MS);
        Ok(Some(report))
    }

    /// Consume one HID input report. Unrelated or incomplete reports do not
    /// acknowledge a request or extend its deadline. A terminal error tells
    /// the caller to close this session.
    pub fn input_report(&mut self, hid: &[u8]) -> Result<(), &'static str> {
        match &mut self.phase {
            Phase::Playing(gamepad) => {
                // Subcommand replies also carry controls, including the light ACK.
                if let Some(sample) = Report::decode(self.side, hid) {
                    gamepad.update(sample);
                }
            }
            Phase::Failed(reason) => return Err(reason),
            Phase::Setup => (),
        }

        if hid.first() == Some(&SUBCMD_REPLY)
            && hid.get(14) == Some(&SET_PLAYER_LIGHTS)
            && matches!(hid[13], 0x00 | 0x80)
        {
            self.outstanding_light_replies = self.outstanding_light_replies.saturating_sub(1);
        }
        let Some(request) = &self.pending else {
            return Ok(());
        };
        if request.sends == 0
            || hid.first() != Some(&SUBCMD_REPLY)
            || hid.get(14) != Some(&request.subcmd)
        {
            return Ok(());
        }

        match (request.subcmd, hid[13]) {
            // The 12-byte device-info body starts with two firmware bytes,
            // then controller type (1 = L, 2 = R, 3 = Pro).
            (DEVICE_INFO, 0x82) if hid.len() >= 27 => {
                if hid[17] != self.side as u8 {
                    let reason = "Joy-Con name and controller type disagree";
                    self.phase = Phase::Failed(reason);
                    self.pending = None;
                    return Err(reason);
                }
                self.pending = Some(Pending::new(SET_MODE));
            }
            (SET_MODE, 0x80) => {
                // The mode ACK itself carries the first authoritative sample.
                let sample = Report::from_common_header(self.side, hid);
                match Gamepad::register(self.side, sample, self.notify.clone()) {
                    Ok(gamepad) => {
                        self.phase = Phase::Playing(gamepad);
                        self.pending = Some(Pending::new(SET_PLAYER_LIGHTS));
                    }
                    Err(reason) => {
                        self.phase = Phase::Failed(reason);
                        self.pending = None;
                        return Err(reason);
                    }
                }
            }
            (SET_PLAYER_LIGHTS, 0x80) if self.light_ack != LightAck::ResendBeforeAccepting => {
                self.light_ack = LightAck::Confirmed;
                self.pending = None;
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
// upright orientation for the userspace grouping daemon.
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
    fn register(side: Side, sample: Report, notify: OutputNotify) -> Result<Self, &'static str> {
        Ok(Self {
            endpoint: sample.with_frame(side, |frame| Endpoint::register(side, frame, notify))?,
            side,
            published: sample,
        })
    }

    fn update(&mut self, sample: Report) {
        // A new reader gets the latest raw stick position even after a one-unit
        // change. Identical periodic reports still drive HID without queueing.
        if self.published != sample {
            sample.with_frame(self.side, |frame| self.endpoint.push(frame));
            self.published = sample;
        }
    }
}

impl Report {
    fn with_frame<T>(self, side: Side, publish: impl FnOnce(&[(u16, u16, i32)]) -> T) -> T {
        let buttons = match side {
            Side::Left => LEFT_BUTTONS,
            Side::Right => RIGHT_BUTTONS,
        };
        let bit = |index: u8| ((self.buttons >> index) & 1) as i32;
        // One complete snapshot in one queue operation, including SYN_REPORT.
        let mut frame = [(0, 0, 0); RIGHT_BUTTONS.len() + 5];
        for (event, &(index, code)) in frame.iter_mut().zip(buttons) {
            *event = (1, code, bit(index));
        }
        let n = buttons.len();
        let (x, y) = side.stick_axes();
        frame[n] = (3, x, self.x);
        frame[n + 1] = (3, y, self.y);
        let end = if side == Side::Left {
            frame[n + 2] = (3, 0x10, bit(18) - bit(19));
            frame[n + 3] = (3, 0x11, bit(16) - bit(17));
            n + 4
        } else {
            n + 2
        };
        publish(&frame[..=end])
    }
}

impl Drop for Gamepad {
    fn drop(&mut self) {
        Report::NEUTRAL.with_frame(self.side, |frame| self.endpoint.push(frame));
        self.endpoint.unregister();
    }
}

#[cfg(any(not(test), target_os = "none"))]
use native::Endpoint;
#[cfg(any(not(test), target_os = "none"))]
mod native {
    use super::{OutputNotify, Side};
    use crate::device::{
        input::event_device::{
            EventDevice, INPUT_CAP_KEY, InputDeviceKind, InputDeviceMetadata, JoyConSide,
        },
        manager::DeviceManager,
    };
    use alloc::{string::ToString, sync::Arc};

    pub(super) struct Endpoint {
        event: Arc<EventDevice>,
        id: usize,
    }

    impl Endpoint {
        pub(super) fn register(
            side: Side,
            initial: &[(u16, u16, i32)],
            notify: OutputNotify,
        ) -> Result<Self, &'static str> {
            let (x, y) = side.stick_axes();
            let mut metadata = InputDeviceMetadata::new(InputDeviceKind::Gamepad, INPUT_CAP_KEY)
                .with_absolute_axis(x, 0, 4095)?
                .with_absolute_axis(y, 0, 4095)?;
            if side == Side::Left {
                metadata = metadata
                    .with_absolute_axis(0x10, -1, 1)?
                    .with_absolute_axis(0x11, -1, 1)?;
            }
            let raw_side = match side {
                Side::Left => JoyConSide::Left,
                Side::Right => JoyConSide::Right,
            };
            let event = Arc::new(EventDevice::new_removable_joycon(raw_side, metadata)?);
            event.set_joycon_player_notify(notify);
            // A daemon opening immediately after registration can read held
            // buttons and axes even if the controller never moves again.
            event.push_events(initial);
            let id = DeviceManager::get_manager()
                .register_device_with_name(event.get_name().to_string(), event.clone());
            Ok(Self { event, id })
        }

        pub(super) fn push(&self, frame: &[(u16, u16, i32)]) {
            self.event.push_events(frame);
        }

        pub(super) fn take_player(&self) -> Option<u8> {
            self.event.take_joycon_player()
        }

        pub(super) fn unregister(&self) {
            DeviceManager::get_manager().unregister_device(self.id);
            self.event.detach_gamepad();
        }
    }

    #[cfg(test)]
    #[test_case]
    fn raw_half_registration_and_reader_restart_replay_current_controls() {
        use super::{Gamepad, Report};
        use crate::device::{
            Device,
            input::{InputEvent, event_device::SCTL_INPUT_GET_ABS_MAX},
        };

        fn read_frame(reader: &dyn Device, side: Side, sample: Report) {
            sample.with_frame(side, |expected| {
                let mut bytes = [0; 16 * InputEvent::size()];
                let n = reader
                    .as_char_device()
                    .unwrap()
                    .try_read(&mut bytes)
                    .unwrap();
                assert_eq!(n, expected.len() * InputEvent::size());
                for (record, &(ty, code, value)) in
                    bytes[..n].chunks_exact(InputEvent::size()).zip(expected)
                {
                    assert_eq!(u16::from_ne_bytes(record[8..10].try_into().unwrap()), ty);
                    assert_eq!(u16::from_ne_bytes(record[10..12].try_into().unwrap()), code);
                    assert_eq!(
                        i32::from_ne_bytes(record[12..16].try_into().unwrap()),
                        value
                    );
                }
            });
        }

        for (side, prefix, x, opposite_x) in [
            (Side::Left, "joycon-left", 0, 3),
            (Side::Right, "joycon-right", 3, 0),
        ] {
            let held = Report {
                buttons: 0xff_ffff,
                x: 1000,
                y: 3000,
            };
            let mut half = Gamepad::register(side, held, Arc::new(|| {})).unwrap();
            let name = half.endpoint.event.get_name().to_string();
            assert!(name.strip_prefix(prefix).unwrap().parse::<usize>().unwrap() < 8);
            let manager = DeviceManager::get_manager();
            let open = || manager.get_device_by_name(&name).unwrap().open().unwrap();
            let first = open();
            assert_eq!(first.control(SCTL_INPUT_GET_ABS_MAX, x), Ok(4095));
            assert!(first.control(SCTL_INPUT_GET_ABS_MAX, opposite_x).is_err());
            read_frame(&*first, side, held);

            let moved = Report {
                buttons: 0,
                x: 321,
                y: 4000,
            };
            half.update(moved);
            let restarted = open();
            read_frame(&*restarted, side, moved);
            read_frame(&*first, side, moved);
            drop(half);
            assert!(manager.get_device_by_name(&name).is_none());
            for reader in [first, restarted] {
                read_frame(&*reader, side, Report::NEUTRAL);
                assert!(reader.is_hung_up());
            }
        }
    }

    #[cfg(test)]
    #[test_case]
    fn raw_player_control_wakes_an_idle_hid_profile() {
        use super::{JoyCon, Phase};
        use crate::task::{clear_mock_current_task, new_user_task, set_mock_current_task};
        use core::sync::atomic::{AtomicBool, Ordering};
        use scarlet_abi::gamepad::SET_JOYCON_PLAYER;

        let queued = Arc::new(AtomicBool::new(false));
        let wake = queued.clone();
        let mut profile =
            JoyCon::connect(b"Joy-Con (R)", move || wake.store(true, Ordering::Release)).unwrap();
        let mut reply = [0; 27];
        reply[0] = 0x21;
        reply[17] = 2;
        for (cmd, ack) in [(0x02, 0x82), (0x03, 0x80), (0x30, 0x80)] {
            assert_eq!(profile.poll_output(0).unwrap().unwrap()[10], cmd);
            reply[13] = ack;
            reply[14] = cmd;
            profile.input_report(&reply).unwrap();
        }
        assert_eq!(profile.next_deadline_ms(), None);
        let Phase::Playing(half) = &profile.phase else {
            panic!("missing raw half")
        };
        let fd = DeviceManager::get_manager()
            .get_device_by_name(half.endpoint.event.get_name())
            .unwrap()
            .open()
            .unwrap();
        let caller = Arc::new(new_user_task("joycon-lights-test".into(), 0));
        set_mock_current_task(caller.clone());
        assert!(fd.control(SET_JOYCON_PLAYER, 5).is_err());
        assert!(!queued.load(Ordering::Acquire));
        fd.control(SET_JOYCON_PLAYER, 2).unwrap();
        assert!(queued.swap(false, Ordering::AcqRel));
        assert_eq!(
            &profile.poll_output(100).unwrap().unwrap()[10..],
            &[0x30, 3]
        );
        caller.credentials.write().set_authority(0, 0).unwrap();
        assert!(fd.control(SET_JOYCON_PLAYER, 3).is_err());
        assert!(!queued.load(Ordering::Acquire));
        clear_mock_current_task();
    }
}

#[cfg(all(test, not(target_os = "none")))]
#[path = "joycon_tests.rs"]
mod tests;
#[cfg(all(test, not(target_os = "none")))]
use tests::Endpoint;
