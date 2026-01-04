//! Socket Pair Example
//!
//! This program demonstrates bidirectional IPC using socketpair().
//! It creates a connected pair of sockets and sends data between them.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::socket::{ShutdownHow, Socket};
use std::println;

#[unsafe(no_mangle)]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    println!("=== Scarlet Socket Pair Example ===");

    // Create socket pair
    let (socket1, socket2) = match Socket::pair() {
        Ok(pair) => pair,
        Err(e) => {
            println!("Failed to create socket pair: {:?}", e);
            return 1;
        }
    };

    println!("Created connected socket pair");
    println!("Socket 1 handle: {}", socket1.as_raw_handle());
    println!("Socket 2 handle: {}", socket2.as_raw_handle());

    // Test 1: Send from socket1 to socket2
    println!("\n--- Test 1: Socket 1 -> Socket 2 ---");
    let msg1 = b"Hello from Socket 1!";
    println!("Socket 1 sending: {:?}", core::str::from_utf8(msg1).unwrap());

    let stream1 = match socket1.as_stream() {
        Ok(s) => s,
        Err(e) => {
            println!("Failed to get stream from socket 1: {:?}", e);
            return 1;
        }
    };

    match stream1.write(msg1) {
        Ok(n) => println!("Socket 1 sent {} bytes", n),
        Err(e) => {
            println!("Failed to send from socket 1: {:?}", e);
            return 1;
        }
    }

    let stream2 = match socket2.as_stream() {
        Ok(s) => s,
        Err(e) => {
            println!("Failed to get stream from socket 2: {:?}", e);
            return 1;
        }
    };

    let mut buffer1 = [0u8; 256];
    match stream2.read(&mut buffer1) {
        Ok(n) if n > 0 => {
            let received = core::str::from_utf8(&buffer1[..n]).unwrap_or("<invalid utf8>");
            println!("Socket 2 received: {}", received);
        }
        Ok(_) => println!("Socket 2 received no data"),
        Err(e) => {
            println!("Failed to read from socket 2: {:?}", e);
            return 1;
        }
    }

    // Test 2: Send from socket2 to socket1
    println!("\n--- Test 2: Socket 2 -> Socket 1 ---");
    let msg2 = b"Hello from Socket 2!";
    println!("Socket 2 sending: {:?}", core::str::from_utf8(msg2).unwrap());

    match stream2.write(msg2) {
        Ok(n) => println!("Socket 2 sent {} bytes", n),
        Err(e) => {
            println!("Failed to send from socket 2: {:?}", e);
            return 1;
        }
    }

    let mut buffer2 = [0u8; 256];
    match stream1.read(&mut buffer2) {
        Ok(n) if n > 0 => {
            let received = core::str::from_utf8(&buffer2[..n]).unwrap_or("<invalid utf8>");
            println!("Socket 1 received: {}", received);
        }
        Ok(_) => println!("Socket 1 received no data"),
        Err(e) => {
            println!("Failed to read from socket 1: {:?}", e);
            return 1;
        }
    }

    // Test 3: Bidirectional exchange
    println!("\n--- Test 3: Bidirectional Exchange ---");
    
    // Socket 1 sends
    let msg3 = b"Ping";
    println!("Socket 1 sending: {:?}", core::str::from_utf8(msg3).unwrap());
    let _ = stream1.write(msg3);

    // Socket 2 receives and responds
    let mut buffer3 = [0u8; 256];
    if let Ok(n) = stream2.read(&mut buffer3) {
        if n > 0 {
            println!("Socket 2 received: {:?}", core::str::from_utf8(&buffer3[..n]).unwrap());
            
            let response = b"Pong";
            println!("Socket 2 responding: {:?}", core::str::from_utf8(response).unwrap());
            let _ = stream2.write(response);
        }
    }

    // Socket 1 receives response
    let mut buffer4 = [0u8; 256];
    if let Ok(n) = stream1.read(&mut buffer4) {
        if n > 0 {
            println!("Socket 1 received: {:?}", core::str::from_utf8(&buffer4[..n]).unwrap());
        }
    }

    // Shutdown both sockets
    println!("\n--- Cleanup ---");
    let _ = socket1.shutdown(ShutdownHow::Both);
    let _ = socket2.shutdown(ShutdownHow::Both);
    println!("Both sockets closed");

    println!("\nSocket pair example finished successfully");
    0
}
