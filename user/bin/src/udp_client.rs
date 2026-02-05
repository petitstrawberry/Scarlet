// Simple UDP client - sends datagrams and receives responses
// Usage: udp_client <host> <port> [message]
// Example: udp_client 10.0.2.15 18080
//         udp_client 10.0.2.15 18080 "Hello from UDP client!"

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::env;
use std::io::{Read, Write};
use std::println;
use std::socket::{Inet4SocketAddress, Socket, SocketDomain, SocketProtocol, SocketType};
use std::vec::Vec;

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    let args = env::args_vec();

    if args.len() != 3 && args.len() != 4 {
        println!("Usage: udp_client <host> <port> [message]");
        println!("Example: udp_client 10.0.2.15 18080");
        println!("         udp_client 10.0.2.15 18080 \"Hello\"");
        return 1;
    }

    let host_str = &args[1];
    let host = match parse_ip(host_str) {
        Some(ip) => ip,
        None => {
            println!("[udp-client] Invalid IP address");
            return 1;
        }
    };

    let port = match parse_port(&args[2]) {
        Some(p) => p,
        None => {
            println!("[udp-client] Invalid port number");
            return 1;
        }
    };

    let message = if args.len() == 4 {
        &args[3]
    } else {
        "Hello from UDP client!"
    };

    println!("[udp-client] Sending to {}:{}", host_str, port);

    let mut socket = match Socket::new_with_domain(
        SocketDomain::Inet4,
        SocketType::Datagram,
        SocketProtocol::Udp,
    ) {
        Ok(s) => s,
        Err(_) => {
            println!("[udp-client] Failed to create socket");
            return 1;
        }
    };

    // NOTE: For UDP client, we use connect() + write() pattern
    // This allows us to send to a fixed destination without bind
    let server_addr = Inet4SocketAddress::new(host, port);
    if let Err(_e) = socket.connect_inet(server_addr) {
        println!("[udp-client] Connect failed");
        return 1;
    }

    println!("[udp-client] Connected to {}:{}", host_str, port);

    // Send message
    let message_bytes = message.as_bytes();
    match socket.write(message_bytes) {
        Ok(n) => {
            println!("[udp-client] Sent {} bytes", n);
        }
        Err(_) => {
            println!("[udp-client] Send failed");
            return 1;
        }
    }

    // Receive response
    let mut buf = [0u8; 1024];
    match socket.read(&mut buf) {
        Ok(0) => {
            println!("[udp-client] Connection closed");
        }
        Ok(n) => {
            let response = core::str::from_utf8(&buf[..n]).unwrap_or("<invalid utf8>");
            println!("[udp-client] Received {} bytes: {}", n, response);
        }
        Err(_) => {
            println!("[udp-client] Receive error");
        }
    }

    println!("[udp-client] Done");
    0
}

fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut parts = Vec::new();
    for part in s.split('.') {
        parts.push(part);
    }

    if parts.len() != 4 {
        return None;
    }

    let mut result = [0u8; 4];
    for (i, part) in parts.iter().enumerate() {
        match parse_u8(part) {
            Some(val) => result[i] = val,
            None => return None,
        }
    }

    Some(result)
}

fn parse_u8(s: &str) -> Option<u8> {
    let mut result = 0u8;

    for c in s.bytes() {
        match c {
            b'0'..=b'9' => {
                result = result * 10 + (c - b'0') as u8;
            }
            _ => return None,
        }
    }

    Some(result)
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
