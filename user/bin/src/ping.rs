//! Simple ping client for INET

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::env;
use std::io::{Read, Write};
use std::println;
use std::socket::{Inet4SocketAddress, Socket, SocketDomain, SocketProtocol, SocketType};

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    println!("[ping] start");

    let args = env::args_vec();
    let dest = if args.len() > 1 {
        parse_ipv4(&args[1]).unwrap_or([10, 0, 2, 2])
    } else {
        [10, 0, 2, 2]
    };

    let socket = match Socket::new_with_domain(
        SocketDomain::Inet4,
        SocketType::Datagram,
        SocketProtocol::Icmp,
    ) {
        Ok(sock) => sock,
        Err(_) => {
            println!("[ping] socket create failed");
            return 1;
        }
    };

    let dest = Inet4SocketAddress::new(dest, 0);
    let payload = b"scarlet";

    if let Err(_) = socket.connect_inet(dest) {
        println!("[ping] connect failed");
        return 1;
    }

    if let Ok(mut stream) = socket.as_stream() {
        if let Err(_) = stream.write(payload) {
            println!("[ping] send failed (check netcfg)");
            return 1;
        }
    } else {
        println!("[ping] send failed (no stream)");
        return 1;
    }

    let mut buf = [0u8; 64];
    if let Err(_) = socket.set_nonblocking(true) {
        println!("[ping] recv failed");
        return 1;
    }

    let mut n = 0usize;
    let mut got_reply = false;
    for _ in 0..200 {
        if let Ok(mut stream) = socket.as_stream() {
            match stream.read(&mut buf) {
                Ok(read) => {
                    n = read;
                    got_reply = true;
                    break;
                }
                Err(_) => {
                    // WouldBlock or no data yet
                }
            }
        }
        std::thread::sleep(core::time::Duration::from_millis(10));
    }

    if !got_reply {
        println!("[ping] timeout");
        return 1;
    }

    if n == payload.len() && &buf[..n] == payload {
        println!("[ping] reply {} bytes", n);
        return 0;
    }

    println!("[ping] invalid reply (payload mismatch)");
    1
}

fn parse_ipv4(value: &str) -> Option<[u8; 4]> {
    let mut parts = [0u8; 4];
    let mut index = 0;
    for part in value.split('.') {
        if index >= parts.len() {
            return None;
        }
        parts[index] = part.parse::<u8>().ok()?;
        index += 1;
    }
    if index == parts.len() {
        Some(parts)
    } else {
        None
    }
}
