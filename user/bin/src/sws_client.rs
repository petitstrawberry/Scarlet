//! SWS Test Client - Simple client for testing Scarlet Window Server IPC

#![no_std]
#![no_main]

extern crate scarlet_std as std;

extern crate userprogram;

use userprogram::sws_protocol as protocol;

use std::println;
use std::socket::Socket;

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("=== SWS Test Client ===");
    println!("Connecting to /tmp/sws.sock...");

    // Create socket and connect
    let mut socket = match Socket::new() {
        Ok(s) => s,
        Err(e) => {
            println!("Failed to create socket: {:?}", e);
            return 1;
        }
    };

    if let Err(e) = socket.connect("/tmp/sws.sock") {
        println!("Failed to connect: {:?}", e);
        println!("Make sure SWS is running!");
        return 1;
    }

    println!(
        "Connected to SWS server (socket handle: {})",
        socket.as_raw()
    );

    // Send CreateWindow message
    println!("Sending CreateWindow request (400x300)...");
    if let Err(e) = protocol::write_create_window(&mut socket, 400, 300) {
        println!("Failed to send: {:?}", e);
        return 1;
    }

    // Read response
    println!("Waiting for WindowCreated response...");
    match protocol::read_window_created(&mut socket) {
        Ok(protocol::WindowCreated { window_id, shm_size }) => {
            println!("Window created successfully!");
            println!("  Window ID: {}", window_id);
            println!("  Shared memory size: {} bytes", shm_size);

            // Keep window open briefly
            println!("Keeping window open for 1 second...");
            scarlet_std::thread::sleep(core::time::Duration::from_secs(1));

            // Send DestroyWindow message
            println!("Closing window...");
            if let Err(e) = protocol::write_destroy_window(&mut socket, window_id) {
                println!("Failed to send destroy request: {:?}", e);
            }

            println!("Success!");
            0
        }
        Err(e) => {
            println!("Failed to receive response: {:?}", e);
            1
        }
    }
}
