// Simple TCP server - listens for connections and echoes data
// Usage: tcp_server <port>
// Example: tcp_server 8080

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

    if args.len() != 2 {
        println!("Usage: tcp_server <port>");
        println!("Example: tcp_server 8080");
        return 1;
    }

    // Parse port
    let port = match parse_port(&args[1]) {
        Some(p) => p,
        None => {
            println!("[tcp-server] Invalid port number");
            return 1;
        }
    };

    println!("[tcp-server] Starting on port {}...", port);

    // Create TCP socket
    let listen_socket =
        match Socket::new_with_domain(SocketDomain::Inet4, SocketType::Stream, SocketProtocol::Tcp)
        {
            Ok(s) => s,
            Err(_) => {
                println!("[tcp-server] Failed to create socket");
                return 1;
            }
        };

    // Bind to port
    let bind_addr = Inet4SocketAddress::new([0, 0, 0, 0], port);
    if let Err(_e) = listen_socket.bind_inet(bind_addr) {
        println!("[tcp-server] Bind failed");
        return 1;
    }

    // Start listening
    if let Err(e) = listen_socket.listen(5) {
        println!("[tcp-server] Listen failed: {:?}", e);
        return 1;
    }

    println!("[tcp-server] Listening on port {}", port);

    // Accept connection (blocking)
    let mut client_socket = match listen_socket.accept() {
        Ok(s) => s,
        Err(e) => {
            println!("[tcp-server] Accept failed: {:?}", e);
            return 1;
        }
    };

    println!("[tcp-server] Client connected!");

    // Echo loop
    let mut buf = [0u8; 1024];
    loop {
        match client_socket.read(&mut buf) {
            Ok(0) => {
                println!("[tcp-server] Client disconnected");
                break;
            }
            Ok(n) => {
                println!("[tcp-server] Received {} bytes", n);
                match client_socket.write(&buf[..n]) {
                    Ok(_) => {}
                    Err(_) => {
                        println!("[tcp-server] Send failed");
                        break;
                    }
                }
            }
            Err(_) => {
                println!("[tcp-server] Receive error");
                break;
            }
        }
    }

    println!("[tcp-server] Done");
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
