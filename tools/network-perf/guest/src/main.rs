#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{
    io::{Read, Write},
    network::{Ipv4Address, configure_interface_ipv4, list_interface_configs},
    println,
    socket::{Inet4SocketAddress, Socket, SocketDomain, SocketProtocol, SocketType},
    time::Duration,
};

fn profile(command: &[u8]) {
    std::fs::File::open_with_flags("/dev/net_profile", 2)
        .unwrap()
        .write_all(command)
        .unwrap();
}

fn snapshot() {
    let mut file = std::fs::File::open("/dev/net_profile").unwrap();
    let mut buffer = [0; 4096];
    loop {
        let n = file.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        println!("{}", core::str::from_utf8(&buffer[..n]).unwrap());
    }
}

fn read_exact(socket: &mut Socket, mut buffer: &mut [u8]) {
    while !buffer.is_empty() {
        let n = socket.read(buffer).unwrap();
        assert!(n > 0);
        buffer = &mut buffer[n..];
    }
}

fn write_all(socket: &mut Socket, mut buffer: &[u8]) {
    while !buffer.is_empty() {
        let n = socket.write(buffer).unwrap();
        assert!(n > 0);
        buffer = &buffer[n..];
    }
}

#[repr(C)]
#[derive(Default)]
struct CpuUsage {
    online_cpus: usize,
    busy_ns: u64,
    idle_ns: u64,
    total_ns: u64,
    per_mille: u32,
    reserved: u32,
}

fn cpu_usage() -> CpuUsage {
    let mut result = CpuUsage::default();
    // SAFETY: exclusive output storage matches GetCpuUsageInfo's fixed ABI.
    let status = unsafe {
        std::syscall::syscall1(
            std::syscall::Syscall::GetCpuUsageInfo,
            &mut result as *mut CpuUsage as usize,
        )
    };
    assert_ne!(status, usize::MAX);
    result
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let interface = loop {
        if let Some(config) = list_interface_configs()
            .unwrap()
            .into_iter()
            .find(|c| c.interface_name() != Some("lo"))
        {
            break config;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let name = interface.interface_name().unwrap();
    configure_interface_ipv4(
        name,
        Ipv4Address([10, 0, 2, 15]),
        Ipv4Address([255, 255, 255, 0]),
        Some(Ipv4Address([10, 0, 2, 2])),
        10,
        true,
    )
    .unwrap();
    let listener =
        Socket::new_with_domain(SocketDomain::Inet4, SocketType::Stream, SocketProtocol::Tcp)
            .unwrap();
    listener
        .bind_inet(Inet4SocketAddress::new([0, 0, 0, 0], 8081))
        .unwrap();
    listener.listen(4).unwrap();
    let mut buffer = std::vec![0x5a; 65536];
    profile(b"0");
    println!("NETWORK_PERF READY interface={}", name);
    loop {
        let mut socket = listener.accept().unwrap();
        let mut header = [0; 10];
        read_exact(&mut socket, &mut header);
        let size = u64::from_le_bytes(header[2..].try_into().unwrap()) as usize;
        assert!(size > 0 && size <= 64 * 1024 * 1024);
        let receive = header[0] == 0;
        assert!(header[0] <= 1 && header[1] <= 1);
        if header[1] == 1 {
            profile(b"1");
        }
        let cpu_start = cpu_usage();
        write_all(&mut socket, b"R");
        let start = scarlet_os::time::monotonic_time_ns();
        let mut bytes = 0;
        let mut calls = 0;
        while bytes < size {
            let n = (size - bytes).min(buffer.len());
            let n = if receive {
                let n = socket.read(&mut buffer[..n]).unwrap();
                assert!(n != 0 && buffer[..n].iter().all(|&b| b == 0x5a));
                n
            } else {
                let n = socket.write(&buffer[..n]).unwrap();
                assert!(n != 0);
                n
            };
            bytes += n;
            calls += 1;
        }
        if !receive {
            let mut confirmation = [0; 1];
            read_exact(&mut socket, &mut confirmation);
            assert_eq!(confirmation, [b'V']);
        }
        let elapsed = scarlet_os::time::monotonic_time_ns() - start;
        let cpu_end = cpu_usage();
        profile(b"0");
        println!(
            "NETWORK_PERF RESULT direction={} bytes={} calls={} elapsed_ns={} profile={} busy_ns={} idle_ns={}",
            if receive { "rx" } else { "tx" },
            bytes,
            calls,
            elapsed,
            header[1],
            cpu_end.busy_ns.saturating_sub(cpu_start.busy_ns),
            cpu_end.idle_ns.saturating_sub(cpu_start.idle_ns)
        );
        write_all(&mut socket, &(bytes as u64).to_le_bytes());
        if header[1] == 1 {
            snapshot();
        }
    }
}
