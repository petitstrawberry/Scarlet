//! Exercise the shipped CLI and native input handles inside Scarlet.
use scarlet_os::input::{INPUT_CAP_INTERNAL, InputDevice, InputDeviceKind};
use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[allow(dead_code)]
#[path = "../../../user/std-bin/src/sws/gamepad.rs"]
mod gamepad;

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

type Event = (u16, u16, i32);

fn read_frame(input: &InputDevice, nav: &mut gamepad::Navigation) -> (Vec<Event>, Vec<(u16, i32)>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut frame = Vec::new();
    loop {
        assert!(Instant::now() < deadline, "timeout reading gamepad frame");
        let mut record = [0; 16];
        let size = input.read(&mut record).unwrap();
        if size == 0 {
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        assert_eq!(size, 16);
        let ty = u16::from_ne_bytes(record[8..10].try_into().unwrap());
        let code = u16::from_ne_bytes(record[10..12].try_into().unwrap());
        let value = i32::from_ne_bytes(record[12..16].try_into().unwrap());
        assert!(
            matches!(
                (ty, code),
                (0, 0) | (1, 0x130..=0x2ff) | (3, 0 | 1 | 0x10 | 0x11)
            ),
            "unexpected native event ({ty}, {code}, {value})"
        );
        frame.push((ty, code, value));
        let translated = nav.consume(ty, code, value);
        if ty == 0 {
            return (
                frame,
                match translated {
                    gamepad::Frame::Keys(keys) => keys,
                    gamepad::Frame::None => vec![],
                    other => panic!("unexpected SWS frame {other:?}"),
                },
            );
        }
    }
}

fn gamepads() -> Vec<(String, InputDevice)> {
    // Scan the same eight nodes as SWS. A new controller can reuse a slot.
    (0..8)
        .filter_map(|index| {
            let path = format!("/dev/gamepad{index}");
            let input = InputDevice::open(&path).ok()?;
            Some((path, input))
        })
        .collect()
}

fn exercise(address: &str, right: bool) {
    assert!(gamepads().is_empty(), "gamepad present before pairing");
    let result = btctl(&["pair", address]);
    assert!(result.contains(&format!(
        "peer={address} connected=true encrypted=true profile-ready=true key-readable=false"
    )));
    assert!(result.contains(&format!("bond saved for {address}")));

    let mut pads = gamepads();
    assert_eq!(pads.len(), 1, "expected exactly one registered gamepad");
    let (path, input) = pads.pop().unwrap();
    assert_eq!(input.kind().unwrap(), InputDeviceKind::Gamepad);
    assert_eq!(input.capabilities().unwrap() & INPUT_CAP_INTERNAL, 0);
    input.set_nonblocking(true).unwrap();
    let mut nav = gamepad::Navigation::new(gamepad::Config::default());
    for code in [0, 1] {
        let axis = input.absolute_axis(code).unwrap();
        assert_eq!((axis.minimum, axis.maximum), (0, 4095));
        nav.set_axis_range(code, axis.minimum, axis.maximum);
    }
    let (initial, keys) = read_frame(&input, &mut nav);
    assert!(keys.is_empty());
    assert!(initial.contains(&(3, 0, 2048)) && initial.contains(&(3, 1, 2048)));
    let (previous, keys) = read_frame(&input, &mut nav);
    let button = if right { 0x137 } else { 0x136 };
    assert!(previous.contains(&(1, button, 1)));
    assert!(previous.contains(&if right { (3, 0, 0) } else { (3, 0x10, -1) }));
    assert_eq!(keys, [(105, 1)], "SWS previous-page press");
    let (next, keys) = read_frame(&input, &mut nav);
    assert!(next.contains(&(1, button, 1)));
    assert!(next.contains(&if right { (3, 0, 4095) } else { (3, 0x10, 1) }));
    assert_eq!(keys, [(105, 0), (106, 1)]);
    let state = btctl(&["disconnect"]);
    assert!(state.contains("peer=none connected=false encrypted=false profile-ready=false"));
    let (released, keys) = read_frame(&input, &mut nav);
    assert_eq!(released, initial);
    assert!(released.contains(&(1, button, 0)));
    assert_eq!(keys, [(106, 0)], "SWS next-page release");
    assert!(
        InputDevice::open(&path).is_err(),
        "disconnected gamepad still discoverable"
    );
    println!("BT_QA_INPUT_PASS {address} previous next held-button disconnect-neutral");
}

fn main() {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if btctl(&["status"]).contains("ready=true") {
            break;
        }
        assert!(Instant::now() < deadline, "controller failed to initialize");
        thread::sleep(Duration::from_millis(10));
    }
    let scan = btctl(&["scan"]);
    assert!(scan.contains("02:00:00:00:00:10  \"Joy-Con (L)\""));
    assert!(scan.contains("02:00:00:00:00:20  \"Joy-Con (R)\""));
    assert!(scan.contains("2 device(s) found"));
    // Deliberately select the second discovered peer first.
    exercise("02:00:00:00:00:20", true);
    exercise("02:00:00:00:00:10", false);
    println!("BLUETOOTH_QEMU_PASS");
}
