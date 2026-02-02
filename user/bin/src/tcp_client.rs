// Simple TCP client - connects to a server and sends/receives data
// Usage: tcp_client <host> <port>
// Example: tcp_client 10.0.2.2 8080

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::env;
use std::io::{Read, Write};
use std::println;
use std::socket::{Inet4SocketAddress, Socket, SocketDomain, SocketProtocol, SocketType};

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    let args = env::args_vec();

    if args.len() != 3 {
        println!("Usage: tcp_client <host> <port>");
        println!("Example: tcp_client 10.0.2.2 8080");
        return 1;
    }

    // Parse IP address
    let ip = match parse_ipv4(&args[1]) {
        Some(addr) => addr,
        None => {
            println!("[tcp-client] Invalid IP address format");
            return 1;
        }
    };

    // Parse port
    let port = match parse_port(&args[2]) {
        Some(p) => p,
        None => {
            println!("[tcp-client] Invalid port number");
            return 1;
        }
    };

    println!(
        "[tcp-client] Connecting to {}.{}.{}.{}:{}...",
        ip[0], ip[1], ip[2], ip[3], port
    );

    // Create TCP socket
    let socket = match Socket::new_with_domain(
        SocketDomain::Inet,
        SocketType::Stream,
        SocketProtocol::Tcp,
    ) {
        Ok(s) => s,
        Err(_) => {
            println!("[tcp-client] Failed to create socket");
            return 1;
        }
    };

    // Connect to server
    let server_addr = Inet4SocketAddress::new(ip, port);
    if let Err(_) = socket.connect_inet(server_addr) {
        println!("[tcp-client] Connection failed");
        return 1;
    }

    println!("[tcp-client] Connected!");

    // Send message
    let message = b"Hello from Scarlet TCP client!";
    let mut stream = match socket.as_stream() {
        Ok(s) => s,
        Err(_) => {
            println!("[tcp-client] Failed to get stream");
            return 1;
        }
    };

    match stream.write(message) {
        Ok(n) => println!("[tcp-client] Sent {} bytes", n),
        Err(_) => {
            println!("[tcp-client] Send failed");
            return 1;
        }
    }

    // Receive response
    let mut buf = [0u8; 1024];
    let n = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(_) => {
            println!("[tcp-client] Receive failed");
            return 1;
        }
    };

    if n > 0 {
        println!("[tcp-client] Received {} bytes", n);
    } else {
        println!("[tcp-client] Server closed connection");
    }

    println!("[tcp-client] Done");
    0
}

fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut result = [0u8; 4];
    let mut idx = 0;
    let mut current = 0u16;

    for c in s.bytes() {
        match c {
            b'0'..=b'9' => {
                current = current * 10 + (c - b'0') as u16;
                if current > 255 {
                    return None;
                }
            }
            b'.' => {
                if idx >= 4 {
                    return None;
                }
                result[idx] = current as u8;
                idx += 1;
                current = 0;
            }
            _ => return None,
        }
    }

    if idx != 3 {
        return None;
    }
    result[3] = current as u8;
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
