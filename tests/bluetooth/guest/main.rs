//! Production Bluetooth broker, SDP, generic HID input and raw report checks.
use scarlet_abi::{bluetooth as bt, fs::VFS_O_RDWR, hidraw};
use scarlet_bluetooth::{Address, Client};
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
const OTHER_KEY: &str = "/etc/bluetooth-020000000001-020000000020.bond";
const OTHER_NAME_FILE: &str = "/etc/bluetooth-020000000001-020000000020.name";

fn wait<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    wait_for(what, Duration::from_secs(8), &mut check)
}

fn wait_for<T>(what: &str, timeout: Duration, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + timeout;
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

fn login_client(mode: &str) {
    let uid = if mode == "locked" { 1002 } else { 1001 };
    // SAFETY: fresh single-threaded child with no handle owners above stdio.
    unsafe { scarlet_os::shield::enter_user_session(uid, uid, &[]) }.unwrap();
    let client = Client::new(0);
    if mode == "locked" {
        assert!(
            client
                .status()
                .unwrap_err()
                .contains("unconfined login account")
        );
        return;
    }
    let other = Address(wire::OTHER_ADDR);
    let status = client.status().unwrap();
    assert!(status.ready);
    assert_eq!(status.adapter, Address(wire::ADAPTER));
    if mode == "pair" {
        assert!(client.bonded().unwrap().is_empty());
        assert!(client.saved().unwrap().is_empty());
        assert_eq!(client.status().unwrap().generation, status.generation);
        assert!(
            client
                .scan()
                .unwrap()
                .iter()
                .any(|row| row.address == other && row.name.as_deref() == Some(wire::OTHER_NAME))
        );
        let paired = client.pair(other).unwrap();
        assert!(paired.connected && paired.encrypted && !paired.key_readable);
        return;
    }
    if mode == "forget" {
        client.forget(other).unwrap();
        assert!(client.connect(other).unwrap_err().contains("no saved key"));
        assert_eq!(client.bonded().unwrap(), [Address(wire::HID_ADDR)]);
        return;
    }
    assert_eq!(mode, "reconnect");
    assert_eq!(client.list().unwrap().len(), 2);
    let down = client.disconnect(Some(other)).unwrap();
    assert!(!down.connected);
    let saved = client.bonded().unwrap();
    assert_eq!(saved.len(), 2);
    assert!(saved.contains(&other));
    assert!(
        client
            .saved()
            .unwrap()
            .iter()
            .any(|p| p.address == other && p.name.as_deref() == Some(wire::OTHER_NAME))
    );
    assert_eq!(client.status().unwrap().generation, down.generation);
    assert!(
        client
            .list()
            .unwrap()
            .iter()
            .all(|peer| peer.address != other)
    );
    assert!(std::fs::File::open(OTHER_KEY).is_err());
    assert!(std::fs::File::open(OTHER_NAME_FILE).is_err());
    let up = client.connect(other).unwrap();
    assert_eq!(up.peer, Some(other));
    assert!(up.connected && up.encrypted);
}

fn as_login(mode: &str) {
    assert!(
        Command::new("/bin/bluetooth-qa")
            .arg(mode)
            .status()
            .unwrap()
            .success()
    );
}

fn keyless_reconnect_fails(peer: bt::Address) {
    // Exercise the kernel directly, bypassing the broker's missing-file check.
    let controller = Handle::open("/dev/bluetooth0", VFS_O_RDWR as usize).unwrap();
    let mut row = [0; bt::RECORD_SIZE];
    row[bt::PEER].copy_from_slice(&peer);
    let ctl =
        |op, row: &mut bt::Record| unsafe { controller.control(op, row.as_mut_ptr() as usize) };
    assert_eq!(ctl(bt::CONNECT, &mut row), Ok(0));
    let generation = row[bt::GENERATION].to_vec();
    wait("keyless reconnect rejected", || {
        row.fill(0);
        row[bt::PEER].copy_from_slice(&peer);
        assert_eq!(ctl(bt::PEER_STATUS, &mut row), Ok(0));
        assert_eq!(row[bt::GENERATION], generation);
        let flags = u32::from_le_bytes(row[bt::FLAGS].try_into().unwrap());
        if flags & bt::BUSY != 0 {
            return None;
        }
        assert_eq!(flags & (bt::CONNECTED | bt::ENCRYPTED), 0);
        assert_ne!(row[bt::LAST_ERROR], [0; 4]);
        Some(())
    });
}

fn registration_after_reset(radio: &Handle) {
    let controller = Handle::open("/dev/bluetooth0", VFS_O_RDWR as usize).unwrap();
    let ctl = |op, row: &mut bt::Record| {
        unsafe { controller.control(op, row.as_mut_ptr() as usize) }.unwrap()
    };
    let epoch = || {
        let mut row = [0; bt::RECORD_SIZE];
        assert_eq!(ctl(bt::STATUS, &mut row), 0);
        (u32::from_le_bytes(row[bt::FLAGS].try_into().unwrap()) & bt::READY != 0)
            .then(|| u64::from_le_bytes(row[bt::ADAPTER_GENERATION].try_into().unwrap()))
    };
    let before = epoch().unwrap();
    let scanning = || radio_get(radio, wire::PAGE_SCAN_ENABLED, wire::OTHER, 0) != 0;
    // Withdraw only the controller registration, leaving the boot bond on disk.
    // A worker that rereads keys on idle polls would re-enable page scan here.
    let mut request = [0; bt::RECORD_SIZE];
    request[bt::PEER].copy_from_slice(&wire::OTHER_ADDR);
    assert_eq!(ctl(bt::EVICT_KEY, &mut request), 0);
    wait("key evicted from idle controller", || {
        (!scanning()).then_some(())
    });
    thread::sleep(Duration::from_secs(6));
    assert!(!scanning(), "idle poll re-registered a key");
    radio_get(radio, wire::CONTROLLER_FAULT, wire::OTHER, 0);
    wait("adapter reinitialized", || epoch().filter(|e| *e != before));
    wait("bond restored on new controller generation", || {
        scanning().then_some(())
    });
}

fn saved_and_forget(radio: &Handle) {
    let client = Client::new(0);
    let key_id = || {
        Handle::open(OTHER_KEY, 0)
            .unwrap()
            .as_file()
            .unwrap()
            .metadata()
            .unwrap()
            .file_id
    };
    let id = key_id();
    assert_eq!(
        std::fs::read_to_string(OTHER_NAME_FILE).unwrap(),
        wire::OTHER_NAME
    );
    std::fs::remove_file(OTHER_NAME_FILE).unwrap(); // Original 17-byte bond, no sidecar.
    let generation = client.status().unwrap().generation;
    let saved = btctl(&["saved"]);
    assert!(saved.contains(&format!("{}  \"{}\"", ADDR[1], wire::OTHER_NAME)));
    assert_eq!(
        std::fs::read_to_string(OTHER_NAME_FILE).unwrap(),
        wire::OTHER_NAME
    );
    assert_eq!(key_id(), id);
    assert_eq!(client.status().unwrap().generation, generation);

    as_login("forget");
    assert!(std::fs::metadata(OTHER_KEY).is_err() && std::fs::metadata(OTHER_NAME_FILE).is_err());
    keyless_reconnect_fails(wire::OTHER_ADDR);
    // Allow the manual-disconnect cooldown and a full registration pass.
    // HID is still registered, so the controller is listening on the air.
    thread::sleep(Duration::from_secs(6));
    let rejected = radio_get(radio, wire::REJECTED_PAGES, wire::OTHER, 0);
    assert_eq!(radio_get(radio, wire::WAKE, wire::OTHER, 0), 1);
    wait("forgotten peer's incoming page rejected", || {
        (radio_get(radio, wire::REJECTED_PAGES, wire::OTHER, 0) > rejected).then_some(())
    });
    assert_eq!(client.list().unwrap()[0].address, Address(wire::HID_ADDR));

    // A failed HCI disconnect leaves a physical ACL quarantined. Both the
    // initial request and an immediate retry must fail and preserve the bond.
    let hid = Address(wire::HID_ADDR);
    radio_get(radio, wire::FAIL_DISCONNECT, wire::HID, 0);
    assert!(client.forget(hid).is_err());
    assert!(client.forget(hid).is_err());
    assert_eq!(client.bonded().unwrap(), [hid]);
    radio_get(radio, wire::UNPLUG, wire::HID, 0);
    wait("physical detach and eviction", || client.forget(hid).ok());
    assert!(btctl(&["unpair", ADDR[0]]).contains("forgot"));
    assert!(client.saved().unwrap().is_empty());
    keyless_reconnect_fails(wire::HID_ADDR);
    println!(
        "BT_QA_FORGET_PASS named offline bonds, legacy migration, failed disconnect, retry and key eviction"
    );
}

fn overlapping_wakes(radio: &Handle) {
    let client = Client::new(0);
    client.disconnect(Some(Address(wire::HID_ADDR))).unwrap();
    let command = client.disconnect(Some(Address(wire::OTHER_ADDR))).unwrap();
    let controller = Handle::open("/dev/bluetooth0", VFS_O_RDWR as usize).unwrap();
    let peer = |addr: bt::Address| {
        let mut row = [0; bt::RECORD_SIZE];
        row[bt::PEER].copy_from_slice(&addr);
        assert_eq!(
            unsafe { controller.control(bt::PEER_STATUS, row.as_mut_ptr() as usize) },
            Ok(0)
        );
        let [flags, generation, error] = [bt::FLAGS, bt::GENERATION, bt::LAST_ERROR]
            .map(|field| u32::from_le_bytes(row[field].try_into().unwrap()));
        assert_ne!(flags & bt::READY, 0);
        (flags & !bt::READY, generation, error)
    };
    let adapter = |busy| {
        let status = client.status().unwrap();
        assert_eq!(status.generation, command.generation);
        assert_eq!(status.last_error, command.last_error);
        assert_eq!(status.busy, busy);
    };
    assert_eq!(radio_get(radio, wire::WAKE_BOTH, 0, 0), 1);
    let [hid_gen, other_gen] = wait("two pending bonded pages", || {
        let [(hid_flags, hid_gen, _), (other_flags, other_gen, _)] =
            [wire::HID_ADDR, wire::OTHER_ADDR].map(peer);
        (hid_flags == bt::BUSY && other_flags == bt::BUSY).then_some([hid_gen, other_gen])
    });
    assert_ne!(hid_gen, other_gen);
    assert!(![hid_gen, other_gen].contains(&command.generation));
    adapter(true);
    let mut scan = [0; bt::RECORD_SIZE];
    assert!(unsafe { controller.control(bt::SCAN_START, scan.as_mut_ptr() as usize) }.is_err());

    // Fail the peer selected by the last user command while HID is still pending.
    wait("second page accepted", || {
        (radio_get(radio, wire::COMPLETE_WAKE, wire::OTHER, 0x05) == 1).then_some(())
    });
    wait("peer authentication failure", || {
        (peer(wire::OTHER_ADDR) == (0, other_gen, 0x05)).then_some(())
    });
    assert_eq!(peer(wire::HID_ADDR), (bt::BUSY, hid_gen, 0));
    adapter(true);
    wait("first page accepted", || {
        (radio_get(radio, wire::COMPLETE_WAKE, wire::HID, 0) == 1).then_some(())
    });
    let survivor = raw();
    verify_description(&survivor);
    radio_get(radio, wire::SEND_INPUT, wire::HID, 0);
    read_report(&survivor, wire::PRESS);
    assert_eq!(
        peer(wire::HID_ADDR),
        (
            bt::CONNECTED | bt::ENCRYPTED | bt::PROFILE_READY,
            hid_gen,
            0
        )
    );
    assert_eq!(peer(wire::OTHER_ADDR), (0, other_gen, 0x05));
    adapter(false);
    let next = client.connect(Address(wire::OTHER_ADDR)).unwrap();
    assert!(![command.generation, hid_gen, other_gen].contains(&next.generation));
    assert_eq!(peer(wire::OTHER_ADDR).1, next.generation);
    println!(
        "BT_QA_OVERLAPPING_WAKE_PASS simultaneous bonded pages, isolated authentication failure and encrypted HID input"
    );
}

fn host_reconnect(radio: &Handle) {
    let client = Client::new(0);
    let hid = Address(wire::HID_ADDR);
    assert!(btctl(&["devices"]).contains("hid-reconnect-mode=any"));
    let pages = || radio_get(radio, wire::HOST_PAGES, wire::HID, 0);
    thread::sleep(Duration::from_secs(6)); // the broker has observed the live SDP mode
    let before = pages();
    let lost = Instant::now();
    radio_get(radio, wire::UNPLUG, wire::HID, 0);
    wait_for("outgoing HID host retry", Duration::from_secs(45), || {
        (pages() > before).then_some(())
    });
    assert!(lost.elapsed() >= Duration::from_secs(30));
    verify_description(&raw());
    assert!(
        client
            .list()
            .unwrap()
            .iter()
            .any(|p| p.address == hid && p.encrypted)
    );
    thread::sleep(Duration::from_secs(6));
    client.disconnect(Some(hid)).unwrap();
    let down = pages();
    thread::sleep(Duration::from_secs(41)); // loss observation, 30 s delay and dispatch
    assert_eq!(pages(), down, "explicit disconnect started a host retry");
    assert_eq!(client.list().unwrap().len(), 1);
    client.connect(hid).unwrap();
    verify_description(&raw());
    println!(
        "BT_QA_HOST_RETRY_PASS HID host/any loss triggered delayed authenticated paging, explicit disconnect stayed offline"
    );
}

fn main() {
    if let Some(mode) = std::env::args().nth(1) {
        return login_client(&mode);
    }
    let mut broker = Command::new("/bin/btctl").arg("--serve").spawn().unwrap();
    let radio = Handle::open("/dev/bt-qa-radio", 0).unwrap();
    // No IPC client has contacted the broker. The boot image contains only a
    // legacy key for OTHER; page scan proves registration ran autonomously.
    wait("startup bond registration", || {
        (radio_get(&radio, wire::PAGE_SCAN_ENABLED, wire::OTHER, 0) == 1).then_some(())
    });
    registration_after_reset(&radio);
    broker_ready();
    wait("initialized adapter", || {
        btctl(&["status"]).contains("ready=true").then_some(())
    });
    let client = Client::new(0);
    let legacy = &client.saved().unwrap()[0];
    assert_eq!(legacy.address, Address(wire::OTHER_ADDR));
    assert!(legacy.name.is_none());
    let scan = btctl(&["scan"]);
    for (name, mac) in [wire::HID_NAME, wire::OTHER_NAME].into_iter().zip(ADDR) {
        assert!(scan.contains(&format!("{mac}  \"{name}\"")));
    }
    assert_eq!(
        std::fs::read_to_string(OTHER_NAME_FILE).unwrap(),
        wire::OTHER_NAME
    );
    std::fs::remove_file(OTHER_NAME_FILE).unwrap();
    assert_eq!(
        client.saved().unwrap()[0].name.as_deref(),
        Some(wire::OTHER_NAME)
    );
    assert_eq!(
        std::fs::read_to_string(OTHER_NAME_FILE).unwrap(),
        wire::OTHER_NAME
    );
    client.forget(Address(wire::OTHER_ADDR)).unwrap();
    wait("unregister boot bond", || {
        (radio_get(&radio, wire::PAGE_SCAN_ENABLED, wire::OTHER, 0) == 0).then_some(())
    });
    println!("BT_QA_REGISTER_PASS broker restored a legacy bond before first client contact");

    as_login("locked");
    as_login("pair");
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
    assert_eq!(radio_get(&radio, wire::WAKE, wire::HID, 0), 1);
    // There is no client connect request: the saved peripheral initiates this ACL.
    let replacement = raw();
    verify_description(&replacement);
    assert!(btctl(&["devices"]).contains("2 device(s) connected"));
    assert!(broker.try_wait().unwrap().is_none());
    println!(
        "BT_QA_RECONNECT_PASS saved HID peer initiated an encrypted reconnect while the other link stayed up"
    );
    as_login("reconnect");
    overlapping_wakes(&radio);
    host_reconnect(&radio);
    assert!(btctl(&["bonded"]).contains("2 device(s) bonded"));
    saved_and_forget(&radio);
    let policy = scarlet_os::shield::Policy::create(&[(1, "app")], &[]).unwrap();
    policy.apply(1).unwrap();
    assert!(
        Client::new(0)
            .status()
            .unwrap_err()
            .contains("unconfined login account")
    );
    println!(
        "BT_QA_BROKER_PASS typed non-root management, offline bonds and restricted caller denial"
    );
    println!("BLUETOOTH_QEMU_PASS");
}
