//! sbusd - Scarlet Bus daemon
//!
//! Central message routing daemon for inter-process communication.

#![no_std]
#![no_main]

extern crate alloc;

extern crate scarlet_std as std;

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use sbus::{DEFAULT_SOCKET_PATH, Message, ServiceInfo};
use std::io::{Read, Write};
use std::println;
use std::socket::Socket;
use std::sync::Mutex;
use std::thread;

/// Service registry
static SERVICES: Mutex<BTreeMap<String, ServiceInfo>> = Mutex::new(BTreeMap::new());

/// Connected clients registry: client_id -> socket
/// Wrapped in Arc<Mutex<>> so it can be shared between threads
static CLIENTS: Mutex<BTreeMap<usize, Arc<Mutex<Socket>>>> = Mutex::new(BTreeMap::new());

/// Pending method calls: caller_client_id -> PendingCall
/// Tracks in-flight method calls so we can route responses back to callers
/// Key is caller_client_id, value contains which service they called
static PENDING_CALLS: Mutex<BTreeMap<usize, PendingCall>> = Mutex::new(BTreeMap::new());

/// Serial number generator for method calls
static NEXT_SERIAL: Mutex<u32> = Mutex::new(1);

/// Pending method call information
#[derive(Clone, Debug)]
struct PendingCall {
    caller_client_id: usize,
    destination: String,
}

/// Connected clients
struct Client {
    id: usize,
    socket: Socket,
    name: core::option::Option<String>,
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("sbusd: Starting...");

    // Create listening socket
    let socket = match Socket::new() {
        Ok(s) => s,
        Err(e) => {
            println!("sbusd: Failed to create socket: {:?}", e);
            return 1;
        }
    };

    // Bind and listen
    if let Err(e) = socket.bind(DEFAULT_SOCKET_PATH) {
        println!("sbusd: Failed to bind socket: {:?}", e);
        return 1;
    }

    if let Err(e) = socket.listen(128) {
        println!("sbusd: Failed to listen: {:?}", e);
        return 1;
    }

    println!("sbusd: Listening on {}", DEFAULT_SOCKET_PATH);

    // Accept clients
    let mut client_id_counter = 0usize;
    loop {
        match socket.accept() {
            Ok(client_socket) => {
                let client_id = client_id_counter;
                client_id_counter += 1;

                println!("sbusd: Accepted client {}", client_id);

                // Wrap socket in Arc<Mutex<>> so it can be shared
                let client_socket = Arc::new(Mutex::new(client_socket));

                // Register client
                {
                    let mut clients = CLIENTS.lock();
                    clients.insert(client_id, client_socket.clone());
                }

                // Spawn client handler thread
                thread::spawn(move || {
                    handle_client(client_id, client_socket);
                });
            }
            Err(e) => {
                println!("sbusd: Accept failed: {:?}", e);
                // Sleep a bit before retrying
                let _ = std::thread::sleep(core::time::Duration::from_millis(100));
            }
        }
    }
}

fn handle_client(client_id: usize, socket: Arc<Mutex<Socket>>) {
    println!("[Client {}] Handler started", client_id);

    // Set non-blocking mode
    {
        let sock = socket.lock();
        if let Err(e) = sock.set_nonblocking(true) {
            println!("[Client {}] Failed to set non-blocking: {:?}", client_id, e);
            return;
        }
    }

    let mut buffer = [0u8; 4096];
    let mut read_buffer = Vec::new();

    loop {
        // Try to read data
        let read_result = {
            let mut sock = socket.lock();
            sock.read(&mut buffer)
        };

        match read_result {
            Ok(0) => {
                println!("[Client {}] Connection closed (read returned 0)", client_id);
                break;
            }
            Ok(n) => {
                println!("[Client {}] Read {} bytes", client_id, n);
                read_buffer.extend_from_slice(&buffer[..n]);

                // Try to parse complete messages
                while read_buffer.len() >= 16 {
                    // Check if we have a complete message
                    let mut header_bytes = [0u8; 16];
                    header_bytes.copy_from_slice(&read_buffer[0..16]);
                    let header = sbus::MessageHeader::from_le_bytes(header_bytes);

                    let total_len = 16 + header.payload_length as usize;
                    if read_buffer.len() < total_len {
                        // Need more data
                        break;
                    }

                    // Extract complete message
                    let msg_bytes = read_buffer.drain(..total_len).collect::<Vec<_>>();

                    // Parse and handle message
                    match sbus::from_bytes(msg_bytes) {
                        Ok(msg) => {
                            if let Err(e) = handle_message(client_id, &socket, &msg) {
                                println!("[Client {}] Error handling message: {:?}", client_id, e);
                            }
                        }
                        Err(e) => {
                            println!("[Client {}] Failed to parse message: {:?}", client_id, e);
                        }
                    }
                }
            }
            Err(_) => {
                // WouldBlock - no data available
                let _ = std::thread::sleep(core::time::Duration::from_millis(10));
            }
        }
    }

    // Cleanup: unregister services from this client
    let mut services = SERVICES.lock();
    let mut to_remove = Vec::new();
    for (bus_name, service) in services.iter() {
        if service.client_id == client_id {
            to_remove.push(bus_name.clone());
        }
    }
    for bus_name in to_remove {
        println!("[Client {}] Unregistering service: {}", client_id, bus_name);
        services.remove(&bus_name);
    }
    drop(services);

    // Remove client from registry
    let mut clients = CLIENTS.lock();
    clients.remove(&client_id);
    println!("[Client {}] Removed from client registry", client_id);
}

fn handle_message(
    client_id: usize,
    socket: &Arc<Mutex<Socket>>,
    msg: &Message,
) -> Result<(), &'static str> {
    match msg {
        Message::Hello { client_name } => {
            println!("[Client {}] Hello: {}", client_id, client_name);
            Ok(())
        }
        Message::RegisterService { bus_name } => {
            println!("[Client {}] Registering service: {}", client_id, bus_name);

            // TODO: Get actual PID
            let service_info = ServiceInfo {
                bus_name: bus_name.clone(),
                pid: 0,
                client_id,
            };

            let mut services = SERVICES.lock();
            services.insert(bus_name.clone(), service_info);

            println!(
                "sbusd: Service registered: {} (client {})",
                bus_name, client_id
            );

            // Send acknowledgment
            send_ok(socket)?;

            Ok(())
        }
        Message::CallMethod {
            destination,
            path: _,
            interface: _,
            method: _,
            args: _,
        } => {
            println!("[Client {}] CallMethod: {}", client_id, destination);

            // Look up destination service
            let services = SERVICES.lock();
            if let Some(service) = services.get(destination) {
                println!(
                    "sbusd: Routing to service {} (client {})",
                    destination, service.client_id
                );

                // Get destination client socket
                let clients = CLIENTS.lock();
                if let Some(dest_socket) = clients.get(&service.client_id) {
                    // Generate a serial number for this call
                    let _serial = {
                        let mut s = NEXT_SERIAL.lock();
                        let serial = *s;
                        *s = s.wrapping_add(1);
                        serial
                    };

                    // Track this pending call
                    // Note: Services like stemd don't read the serial from the message header,
                    // so they always respond with serial=0. We track by caller_id.
                    let pending = PendingCall {
                        caller_client_id: client_id,
                        destination: destination.clone(),
                    };
                    {
                        let mut pending_calls = PENDING_CALLS.lock();
                        pending_calls.insert(client_id, pending);
                    }

                    // Forward the message to the destination service
                    println!(
                        "sbusd: Forwarding CALL_METHOD to client {}",
                        service.client_id
                    );
                    let msg_bytes = msg.to_bytes()?;

                    {
                        let mut sock = dest_socket.lock();
                        write_all(&mut sock, &msg_bytes)?;
                    }
                    drop(clients);

                    // Don't send response yet - wait for the actual response from the service
                    // The response will be handled when we receive MethodReturn or MethodError
                    println!("sbusd: Call forwarded, waiting for response");
                    return Ok(()); // Don't send anything to the caller yet
                } else {
                    println!(
                        "sbusd: Destination client {} socket not found",
                        service.client_id
                    );
                    let reply = Message::MethodError {
                        serial: 0,
                        error_name: "org.scarlet.sbus.Error".to_string(),
                        message: "Destination client socket not found".to_string(),
                    };
                    send_message(socket, &reply)?;
                }
            } else {
                println!("sbusd: Service not found: {}", destination);

                let mut error_msg = Vec::new();
                error_msg.extend_from_slice("Service '".as_bytes());
                error_msg.extend_from_slice(destination.as_bytes());
                error_msg.extend_from_slice("' not found".as_bytes());
                let error_string = String::from_utf8_lossy(&error_msg).to_string();

                let reply = Message::MethodError {
                    serial: 0,
                    error_name: "org.scarlet.sbus.ServiceNotFound".to_string(),
                    message: error_string,
                };

                println!("[Client {}] Sending MethodError...", client_id);
                match send_message(socket, &reply) {
                    Ok(_) => println!("[Client {}] MethodError sent successfully", client_id),
                    Err(e) => {
                        println!("[Client {}] Failed to send MethodError: {:?}", client_id, e)
                    }
                }
            }

            Ok(())
        }
        Message::MethodReturn { serial, result } => {
            println!(
                "[Client {}] MethodReturn: serial={}, result len={}",
                client_id,
                serial,
                result.len()
            );

            // Look up the pending call by finding one where destination matches this service
            // We need to find which caller called this service
            let pending = {
                let mut pending_calls = PENDING_CALLS.lock();
                let mut found_key = None;
                for (caller_id, pending_call) in pending_calls.iter() {
                    // Check if this service is the destination
                    // We need to look up which service has this client_id
                    let services = SERVICES.lock();
                    for (bus_name, service_info) in services.iter() {
                        if service_info.client_id == client_id
                            && pending_call.destination == *bus_name
                        {
                            found_key = Some(*caller_id);
                            break;
                        }
                    }
                    if found_key.is_some() {
                        break;
                    }
                }
                if let Some(key) = found_key {
                    pending_calls.remove(&key)
                } else {
                    None
                }
            };

            if let Some(pending) = pending {
                println!(
                    "sbusd: Found pending call: caller={}, destination={}",
                    pending.caller_client_id, pending.destination
                );

                // Get the caller's socket
                let clients = CLIENTS.lock();
                if let Some(caller_socket) = clients.get(&pending.caller_client_id) {
                    // Forward the MethodReturn back to the caller
                    println!(
                        "sbusd: Forwarding MethodReturn to caller {}",
                        pending.caller_client_id
                    );

                    // Serialize the original message and reset serial to 0
                    let mut bytes = msg.to_bytes()?;
                    // Set serial to 0 in the header (bytes 4-7)
                    bytes[4..8].copy_from_slice(&0u32.to_le_bytes());

                    let mut sock = caller_socket.lock();
                    write_all(&mut sock, &bytes)?;
                } else {
                    println!(
                        "sbusd: Caller {} socket not found",
                        pending.caller_client_id
                    );
                }
            } else {
                println!(
                    "sbusd: No pending call found for service client {}",
                    client_id
                );
            }

            Ok(())
        }
        Message::MethodError {
            serial,
            error_name,
            message,
        } => {
            println!(
                "[Client {}] MethodError: serial={}, error={}, message={}",
                client_id, serial, error_name, message
            );

            // Look up the pending call by finding one where destination matches this service
            let pending = {
                let mut pending_calls = PENDING_CALLS.lock();
                let mut found_key = None;
                for (caller_id, pending_call) in pending_calls.iter() {
                    // Check if this service is the destination
                    let services = SERVICES.lock();
                    for (bus_name, service_info) in services.iter() {
                        if service_info.client_id == client_id
                            && pending_call.destination == *bus_name
                        {
                            found_key = Some(*caller_id);
                            break;
                        }
                    }
                    if found_key.is_some() {
                        break;
                    }
                }
                if let Some(key) = found_key {
                    pending_calls.remove(&key)
                } else {
                    None
                }
            };

            if let Some(pending) = pending {
                println!(
                    "sbusd: Found pending call: caller={}, destination={}",
                    pending.caller_client_id, pending.destination
                );

                // Get the caller's socket
                let clients = CLIENTS.lock();
                if let Some(caller_socket) = clients.get(&pending.caller_client_id) {
                    // Forward the MethodError back to the caller
                    println!(
                        "sbusd: Forwarding MethodError to caller {}",
                        pending.caller_client_id
                    );

                    // Serialize the original message and reset serial to 0
                    let mut bytes = msg.to_bytes()?;
                    // Set serial to 0 in the header (bytes 4-7)
                    bytes[4..8].copy_from_slice(&0u32.to_le_bytes());

                    let mut sock = caller_socket.lock();
                    write_all(&mut sock, &bytes)?;
                } else {
                    println!(
                        "sbusd: Caller {} socket not found",
                        pending.caller_client_id
                    );
                }
            } else {
                println!(
                    "sbusd: No pending call found for service client {}",
                    client_id
                );
            }

            Ok(())
        }
        _ => {
            println!(
                "[Client {}] Unhandled message type: {:?}",
                client_id,
                msg.msg_type()
            );
            Ok(())
        }
    }
}

fn send_ok(socket: &Arc<Mutex<Socket>>) -> Result<(), &'static str> {
    let msg = Message::MethodReturn {
        serial: 0,
        result: Vec::new(),
    };

    let bytes = msg.to_bytes()?;

    let mut sock = socket.lock();
    write_all(&mut sock, &bytes)
}

fn send_message(socket: &Arc<Mutex<Socket>>, msg: &Message) -> Result<(), &'static str> {
    let bytes = msg.to_bytes()?;
    println!("[send_message] Sending {} bytes", bytes.len());

    let mut sock = socket.lock();
    write_all(&mut sock, &bytes)?;

    println!("[send_message] Successfully sent all {} bytes", bytes.len());
    Ok(())
}

/// Write all bytes to socket
fn write_all(socket: &mut Socket, bytes: &[u8]) -> Result<(), &'static str> {
    let mut written = 0;
    while written < bytes.len() {
        match socket.write(&bytes[written..]) {
            Ok(0) => {
                println!("[write_all] Write returned 0 (disconnected)");
                return Err("Failed to send: disconnected");
            }
            Ok(n) => {
                written += n;
                println!("[write_all] Wrote {} bytes (total: {})", n, written);
            }
            Err(e) => {
                println!("[write_all] Write error: {:?}", e);
                return Err("Failed to send");
            }
        }
    }
    Ok(())
}
