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
use std::{fs::File, io::Write, println, string::String, thread, time::Duration};
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
    println!("WPA_QA ASSOC {} {}", label, connection);
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
#[unsafe(no_mangle)]
fn main() -> i32 {
    start("/bin/netcfgd", &["netcfgd"]);
    let mut observer = WirelessSession::open("/dev/wifi0").unwrap();
    assert_ne!(observer.capabilities().unwrap().features & FEATURE_RSN, 0);
    let mut random = [0u8; 32];
    let entropy = unsafe {
        scarlet_sys::syscall3(
            scarlet_sys::Syscall::GetRandom,
            random.as_mut_ptr() as usize,
            random.len(),
            scarlet_abi::GET_RANDOM_FLAG_REQUIRE_ENTROPY,
        )
    } == 32;
    let port: u16 = text("/etc/wpa-qa-port").trim().parse().unwrap();
    let to = SocketAddress::Inet(Inet4SocketAddress::new([10, 0, 2, 2], port));
    let socket = udp();
    if !entropy {
        println!("WPA_QA MODE NO_ENTROPY");
        let pid = start(
            "/bin/wpa-supplicant",
            &["wpa-supplicant", "/dev/wifi0", "/etc/wpa-good.conf"],
        );
        associated(&mut observer, "no-entropy");
        wait("entropy rejection", || {
            observer.status().unwrap().phase == IDLE
        });
        assert!(!up());
        assert_eq!(observer.security_status().unwrap().keys_installed, 0);
        assert!(socket.sendto(b"BLOCKED_NO_ENTROPY", &to).is_err());
        stop(pid, &mut observer);
        println!("WPA_QA NO_ENTROPY PASS");
        std::task::shutdown(std::task::ShutdownType::PowerOff);
    }
    println!("WPA_QA MODE NORMAL");
    let pid = start(
        "/bin/wpa-supplicant",
        &["wpa-supplicant", "/dev/wifi0", "/etc/wpa-good.conf"],
    );
    associated(&mut observer, "good");
    assert!(socket.sendto(b"BLOCKED_INITIAL", &to).is_err());
    authenticated(&mut observer);
    echo(&socket, &to, b"GOOD1");
    let revision = observer.security_status().unwrap().key_revision;
    println!("WPA_QA DUPLICATE");
    thread::sleep(Duration::from_millis(300));
    assert!(up());
    assert_eq!(observer.security_status().unwrap().key_revision, revision);
    println!("WPA_QA GOOD1");
    stop(pid, &mut observer);
    for label in ["bad-mic", "wrong-password", "timeout", "stale-replay"] {
        let profile = if label == "wrong-password" {
            "/etc/wpa-bad.conf"
        } else {
            "/etc/wpa-good.conf"
        };
        let pid = start(
            "/bin/wpa-supplicant",
            &["wpa-supplicant", "/dev/wifi0", profile],
        );
        associated(&mut observer, label);
        wait("failed authentication", || {
            observer.status().unwrap().phase == IDLE
        });
        assert!(!up());
        assert_eq!(config().ip_set, 0);
        assert_eq!(observer.security_status().unwrap().keys_installed, 0);
        assert!(socket.sendto(b"BLOCKED_AUTH_FAILURE", &to).is_err());
        stop(pid, &mut observer);
        println!("WPA_QA FAILURE {}", label);
    }
    let pid = start(
        "/bin/wpa-supplicant",
        &["wpa-supplicant", "/dev/wifi0", "/etc/wpa-good.conf"],
    );
    associated(&mut observer, "reconnect");
    authenticated(&mut observer);
    echo(&socket, &to, b"GOOD2");
    let old = observer.identity().connection_id;
    println!("WPA_QA CARRIER_READY");
    wait("carrier retirement", || {
        observer.status().unwrap().phase == IDLE && !up() && config().ip_set == 0
    });
    assert_eq!(observer.security_status().unwrap().keys_installed, 0);
    println!("WPA_QA CARRIER_DOWN");
    wait("daemon reconnect", || {
        observer.status().unwrap().phase == ASSOCIATED && observer.identity().connection_id != old
    });
    associated(&mut observer, "automatic");
    authenticated(&mut observer);
    echo(&socket, &to, b"GOOD3");
    stop(pid, &mut observer);
    // The resident service must wake on radio events and IPC, and stay asleep
    // without periodic status queries after authentication.
    File::create("/etc/wireless.conf")
        .unwrap()
        .write_all(text("/etc/wpa-good.conf").as_bytes())
        .unwrap();
    let pid = start("/bin/wirelessd", &["wirelessd"]);
    associated(&mut observer, "service");
    authenticated(&mut observer);
    echo(&socket, &to, b"GOOD4");
    use scarlet_os::wireless_service as api;
    wait("service connected", || {
        api::call(api::STATUS, &[], &[]).is_ok_and(|s| s.contains("STATE Connected\n"))
    });
    let cpu = || {
        std::task::info()
            .iter()
            .filter(|t| t.tgid() == pid as usize)
            .map(|t| t.cpu_time_ns())
            .sum::<u64>()
    };
    let before = cpu();
    thread::sleep(Duration::from_secs(2));
    let idle = cpu().saturating_sub(before);
    assert!(idle < 20_000_000, "wirelessd idle CPU: {}ns", idle);
    println!("WPA_QA SERVICE_IDLE cpu_ns={}", idle);
    api::call(api::DISCONNECT, &[], &[]).unwrap();
    wait("service IPC disconnect", || {
        observer.status().unwrap().phase == IDLE && !up() && config().ip_set == 0
    });
    api::call(api::SCAN, &[], &[]).unwrap();
    wait("service scan", || {
        api::call(api::STATUS, &[], &[])
            .is_ok_and(|s| s.contains("STATE Choose a network\n") && s.contains("AP "))
    });
    let cli = start("/bin/wifictl", &["wifictl", "connect", "ScarletMock"]);
    associated(&mut observer, "service-reconnect");
    authenticated(&mut observer);
    assert_eq!(std::task::waitpid(cli, 0), (cli, 0));
    println!("WPA_QA CLI_RENAMED_INTERFACE PASS");
    echo(&socket, &to, b"GOOD5");
    let old = observer.identity().connection_id;
    println!("WPA_QA SERVICE_CARRIER_READY");
    wait("service carrier retirement", || {
        observer.status().unwrap().phase == IDLE && !up() && config().ip_set == 0
    });
    println!("WPA_QA SERVICE_CARRIER_DOWN");
    wait("service automatic reconnect", || {
        observer.status().unwrap().phase == ASSOCIATED && observer.identity().connection_id != old
    });
    associated(&mut observer, "service-automatic");
    authenticated(&mut observer);
    echo(&socket, &to, b"GOOD6");
    stop(pid, &mut observer);
    println!("WPA_QA SERVICE_EVENTS PASS");
    println!("WPA_QA PASS");
    std::task::shutdown(std::task::ShutdownType::PowerOff);
}
