#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::{
    fs::File,
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
            println!("pty-smoke: PtyPair::open failed: {}", error);
            return 1;
        }
    };

    println!("pty-smoke: opened {}", slave_path);

    match master.is_slave_locked() {
        Ok(false) => println!("pty-smoke: slave locked=false"),
        Ok(true) => {
            println!("pty-smoke: slave unexpectedly locked");
            return 1;
        }
        Err(error) => {
            println!("pty-smoke: lock query failed: {}", error);
            return 1;
        }
    }

    if let Err(error) = master.set_winsize(100, 40) {
        println!("pty-smoke: set winsize failed: {}", error);
        return 1;
    }
    match master.winsize() {
        Ok((100, 40)) => println!("pty-smoke: winsize=100x40"),
        Ok((cols, rows)) => {
            println!("pty-smoke: unexpected winsize={}x{}", cols, rows);
            return 1;
        }
        Err(error) => {
            println!("pty-smoke: get winsize failed: {}", error);
            return 1;
        }
    }

    if let Err(error) = master.write(b"hello from master\n") {
        println!("pty-smoke: master write failed: {}", error);
        return 1;
    }

    let mut buffer = [0u8; 128];
    let slave_count = match slave.read(&mut buffer) {
        Ok(count) => count,
        Err(error) => {
            println!("pty-smoke: slave read failed: {}", error);
            return 1;
        }
    };
    print_bytes("pty-smoke: slave read", &buffer[..slave_count]);
    if !expect_bytes(
        "pty-smoke: slave read",
        &buffer[..slave_count],
        b"hello from master\n",
    ) {
        return 1;
    }

    if let Err(error) = slave.write(b"hello from slave\n") {
        println!("pty-smoke: slave write failed: {}", error);
        return 1;
    }

    let master_count = match master.read(&mut buffer) {
        Ok(count) => count,
        Err(error) => {
            println!("pty-smoke: master read failed: {}", error);
            return 1;
        }
    };
    print_bytes("pty-smoke: master read", &buffer[..master_count]);
    if !expect_bytes(
        "pty-smoke: master read",
        &buffer[..master_count],
        b"hello from master\r\nhello from slave\r\n",
    ) {
        return 1;
    }

    let original_slave_path = slave_path.clone();
    drop(slave);
    drop(master);

    if devpts_has_entry(&original_slave_path) {
        println!(
            "pty-smoke: released slave still visible: {}",
            original_slave_path
        );
        return 1;
    }
    println!("pty-smoke: released {} disappeared", original_slave_path);

    let recycled = match PtyPair::open() {
        Ok(opened) => opened,
        Err(error) => {
            println!("pty-smoke: second PtyPair::open failed: {}", error);
            return 1;
        }
    };
    if recycled.slave_path != original_slave_path {
        println!(
            "pty-smoke: expected recycled path {}, got {}",
            original_slave_path, recycled.slave_path
        );
        return 1;
    }
    println!("pty-smoke: recycled {}", recycled.slave_path);

    println!("pty-smoke: ok");
    0
}

fn devpts_has_entry(path: &str) -> bool {
    let Some(name) = path.rsplit('/').next() else {
        return false;
    };
    let Ok(mut dir) = File::open("/dev/pts") else {
        return false;
    };
    loop {
        match dir.read_dir() {
            Ok(Some(entry)) => {
                if entry.name_str() == name {
                    return true;
                }
            }
            Ok(None) | Err(_) => return false,
        }
    }
}

fn print_bytes(label: &str, bytes: &[u8]) {
    match core::str::from_utf8(bytes) {
        Ok(text) => println!("{}: {:?}", label, text),
        Err(_) => println!("{}: {} bytes", label, bytes.len()),
    }
}

fn expect_bytes(label: &str, actual: &[u8], expected: &[u8]) -> bool {
    if actual == expected {
        return true;
    }

    print_bytes(label, actual);
    print_bytes("pty-smoke: expected", expected);
    false
}
