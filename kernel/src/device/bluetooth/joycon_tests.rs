//! Run with rustc --edition=2024 --test kernel/src/device/bluetooth/joycon.rs.
use super::*;
extern crate std;
use self::std::sync::{Arc, Mutex};

type Frame = Vec<(u16, u16, i32)>;

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Frame(Frame),
    Removed,
}

// Only the kernel registry and queue are replaced. Tests exercise the same
// public session, negotiation, event batches and Drop path as the HID worker.
pub(super) struct Endpoint(Arc<Mutex<Vec<Event>>>);
impl Endpoint {
    pub(super) fn register(_: Side) -> Result<Self, &'static str> {
        Ok(Self(Arc::default()))
    }
    pub(super) fn push(&self, frame: &[(u16, u16, i32)]) {
        self.0.lock().unwrap().push(Event::Frame(frame.to_vec()));
    }
    pub(super) fn unregister(&self) {
        self.0.lock().unwrap().push(Event::Removed);
    }
}

fn ack(cmd: u8, status: u8) -> [u8; 49] {
    let mut hid = [0; 49];
    hid[0] = 0x21;
    hid[6..12].copy_from_slice(&[0x00, 0xf8, 0x7f, 0x00, 0xf8, 0x7f]);
    hid[13] = status;
    hid[14] = cmd;
    hid
}

fn info(kind: u8) -> [u8; 49] {
    let mut reply = ack(0x02, 0x82);
    reply[15..27].copy_from_slice(&[3, 0x48, kind, 2, 1, 2, 3, 4, 5, 6, 1, 1]);
    reply
}

fn connect(name: &[u8], kind: u8) -> JoyCon {
    let mut pad = JoyCon::connect(name).unwrap();
    let cmd = pad.poll_output(10).unwrap().unwrap();
    assert_eq!(cmd, [1, 0, 0, 1, 0x40, 0x40, 0, 1, 0x40, 0x40, 2]);
    pad.input_report(&info(kind)).unwrap();
    assert!(!pad.is_ready());
    assert_eq!(pad.next_deadline_ms(), Some(0));
    let mode = pad.poll_output(20).unwrap().unwrap();
    assert_eq!(mode, [1, 1, 0, 1, 0x40, 0x40, 0, 1, 0x40, 0x40, 3, 0x30]);
    pad.input_report(&ack(3, 0x80)).unwrap();
    assert!(pad.is_ready());
    assert_eq!(pad.next_deadline_ms(), None);
    assert_eq!(pad.poll_output(99999).unwrap(), None);
    pad
}

fn events(pad: &JoyCon) -> Arc<Mutex<Vec<Event>>> {
    let Phase::Playing(gamepad) = &pad.phase else {
        panic!("expected registered gamepad")
    };
    gamepad.endpoint.0.clone()
}

fn frame(event: &Event) -> &Frame {
    let Event::Frame(frame) = event else {
        panic!("expected frame")
    };
    assert_eq!(frame.last(), Some(&(0, 0, 0)));
    assert!(
        frame[..frame.len() - 1]
            .iter()
            .all(|&(ty, code, _)| ty == 3 || ty == 1 && code >= 0x130)
    );
    frame
}

#[test]
fn exact_first_generation_names_and_matching_controller_types() {
    for bad in [
        &b""[..],
        b"Joy-Con",
        b"Joy-Con (L) 2",
        b"Joy-Con (R)\0X",
        b"Pro Controller",
    ] {
        assert!(JoyCon::connect(bad).is_none());
    }
    for (name, kind) in [(&b"Joy-Con (L)\0\0"[..], 1), (b"Joy-Con (R)", 2)] {
        connect(name, kind).disconnect();
        for wrong in [3 - kind, 3, 9, 0xff] {
            let mut pad = JoyCon::connect(name).unwrap();
            pad.poll_output(0).unwrap();
            assert!(pad.input_report(&info(wrong)).is_err());
            assert!(pad.input_report(&info(kind)).is_err());
            assert_eq!(pad.next_deadline_ms(), None);
            assert!(pad.poll_output(2000).is_err());
        }
    }
}

#[test]
fn unsolicited_malformed_or_out_of_order_reports_cannot_publish_a_gamepad() {
    let mut pad = JoyCon::connect(b"Joy-Con (R)").unwrap();
    let mode = ack(3, 0x80);
    let info = info(2);
    pad.input_report(&info).unwrap(); // No request issued.
    assert_eq!(pad.poll_output(100).unwrap().unwrap()[10], 2);
    for len in 0..27 {
        pad.input_report(&info[..len]).unwrap();
    }
    for garbage in [
        &mode[..],
        &[0x30; 49],
        &[0x3f; 12],
        &ack(2, 0x80),
        &ack(2, 0x00),
    ] {
        pad.input_report(garbage).unwrap();
    }
    assert_eq!(pad.next_deadline_ms(), Some(2100));
    pad.input_report(&info).unwrap();
    pad.input_report(&mode).unwrap(); // Mode not sent yet.
    assert_eq!(pad.poll_output(110).unwrap().unwrap()[10..], [3, 0x30]);
    pad.input_report(&info).unwrap(); // Duplicate info.
    pad.input_report(&mode[..14]).unwrap();
    pad.input_report(&ack(3, 0x82)).unwrap();
    assert_eq!(pad.next_deadline_ms(), Some(2110));
    assert!(!pad.is_ready());
    let mut held = mode;
    held[3] = 8;
    pad.input_report(&held).unwrap();
    let log = events(&pad);
    assert!(frame(&log.lock().unwrap()[0]).contains(&(1, 0x131, 1)));
}

#[test]
fn commands_retry_once_on_their_own_deadlines_then_fail_closed() {
    for cmd in [2, 3] {
        let mut pad = JoyCon::connect(b"Joy-Con (L)").unwrap();
        if cmd == 3 {
            pad.poll_output(10).unwrap();
            pad.input_report(&info(1)).unwrap();
        }
        let first = pad.poll_output(41).unwrap().unwrap();
        pad.input_report(&ack(cmd, 0)).unwrap(); // NACK cannot trigger a send storm.
        assert_eq!(pad.poll_output(2040).unwrap(), None);
        let second = pad.poll_output(2041).unwrap().unwrap();
        assert_eq!(&second[2..], &first[2..]);
        assert_eq!(second[1], first[1] + 1);
        assert_eq!(pad.poll_output(4040).unwrap(), None);
        assert!(pad.poll_output(4041).is_err());
        assert!(pad.poll_output(6000).is_err());
        assert!(pad.input_report(&ack(cmd, 0x80)).is_err());
        assert_eq!(pad.next_deadline_ms(), None);
    }
}

#[test]
fn each_solo_half_emits_only_its_own_controls_and_releases_before_removal() {
    let mut left = connect(b"Joy-Con (L)", 1);
    let mut right = connect(b"Joy-Con (R)", 2);
    let l = events(&left);
    let r = events(&right);
    let mut input = ack(0, 0);
    input[0] = 0x30;
    input[3..6].copy_from_slice(&[0xff, 0xff, 0xf6]); // L: up and right.
    input[6..12].copy_from_slice(&[0x23, 0x61, 0x45, 0x34, 0x72, 0x56]);
    left.input_report(&input).unwrap();
    right.input_report(&input).unwrap();

    left.disconnect();
    {
        let log = l.lock().unwrap();
        assert_eq!(log.len(), 4);
        let held = frame(&log[1]);
        assert_eq!(held.len(), 12);
        for control in [
            (3, 0, 0x123),
            (3, 1, 4095 - 0x456),
            (3, 0x10, 1),
            (3, 0x11, -1),
            (1, 0x136, 1),
            (1, 0x138, 1),
            (1, 0x13a, 1),
            (1, 0x13d, 1),
            (1, 0x2c0, 1),
            (1, 0x2c1, 1),
            (1, 0x2c2, 1),
        ] {
            assert!(held.contains(&control));
        }
        assert_eq!(frame(&log[2]), frame(&log[0]));
        assert_eq!(log[3], Event::Removed);
    }
    {
        let log = r.lock().unwrap();
        assert_eq!(log.len(), 2);
        let held = frame(&log[1]);
        assert_eq!(held.len(), 14);
        for control in [
            (3, 0, 0x234),
            (3, 1, 4095 - 0x567),
            (1, 0x130, 1),
            (1, 0x131, 1),
            (1, 0x133, 1),
            (1, 0x134, 1),
            (1, 0x137, 1),
            (1, 0x139, 1),
            (1, 0x13b, 1),
            (1, 0x13c, 1),
            (1, 0x13e, 1),
            (1, 0x2c3, 1),
            (1, 0x2c4, 1),
        ] {
            assert!(held.contains(&control));
        }
    }
    drop(right);
    let log = r.lock().unwrap();
    assert_eq!(frame(&log[2]), frame(&log[0]));
    assert_eq!(log[3], Event::Removed);
}

#[test]
fn supported_headers_share_stick_decoding_and_keep_each_button_edge() {
    let mut pad = connect(b"Joy-Con (R)", 2);
    let log = events(&pad);
    let mut report = ack(0, 0);
    // Other-side button noise and sub-threshold stick movement are quiet.
    report[5] = 0xff;
    report[9] = 8;
    pad.input_report(&report).unwrap();
    assert_eq!(log.lock().unwrap().len(), 1);
    report[0] = 0x31;
    report[9] = 9;
    pad.input_report(&report[..12]).unwrap(); // Accumulated stick movement.
    report[0] = 0x30;
    report[3] = 4;
    pad.input_report(&report[..12]).unwrap();
    report[0] = 0x21;
    report[3] = 0;
    pad.input_report(&report[..14]).unwrap(); // Truncated reply.
    pad.input_report(&report[..15]).unwrap();
    for id in [0x30, 0x31] {
        report[0] = id;
        report[3] = 8;
        for len in 0..12 {
            pad.input_report(&report[..len]).unwrap();
        }
    }
    for id in [0x3f, 0xa1, 0, 0xff] {
        report[0] = id;
        pad.input_report(&report).unwrap();
    }
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 4);
    assert!(frame(&log[1]).contains(&(3, 0, 2057)));
    assert!(frame(&log[2]).contains(&(1, 0x130, 1)));
    assert!(frame(&log[3]).contains(&(1, 0x130, 0)));
}

#[test]
fn late_ack_after_retry_succeeds_and_reconnect_starts_fresh() {
    let name = b"Joy-Con (R)";
    let mut pad = JoyCon::connect(name).unwrap();
    for now in [0, 2000] {
        assert_eq!(pad.poll_output(now).unwrap().unwrap()[10], 2);
    }
    pad.input_report(&info(2)).unwrap();
    for now in [2001, 4001] {
        assert_eq!(pad.poll_output(now).unwrap().unwrap()[10..], [3, 0x30]);
    }
    let mut held = ack(3, 0x80);
    held[3] = 4; // B
    pad.input_report(&held).unwrap();
    assert!(pad.is_ready());
    let old = events(&pad);
    drop(pad);
    let next = connect(name, 2);
    let fresh = events(&next);
    assert!(!Arc::ptr_eq(&old, &fresh));
    let old = old.lock().unwrap();
    assert!(frame(&old[0]).contains(&(1, 0x130, 1)));
    assert!(frame(&old[1]).contains(&(1, 0x130, 0)));
    assert_eq!(old[2], Event::Removed);
    assert_eq!(frame(&old[1]), frame(&fresh.lock().unwrap()[0]));
}
