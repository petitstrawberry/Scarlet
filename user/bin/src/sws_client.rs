//! SWS Test Client - Simple client for testing Scarlet Window Server IPC

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use sws_protocol as protocol;

use std::ipc::{SharedMemory, permissions};
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
        Ok(protocol::WindowCreated {
            window_id,
            shm_size,
        }) => {
            println!("Window created successfully!");
            println!("  Window ID: {}", window_id);
            println!("  Shared memory size: {} bytes", shm_size);

            // Receive SHM handle out-of-band
            println!("Receiving SHM handle...");
            let shm_handle = match protocol::recv_shm_handle(&socket) {
                Ok(h) => h,
                Err(e) => {
                    println!("Failed to receive SHM handle: {:?}", e);
                    return 1;
                }
            };
            println!("SHM handle received (raw: {})", shm_handle.as_raw());

            // Reconstruct SharedMemory wrapper from handle
            let shm = match SharedMemory::from_handle(shm_handle) {
                Ok(s) => s,
                Err(e) => {
                    println!("Failed to wrap handle as SharedMemory: {:?}", e);
                    return 1;
                }
            };

            // Map SHM into client address space
            println!("Mapping SHM into client address space...");
            let mapper = match shm.as_handle().as_memory_mapping() {
                Ok(m) => m,
                Err(_) => {
                    println!("SharedMemory does not support mapping");
                    return 1;
                }
            };
            let mapped_addr = match mapper.mmap(0, shm_size as usize, permissions::READ_WRITE, 0, 0)
            {
                Ok(addr) => addr,
                Err(_) => {
                    println!("Failed to mmap SHM");
                    return 1;
                }
            };
            println!("SHM mapped at 0x{:x}", mapped_addr);

            // Draw something to SHM (simple red gradient)
            println!("Drawing to SHM buffer...");
            unsafe {
                let width = 400u32;
                let height = 300u32;
                let ptr = mapped_addr as *mut u8;
                for y in 0..height {
                    for x in 0..width {
                        let idx = ((y * width + x) * 4) as usize;
                        let intensity = ((x + y) * 255 / (width + height)) as u8;
                        *ptr.add(idx + 0) = 0; // B
                        *ptr.add(idx + 1) = 0; // G
                        *ptr.add(idx + 2) = intensity; // R
                        *ptr.add(idx + 3) = 255; // A
                    }
                }

                // Verify first few pixels after drawing
                println!(
                    "Client drew first 16 bytes: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                    *ptr.add(0),
                    *ptr.add(1),
                    *ptr.add(2),
                    *ptr.add(3),
                    *ptr.add(4),
                    *ptr.add(5),
                    *ptr.add(6),
                    *ptr.add(7),
                    *ptr.add(8),
                    *ptr.add(9),
                    *ptr.add(10),
                    *ptr.add(11),
                    *ptr.add(12),
                    *ptr.add(13),
                    *ptr.add(14),
                    *ptr.add(15)
                );
            }
            println!("Drawing complete");

            // Send damage notification (entire window)
            println!("Sending damage notification...");
            if let Err(e) = protocol::write_update_buffer(&mut socket, window_id, 0, 0, 400, 300) {
                println!("Failed to send update_buffer: {:?}", e);
            } else {
                println!("Damage notification sent");
            }

            // Now enter event loop to receive input events
            println!("Window open - move your mouse over it!");
            println!("Receiving input events... (press Ctrl+C to exit)");

            // Enable non-blocking mode for event loop
            if let Err(e) = socket.set_nonblocking(true) {
                println!("Failed to enable non-blocking mode: {:?}", e);
                return 1;
            }
            println!("Non-blocking mode enabled for event reception");

            let mut event_count = 0;
            let max_events = 1000; // Limit to prevent infinite loop

            // Event loop: receive and log input events
            loop {
                if event_count >= max_events {
                    println!("Received {} events, exiting...", event_count);
                    break;
                }

                // Try to read next message (non-blocking)
                match protocol::read_frame(&mut socket) {
                    Ok((msg_type, payload)) => {
                        match protocol::parse_server_message(msg_type, &payload) {
                            Ok(protocol::ServerMessage::InputEvent {
                                type_, code, value, ..
                            }) => {
                                // Decode common event types
                                const EV_SYN: u16 = 0x00;
                                const EV_KEY: u16 = 0x01;
                                const EV_ABS: u16 = 0x03;
                                const ABS_X: u16 = 0x00;
                                const ABS_Y: u16 = 0x01;
                                const BTN_LEFT: u16 = 0x110;

                                match type_ {
                                    EV_ABS if code == ABS_X => {
                                        println!("[Input] Mouse X: {}", value);
                                    }
                                    EV_ABS if code == ABS_Y => {
                                        println!("[Input] Mouse Y: {}", value);
                                    }
                                    EV_KEY if code == BTN_LEFT => {
                                        println!(
                                            "[Input] Left Button: {}",
                                            if value == 1 { "PRESSED" } else { "RELEASED" }
                                        );
                                    }
                                    EV_SYN => {
                                        // Frame boundary - don't log
                                    }
                                    _ => {
                                        println!(
                                            "[Input] Event - type: {:#x}, code: {:#x}, value: {}",
                                            type_, code, value
                                        );
                                    }
                                }

                                event_count += 1;
                            }
                            Ok(_) => {
                                // Ignore other server messages
                            }
                            Err(e) => {
                                println!("[Client] Parse error: {:?}", e);
                            }
                        }
                    }
                    Err(protocol::ProtocolError::IoDisconnected) => {
                        println!("[Client] Server disconnected");
                        break;
                    }
                    Err(protocol::ProtocolError::IoWouldBlock) => {
                        // No data available, continue polling
                        continue;
                    }
                    Err(e) => {
                        println!("[Client] Read error: {:?}", e);
                        break;
                    }
                }
            }

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
