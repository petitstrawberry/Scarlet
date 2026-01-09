//! SWS Test Client - Simple client for testing Scarlet Window Server IPC

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use sws_protocol as protocol;

use std::ipc::{SharedMemory, permissions};
use std::println;
use std::socket::Socket;
use std::sync::Mutex;
use std::thread;

/// Global flag to signal input thread to stop
static SHOULD_EXIT: Mutex<bool> = Mutex::new(false);

/// Input event handling thread
fn input_event_thread(socket: Socket) {
    println!("[InputThread] Started");
    let mut socket = socket;

    loop {
        {
            let should_exit = SHOULD_EXIT.lock();
            if *should_exit {
                println!("[InputThread] Exit signal received");
                break;
            }
        }

        // Try to read input event
        match protocol::read_frame(&mut socket) {
            Ok((msg_type, payload)) => {
                match protocol::parse_server_message(msg_type, &payload) {
                    Ok(protocol::ServerMessage::InputEvent {
                        time,
                        type_,
                        code,
                        value,
                    }) => {
                        println!(
                            "[Input] Event - type: {:#x}, code: {:#x}, value: {}",
                            type_, code, value
                        );

                        // Decode common event types
                        const EV_SYN: u16 = 0x00;
                        const EV_KEY: u16 = 0x01;
                        const EV_ABS: u16 = 0x03;
                        const ABS_X: u16 = 0x00;
                        const ABS_Y: u16 = 0x01;
                        const BTN_LEFT: u16 = 0x110;
                        const BTN_RIGHT: u16 = 0x111;

                        match type_ {
                            EV_ABS if code == ABS_X => {
                                println!("  → Mouse X: {}", value);
                            }
                            EV_ABS if code == ABS_Y => {
                                println!("  → Mouse Y: {}", value);
                            }
                            EV_KEY if code == BTN_LEFT => {
                                println!(
                                    "  → Left Button: {}",
                                    if value == 1 { "PRESSED" } else { "RELEASED" }
                                );
                            }
                            EV_KEY if code == BTN_RIGHT => {
                                println!(
                                    "  → Right Button: {}",
                                    if value == 1 { "PRESSED" } else { "RELEASED" }
                                );
                            }
                            EV_SYN => {
                                // Frame boundary
                            }
                            _ => {}
                        }
                    }
                    Ok(_) => {
                        // Ignore other server messages
                    }
                    Err(e) => {
                        println!("[InputThread] Parse error: {:?}", e);
                    }
                }
            }
            Err(protocol::ProtocolError::IoDisconnected) => {
                println!("[InputThread] Disconnected");
                break;
            }
            Err(e) => {
                println!("[InputThread] Read error: {:?}", e);
                // Small delay to avoid busy loop on error
                thread::sleep(core::time::Duration::from_millis(100));
            }
        }
    }

    println!("[InputThread] Exiting");
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

            // Start input event thread to receive events asynchronously
            // Note: We use the same socket for both send and receive
            // The input thread will read events, main thread just waits
            println!("Starting input event receiver thread...");

            // Create a second socket connection for input events
            // (since we can't clone Socket and need non-blocking behavior)
            let input_socket = match Socket::new() {
                Ok(s) => s,
                Err(e) => {
                    println!("Failed to create input socket: {:?}", e);
                    return 1;
                }
            };

            if let Err(e) = input_socket.connect("/tmp/sws.sock") {
                println!("Failed to connect input socket: {:?}", e);
                // Continue anyway without input events
            } else {
                thread::spawn(move || {
                    input_event_thread(input_socket);
                });
            }

            // Keep window open for demonstration (input events will be logged)
            println!("Window open - move your mouse over it! (closing in 60 seconds)");
            scarlet_std::thread::sleep(core::time::Duration::from_secs(60));

            // Signal input thread to exit
            {
                let mut should_exit = SHOULD_EXIT.lock();
                *should_exit = true;
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
