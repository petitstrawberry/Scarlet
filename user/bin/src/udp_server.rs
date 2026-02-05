// Simple UDP server - receives datagrams and echoes data
// Usage: udp_server <port>
// Example: udp_server 18080

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::env;
use std::println;
use std::socket::{
    DatagramOps, Inet4SocketAddress, Socket, SocketAddress, SocketDomain, SocketProtocol,
    SocketType,
};

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    let args = env::args_vec();

    if args.len() != 2 {
        println!("Usage: udp_server <port>");
        println!("Example: udp_server 18080");
        return 1;
    }

    let port = match parse_port(&args[1]) {
        Some(p) => p,
        None => {
            println!("[udp-server] Invalid port number");
            return 1;
        }
    };

    println!("[udp-server] Starting on port {}...", port);

    let socket = match Socket::new_with_domain(
        SocketDomain::Inet4,
        SocketType::Datagram,
        SocketProtocol::Udp,
    ) {
        Ok(s) => s,
        Err(_) => {
            println!("[udp-server] Failed to create socket");
            return 1;
        }
    };

    let bind_addr = Inet4SocketAddress::new([0, 0, 0, 0], port);
    if let Err(_e) = socket.bind_inet(bind_addr) {
        println!("[udp-server] Bind failed");
        return 1;
    }

    println!("[udp-server] Listening on port {}", port);

    let mut buf = [0u8; 1024];
    let mut response = [0u8; 1024];

    loop {
        // Receive datagram with sender address
        match socket.recvfrom(&mut buf) {
            Ok((n, src_addr)) => {
                if n == 0 {
                    println!("[udp-server] Connection closed");
                    break;
                }

                println!("[udp-server] Received {} bytes from {:?}", n, src_addr);

                // Prepare response with prefix
                let response_len = n.min(1024);
                response[0] = b'[';
                let data_len = response_len.saturating_sub(1);

                if n > 0 && data_len > 0 {
                    response[1..1 + data_len].copy_from_slice(&buf[..data_len]);
                }

                // Send response back to sender
                match socket.sendto(&response[..response_len], &src_addr) {
                    Ok(_) => {}
                    Err(_) => {
                        println!("[udp-server] Send failed");
                        break;
                    }
                }
            }
            Err(_) => {
                println!("[udp-server] Receive error");
                break;
            }
        }
    }

    println!("[udp-server] Done");
    0
}

fn parse_port(s: &str) -> Option<u16> {
    let mut result = 0u32;

    for c in s.bytes() {
        match c {
            b'0'..=b'9' => {
                result = result * 10 + (c - b'0') as u32;
                if result > 65535 {
                    return None;
                }
            }
            _ => return None,
        }
    }

    if result == 0 {
        return None;
    }
    Some(result as u16)
}
