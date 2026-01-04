//! Socket Echo Client Example
//!
//! This program demonstrates a simple echo client using Scarlet Native sockets.
//! It connects to the echo server and sends/receives test messages.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;
use std::socket::{ShutdownHow, Socket};

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    println!("=== Scarlet Socket Echo Client ===");

    // Create client socket
    let client = match Socket::new() {
        Ok(s) => s,
        Err(e) => {
            println!("Failed to create socket: {:?}", e);
            return 1;
        }
    };

    // Connect to server
    let socket_path = "/tmp/echo.sock";
    println!("Connecting to {}...", socket_path);

    match client.connect(socket_path) {
        Ok(_) => println!("Connected to server!"),
        Err(e) => {
            println!("Failed to connect: {:?}", e);
            println!("Make sure the server is running first.");
            return 1;
        }
    };

    // Send test messages
    let messages: &[&[u8]] = &[
        b"Hello, Server!",
        b"This is a test message.",
        b"Scarlet Native Sockets",
    ];

    for (i, msg) in messages.iter().enumerate() {
        println!("\n--- Message {} ---", i + 1);
        println!("Sending: {:?}", core::str::from_utf8(msg).unwrap());

        // Send message
        let stream = match client.as_stream() {
            Ok(s) => s,
            Err(e) => {
                println!("Failed to get stream: {:?}", e);
                return 1;
            }
        };

        match stream.write(msg) {
            Ok(n) => {
                println!("Sent {} bytes", n);
            }
            Err(e) => {
                println!("Failed to send: {:?}", e);
                return 1;
            }
        }

        // Receive echo
        let mut buffer = [0u8; 256];
        match stream.read(&mut buffer) {
            Ok(n) if n > 0 => {
                println!("Received {} bytes: {:?}", n, &buffer[..n]);
                let received = core::str::from_utf8(&buffer[..n]).unwrap_or("<invalid utf8>");
                println!("Echo: {}", received);
            }
            Ok(_) => {
                println!("No data received from server");
            }
            Err(e) => {
                println!("Failed to receive: {:?}", e);
                return 1;
            }
        }
    }

    // Shutdown connection
    println!("\nClosing connection...");
    match client.shutdown(ShutdownHow::Both) {
        Ok(_) => println!("Connection closed"),
        Err(e) => println!("Shutdown error: {:?}", e),
    };

    println!("Client finished successfully");
    0
}
