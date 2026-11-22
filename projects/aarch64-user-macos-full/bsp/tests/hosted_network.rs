use scarlet::{
    device::manager::DeviceManager,
    hosted,
    interrupt::InterruptManager,
    network::{
        NetworkManager,
        config::configure_interface_ipv4,
        ipv4::Ipv4Address,
        socket::{Inet4SocketAddress, SocketAddress, SocketControl, SocketObject},
        tcp::TcpLayer,
        udp::UdpLayer,
    },
    object::capability::StreamOps,
    sched::scheduler,
    task,
    timer::get_kernel_timer,
};
use scarlet_aarch64_user_macos_full_bsp::network;
use std::{
    env,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, UdpSocket},
    process::{self, Command},
    sync::OnceLock,
    thread,
    time::Duration,
};

static HOST_PEER: OnceLock<SocketAddr> = OnceLock::new();
const PROBE: &[u8] = b"scarlet-hosted-ethernet";
const TRANSACTION: [u8; 4] = [0x39, 0xa1, 0xe7, 0x42];

#[test]
#[ignore = "requires macOS libslirp and an active IPv4 route"]
fn scarlet_udp_dhcp_and_tcp_use_hosted_ethernet() {
    const CHILD: &str = "SCARLET_HOSTED_NETWORK_TEST_CHILD";
    if env::var_os(CHILD).is_some() {
        run_kernel();
    }
    let status = Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "scarlet_udp_dhcp_and_tcp_use_hosted_ethernet",
            "--ignored",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .status()
        .unwrap();
    assert!(
        status.success(),
        "Hosted network child exited with {status}"
    );
}

fn run_kernel() -> ! {
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(15));
        eprintln!("Hosted network test did not complete");
        process::exit(1);
    });
    // Select the local IPv4 address using routing without sending a datagram.
    // Bind the echo peer there because the NAT disables host loopback access.
    let route = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
    route.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).unwrap();
    let listener = TcpListener::bind((route.local_addr().unwrap().ip(), 0)).unwrap();
    HOST_PEER.set(listener.local_addr().unwrap()).unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut received = [0; PROBE.len()];
        stream.read_exact(&mut received).unwrap();
        assert_eq!(received, PROBE);
        stream.write_all(&received).unwrap();
    });

    let mut platform = hosted::prepare(2, 256 * 1024 * 1024).unwrap();
    let init = task::new_user_task("init".into(), 0);
    init.init();
    assert_eq!(scheduler::register_task(init), 1);
    let interrupts = InterruptManager::global();
    interrupts.init_controllers();
    interrupts.init_controllers_for_cpu(0);
    NetworkManager::init();
    network::link_boot_ethernet();
    scarlet::initcall::call_initcalls();
    scarlet::interrupt::enable_cpu_interrupts();
    get_kernel_timer().init(0);
    let test = task::new_kernel_task("hosted-network-test".into(), 0, exercise_network);
    test.set_pinned_cpu(Some(0));
    test.init();
    let id = scheduler::register_task(test);
    scheduler::register_online_cpu(0);
    scheduler::spawn_idle_task(0);
    scheduler::enqueue_task(id, 0);
    let first = scheduler::start_scheduler().unwrap();
    platform.start_secondary_cpus().unwrap();
    scheduler::first_switch_to_kernel_task(first);
}

fn exercise_network() {
    let network = NetworkManager::get_manager();
    let device = DeviceManager::get_manager()
        .get_device_by_name("eth0")
        .unwrap();
    let device = device.as_network_device().unwrap();
    assert_eq!(device.get_mtu().unwrap(), 1500);
    assert!(device.is_link_up());
    let mac = *device.get_mac_address().unwrap().as_bytes();

    let udp = network.get_layer("udp").unwrap();
    let udp = udp.as_any().downcast_ref::<UdpLayer>().unwrap();
    let dhcp = udp.create_socket();
    dhcp.bind_interface("eth0").unwrap();
    dhcp.bind(&inet([0; 4], 68)).unwrap();
    let dhcp_server = inet([255; 4], 67);
    let discover = dhcp_request(mac, &[53, 1, 1, 255]);
    assert_eq!(
        dhcp.sendto(&discover, &dhcp_server, 0).unwrap(),
        discover.len()
    );

    let mut reply = [0; 576];
    let (count, _) = dhcp.recvfrom(&mut reply, 0).unwrap();
    assert_eq!(option(&reply[..count], 53), Some(&[2][..]));
    let offered: [u8; 4] = reply[16..20].try_into().unwrap();
    let server: [u8; 4] = option(&reply[..count], 54).unwrap().try_into().unwrap();
    assert_eq!(offered, [10, 0, 2, 15]);
    assert_eq!(server, [10, 0, 2, 2]);

    let mut options = vec![53, 1, 3, 50, 4];
    options.extend_from_slice(&offered);
    options.extend_from_slice(&[54, 4]);
    options.extend_from_slice(&server);
    options.push(255);
    let request = dhcp_request(mac, &options);
    dhcp.sendto(&request, &dhcp_server, 0).unwrap();
    let (count, _) = dhcp.recvfrom(&mut reply, 0).unwrap();
    let ack = &reply[..count];
    assert_eq!(option(ack, 53), Some(&[5][..]));
    assert_eq!(ack[16..20], offered);
    assert_eq!(option(ack, 6), Some(&[10, 0, 2, 3][..]));
    configure_interface_ipv4(
        "eth0",
        Ipv4Address::from_bytes(offered),
        Ipv4Address::from_bytes(option(ack, 1).unwrap().try_into().unwrap()),
        Some(Ipv4Address::from_bytes(
            option(ack, 3).unwrap().try_into().unwrap(),
        )),
        0,
        true,
    )
    .unwrap();

    let tcp = network.get_layer("tcp").unwrap();
    let tcp = tcp
        .as_any()
        .downcast_ref::<TcpLayer>()
        .unwrap()
        .create_socket();
    let SocketAddr::V4(peer) = *HOST_PEER.get().unwrap() else {
        panic!("Expected an IPv4 TCP peer");
    };
    assert_ne!(IpAddr::V4(*peer.ip()), IpAddr::V4(Ipv4Addr::LOCALHOST));
    tcp.connect(&inet(peer.ip().octets(), peer.port())).unwrap();
    assert_eq!(tcp.write(PROBE).unwrap(), PROBE.len());
    let mut echo = [0; PROBE.len()];
    let mut read = 0;
    while read < echo.len() {
        let size = tcp.read(&mut echo[read..]).unwrap();
        assert_ne!(size, 0);
        read += size;
    }
    assert_eq!(echo, PROBE);
    assert!(device.get_stats().tx_packets > 2);
    assert!(device.get_stats().rx_packets > 2);
    assert!(InterruptManager::global().statistics().iter().any(|irq| {
        irq.interrupt_id == scarlet_aarch64_user_macos_full_bsp::network::COMPLETION_IRQ
            && irq.deliveries > 2
    }));
    process::exit(0);
}

fn inet(ip: [u8; 4], port: u16) -> SocketAddress {
    SocketAddress::Inet(Inet4SocketAddress::new(ip, port))
}

fn dhcp_request(mac: [u8; 6], options: &[u8]) -> Vec<u8> {
    let mut request = vec![0; 240];
    request[..3].copy_from_slice(&[1, 1, 6]);
    request[4..8].copy_from_slice(&TRANSACTION);
    request[10] = 0x80;
    request[28..34].copy_from_slice(&mac);
    request[236..240].copy_from_slice(&[99, 130, 83, 99]);
    request.extend_from_slice(options);
    request.resize(request.len().max(300), 0);
    request
}

fn option(reply: &[u8], wanted: u8) -> Option<&[u8]> {
    assert_eq!(reply[4..8], TRANSACTION);
    assert_eq!(reply[236..240], [99, 130, 83, 99]);
    let mut options = &reply[240..];
    while let Some((&kind, rest)) = options.split_first() {
        options = rest;
        match kind {
            0 => continue,
            255 => break,
            _ => {}
        }
        let (&length, rest) = options.split_first()?;
        let (value, trailing) = rest.split_at_checked(length as usize)?;
        if kind == wanted {
            return Some(value);
        }
        options = trailing;
    }
    None
}
