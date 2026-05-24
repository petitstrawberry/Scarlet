#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{
    io::{Read, Write},
    println,
    pty::PtyPair,
};

#[unsafe(no_mangle)]
fn main() -> i32 {
    let PtyPair {
        mut master,
        mut slave,
        slave_path,
    } = match PtyPair::open() {
        Ok(opened) => opened,
        Err(error) => {
            println!("pty_smoke: PtyPair::open failed: {}", error);
            return 1;
        }
    };

    println!("pty_smoke: opened {}", slave_path);

    match master.is_slave_locked() {
        Ok(locked) => println!("pty_smoke: slave locked={}", locked),
        Err(error) => {
            println!("pty_smoke: lock query failed: {}", error);
            return 1;
        }
    }

    if let Err(error) = master.write(b"hello from master\n") {
        println!("pty_smoke: master write failed: {}", error);
        return 1;
    }

    let mut buffer = [0u8; 128];
    let slave_count = match slave.read(&mut buffer) {
        Ok(count) => count,
        Err(error) => {
            println!("pty_smoke: slave read failed: {}", error);
            return 1;
        }
    };
    print_bytes("pty_smoke: slave read", &buffer[..slave_count]);

    if let Err(error) = slave.write(b"hello from slave\n") {
        println!("pty_smoke: slave write failed: {}", error);
        return 1;
    }

    let master_count = match master.read(&mut buffer) {
        Ok(count) => count,
        Err(error) => {
            println!("pty_smoke: master read failed: {}", error);
            return 1;
        }
    };
    print_bytes("pty_smoke: master read", &buffer[..master_count]);

    println!("pty_smoke: ok");
    0
}

fn print_bytes(label: &str, bytes: &[u8]) {
    match core::str::from_utf8(bytes) {
        Ok(text) => println!("{}: {:?}", label, text),
        Err(_) => println!("{}: {} bytes", label, bytes.len()),
    }
}
