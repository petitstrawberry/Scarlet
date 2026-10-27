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
    let id = s.connect_open(b"ScarletMock", 15000).unwrap();
    wait("associated", || s.status().unwrap().phase == ASSOCIATED);
    assert_eq!(s.request(id).unwrap().result, ACCEPTED);
    assert!(!link().is_up());
    assert_eq!(config("veth0").ip_set, 0);
    id
}
fn authorize(s: &mut WirelessSession, id: u64) {
    let h = s.identity();
    s.mock_step(id, h.connection_id, h.device_epoch, STEP_SUCCESS)
        .unwrap();
    completed(s, id);
    assert!(link().is_up());
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
    println!("WIRELESS_DATA PREAUTH_BLOCKED");
    authorize(&mut s, id);
    exchange(&socket, &destination, b"LIVE1");
    println!("WIRELESS_DATA AUTHORIZED_DHCP");
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
    println!("WIRELESS_DATA HOLD");
    wait("held TX RX ARP", || {
        let c = s.data_test(DATA_STATS).unwrap();
        c.tx_queued > 0 && c.rx_queued > 0 && c.arp_queued > 0
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
    println!("WIRELESS_DATA RETIRED");
    println!("WIRELESS_DATA PREAUTH_RX");
    // Host capture independently confirms the injected frame reached virtio.
    thread::sleep(Duration::from_millis(300));
    let mut closed_bytes = [0; 128];
    assert!(socket.recvfrom(&mut closed_bytes).is_err());
    let id = associate(&mut s);
    authorize(&mut s, id);
    assert!(link().generation > original.generation);
    assert!(update_link_ipv4(&stale).is_err());
    let before = s.data_test(DATA_STATS).unwrap();
    let after = s.data_test(DATA_REPLAY_STALE).unwrap();
    assert!(after.tx_dropped > before.tx_dropped && after.rx_dropped > before.rx_dropped);
    let mut bytes = [0; 128];
    assert!(socket.recvfrom(&mut bytes).is_err());
    exchange(&socket, &destination, b"LIVE2");
    println!("WIRELESS_DATA STALE_REJECTED");
    // Short down/up interval must still force a new lease acquisition.
    disconnected(&mut s);
    let id = associate(&mut s);
    authorize(&mut s, id);
    exchange(&socket, &destination, b"LIVE3");
    println!("WIRELESS_DATA RAPID_RECONNECT");
    println!("WIRELESS_DATA CARRIER_READY");
    wait("carrier loss", || {
        s.status().unwrap().phase == IDLE && !link().is_up() && config("veth0").ip_set == 0
    });
    assert_eq!(config("veth0").gateway_set, 0);
    assert!(update_link_ipv4(&stale).is_err());
    println!("WIRELESS_DATA CARRIER_DOWN");
    thread::sleep(Duration::from_millis(300));
    let id = associate(&mut s);
    authorize(&mut s, id);
    exchange(&socket, &destination, b"LIVE4");
    println!("WIRELESS_DATA CARRIER_RECOVERED");
    let epoch = s.identity().device_epoch;
    s.mock_step(0, 0, epoch, STEP_RESET).unwrap();
    wait("reset retirement", || {
        s.status().unwrap().phase == IDLE && !link().is_up() && config("veth0").ip_set == 0
    });
    assert!(s.identity().device_epoch > epoch);
    let id = associate(&mut s);
    authorize(&mut s, id);
    exchange(&socket, &destination, b"LIVE5");
    println!("WIRELESS_DATA RESET_RECOVERED");
    // Controller close retires the data link synchronously too.
    drop(s);
    assert!(!link().is_up());
    assert_eq!(
        (config("veth0").ip_set, config("veth0").gateway_set),
        (0, 0)
    );
    println!("WIRELESS_DATA PASS");
    std::task::shutdown(std::task::ShutdownType::PowerOff);
}
