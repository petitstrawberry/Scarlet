use std::env;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 3 && args.len() != 4 {
        println!("Usage: udp-client <host> <port> [message]");
        println!("Example: udp-client 10.0.2.15 18080");
        return ExitCode::from(1);
    }

    let host = match args[1].parse::<Ipv4Addr>() {
        Ok(host) => host,
        Err(err) => {
            println!("[udp-client] Invalid IPv4 address: {err}");
            return ExitCode::from(1);
        }
    };
    let port = match args[2].parse::<u16>() {
        Ok(port) if port != 0 => port,
        _ => {
            println!("[udp-client] Invalid port number");
            return ExitCode::from(1);
        }
    };
    let message = args
        .get(3)
        .map(String::as_str)
        .unwrap_or("Hello from UDP client!");
    let remote = SocketAddrV4::new(host, port);

    let socket = match UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)) {
        Ok(socket) => socket,
        Err(err) => {
            println!("[udp-client] Bind failed: {err}");
            return ExitCode::from(1);
        }
    };

    println!("[udp-client] Sending to {remote}");
    if let Err(err) = socket.connect(remote) {
        println!("[udp-client] Connect failed: {err}");
        return ExitCode::from(1);
    }

    match socket.send(message.as_bytes()) {
        Ok(n) => println!("[udp-client] Sent {n} bytes"),
        Err(err) => {
            println!("[udp-client] Send failed: {err}");
            return ExitCode::from(1);
        }
    }

    let mut buf = [0; 1024];
    match socket.recv(&mut buf) {
        Ok(0) => println!("[udp-client] Connection closed"),
        Ok(n) => {
            let response = String::from_utf8_lossy(&buf[..n]);
            println!("[udp-client] Received {n} bytes: {response}");
        }
        Err(err) => {
            println!("[udp-client] Receive error: {err}");
            return ExitCode::from(1);
        }
    }

    println!("[udp-client] Done");
    ExitCode::SUCCESS
}
