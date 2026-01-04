//! Socket Reverse Server Example
//!
//! This program demonstrates a string-reversing server using Scarlet Native sockets.
//! It listens on a socket path and reverses any strings received from clients.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;
use std::socket::{ShutdownHow, Socket};

/// Reverse a byte slice in place
fn reverse_bytes(data: &mut [u8]) {
    let len = data.len();
    for i in 0..len / 2 {
        data.swap(i, len - 1 - i);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    println!("[Server] String Reverse Server starting...");

    // Create server socket
    let server = match Socket::new() {
        Ok(s) => s,
        Err(e) => {
            println!("[Server] Failed to create socket: {:?}", e);
            return 1;
        }
    };

    // Bind to socket path
    let socket_path = "/tmp/reverse.sock";
    match server.bind(socket_path) {
        Ok(_) => println!("[Server] Listening on {}", socket_path),
        Err(e) => {
            println!("[Server] Failed to bind socket: {:?}", e);
            return 1;
        }
    };

    // Start listening
    match server.listen(5) {
        Ok(_) => println!("[Server] Ready to accept connections"),
        Err(e) => {
            println!("[Server] Failed to listen: {:?}", e);
            return 1;
        }
    };

    // Accept client connection (blocking)
    let client = match server.accept() {
        Ok(c) => {
            println!("[Server] Client connected");
            c
        }
        Err(e) => {
            println!("[Server] Failed to accept connection: {:?}", e);
            return 1;
        }
    };

    let stream = match client.as_stream() {
        Ok(s) => s,
        Err(e) => {
            println!("[Server] Failed to get stream: {:?}", e);
            return 1;
        }
    };

    let mut buffer = [0u8; 256];

    // Service loop - reverse strings until client disconnects
    loop {
        match stream.read(&mut buffer) {
            Ok(n) if n > 0 => {
                let input = core::str::from_utf8(&buffer[..n]).unwrap_or("<invalid utf8>");
                println!("[Server] Received: {}", input);

                // Skip empty messages (shouldn't happen, but handle gracefully)
                if n == 0 {
                    continue;
                }

                // Reverse the string
                reverse_bytes(&mut buffer[..n]);

                let reversed = core::str::from_utf8(&buffer[..n]).unwrap_or("<invalid utf8>");
                println!("[Server] Sending back: {}", reversed);

                // Send reversed string back
                match stream.write(&buffer[..n]) {
                    Ok(_) => {}
                    Err(e) => {
                        println!("[Server] Failed to write: {:?}", e);
                        break;
                    }
                }
            }
            Ok(_) => {
                println!("[Server] Client disconnected");
                break;
            }
            Err(e) => {
                println!("[Server] Read error: {:?}", e);
                break;
            }
        }
    }

    // Cleanup
    match client.shutdown(ShutdownHow::Both) {
        Ok(_) => {}
        Err(e) => println!("[Server] Shutdown error: {:?}", e),
    };

    println!("[Server] Server shutting down");
    0
}
