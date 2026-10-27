#![no_std]
#![no_main]
extern crate scarlet_std as std;
use scarlet_abi::network::{IPV4_HAS_GATEWAY, IPV4_MAKE_DEFAULT, LINK_KIND_WIFI};
use scarlet_os::{
    network::*,
    socket::{
        DatagramOps, Inet4SocketAddress, Socket, SocketAddress, SocketDomain, SocketProtocol,
        SocketType,
    },
    wireless::*,
};
use std::{fs::File, println, string::String, thread, time::Duration};

fn wait(label: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..300 {
        if condition() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("wireless data timed out: {}", label);
}
fn text(path: &str) -> String {
    let mut bytes = [0; 1024];
    let count = File::open(path)
        .and_then(|mut f| f.read(&mut bytes))
        .unwrap_or(0);
    String::from_utf8_lossy(&bytes[..count]).into_owned()
}
fn config(name: &str) -> NetworkInterfaceConfig {
    list_interface_configs()
        .unwrap()
        .into_iter()
        .find(|c| c.interface_name() == Some(name))
        .unwrap()
}
fn link() -> scarlet_abi::network::NetworkLinkInfoV1 {
    list_links()
        .unwrap()
        .into_iter()
        .find(|c| c.interface_name() == Some("veth0"))
        .unwrap()
}
fn completed(s: &mut WirelessSession, id: u64) {
    wait("request", || s.request(id).unwrap().result != ACCEPTED);
    assert_eq!(s.request(id).unwrap().result, OK);
    s.acknowledge(id).unwrap();
    s.resync().unwrap();
}
fn associate(s: &mut WirelessSession) -> u64 {
    let id = s.connect_external(b"ScarletMock", 15000).unwrap();
    wait("associated", || s.status().unwrap().phase == ASSOCIATED);
    assert_eq!(s.request(id).unwrap().result, ACCEPTED);
    assert!(!link().is_up());
    assert_eq!(config("veth0").ip_set, 0);
    id
}
fn install(s: &mut WirelessSession, kind: u32, version: u64, value: u8) -> u64 {
    let peer = s.security_status().unwrap().peer;
    let mut key = KeyInstall {
        version,
        cipher: CIPHER_CCMP128,
        kind,
        index: if kind == KEY_PAIRWISE { 0 } else { 1 },
        key_len: 16,
        peer: if kind == KEY_PAIRWISE { peer } else { [255; 6] },
        ..KeyInstall::default()
    };
    key.key[..16].fill(value);
    let id = s.install_key(&key).unwrap();
    assert_eq!(s.request(id).unwrap().result, ACCEPTED);
    let state = s.security_status().unwrap();
    assert_eq!(
        s.set_controlled_port(true, state.key_revision),
        Err(WirelessError::Operation(BUSY))
    );
    let h = s.identity();
    s.mock_step(id, h.connection_id, h.device_epoch, STEP_SUCCESS)
        .unwrap();
    completed(s, id);
    id
}
fn authorize(s: &mut WirelessSession, id: u64) {
    let state = s.security_status().unwrap();
    assert_eq!(
        s.mock_security_stats().unwrap().installed,
        0,
        "old firmware keys survived reconnect"
    );
    assert_eq!(state.keys_installed, 0);
    assert_eq!(state.eapol_pending, 0, "old EAPOL survived reconnect");
    assert_eq!(
        s.set_controlled_port(true, state.key_revision),
        Err(WirelessError::Operation(BUSY))
    );
    let h = s.identity();
    // A scripted CONNECT completion cannot authorize an external authenticator.
    s.mock_step(id, h.connection_id, h.device_epoch, STEP_SUCCESS)
        .unwrap();
    thread::sleep(Duration::from_millis(20));
    assert_eq!(s.status().unwrap().phase, ASSOCIATED);
    let mut pdu = [2, 3, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0];
    pdu[4..].copy_from_slice(&h.connection_id.to_le_bytes());
    s.transmit_eapol(state.peer, &pdu).unwrap();
    let mut got = false;
    wait("EAPOL reply", || {
        if let Some(frame) = s.receive_eapol().unwrap() {
            assert_eq!(frame.peer, state.peer);
            assert_eq!(&frame.payload[..frame.length as usize], &pdu);
            got = true;
            true
        } else {
            false
        }
    });
    assert!(got);
    assert!(!link().is_up());
    let old_revision = s.security_status().unwrap().key_revision;
    install(s, KEY_PAIRWISE, 1, 0x11);
    assert_eq!(
        s.set_controlled_port(true, old_revision),
        Err(WirelessError::Operation(STATE_CHANGED))
    );
    install(s, KEY_GROUP, 1, 0x22);
    let state = s.security_status().unwrap();
    assert_eq!(state.keys_installed, 2);
    assert_eq!(s.mock_security_stats().unwrap().installed, 2);
    s.set_controlled_port(true, state.key_revision).unwrap();
    completed(s, id);
    assert!(link().is_up());
    wait_dhcp();
    println!("WIRELESS_AUTH EAPOL_KEYS_AUTHORIZED {}", h.connection_id);
}
fn wait_dhcp() {
    wait("dhcp", || {
        let c = config("veth0");
        c.ip_set == 1
            && c.ip_address == [10, 0, 2, 15]
            && c.gateway == [10, 0, 2, 2]
            && c.is_default == 1
            && text("/etc/resolv.conf").matches("nameserver").count() == 2
    });
}
fn disconnected(s: &mut WirelessSession) {
    let id = s.disconnect(5000).unwrap();
    // The controlled port closes and configuration retires at submission.
    assert!(!link().is_up());
    let c = config("veth0");
    assert_eq!((c.ip_set, c.gateway_set, c.is_default), (0, 0, 0));
    assert_eq!(config("veth1").is_default, 1);
    completed(s, id);
    assert_eq!(s.security_status().unwrap().keys_installed, 0);
    let keys = s.mock_security_stats().unwrap();
    assert_eq!((keys.installed, keys.pending), (0, 0));
}
fn socket() -> Socket {
    let socket = Socket::new_with_domain(
        SocketDomain::Inet4,
        SocketType::Datagram,
        SocketProtocol::Udp,
    )
    .unwrap();
    socket.bind_interface("veth0").unwrap();
    socket
        .bind_inet(Inet4SocketAddress::new([0; 4], 40002))
        .unwrap();
    socket.set_nonblocking(true).unwrap();
    socket
}
fn exchange(socket: &Socket, destination: &SocketAddress, message: &[u8]) {
    socket.sendto(message, destination).unwrap();
    let mut bytes = [0; 128];
    wait("UDP echo", || match socket.recvfrom(&mut bytes) {
        Ok((n, _)) => {
            assert_eq!(&bytes[..n], message);
            true
        }
        Err(_) => false,
    });
}
#[unsafe(no_mangle)]
fn main() -> i32 {
    let port: u16 = text("/etc/wireless-data-port").trim().parse().unwrap();
    let destination = SocketAddress::Inet(Inet4SocketAddress::new([10, 0, 2, 2], port));
    let pid = std::task::fork();
    assert!(pid >= 0);
    if pid == 0 {
        let result = std::task::execve("/bin/netcfgd", &["netcfgd"], &[]);
        panic!("netcfgd exec {}", result);
    }
    let mut s = WirelessSession::open("/dev/wifi0").unwrap();
    assert_eq!(
        s.capabilities().unwrap().features & (FEATURE_MOCK | FEATURE_DATA),
        FEATURE_MOCK | FEATURE_DATA
    );
    s.claim_controller().unwrap();
    s.subscribe().unwrap();
    assert_eq!(link().kind, LINK_KIND_WIFI);
    wait("fallback DHCP", || {
        config("veth1").ip_set == 1 && text("/etc/resolv.conf").matches("nameserver").count() == 1
    });
    assert_eq!(config("veth0").ip_set, 0);
    let id = associate(&mut s);
    thread::sleep(Duration::from_millis(1200));
    assert!(!link().is_up());
    assert_eq!(config("veth0").ip_set, 0);
    // Even a legacy static address cannot bypass the controlled port.
    configure_interface_ipv4(
        "veth0",
        Ipv4Address([10, 0, 2, 99]),
        Ipv4Address([255, 255, 255, 0]),
        Some(Ipv4Address([10, 0, 2, 2])),
        10,
        false,
    )
    .unwrap();
    let socket = socket();
    assert!(socket.sendto(b"BLOCKED", &destination).is_err());
    println!("WIRELESS_AUTH PREAUTH_BLOCKED");
    authorize(&mut s, id);
    exchange(&socket, &destination, b"LIVE1");
    println!("WIRELESS_AUTH AUTHORIZED_DHCP");
    // Removing an active key closes data before backend confirmation.
    let peer = s.security_status().unwrap().peer;
    let mut duplicate = WirelessSession::from_handle(s.handle().duplicate().unwrap()).unwrap();
    let mut duplicate_port = WirelessSession::from_handle(s.handle().duplicate().unwrap()).unwrap();
    let deleted = s
        .delete_key(KeyRef {
            kind: KEY_PAIRWISE,
            index: 0,
            version: 1,
        })
        .unwrap();
    assert!(!link().is_up());
    assert_eq!(config("veth0").ip_set, 0);
    assert!(socket.sendto(b"REKEY_BLOCKED", &destination).is_err());
    let h = s.identity();
    s.mock_step(deleted, h.connection_id, h.device_epoch, STEP_SUCCESS)
        .unwrap();
    completed(&mut s, deleted);
    let revision = s.security_status().unwrap().key_revision;
    assert_eq!(
        s.set_controlled_port(true, revision),
        Err(WirelessError::Operation(BUSY))
    );
    // Deleted material cannot be reinstalled to reset receive replay state.
    let mut reused = KeyInstall {
        version: 2,
        cipher: CIPHER_CCMP128,
        kind: KEY_PAIRWISE,
        index: 0,
        key_len: 16,
        peer,
        ..KeyInstall::default()
    };
    reused.key[..16].fill(0x11);
    assert_eq!(
        s.install_key(&reused),
        Err(WirelessError::Operation(STATE_CHANGED))
    );
    install(&mut s, KEY_PAIRWISE, 2, 0x33);
    let revision = s.security_status().unwrap().key_revision;
    s.set_controlled_port(true, revision).unwrap();
    wait_dhcp();
    exchange(&socket, &destination, b"REKEY");
    println!("WIRELESS_AUTH KEY_DELETE_REPLACE");
    let old_connection = s.identity().connection_id;
    let old_epoch = s.identity().device_epoch;
    let original = link();
    let stale = NetworkUpdateIpv4V1 {
        id: original.id,
        generation: original.generation,
        address: [10, 0, 2, 99],
        netmask: [255, 255, 255, 0],
        gateway: [10, 0, 2, 2],
        metric: 10,
        flags: IPV4_HAS_GATEWAY | IPV4_MAKE_DEFAULT,
        reserved: 0,
    };
    s.data_test(DATA_HOLD).unwrap();
    socket.sendto(b"OLD_TX", &destination).unwrap();
    // Real unresolved-neighbour queue, in addition to the held device queues.
    let _ = socket.sendto(
        b"OLD_ARP",
        &SocketAddress::Inet(Inet4SocketAddress::new([10, 0, 2, 250], port)),
    );
    println!("WIRELESS_AUTH HOLD");
    wait("held TX RX ARP", || {
        let c = s.data_test(DATA_STATS).unwrap();
        c.tx_queued > 0 && c.rx_queued > 0 && c.arp_queued > 0
    });
    // Leave one control frame queued; it must not cross the connection boundary.
    s.transmit_eapol(
        peer,
        &[2, 3, 0, 8, b'E', b'A', b'P', b'_', b'O', b'L', b'D', b'!'],
    )
    .unwrap();
    wait("old EAPOL queued", || {
        s.security_status().unwrap().eapol_pending > 0
    });
    let before = s.data_test(DATA_STATS).unwrap();
    disconnected(&mut s);
    let after = s.data_test(DATA_STATS).unwrap();
    assert_eq!(
        (after.tx_queued, after.rx_queued, after.arp_queued),
        (0, 0, 0)
    );
    assert!(after.tx_dropped > before.tx_dropped && after.rx_dropped > before.rx_dropped);
    assert!(update_link_ipv4(&stale).is_err());
    wait("DNS fallback", || {
        text("/etc/resolv.conf").matches("nameserver").count() == 1
    });
    assert_eq!(config("veth1").is_default, 1);
    println!("WIRELESS_AUTH RETIRED");
    println!("WIRELESS_AUTH PREAUTH_RX");
    // Host capture independently confirms the injected frame reached virtio.
    thread::sleep(Duration::from_millis(300));
    let mut closed_bytes = [0; 128];
    assert!(socket.recvfrom(&mut closed_bytes).is_err());
    let id = associate(&mut s);
    authorize(&mut s, id);
    assert!(link().generation > original.generation);
    assert_eq!(
        duplicate_port.set_controlled_port(true, revision),
        Err(WirelessError::Operation(STALE_CONNECTION))
    );
    assert_eq!(
        duplicate.install_key(&reused),
        Err(WirelessError::Operation(STALE_CONNECTION))
    );
    drop(duplicate);
    drop(duplicate_port);
    s.mock_step(2, old_connection, old_epoch, STEP_SUCCESS)
        .unwrap();
    thread::sleep(Duration::from_millis(20));
    assert_eq!(s.mock_security_stats().unwrap().installed, 2);
    assert!(update_link_ipv4(&stale).is_err());
    let before = s.data_test(DATA_STATS).unwrap();
    let after = s.data_test(DATA_REPLAY_STALE).unwrap();
    assert!(after.tx_dropped > before.tx_dropped && after.rx_dropped > before.rx_dropped);
    let mut bytes = [0; 128];
    assert!(socket.recvfrom(&mut bytes).is_err());
    exchange(&socket, &destination, b"LIVE2");
    println!("WIRELESS_AUTH STALE_REJECTED");
    // Short down/up interval must still force a new lease acquisition.
    disconnected(&mut s);
    let id = associate(&mut s);
    authorize(&mut s, id);
    exchange(&socket, &destination, b"LIVE3");
    println!("WIRELESS_AUTH RAPID_RECONNECT");
    println!("WIRELESS_AUTH CARRIER_READY");
    wait("carrier loss", || {
        s.status().unwrap().phase == IDLE && !link().is_up() && config("veth0").ip_set == 0
    });
    assert_eq!(config("veth0").gateway_set, 0);
    assert!(update_link_ipv4(&stale).is_err());
    assert_eq!(s.mock_security_stats().unwrap().installed, 0);
    println!("WIRELESS_AUTH CARRIER_DOWN");
    thread::sleep(Duration::from_millis(300));
    let id = associate(&mut s);
    authorize(&mut s, id);
    exchange(&socket, &destination, b"LIVE4");
    println!("WIRELESS_AUTH CARRIER_RECOVERED");
    let epoch = s.identity().device_epoch;
    s.mock_step(0, 0, epoch, STEP_RESET).unwrap();
    wait("reset retirement", || {
        s.status().unwrap().phase == IDLE && !link().is_up() && config("veth0").ip_set == 0
    });
    assert!(s.identity().device_epoch > epoch);
    assert_eq!(s.mock_security_stats().unwrap().installed, 0);
    let id = associate(&mut s);
    authorize(&mut s, id);
    exchange(&socket, &destination, b"LIVE5");
    println!("WIRELESS_AUTH RESET_RECOVERED");
    // Explicit port revocation retires data, preserves same-connection keys,
    // and invalidates old authorization revisions.
    let revision = s.security_status().unwrap().key_revision;
    s.set_controlled_port(false, revision).unwrap();
    assert!(!link().is_up());
    assert_eq!(config("veth0").ip_set, 0);
    assert!(socket.sendto(b"PORT_CLOSED", &destination).is_err());
    assert_eq!(
        s.set_controlled_port(true, revision),
        Err(WirelessError::Operation(STATE_CHANGED))
    );
    let revision = s.security_status().unwrap().key_revision;
    s.set_controlled_port(true, revision).unwrap();
    wait_dhcp();
    exchange(&socket, &destination, b"PORT_REOPEN");
    println!("WIRELESS_AUTH PORT_REVOKE_REOPEN");
    // Closing the controller also cancels a not-yet-confirmed key install.
    let mut pending = KeyInstall {
        version: 2,
        cipher: CIPHER_CCMP128,
        kind: KEY_GROUP,
        index: 1,
        key_len: 16,
        peer: [255; 6],
        ..KeyInstall::default()
    };
    pending.key[..16].fill(0x44);
    let orphan = s.install_key(&pending).unwrap();
    let old = s.identity();
    assert_eq!(s.mock_security_stats().unwrap().pending, 1);
    drop(s);
    assert!(!link().is_up());
    assert_eq!(
        (config("veth0").ip_set, config("veth0").gateway_set),
        (0, 0)
    );
    let mut observer = WirelessSession::open("/dev/wifi0").unwrap();
    observer.claim_controller().unwrap();
    let stats = observer.mock_security_stats().unwrap();
    assert_eq!((stats.installed, stats.pending), (0, 0));
    assert!(stats.retired >= 10);
    observer
        .mock_step(orphan, old.connection_id, old.device_epoch, STEP_SUCCESS)
        .unwrap();
    thread::sleep(Duration::from_millis(20));
    assert_eq!(observer.mock_security_stats().unwrap().installed, 0);
    println!("WIRELESS_AUTH PASS");
    std::task::shutdown(std::task::ShutdownType::PowerOff);
}
