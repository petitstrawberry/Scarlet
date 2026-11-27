//! End-to-end Bluetooth, physical input, joycond and SWS navigation checks.
use scarlet_os::{
    handle::{Handle, capability::StreamError},
    input::{
        INPUT_CAP_INTERNAL, InputDevice, InputDeviceKind, SCTL_INPUT_GET_ABS_MAX,
        SCTL_INPUT_HAS_EVENT_CODE,
    },
    poll::{self, POLLHUP, POLLIN, PollHandle},
    socket::Socket,
};
use std::{
    io::{Read, Write},
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[allow(dead_code)]
#[path = "../../../user/std-bin/src/sws/gamepad.rs"]
mod gamepad;
#[path = "../radio.rs"]
mod wire;

const LEFT: u32 = 0;
const RIGHT: u32 = 1;
const ADDR: [&str; 2] = ["02:00:00:00:00:10", "02:00:00:00:00:20"];
const RAW: [&str; 2] = ["/dev/joycon-left0", "/dev/joycon-right0"];
const MID: u16 = 2048;
type Event = (u16, u16, i32);

fn wait<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn btctl(args: &[&str]) -> String {
    let output = Command::new("/bin/btctl")
        .args(args)
        .output()
        .expect("spawn btctl");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("btctl {args:?}\n{stdout}{stderr}");
    assert!(output.status.success(), "btctl {args:?} failed");
    stdout
}

fn broker_ready() {
    // Complete a status exchange so even the first btctl runs with a live broker.
    let mut socket = wait("Bluetooth broker", || {
        let s = Socket::new().ok()?;
        s.connect_abstract("scarlet.bluetooth.v1").ok()?;
        Some(s)
    });
    let mut allowed = [0];
    socket.read_exact(&mut allowed).unwrap();
    assert_eq!(allowed, [1]);
    let mut request = [0; 11];
    request[0] = 1;
    socket.write_all(&request).unwrap();
    let mut header = [0; 5];
    socket.read_exact(&mut header).unwrap();
    assert_eq!(header[0], 0);
    let len = u32::from_le_bytes(header[1..].try_into().unwrap()) as usize;
    assert!(len < 4096);
    let mut reply = vec![0; len];
    socket.read_exact(&mut reply).unwrap();
    assert!(String::from_utf8(reply).unwrap().contains("ready="));
}

fn ready_peer(side: u32) -> String {
    format!(
        "peer={} connected=true encrypted=true profile-ready=true key-readable=false",
        ADDR[side as usize]
    )
}
fn devices(sides: &[u32]) {
    let list = btctl(&["devices"]);
    assert!(list.contains(&format!("{} device(s) connected", sides.len())));
    for &side in sides {
        assert_eq!(
            list.lines()
                .filter(|s| s.contains(&ready_peer(side)))
                .count(),
            1
        );
    }
}
fn link(side: u32, pair: bool) {
    let address = ADDR[side as usize];
    let output = btctl(&[if pair { "pair" } else { "connect" }, address]);
    assert!(output.contains(&ready_peer(side)));
    if pair {
        assert!(output.contains(&format!("bond saved for {address}")));
    }
}

struct Radio(Handle);
impl Radio {
    fn control(&self, op: u32, side: u32, argument: u64) -> i32 {
        // The test-only controls take a scalar. AArch64 has a 64-bit usize.
        unsafe { self.0.control(op | side, argument as usize) }.unwrap()
    }
    fn report(&self, side: u32, buttons: u32, x: u16, y: u16) {
        self.control(wire::REPORT, side, wire::sample(buttons, x, y));
    }
    fn interleave(&self, left: u64, right: u64, replies: i32) {
        self.control(wire::INTERLEAVE, LEFT, left);
        self.control(wire::INTERLEAVE, RIGHT, right);
        for side in [LEFT, RIGHT] {
            wait("addressed L2CAP echo reply", || {
                (self.control(wire::ECHO_COUNT, side, 0) == replies).then_some(())
            });
        }
    }
    fn lights(&self, side: u32, mask: i32, minimum_sends: i32) {
        wait(
            &format!("peer {side} LED mask {mask} ({minimum_sends} sends)"),
            || {
                let value = self.control(wire::LIGHTS, side, 0);
                (value & 255 == mask && value >> 8 >= minimum_sends).then_some(())
            },
        )
    }
}

struct Frame {
    events: Vec<Event>,
    keys: Vec<(u16, i32)>,
}
impl Frame {
    fn contains(&self, expected: &[Event]) -> bool {
        expected.iter().all(|e| self.events.contains(e))
    }
}
struct Reader {
    input: InputDevice,
    nav: gamepad::Navigation,
}
impl Reader {
    fn new(input: InputDevice) -> Self {
        assert_eq!(input.kind().unwrap(), InputDeviceKind::Gamepad);
        assert_eq!(input.capabilities().unwrap() & INPUT_CAP_INTERNAL, 0);
        input.set_nonblocking(true).unwrap();
        let mut snapshot = gamepad::Snapshot::new(1);
        for code in 0..6 {
            if let Ok(axis) = input.absolute_axis(code) {
                assert_eq!((axis.minimum, axis.maximum), (0, 4095));
                snapshot.set_axis_range(code, axis.minimum, axis.maximum);
            }
        }
        Self {
            input,
            nav: gamepad::Navigation::from_snapshot(gamepad::Config::default(), &snapshot),
        }
    }
    fn raw(side: u32) -> Self {
        let input = InputDevice::open(RAW[side as usize]).expect("physical endpoint");
        let expected = if side == RIGHT {
            vec![3, 4]
        } else {
            vec![0, 1, 0x10, 0x11]
        };
        assert_eq!(axes(&input), expected);
        Self::new(input)
    }
    fn frame(&mut self) -> Frame {
        let mut events = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            assert!(Instant::now() < deadline, "timed out reading input frame");
            let mut record = [0; 16];
            match self.input.read(&mut record) {
                Ok(0) | Err(StreamError::WouldBlock | StreamError::Interrupted) => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Ok(n) => assert_eq!(n, record.len()),
                Err(e) => panic!("input read failed: {e:?}"),
            }
            let ty = u16::from_ne_bytes(record[8..10].try_into().unwrap());
            let code = u16::from_ne_bytes(record[10..12].try_into().unwrap());
            let value = i32::from_ne_bytes(record[12..16].try_into().unwrap());
            let translated = self.nav.consume(ty, code, value);
            if ty == 0 {
                assert_eq!((code, value), (0, 0), "lost or malformed frame");
                return Frame {
                    events,
                    keys: match translated {
                        gamepad::Frame::Keys(keys) => keys,
                        gamepad::Frame::None => vec![],
                        other => panic!("unexpected SWS frame {other:?}"),
                    },
                };
            }
            assert!(
                !events.iter().any(|&(t, c, _)| (t, c) == (ty, code)),
                "duplicate control in frame"
            );
            events.push((ty, code, value));
        }
    }
    fn hung_up(&self) {
        let mut fds = [PollHandle::new(
            self.input.as_handle().as_raw() as u32,
            POLLIN,
        )];
        wait("old input handle to report hangup", || {
            poll::poll(&mut fds, 0).unwrap();
            (fds[0].revents & POLLHUP != 0).then_some(())
        });
    }
    fn expect(&mut self, expected: &[Event]) -> Frame {
        // Publications may start with a neutral frame before the current sample.
        let end = Instant::now() + Duration::from_secs(8);
        loop {
            let frame = self.frame();
            if frame.contains(expected) {
                return frame;
            }
            assert!(
                Instant::now() < end,
                "missing {expected:?} in {:?}",
                frame.events
            );
        }
    }
}
fn axes(input: &InputDevice) -> Vec<u16> {
    [0, 1, 3, 4, 0x10, 0x11]
        .into_iter()
        // Hat minima are signed. A maximum query also covers the physical
        // endpoints, which do not declare a gamepad button descriptor.
        .filter(|&c| {
            unsafe {
                input
                    .as_handle()
                    .control(SCTL_INPUT_GET_ABS_MAX, usize::from(c))
            }
            .is_ok()
        })
        .collect()
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Layout {
    UprightL,
    UprightR,
    Combined,
    MiniL,
    MiniR,
}
fn pads() -> Vec<(Layout, InputDevice)> {
    (0..8)
        .filter_map(|n| {
            let input = InputDevice::open(&format!("/dev/gamepad{n}")).ok()?;
            let layout = match axes(&input).as_slice() {
                [0, 1, 0x10, 0x11] => Layout::UprightL,
                [3, 4] => Layout::UprightR,
                [0, 1, 3, 4, 0x10, 0x11] => Layout::Combined,
                [0, 1] => {
                    // A sideways half preserves its physical shoulder on a side-specific rail code.
                    let has_key = |code: usize| {
                        unsafe {
                            input
                                .as_handle()
                                .control(SCTL_INPUT_HAS_EVENT_CODE, (1 << 16) | code)
                        }
                        .unwrap()
                    };
                    match (has_key(0x2c1), has_key(0x2c3)) {
                        (1, 0) => Layout::MiniL,
                        (0, 1) => Layout::MiniR,
                        pair => panic!("ambiguous mini gamepad {pair:?}"),
                    }
                }
                other => panic!("unexpected gamepad axes {other:?}"),
            };
            Some((layout, input))
        })
        .collect()
}
fn topology(expected: &[Layout]) -> Vec<(Layout, InputDevice)> {
    let mut last = None;
    wait(&format!("logical gamepads {expected:?}"), || {
        let mut pads = pads();
        pads.sort_by_key(|(kind, _)| *kind);
        let kinds: Vec<_> = pads.iter().map(|(kind, _)| *kind).collect();
        let mut listed: Vec<_> = std::fs::read_dir("/dev")
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("gamepad"))
            .collect();
        listed.sort();
        let complete = kinds == expected && listed.len() == expected.len();
        let observation = (kinds.clone(), listed);
        if last.as_ref() != Some(&observation) {
            println!(
                "logical gamepads {kinds:?}, /dev lists {:?}, awaiting {expected:?}",
                observation.1
            );
            last = Some(observation);
        }
        complete.then_some(pads)
    })
}

fn right_navigation(radio: &Radio, raw: &mut Reader, logical: &mut Reader) {
    for (x, keys) in [
        (0, vec![(105, 1)]),
        (4095, vec![(105, 0), (106, 1)]),
        (MID, vec![(106, 0)]),
    ] {
        radio.report(RIGHT, 0, x, MID);
        let expected = [(3, 3, i32::from(x)), (3, 4, i32::from(MID))];
        for reader in [&mut *raw, &mut *logical] {
            let frame = if x == 0 {
                reader.expect(&expected)
            } else {
                reader.frame()
            };
            assert!(
                frame.contains(&expected),
                "duplicate or incorrect right-stick frame {:?}",
                frame.events
            );
            assert_eq!(frame.keys, keys, "right-only SWS navigation");
        }
        // The next distinct report is an ordered barrier for this duplicate.
        if x != MID {
            radio.report(RIGHT, 0, x, MID);
        }
    }
}

fn main() {
    use Layout::*;
    let mut broker = Command::new("/bin/btctl").arg("--serve").spawn().unwrap();
    broker_ready();
    let mut daemon = Command::new("/bin/joycond").spawn().unwrap();
    let radio = Radio(Handle::open("/dev/bt-qa-radio", 0).unwrap());
    wait("initialized adapter", || {
        btctl(&["status"]).contains("ready=true").then_some(())
    });
    let scan = btctl(&["scan"]);
    for (address, name) in ADDR.iter().zip(["Joy-Con (L)", "Joy-Con (R)"]) {
        assert!(scan.contains(&format!("{address}  \"{name}\"")));
    }
    assert!(scan.contains("2 device(s) found"));
    topology(&[]);
    link(RIGHT, true);
    devices(&[RIGHT]);
    let mut raw_r = Reader::raw(RIGHT);
    let mut upright_r = Reader::new(topology(&[UprightR]).pop().unwrap().1);
    radio.lights(RIGHT, 1, 1);
    right_navigation(&radio, &mut raw_r, &mut upright_r);
    println!("BT_QA_RIGHT_PASS physical-RX/RY logical-RX/RY left/right/release duplicate-reports");

    link(LEFT, true);
    devices(&[LEFT, RIGHT]);
    let mut upright_l = Reader::new(topology(&[UprightL, UprightR]).remove(0).1);
    let press = upright_l.expect(&[(3, 0x10, -1), (1, 0x13a, 1)]);
    assert_eq!(press.keys, [(105, 1)]);
    radio.lights(LEFT, 3, 2);
    radio.control(wire::ACK_LIGHTS, LEFT, 0);
    let ack = upright_l.frame();
    assert!(ack.contains(&[(3, 0x10, 1), (1, 0x13a, 1)]));
    assert_eq!(ack.keys, [(105, 0), (106, 1)]);
    radio.lights(RIGHT, 1, 1);
    let mut raw_l = Reader::raw(LEFT);
    radio.interleave(wire::sample(0, 2400, 1200), wire::sample(0, 3200, 900), 1);
    for reader in [&mut raw_l, &mut upright_l] {
        reader.expect(&[(3, 0, 2400), (3, 1, 1200)]);
    }
    for reader in [&mut raw_r, &mut upright_r] {
        reader.expect(&[(3, 3, 3200), (3, 4, 900)]);
    }
    println!(
        "BT_QA_MULTILINK_PASS two-encrypted-peers shared-credit per-handle-signaling interleaved-fragments LED-retry"
    );
    radio.report(LEFT, 1 << 22, 1000, 2000);
    raw_l.expect(&[(1, 0x136, 1)]);
    radio.report(RIGHT, 1 << 6, 3000, 2200);
    raw_r.expect(&[(1, 0x137, 1)]);
    let mut pair = Reader::new(topology(&[Combined]).pop().unwrap().1);
    upright_l.hung_up();
    upright_r.hung_up();
    pair.expect(&[(3, 0, 1000), (3, 3, 3000), (1, 0x136, 0), (1, 0x137, 0)]);
    radio.lights(LEFT, 1, 1);
    radio.lights(RIGHT, 1, 1);
    radio.interleave(
        wire::sample(1 << 19, 900, 2100),
        wire::sample(1 << 3, 3500, 1200),
        2,
    );
    pair.expect(&[
        (3, 0, 900),
        (3, 1, 2100),
        (3, 3, 3500),
        (3, 4, 1200),
        (3, 0x10, -1),
        (1, 0x131, 1),
        (1, 0x136, 0),
        (1, 0x137, 0),
    ]);
    println!(
        "BT_QA_COMBINED_PASS two-encrypted-peers one-gamepad independent-sticks hat face player-one"
    );

    radio.report(LEFT, (1 << 20) | (1 << 21), 700, 1600);
    topology(&[UprightR, MiniL]);
    radio.report(RIGHT, (1 << 4) | (1 << 5), 2700, 2000);
    let mut minis = topology(&[MiniL, MiniR]);
    let mut mini_r = Reader::new(minis.pop().unwrap().1);
    let mut mini_l = Reader::new(minis.pop().unwrap().1);
    radio.lights(LEFT, 1, 1);
    radio.lights(RIGHT, 3, 1);
    pair.hung_up();
    // Non-neutral axes distinguish the held gesture from a device's neutral seed.
    mini_l.expect(&[(3, 0, 1600), (3, 1, 3395), (1, 0x136, 0), (1, 0x137, 0)]);
    mini_r.expect(&[(3, 0, 2095), (3, 1, 2700), (1, 0x136, 0), (1, 0x137, 0)]);
    // Down/right/up/left on the left D-pad and X/Y/B/A on the right face.
    for (left_bit, right_bit, face) in [
        (16, 1, 0x131),
        (18, 0, 0x133),
        (17, 2, 0x134),
        (19, 3, 0x130),
    ] {
        radio.report(LEFT, 1 << left_bit, 800, 3100);
        radio.report(RIGHT, 1 << right_bit, 900, 3000);
        for (reader, x, y) in [(&mut mini_l, 3100, 3295), (&mut mini_r, 1095, 900)] {
            let frame = reader.expect(&[(3, 0, x), (3, 1, y), (1, face, 1)]);
            for button in [0x130, 0x131, 0x133, 0x134] {
                assert!(frame.contains(&[(1, button, i32::from(button == face))]));
            }
        }
    }
    raw_l.expect(&[(3, 0, 800), (3, 1, 3100), (3, 0x10, -1)]);
    devices(&[LEFT, RIGHT]);
    println!("BT_QA_MINIS_PASS two-gamepads opposite-rotations face-remap player-one-and-two");

    radio.control(wire::UNPLUG, LEFT, 0);
    topology(&[MiniR]);
    mini_l.expect(&[(3, 0, 2048), (3, 1, 2048), (1, 0x130, 0)]);
    raw_l.expect(&[(3, 0, 2048), (3, 1, 2048), (3, 0x10, 0), (1, 0x136, 0)]);
    assert!(InputDevice::open(RAW[0]).is_err());
    raw_l.hung_up();
    mini_l.hung_up();
    drop(raw_l);
    devices(&[RIGHT]);
    radio.report(RIGHT, 0, 1234, 2345);
    mini_r.expect(&[(3, 0, 1750), (3, 1, 1234), (1, 0x130, 0)]);
    link(LEFT, false);
    topology(&[UprightL, MiniR]);
    devices(&[LEFT, RIGHT]);
    radio.lights(LEFT, 1, 1);
    radio.lights(RIGHT, 3, 1);
    let mut reconnected = Reader::raw(LEFT);
    btctl(&["disconnect", ADDR[1]]);
    topology(&[UprightL]);
    mini_r.expect(&[(3, 0, 2048), (3, 1, 2048)]);
    assert!(InputDevice::open(RAW[1]).is_err());
    raw_r.hung_up();
    mini_r.hung_up();
    devices(&[LEFT]);
    radio.report(LEFT, 1 << 22, 3333, MID);
    reconnected.expect(&[(1, 0x136, 1), (3, 0, 3333)]);
    let mut remaining = Reader::new(topology(&[UprightL]).pop().unwrap().1);
    assert_eq!(
        remaining.expect(&[(1, 0x136, 1), (3, 0, 3333)]).keys,
        [(106, 1)]
    );
    link(RIGHT, false);
    let mut right_again = Reader::new(topology(&[UprightL, UprightR]).pop().unwrap().1);
    let mut right_raw_again = Reader::raw(RIGHT);
    devices(&[LEFT, RIGHT]);
    radio.lights(RIGHT, 3, 1);
    btctl(&["disconnect", ADDR[0]]);
    topology(&[UprightR]);
    assert_eq!(
        remaining.expect(&[(1, 0x136, 0), (3, 0, 2048)]).keys,
        [(106, 0)]
    );
    reconnected.expect(&[(1, 0x136, 0), (3, 0, 2048)]);
    assert!(InputDevice::open(RAW[0]).is_err());
    devices(&[RIGHT]);
    radio.report(RIGHT, 0, MID, 0);
    for reader in [&mut right_raw_again, &mut right_again] {
        assert_eq!(reader.expect(&[(3, 3, 2048), (3, 4, 0)]).keys, [(103, 1)]);
    }
    radio.control(wire::UNPLUG, RIGHT, 0);
    topology(&[]);
    for reader in [&mut right_raw_again, &mut right_again] {
        assert_eq!(
            reader.expect(&[(3, 3, 2048), (3, 4, 2048)]).keys,
            [(103, 0)]
        );
    }
    assert!(InputDevice::open(RAW[1]).is_err());
    devices(&[]);
    assert!(broker.try_wait().unwrap().is_none());
    assert!(daemon.try_wait().unwrap().is_none());
    println!("BT_QA_RECONNECT_PASS selected-bond new-handle surviving-peer neutral-cleanup");
    println!("BLUETOOTH_QEMU_PASS");
}
