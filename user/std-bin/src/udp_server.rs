use std::env;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::process::ExitCode;

fn main() -> ExitCode {
    println!("[udp-server] Rust std version");

    let args = env::args().collect::<Vec<_>>();
    if args.len() != 2 {
        println!("Usage: udp_server <port>");
        println!("Example: udp_server 18080");
        return ExitCode::from(1);
    }

    let port = match args[1].parse::<u16>() {
        Ok(port) if port != 0 => port,
        _ => {
            println!("[udp-server] Invalid port number");
            return ExitCode::from(1);
        }
    };

    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
    let socket = match UdpSocket::bind(addr) {
        Ok(socket) => socket,
        Err(err) => {
            println!("[udp-server] Bind failed: {err}");
            return ExitCode::from(1);
        }
    };

    println!("[udp-server] Listening on port {port}");
    let mut buf = [0; 1024];
    let (n, peer) = match socket.recv_from(&mut buf) {
        Ok(result) => result,
        Err(err) => {
            println!("[udp-server] Receive error: {err}");
            return ExitCode::from(1);
        }
    };

    if n == 0 {
        println!("[udp-server] Empty datagram");
        return ExitCode::SUCCESS;
    }

    println!("[udp-server] Received {n} bytes from {peer}");
    let response_len = n.min(buf.len());
    buf[0] = b'[';

    match socket.send_to(&buf[..response_len], peer) {
        Ok(_) => {}
        Err(err) => {
            println!("[udp-server] Send failed: {err}");
            return ExitCode::from(1);
        }
    }

    println!("[udp-server] Done");
    ExitCode::SUCCESS
}
