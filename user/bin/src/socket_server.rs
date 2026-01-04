//! Socket Echo Server Example
//!
//! This program demonstrates a simple echo server using Scarlet Native sockets.
//! It listens on a socket path and echoes back any data received from clients.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;
use std::socket::{ShutdownHow, Socket};

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    println!("=== Scarlet Socket Echo Server ===");

    // Create server socket
    let server = match Socket::new() {
        Ok(s) => s,
        Err(e) => {
            println!("Failed to create socket: {:?}", e);
            return 1;
        }
    };

    // Bind to socket path
    let socket_path = "/tmp/echo.sock";
    match server.bind(socket_path) {
        Ok(_) => println!("Server bound to {}", socket_path),
        Err(e) => {
            println!("Failed to bind socket: {:?}", e);
            return 1;
        }
    };

    // Start listening
    match server.listen(5) {
        Ok(_) => println!("Server listening (backlog: 5)"),
        Err(e) => {
            println!("Failed to listen: {:?}", e);
            return 1;
        }
    };

    println!("Waiting for connections...");

    // Accept client connection (now blocking)
    let client = match server.accept() {
        Ok(c) => {
            println!("Client connected!");
            c
        }
        Err(e) => {
            println!("Failed to accept connection: {:?}", e);
            return 1;
        }
    };

    // Echo loop - read and write back data
    println!("Echo loop started. Waiting for data...");

    let stream = match client.as_stream() {
        Ok(s) => s,
        Err(e) => {
            println!("Failed to get stream: {:?}", e);
            return 1;
        }
    };

    let mut buffer = [0u8; 256];
    let mut total_bytes = 0;

    for i in 0..10 {
        // Try to read data (limit to 10 attempts for demo)
        match stream.read(&mut buffer) {
            Ok(n) if n > 0 => {
                println!("Received {} bytes: {:?}", n, &buffer[..n]);
                total_bytes += n;

                // Echo back
                match stream.write(&buffer[..n]) {
                    Ok(written) => {
                        println!("Echoed back {} bytes", written);
                    }
                    Err(e) => {
                        println!("Failed to write: {:?}", e);
                        break;
                    }
                }
            }
            Ok(_) => {
                println!("No data received (attempt {}/10)", i + 1);
                if i >= 2 {
                    // Give up after 3 attempts
                    println!("No client data, exiting...");
                    break;
                }
            }
            Err(e) => {
                println!("Read error: {:?}", e);
                break;
            }
        }
    }

    println!("Total bytes echoed: {}", total_bytes);

    // Shutdown connection
    match client.shutdown(ShutdownHow::Both) {
        Ok(_) => println!("Connection closed"),
        Err(e) => println!("Shutdown error: {:?}", e),
    };

    println!("Server shutting down...");
    0
}
