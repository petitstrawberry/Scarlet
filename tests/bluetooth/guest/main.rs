//! Production Bluetooth broker, SDP, generic HID input and raw report checks.
use scarlet_abi::{fs::VFS_O_RDWR, hidraw};
use scarlet_os::{Handle, InputDevice, Socket, handle::capability::StreamError};
use std::{
    io::{Read, Write},
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[path = "../radio.rs"]
mod wire;

const ADDR: [&str; 2] = ["02:00:00:00:00:10", "02:00:00:00:00:20"];

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

fn radio_get(radio: &Handle, op: u32, side: u32, value: usize) -> i32 {
    unsafe { radio.control(op | side, value) }.unwrap()
}

fn link(side: u32, pair: bool) {
    let addr = ADDR[side as usize];
    let result = btctl(&[if pair { "pair" } else { "connect" }, addr]);
    assert!(result.contains("connected=true encrypted=true"), "{result}");
    if pair {
        assert!(result.contains("bond saved"), "{result}");
    }
}

fn raw() -> Handle {
    wait("generic HID endpoint", || {
        Handle::open("/dev/hidraw0", VFS_O_RDWR as usize).ok()
    })
}

fn verify_description(handle: &Handle) {
    let mut info = hidraw::Info::default();
    let mut descriptor = [0u8; hidraw::MAX_DESCRIPTOR];
    unsafe {
        assert_eq!(
            handle.control(hidraw::INFO, (&mut info as *mut hidraw::Info) as usize),
            Ok(0)
        );
        assert_eq!(
            info.flags & (hidraw::HAS_IDENTITY | hidraw::CAN_OUTPUT),
            hidraw::HAS_IDENTITY | hidraw::CAN_OUTPUT
        );
        assert_eq!(info.transport, hidraw::BLUETOOTH);
        assert_eq!(info.vendor_id_source, 2);
        assert_eq!((info.vendor_id, info.product_id), (0xf001, 0x1234));
        assert_eq!(
            (info.controller, info.peer),
            (wire::ADAPTER, wire::HID_ADDR)
        );
        assert_eq!(
            &info.name[..info.name_len as usize],
            wire::HID_NAME.as_bytes()
        );
        let request = hidraw::Transfer {
            address: descriptor.as_mut_ptr() as u64,
            len: descriptor.len() as u32,
            ..Default::default()
        };
        assert_eq!(
            handle.control(
                hidraw::GET_DESCRIPTOR,
                (&request as *const hidraw::Transfer) as usize
            ),
            Ok(wire::DESCRIPTOR.len() as i32)
        );
    }
    assert_eq!(info.descriptor_len as usize, wire::DESCRIPTOR.len());
    assert_eq!(&descriptor[..wire::DESCRIPTOR.len()], wire::DESCRIPTOR);
}

fn read_report(handle: &Handle, expected: &[u8]) {
    let stream = handle.as_stream().unwrap();
    let mut buffer = [0u8; 64];
    wait("raw report", || match stream.read(&mut buffer) {
        Ok(n) => {
            assert_eq!(&buffer[..n], expected);
            Some(())
        }
        Err(StreamError::WouldBlock | StreamError::Interrupted) => None,
        result => panic!("raw input read failed: {result:?}"),
    });
}

fn read_key(keyboard: &InputDevice, value: i32) {
    let mut buffer = [0u8; 128];
    wait("decoded keyboard event", || {
        match keyboard.read(&mut buffer) {
            Ok(n) => buffer[..n]
                .chunks_exact(16)
                .any(|event| {
                    u16::from_ne_bytes(event[8..10].try_into().unwrap()) == 1
                        && u16::from_ne_bytes(event[10..12].try_into().unwrap()) == 30
                        && i32::from_ne_bytes(event[12..16].try_into().unwrap()) == value
                })
                .then_some(()),
            Err(StreamError::WouldBlock | StreamError::Interrupted) => None,
            result => panic!("keyboard input read failed: {result:?}"),
        }
    });
}

fn main() {
    let mut broker = Command::new("/bin/btctl").arg("--serve").spawn().unwrap();
    let radio = Handle::open("/dev/bt-qa-radio", 0).unwrap();
    broker_ready();
    wait("initialized adapter", || {
        btctl(&["status"]).contains("ready=true").then_some(())
    });
    let scan = btctl(&["scan"]);
    for (name, mac) in [wire::HID_NAME, wire::OTHER_NAME].into_iter().zip(ADDR) {
        assert!(scan.contains(&format!("{mac}  \"{name}\"")));
    }

    link(wire::OTHER, true);
    wait("non-HID SDP query", || {
        (radio_get(&radio, wire::CONNECTION_REQUESTS, wire::OTHER, 0) == 1).then_some(())
    });
    assert_eq!(radio_get(&radio, wire::OPEN_CHANNELS, wire::OTHER, 0), 0);
    assert!(Handle::open("/dev/hidraw0", VFS_O_RDWR as usize).is_err());
    link(wire::HID, true);
    let first = raw();
    let second = raw();
    for fd in [&first, &second] {
        fd.set_nonblocking(true).unwrap();
    }
    verify_description(&first);
    wait("two encrypted peers", || {
        let list = btctl(&["devices"]);
        (list.contains("2 device(s) connected")
            && list.contains(&format!(
                "peer={} connected=true encrypted=true profile-ready=true",
                ADDR[0]
            ))
            && list.contains(&format!(
                "peer={} connected=true encrypted=true profile-ready=false",
                ADDR[1]
            )))
        .then_some(())
    });
    println!("BT_QA_SDP_PASS HID descriptor + PnP identity; no HID channels for non-HID peer");

    let keyboard = InputDevice::open("/dev/keyboard0").unwrap();
    keyboard.set_nonblocking(true).unwrap();
    for (value, report) in [(1, wire::PRESS), (0, wire::RELEASE)] {
        radio_get(&radio, wire::SEND_INPUT, wire::HID, (1 - value) as usize);
        read_report(&first, report);
        read_report(&second, report);
        read_key(&keyboard, value);
    }
    for report in [wire::OUTPUT_LONG, wire::OUTPUT_SHORT] {
        assert_eq!(first.as_stream().unwrap().write(report), Ok(report.len()));
    }
    wait("fragmented output", || {
        (radio_get(&radio, wire::OUTPUT_COUNT, wire::HID, 0) == 2).then_some(())
    });
    println!(
        "BT_QA_REPORT_PASS descriptor-driven keyboard and independent raw readers; complete output reports"
    );

    radio_get(&radio, wire::UNPLUG, wire::HID, 0);
    wait("raw EOF and endpoint removal", || {
        let ended = matches!(
            first.as_stream().unwrap().read(&mut [0u8; 64]),
            Ok(0) | Err(StreamError::EndOfStream)
        );
        (ended && Handle::open("/dev/hidraw0", VFS_O_RDWR as usize).is_err()).then_some(())
    });
    assert!(btctl(&["devices"]).contains("1 device(s) connected"));
    assert!(btctl(&["devices"]).contains(ADDR[1]));
    link(wire::HID, false);
    let replacement = raw();
    verify_description(&replacement);
    assert!(btctl(&["devices"]).contains("2 device(s) connected"));
    assert!(broker.try_wait().unwrap().is_none());
    println!(
        "BT_QA_RECONNECT_PASS a new HID endpoint reused the path while the old handle stayed detached"
    );
    println!("BLUETOOTH_QEMU_PASS");
}
