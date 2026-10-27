#![no_std]
#![no_main]
extern crate scarlet_std as std;
use scarlet_os::{
    ipc::{self, ProcessControl},
    network::*,
    socket::{
        DatagramOps, Inet4SocketAddress, Socket, SocketAddress, SocketDomain, SocketProtocol,
        SocketType,
    },
    wireless::*,
};
use std::{fs::File, println, string::String, thread, time::Duration};
fn wait(label: &str, mut test: impl FnMut() -> bool) {
    for _ in 0..600 {
        if test() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("WPA QA timeout: {}", label);
}
fn text(path: &str) -> String {
    let mut bytes = [0; 1024];
    let n = File::open(path)
        .and_then(|mut f| f.read(&mut bytes))
        .unwrap();
    String::from_utf8_lossy(&bytes[..n]).into_owned()
}
fn config() -> NetworkInterfaceConfig {
    list_interface_configs()
        .unwrap()
        .into_iter()
        .find(|c| c.interface_name() == Some("veth0"))
        .unwrap()
}
fn up() -> bool {
    list_links()
        .unwrap()
        .iter()
        .any(|l| l.interface_name() == Some("veth0") && l.is_up())
}
fn start(program: &str, args: &[&str]) -> i32 {
    let pid = std::task::fork();
    assert!(pid >= 0);
    if pid == 0 {
        let result = std::task::execve(program, args, &[]);
        panic!("exec failed {}", result);
    }
    pid
}
fn stop(pid: i32, observer: &mut WirelessSession) {
    ipc::send_process_control(pid as u32, ProcessControl::Kill).unwrap();
    assert_eq!(std::task::waitpid(pid, 0).0, pid);
    wait("controller teardown", || {
        observer.status().unwrap().phase == IDLE && !up() && config().ip_set == 0
    });
    assert_eq!(observer.security_status().unwrap().keys_installed, 0);
    assert_eq!(observer.ccmp_status().unwrap().keys, 0);
    observer.claim_controller().unwrap();
    let keys = observer.mock_security_stats().unwrap();
    assert_eq!((keys.installed, keys.pending), (0, 0));
    observer.release_controller().unwrap();
}
fn associated(observer: &mut WirelessSession, label: &str) -> u64 {
    wait("association", || {
        observer.status().unwrap().phase == ASSOCIATED
    });
    let connection = observer.identity().connection_id;
    assert_ne!(connection, 0);
    assert!(!up());
    assert_eq!(observer.security_status().unwrap().keys_installed, 0);
    println!("CCMP_QA ASSOC {} {}", label, connection);
    connection
}
fn authenticated(observer: &mut WirelessSession) {
    wait("authentication and DHCP", || {
        observer.status().unwrap().phase == AUTHORIZED
            && up()
            && config().ip_address == [10, 0, 2, 15]
            && config().is_default == 1
    });
    let state = observer.security_status().unwrap();
    assert_eq!((state.keys_installed, state.keys_pending), (2, 0));
}
fn udp() -> Socket {
    let s = Socket::new_with_domain(
        SocketDomain::Inet4,
        SocketType::Datagram,
        SocketProtocol::Udp,
    )
    .unwrap();
    s.bind_interface("veth0").unwrap();
    s.bind_inet(Inet4SocketAddress::new([0; 4], 40003)).unwrap();
    s.set_nonblocking(true).unwrap();
    s
}
fn echo(socket: &Socket, to: &SocketAddress, message: &[u8]) {
    socket.sendto(message, to).unwrap();
    let mut bytes = [0; 128];
    wait("UDP echo", || match socket.recvfrom(&mut bytes) {
        Ok((n, _)) => {
            assert_eq!(&bytes[..n], message);
            true
        }
        Err(_) => false,
    });
}
fn quiet(socket: &Socket) {
    let mut bytes = [0; 128];
    for _ in 0..25 {
        assert!(
            socket.recvfrom(&mut bytes).is_err(),
            "rejected packet delivered to UDP"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
fn committed(session: &mut WirelessSession, id: u64) {
    wait("confirmed key", || {
        session.request(id).unwrap().result != ACCEPTED
    });
    assert_eq!(session.request(id).unwrap().result, OK);
    session.acknowledge(id).unwrap();
}
fn manual(session: &mut WirelessSession, label: &str, variant: u8) {
    let connect = session.connect_wpa2(b"ScarletMock", 15000).unwrap();
    wait("manual association", || {
        session.status().unwrap().phase == ASSOCIATED
    });
    println!(
        "CCMP_QA MANUAL {} {}",
        label,
        session.identity().connection_id
    );
    for kind in [KEY_PAIRWISE, KEY_GROUP] {
        let mut key = KeyInstall {
            version: 1,
            kind,
            index: if kind == KEY_GROUP { 1 } else { 0 },
            cipher: CIPHER_CCMP128,
            key_len: 16,
            peer: if kind == KEY_GROUP {
                [255; 6]
            } else {
                [2, 0, 0, 0, 0, 1]
            },
            ..KeyInstall::default()
        };
        for (i, b) in key.key[..16].iter_mut().enumerate() {
            *b = i as u8 + if kind == KEY_GROUP { 16 } else { 0 };
        }
        key.key[0] ^= variant;
        let id = session.install_key(&key).unwrap();
        committed(session, id);
    }
    let revision = session.security_status().unwrap().key_revision;
    session.set_controlled_port(true, revision).unwrap();
    committed(session, connect);
    authenticated(session);
}
fn receive(socket: &Socket, expected: &[u8]) {
    let mut bytes = [0; 128];
    wait("group RX", || match socket.recvfrom(&mut bytes) {
        Ok((n, _)) => {
            assert_eq!(&bytes[..n], expected);
            true
        }
        Err(_) => false,
    });
}
fn generation() -> u64 {
    list_links()
        .unwrap()
        .into_iter()
        .find(|l| l.interface_name() == Some("veth0"))
        .unwrap()
        .generation
}
fn group_tests(observer: &mut WirelessSession, socket: &Socket, to: &SocketAddress) {
    let initial_generation = generation();
    let initial_connection = observer.identity().connection_id;
    for round in 1..=2 {
        if round == 1 {
            let before = observer.security_status().unwrap();
            println!("CCMP_QA GROUP_PLAINTEXT");
            wait("plaintext group consumed", || {
                let s = observer.security_status().unwrap();
                s.rx_sequence > before.rx_sequence && s.eapol_pending == 0
            });
            thread::sleep(Duration::from_millis(100));
            let after = observer.security_status().unwrap();
            assert_eq!(after.keys_pending, 0);
            assert_eq!(after.key_revision, before.key_revision);
            assert_eq!(observer.status().unwrap().phase, AUTHORIZED);
        }
        let before = observer.ccmp_status().unwrap();
        let revision = observer.security_status().unwrap().key_revision;
        println!("CCMP_QA GROUP_BEGIN {}", round);
        wait("group install pending", || {
            observer.security_status().unwrap().keys_pending == 1
        });
        assert_eq!(observer.status().unwrap().phase, AUTHORIZED);
        assert!(up());
        assert_eq!(generation(), initial_generation);
        println!("CCMP_QA GROUP_PENDING {}", round);
        receive(
            socket,
            if round == 1 {
                b"GROUP_OLD1"
            } else {
                b"GROUP_OLD2"
            },
        );
        echo(
            socket,
            to,
            if round == 1 {
                b"DURING_GROUP1"
            } else {
                b"DURING_GROUP2"
            },
        );
        assert_eq!(observer.security_status().unwrap().keys_pending, 1);
        wait("group cutover", || {
            observer.security_status().unwrap().keys_pending == 0
                && observer.security_status().unwrap().key_revision != revision
        });
        assert_eq!(observer.status().unwrap().phase, AUTHORIZED);
        assert_eq!(observer.security_status().unwrap().keys_installed, 2);
        assert_eq!(observer.identity().connection_id, initial_connection);
        assert_eq!(generation(), initial_generation);
        assert_eq!(config().ip_address, [10, 0, 2, 15]);
        println!("CCMP_QA GROUP_COMMITTED {}", round);
        receive(
            socket,
            if round == 1 {
                b"GROUP_NEW1"
            } else {
                b"GROUP_NEW2"
            },
        );
        quiet(socket);
        let after = observer.ccmp_status().unwrap();
        assert!(after.tx_pn > before.tx_pn);
        assert!(after.replay_dropped >= before.replay_dropped + 2);
        assert!(after.integrity_dropped > before.integrity_dropped);
        let revision = observer.security_status().unwrap().key_revision;
        println!("CCMP_QA GROUP_RETRY {}", round);
        thread::sleep(Duration::from_millis(500));
        quiet(socket);
        let retry = observer.ccmp_status().unwrap();
        assert_eq!(observer.security_status().unwrap().key_revision, revision);
        assert_eq!(retry.keys, 2);
        assert!(retry.tx_pn > after.tx_pn);
        assert!(retry.replay_dropped > after.replay_dropped);
        assert_eq!(generation(), initial_generation);
        echo(
            socket,
            to,
            if round == 1 {
                b"AFTER_GROUP1"
            } else {
                b"AFTER_GROUP2"
            },
        );
    }
    println!("CCMP_QA GROUP_ROLLBACK");
    wait("retired GTK rollback rejection", || {
        observer.status().unwrap().phase == IDLE && !up() && config().ip_set == 0
    });
    assert_eq!(observer.ccmp_status().unwrap().keys, 0);
}
#[unsafe(no_mangle)]
fn main() -> i32 {
    start("/bin/netcfgd", &["netcfgd"]);
    let rekey = File::open("/etc/wpa-rekey-qa").is_ok();
    let mut observer = WirelessSession::open("/dev/wifi0").unwrap();
    assert_ne!(observer.capabilities().unwrap().features & FEATURE_CCMP, 0);
    let port: u16 = text("/etc/wpa-qa-port").trim().parse().unwrap();
    let to = SocketAddress::Inet(Inet4SocketAddress::new([10, 0, 2, 2], port));
    let socket = udp();
    let pid = start(
        "/bin/wpa-supplicant",
        &["wpa-supplicant", "/dev/wifi0", "/etc/wpa-good.conf"],
    );
    associated(&mut observer, "first");
    assert!(socket.sendto(b"BLOCKED_INITIAL", &to).is_err());
    authenticated(&mut observer);
    echo(&socket, &to, b"CCMP_GOOD1");
    println!("CCMP_QA INJECT");
    let mut bytes = [0; 128];
    for expected in [
        b"RX_NQ".as_slice(),
        b"RX_Q0",
        b"RX_Q7",
        b"RX_GROUP",
        b"RX_AFTER_BAD",
        b"RX_Q8",
    ] {
        wait("valid RX probe", || match socket.recvfrom(&mut bytes) {
            Ok((n, _)) => {
                assert_eq!(&bytes[..n], expected);
                true
            }
            Err(_) => false,
        });
    }
    quiet(&socket);
    let state = observer.ccmp_status().unwrap();
    assert_eq!(state.keys, 2);
    assert!(state.replay_dropped >= 5);
    assert!(state.integrity_dropped >= 2);
    let revision = observer.security_status().unwrap().key_revision;
    println!("CCMP_QA DUPLICATE {} {}", state.tx_pn, state.largest_rx_pn);
    thread::sleep(Duration::from_millis(500));
    let after = observer.ccmp_status().unwrap();
    assert_eq!(observer.security_status().unwrap().key_revision, revision);
    assert_eq!(after.tx_pn, state.tx_pn);
    assert_eq!(after.largest_rx_pn, state.largest_rx_pn);
    assert!(after.replay_dropped > state.replay_dropped);
    quiet(&socket);
    echo(&socket, &to, b"CCMP_GOOD2");
    assert!(observer.ccmp_status().unwrap().tx_pn > state.tx_pn);
    if rekey {
        group_tests(&mut observer, &socket, &to);
    }
    stop(pid, &mut observer);
    let pid = start(
        "/bin/wpa-supplicant",
        &["wpa-supplicant", "/dev/wifi0", "/etc/wpa-good.conf"],
    );
    associated(&mut observer, "second");
    authenticated(&mut observer);
    let before = observer.ccmp_status().unwrap();
    println!("CCMP_QA OLD_KEY");
    thread::sleep(Duration::from_millis(500));
    quiet(&socket);
    assert!(observer.ccmp_status().unwrap().integrity_dropped > before.integrity_dropped);
    echo(&socket, &to, b"CCMP_GOOD3");
    let old = observer.identity().connection_id;
    println!("CCMP_QA CARRIER_READY");
    wait("carrier retirement", || {
        observer.status().unwrap().phase == IDLE && !up() && config().ip_set == 0
    });
    assert_eq!(observer.ccmp_status().unwrap().keys, 0);
    assert!(socket.sendto(b"BLOCKED_DOWN", &to).is_err());
    let accepted = observer.ccmp_status().unwrap().rx_accepted;
    println!("CCMP_QA CARRIER_DOWN");
    thread::sleep(Duration::from_millis(400));
    assert_eq!(observer.ccmp_status().unwrap().rx_accepted, accepted);
    println!("CCMP_QA CARRIER_UP");
    wait("automatic reconnect", || {
        observer.status().unwrap().phase == ASSOCIATED && observer.identity().connection_id != old
    });
    associated(&mut observer, "automatic");
    authenticated(&mut observer);
    echo(&socket, &to, b"CCMP_GOOD4");
    stop(pid, &mut observer);
    // A separate explicitly scripted-key phase exercises held software TX/RX
    // retirement. The earlier phases use the actual WPA daemon exclusively.
    observer.claim_controller().unwrap();
    manual(&mut observer, "held-old", 0);
    echo(&socket, &to, b"HOLD_BASE");
    let pn = observer.ccmp_status().unwrap().tx_pn;
    observer.data_test(DATA_HOLD).unwrap();
    socket.sendto(b"STALE_TX", &to).unwrap();
    println!("CCMP_QA HOLD_RX");
    wait("held TX/RX", || {
        let stats = observer.data_test(DATA_STATS).unwrap();
        stats.tx_queued > 0 && stats.rx_queued > 0
    });
    assert_eq!(observer.ccmp_status().unwrap().tx_pn, pn);
    observer.disconnect(1000).unwrap();
    wait("held connection retirement", || {
        observer.status().unwrap().phase == IDLE && config().ip_set == 0 && !up()
    });
    assert_eq!(observer.ccmp_status().unwrap().keys, 0);
    let counters = observer.data_test(DATA_STATS).unwrap();
    assert_eq!((counters.tx_queued, counters.rx_queued), (0, 0));
    manual(&mut observer, "held-new", 42);
    echo(&socket, &to, b"HOLD_FRESH");
    let before = observer.data_test(DATA_STATS).unwrap();
    let crypto = observer.ccmp_status().unwrap();
    let after = observer.data_test(DATA_REPLAY_STALE).unwrap();
    assert!(after.tx_dropped > before.tx_dropped && after.rx_dropped > before.rx_dropped);
    assert_eq!(after.tx_submitted, before.tx_submitted);
    assert_eq!(
        observer.ccmp_status().unwrap().rx_accepted,
        crypto.rx_accepted
    );
    quiet(&socket);
    echo(&socket, &to, b"HOLD_FINAL");
    observer.release_controller().unwrap();
    wait("final retirement", || {
        observer.status().unwrap().phase == IDLE && !up() && config().ip_set == 0
    });
    assert_eq!(observer.ccmp_status().unwrap().keys, 0);
    if rekey {
        let pid = start(
            "/bin/wpa-supplicant",
            &["wpa-supplicant", "/dev/wifi0", "/etc/wpa-good.conf"],
        );
        associated(&mut observer, "bad-group");
        authenticated(&mut observer);
        println!("CCMP_QA GROUP_BAD_MIC");
        wait("protected group MIC failure", || {
            observer.status().unwrap().phase == IDLE && !up() && config().ip_set == 0
        });
        assert_eq!(observer.ccmp_status().unwrap().keys, 0);
        stop(pid, &mut observer);
    }
    println!("CCMP_QA PASS");
    std::task::shutdown(std::task::ShutdownType::PowerOff);
}
