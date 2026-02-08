//! Interactive Socket Client Example
//!
//! This program demonstrates an interactive client using Scarlet Native sockets.
//! It connects to the reverse server and allows interactive message sending.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::io::stdin;
use std::print;
use std::println;
use std::socket::{ShutdownHow, Socket};

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    println!("=== Interactive String Reverse Client ===");
    println!("Type messages to reverse them. Type 'exit' to quit.");
    println!();

    // Create client socket
    let client = match Socket::new() {
        Ok(s) => s,
        Err(e) => {
            println!("Failed to create socket: {:?}", e);
            return 1;
        }
    };

    // Connect to server
    let socket_path = "/tmp/reverse.sock";
    println!("Connecting to {}...", socket_path);

    match client.connect(socket_path) {
        Ok(_) => println!("Connected to server!"),
        Err(e) => {
            println!("Failed to connect: {:?}", e);
            println!("Make sure the server is running first.");
            return 1;
        }
    };

    println!();

    let stream = match client.as_stream() {
        Ok(s) => s,
        Err(e) => {
            println!("Failed to get stream: {:?}", e);
            return 1;
        }
    };

    let stdin = stdin();
    let mut input_buffer = [0u8; 256];

    loop {
        // Display prompt
        print!("> ");

        // Read user input (canonical mode: waits for Enter)
        let bytes_read = match stdin.read(&mut input_buffer) {
            Ok(n) => n,
            Err(_) => {
                println!("\nFailed to read input");
                break;
            }
        };

        if bytes_read == 0 {
            println!("\nEnd of input");
            break;
        }

        // Convert to string and trim newline
        let mut input_len = bytes_read;
        if input_len > 0 && input_buffer[input_len - 1] == b'\n' {
            input_len -= 1;
        }
        if input_len > 0 && input_buffer[input_len - 1] == b'\r' {
            input_len -= 1;
        }

        let input = match core::str::from_utf8(&input_buffer[..input_len]) {
            Ok(s) => s,
            Err(_) => {
                println!("Invalid UTF-8 input");
                continue;
            }
        };

        // Skip empty input
        if input.is_empty() {
            continue;
        }

        // Check for exit command
        if input == "exit" {
            println!("Exiting...");
            break;
        }

        // Send message to server
        match stream.write(input.as_bytes()) {
            Ok(_) => {}
            Err(e) => {
                println!("Failed to send: {:?}", e);
                break;
            }
        }

        // Receive response from server
        let mut response_buffer = [0u8; 256];
        match stream.read(&mut response_buffer) {
            Ok(n) if n > 0 => {
                let received =
                    core::str::from_utf8(&response_buffer[..n]).unwrap_or("<invalid utf8>");
                println!("{}", received);
            }
            Ok(_) => {
                println!("Server disconnected");
                break;
            }
            Err(e) => {
                println!("Failed to receive: {:?}", e);
                break;
            }
        }
    }

    // Shutdown connection
    println!("\nClosing connection...");
    match client.shutdown(ShutdownHow::Both) {
        Ok(_) => println!("Connection closed"),
        Err(e) => println!("Shutdown error: {:?}", e),
    };

    println!("Client finished");
    0
}
