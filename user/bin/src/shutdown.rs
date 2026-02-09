#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;
use std::socket::Socket;
use std::task::exit;

#[unsafe(no_mangle)]
fn main(_argc: usize, _argv: *const *const u8) -> i32 {
    // Shutdown flow:
    // 1. shutdown command (this) sends request to stemd
    // 2. stemd performs proper cleanup (signals, sync, unmount)
    // 3. stemd calls kernel shutdown syscall as final step
    // 4. kernel force-kills any remaining tasks and powers off
    println!("[shutdown] Requesting shutdown from stemd...");

    let socket_path = "/tmp/stemd.sock";

    let socket = match Socket::new() {
        Ok(s) => s,
        Err(e) => {
            println!("[shutdown] Failed to create socket: {:?}", e);
            return 1;
        }
    };

    if let Err(e) = socket.connect(socket_path) {
        println!("[shutdown] Failed to connect to stemd: {:?}", e);
        return 1;
    }

    let stream = match socket.as_stream() {
        Ok(s) => s,
        Err(e) => {
            println!("[shutdown] Failed to get stream: {:?}", e);
            return 1;
        }
    };

    let cmd = [4u8];
    if let Err(e) = stream.write(&cmd) {
        println!("[shutdown] Failed to send command: {:?}", e);
        return 1;
    }

    // Wait for response
    let mut buffer = [0u8; 256];
    match stream.read(&mut buffer) {
        Ok(n) if n > 0 => {
            if let Ok(response) = core::str::from_utf8(&buffer[..n]) {
                println!("[shutdown] Response: {}", response.trim());
            }
        }
        _ => {
            println!("[shutdown] No response from stemd");
        }
    }

    exit(0)
}
