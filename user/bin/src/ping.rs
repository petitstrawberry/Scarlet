//! Simple ping client for INET

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::io::{Read, Write};
use std::println;
use std::socket::{Inet4SocketAddress, Socket, SocketDomain, SocketProtocol, SocketType};

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    println!("[ping] start");

    let socket = match Socket::new_with_domain(
        SocketDomain::Inet,
        SocketType::Datagram,
        SocketProtocol::Icmp,
    ) {
        Ok(sock) => sock,
        Err(_) => {
            println!("[ping] socket create failed");
            return 1;
        }
    };

    let dest = Inet4SocketAddress::new([10, 0, 2, 2], 0);
    let payload = b"scarlet";

    if let Err(_) = socket.connect_inet(dest) {
        println!("[ping] connect failed");
        return 1;
    }

    if let Ok(mut stream) = socket.as_stream() {
        if let Err(_) = stream.write(payload) {
            println!("[ping] send failed");
            return 1;
        }
    } else {
        println!("[ping] send failed");
        return 1;
    }

    let mut buf = [0u8; 64];
    let n = match socket.as_stream() {
        Ok(mut stream) => match stream.read(&mut buf) {
            Ok(n) => n,
            Err(_) => {
                println!("[ping] recv failed");
                return 1;
            }
        },
        Err(_) => {
            println!("[ping] recv failed");
            return 1;
        }
    };

    println!("[ping] reply {} bytes", n);
    0
}
