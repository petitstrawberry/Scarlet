//! SWS Test Client - Simple client for testing Scarlet Window Server IPC

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::io::{Read, Write};
use std::println;
use std::socket::Socket;
use std::vec::Vec;

// Message types from protocol.rs
const MSG_CREATE_WINDOW: u32 = 1;
const MSG_WINDOW_CREATED: u32 = 10;

#[repr(C)]
struct MessageHeader {
    msg_type: u32,
    payload_size: u32,
}

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
    if let Err(e) = send_create_window(&mut socket, 400, 300) {
        println!("Failed to send: {:?}", e);
        return 1;
    }

    // Read response
    println!("Waiting for WindowCreated response...");
    match read_window_created(&mut socket) {
        Ok((window_id, shm_size)) => {
            println!("Window created successfully!");
            println!("  Window ID: {}", window_id);
            println!("  Shared memory size: {} bytes", shm_size);
            
            // Draw lines on window to make it visible
            println!("Drawing test pattern on window...");
            if let Err(e) = draw_test_pattern(&mut socket, window_id, 400, 300) {
                println!("Failed to draw pattern: {:?}", e);
            }
            
            // Sleep for 3 seconds
            println!("Keeping window open for 3 seconds...");
            scarlet_std::thread::sleep(core::time::Duration::from_secs(3));
            
            // Send DestroyWindow message
            println!("Closing window...");
            if let Err(e) = send_destroy_window(&mut socket, window_id) {
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

fn send_create_window(socket: &mut Socket, width: u32, height: u32) -> Result<(), &'static str> {
    println!(
        "[Client] Preparing CreateWindow message... (socket handle: {})",
        socket.as_raw()
    );

    // Prepare payload
    let mut payload = Vec::new();
    payload.extend_from_slice(&width.to_le_bytes());
    payload.extend_from_slice(&height.to_le_bytes());

    // Prepare header
    let header = MessageHeader {
        msg_type: MSG_CREATE_WINDOW,
        payload_size: payload.len() as u32,
    };

    // Send header
    let mut header_bytes = Vec::new();
    header_bytes.extend_from_slice(&header.msg_type.to_le_bytes());
    header_bytes.extend_from_slice(&header.payload_size.to_le_bytes());

    println!("[Client] Writing header ({} bytes)...", header_bytes.len());
    let header_written = socket.write(&header_bytes).map_err(|e| {
        println!("[Client] Header write failed: {:?}", e);
        "Failed to write header"
    })?;
    println!("[Client] Wrote {} bytes for header", header_written);

    println!("[Client] Writing payload ({} bytes)...", payload.len());
    let payload_written = socket.write(&payload).map_err(|e| {
        println!("[Client] Payload write failed: {:?}", e);
        "Failed to write payload"
    })?;
    println!("[Client] Wrote {} bytes for payload", payload_written);

    // Flush to ensure data is sent
    println!("[Client] Flushing socket...");
    socket.flush().map_err(|e| {
        println!("[Client] Flush failed: {:?}", e);
        "Failed to flush"
    })?;
    println!("[Client] Flush complete");

    println!(
        "[Client] Sent {} bytes total (header + payload)",
        header_bytes.len() + payload.len()
    );
    Ok(())
}

fn read_window_created(socket: &mut Socket) -> Result<(u32, usize), &'static str> {
    // Read header
    println!("[Client] Reading response header...");
    let mut header_buf = [0u8; 8];
    let n = socket.read(&mut header_buf).map_err(|e| {
        println!("[Client] Failed to read header: {:?}", e);
        "Failed to read header"
    })?;
    println!("[Client] Read {} bytes for header", n);

    let msg_type = u32::from_le_bytes([header_buf[0], header_buf[1], header_buf[2], header_buf[3]]);
    let payload_size =
        u32::from_le_bytes([header_buf[4], header_buf[5], header_buf[6], header_buf[7]]);

    println!(
        "[Client] Received message type: {}, payload size: {}",
        msg_type, payload_size
    );

    if msg_type != MSG_WINDOW_CREATED {
        return Err("Unexpected message type");
    }

    // Read payload
    let mut payload = Vec::new();
    payload.resize(payload_size as usize, 0);
    socket
        .read(&mut payload)
        .map_err(|_| "Failed to read payload")?;

    if payload.len() < 12 {
        return Err("Payload too small");
    }

    let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let shm_size = usize::from_le_bytes([
        payload[4],
        payload[5],
        payload[6],
        payload[7],
        payload[8],
        payload[9],
        payload[10],
        payload[11],
    ]);

    Ok((window_id, shm_size))
}

fn draw_test_pattern(socket: &mut Socket, window_id: u32, width: u32, height: u32) -> Result<(), &'static str> {
    let buffer_size = (width * height * 4) as usize;
    let mut buffer = Vec::new();
    buffer.resize(buffer_size, 0);
    
    // Draw diagonal lines to make window visible
    for y in 0..height {
        for x in 0..width {
            let offset = ((y * width + x) * 4) as usize;
            
            // Draw X pattern
            if x == y || x == width - y - 1 {
                buffer[offset] = 255;     // B
                buffer[offset + 1] = 255; // G
                buffer[offset + 2] = 255; // R
                buffer[offset + 3] = 255; // A
            }
            // Draw border
            else if x < 5 || x >= width - 5 || y < 5 || y >= height - 5 {
                buffer[offset] = 100;     // B
                buffer[offset + 1] = 100; // G
                buffer[offset + 2] = 255; // R (red border)
                buffer[offset + 3] = 255; // A
            }
            // Fill background
            else {
                buffer[offset] = 50;      // B
                buffer[offset + 1] = 150; // G
                buffer[offset + 2] = 50;  // R (greenish)
                buffer[offset + 3] = 255; // A
            }
        }
    }
    
    // Send BufferUpdated message (type 3)
    let mut payload = Vec::new();
    payload.extend_from_slice(&window_id.to_le_bytes());
    payload.extend(buffer);
    
    let header = MessageHeader {
        msg_type: 3, // BufferUpdated
        payload_size: payload.len() as u32,
    };
    
    let mut header_bytes = Vec::new();
    header_bytes.extend_from_slice(&header.msg_type.to_le_bytes());
    header_bytes.extend_from_slice(&header.payload_size.to_le_bytes());
    
    socket.write(&header_bytes).map_err(|_| "Failed to write header")?;
    socket.write(&payload).map_err(|_| "Failed to write payload")?;
    socket.flush().map_err(|_| "Failed to flush")?;
    
    println!("Sent buffer update ({} bytes)", buffer_size);
    Ok(())
}

fn send_destroy_window(socket: &mut Socket, window_id: u32) -> Result<(), &'static str> {
    // Prepare payload
    let mut payload = Vec::new();
    payload.extend_from_slice(&window_id.to_le_bytes());

    // Prepare header
    let header = MessageHeader {
        msg_type: 2, // DestroyWindow
        payload_size: payload.len() as u32,
    };

    // Send header
    let mut header_bytes = Vec::new();
    header_bytes.extend_from_slice(&header.msg_type.to_le_bytes());
    header_bytes.extend_from_slice(&header.payload_size.to_le_bytes());

    socket.write(&header_bytes).map_err(|_| "Failed to write header")?;
    socket.write(&payload).map_err(|_| "Failed to write payload")?;
    socket.flush().map_err(|_| "Failed to flush")?;

    Ok(())
}
