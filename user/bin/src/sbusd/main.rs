//! sbusd - Scarlet Bus daemon
//!
//! Central message routing daemon for inter-process communication.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

mod pending;

use pending::{
    PendingCall, oldest_pending_id_for_destination, remove_pending_calls_for_disconnected_client,
};
use sbus::{Argument, DEFAULT_SOCKET_PATH, Message, ServiceInfo};
use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::poll::{POLLERR, POLLHUP, POLLIN, POLLNVAL, POLLOUT, PollHandle, poll};
use std::println;
use std::socket::{ShutdownHow, Socket};
use std::string::{String, ToString};
use std::sync::{Arc, Mutex};
use std::thread;
use std::vec::Vec;

/// Service registry
static SERVICES: Mutex<BTreeMap<String, ServiceInfo>> = Mutex::new(BTreeMap::new());

/// Connected clients registry: client_id -> socket
/// Wrapped in Arc<Mutex<>> so it can be shared between threads
static CLIENTS: Mutex<BTreeMap<usize, Arc<Mutex<Socket>>>> = Mutex::new(BTreeMap::new());

/// Pending method calls ordered by their forwarding sequence.
///
/// Services currently reply with serial 0, so per-destination insertion order
/// is the only wire-compatible way to correlate concurrent calls.
static PENDING_CALLS: Mutex<BTreeMap<u64, PendingCall>> = Mutex::new(BTreeMap::new());
static NEXT_PENDING_ID: Mutex<u64> = Mutex::new(1);

const CLIENT_WRITE_TIMEOUT_MS: u64 = 1_000;
const MAX_MESSAGE_SIZE_BYTES: usize = 64 * 1024;
const MAX_MESSAGE_PAYLOAD_BYTES: usize = MAX_MESSAGE_SIZE_BYTES - sbus::MessageHeader::SIZE;

// When a service disconnects, sbusd broadcasts this signal so callers waiting
// on that service can abort instead of hanging forever.
const SBUS_SELF_BUS_NAME: &str = "org.scarlet.sbus";
const SBUS_SELF_OBJECT_PATH: &str = "/org/scarlet/sbus";
const SBUS_SELF_INTERFACE: &str = "org.scarlet.sbus";
const SBUS_SERVICE_UNREGISTERED_SIGNAL: &str = "ServiceUnregistered";

/// How long `poll` blocks waiting for a client socket to become readable.
/// Long enough that an idle handler thread sleeps instead of spinning, short
/// enough to stay responsive. This replaces the previous 1 ms busy-poll.
const CLIENT_READ_POLL_TIMEOUT_NS: i64 = 250_000_000;
/// Total deadline for a single `write_all` call when the peer stops draining.
const CLIENT_WRITE_TIMEOUT_NS: u64 = CLIENT_WRITE_TIMEOUT_MS * 1_000_000;
/// Slice size for write-backoff polling so the total timeout is still enforced.
const CLIENT_WRITE_POLL_SLICE_NS: u64 = 100_000_000;

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

                if let Err(error) = client_socket.set_nonblocking(true) {
                    println!(
                        "sbusd: Failed to enable non-blocking mode for client {}: {:?}",
                        client_id, error
                    );
                    continue;
                }

                // Keep reads and writes on duplicated handles. Writers still
                // share one mutex so complete sbus messages cannot interleave,
                // while an outbound retry cannot prevent the handler from
                // draining inbound messages on the read handle.
                let write_handle = match client_socket.as_handle().duplicate() {
                    Ok(handle) => handle,
                    Err(error) => {
                        println!(
                            "sbusd: Failed to duplicate client {} socket: {:?}",
                            client_id, error
                        );
                        continue;
                    }
                };
                let write_socket = match Socket::from_handle(write_handle) {
                    Ok(socket) => Arc::new(Mutex::new(socket)),
                    Err(error) => {
                        println!(
                            "sbusd: Failed to create client {} write socket: {:?}",
                            client_id, error
                        );
                        continue;
                    }
                };

                // Register client
                {
                    let mut clients = CLIENTS.lock();
                    clients.insert(client_id, write_socket.clone());
                }

                // Spawn client handler thread
                thread::spawn(move || {
                    handle_client(client_id, client_socket, write_socket);
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

fn handle_client(client_id: usize, mut read_socket: Socket, write_socket: Arc<Mutex<Socket>>) {
    println!("[Client {}] Handler started (non-blocking mode)", client_id);

    let mut buffer = [0u8; 4096];
    let mut read_buffer = Vec::new();
    let raw_read_handle = read_socket.as_raw() as u32;

    'client: loop {
        // Block until the socket is readable instead of busy-polling. A
        // duplicated read handle that never returns EOF (a known risk with
        // the handle-table duplication path) previously turned the 1 ms sleep
        // into a full-core spin. `poll` sleeps in the kernel until there is
        // genuine work, eliminating that failure mode entirely.
        let mut poll_handles = [PollHandle::new(raw_read_handle, POLLIN)];
        match poll(&mut poll_handles, CLIENT_READ_POLL_TIMEOUT_NS) {
            Ok(0) => continue,
            Ok(_) => {
                let revents = poll_handles[0].revents;
                if revents & POLLNVAL != 0 {
                    println!("[Client {}] Poll returned POLLNVAL", client_id);
                    break;
                }
                // POLLIN, POLLHUP, and POLLERR may all coexist with pending
                // data. Attempt a read in every case and let the read result
                // decide whether the connection is truly gone.
            }
            Err(code) => {
                println!("[Client {}] Poll error: {}", client_id, code);
                break;
            }
        }

        let read_result = read_socket.read(&mut buffer);

        match read_result {
            Ok(0) => {
                println!("[Client {}] Connection closed (read returned 0)", client_id);
                break;
            }
            Ok(n) => {
                read_buffer.extend_from_slice(&buffer[..n]);

                while read_buffer.len() >= sbus::MessageHeader::SIZE {
                    let mut header_bytes = [0u8; sbus::MessageHeader::SIZE];
                    header_bytes.copy_from_slice(&read_buffer[..sbus::MessageHeader::SIZE]);
                    let header = sbus::MessageHeader::from_le_bytes(header_bytes);

                    let payload_len = header.payload_length as usize;
                    if payload_len > MAX_MESSAGE_PAYLOAD_BYTES {
                        println!(
                            "[Client {}] Closing connection with oversized sbus payload: {} bytes",
                            client_id, payload_len
                        );
                        let _ = read_socket.shutdown(ShutdownHow::Both);
                        break 'client;
                    }

                    let total_len = sbus::MessageHeader::SIZE + payload_len;
                    if read_buffer.len() < total_len {
                        break;
                    }

                    let msg_bytes = read_buffer.drain(..total_len).collect::<Vec<_>>();

                    match sbus::from_bytes(msg_bytes) {
                        Ok(msg) => {
                            if let Err(e) = handle_message(client_id, &write_socket, &msg) {
                                println!("[Client {}] Error handling message: {:?}", client_id, e);
                            }
                        }
                        Err(e) => {
                            println!("[Client {}] Failed to parse message: {:?}", client_id, e);
                        }
                    }
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                // Spurious wakeup after poll reported readability. Just loop
                // back to poll instead of sleeping — the kernel will block
                // until real data arrives.
                continue;
            }
            Err(e) => {
                println!("[Client {}] Read error: {:?}", client_id, e);
                break;
            }
        }
    }

    // Cleanup registries without holding one global lock while acquiring
    // another or while writing a failure response.
    let removed_services = {
        let mut services = SERVICES.lock();
        let to_remove: Vec<String> = services
            .iter()
            .filter(|(_, service)| service.client_id == client_id)
            .map(|(bus_name, _)| bus_name.clone())
            .collect();
        for bus_name in &to_remove {
            services.remove(bus_name);
        }
        to_remove
    };
    for bus_name in &removed_services {
        println!("[Client {}] Unregistering service: {}", client_id, bus_name);
    }

    // Remove client from registry
    {
        let mut clients = CLIENTS.lock();
        clients.remove(&client_id);
    }
    println!("[Client {}] Removed from client registry", client_id);

    // Broadcast ServiceUnregistered for each removed service so callers
    // waiting on them can abort their wait instead of hanging forever.
    for bus_name in &removed_services {
        broadcast_service_unregistered(bus_name);
    }

    let failed_calls = {
        let mut pending_calls = PENDING_CALLS.lock();
        remove_pending_calls_for_disconnected_client(&mut pending_calls, client_id)
    };

    notify_failed_calls(
        failed_calls,
        "org.scarlet.sbus.Disconnected",
        "Destination service disconnected",
    );
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

            {
                let mut services = SERVICES.lock();
                services.insert(bus_name.clone(), service_info);
            }

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

            // Snapshot routing information under short, non-nested registry
            // locks. No global registry lock may be held across socket I/O.
            let destination_client_id = {
                let services = SERVICES.lock();
                services.get(destination).map(|service| service.client_id)
            };
            if let Some(destination_client_id) = destination_client_id {
                println!(
                    "sbusd: Routing to service {} (client {})",
                    destination, destination_client_id
                );

                if let Some(dest_socket) = get_client_socket(destination_client_id) {
                    let pending = PendingCall {
                        caller_client_id: client_id,
                        destination: destination.clone(),
                        destination_client_id,
                    };
                    let msg_bytes = msg.to_bytes()?;

                    // Forward the message to the destination service
                    println!(
                        "sbusd: Forwarding CALL_METHOD to client {}",
                        destination_client_id
                    );
                    // The destination write mutex defines request order on the
                    // wire. Allocate and insert the FIFO key only after taking
                    // it so reply correlation follows that same order.
                    let (pending_id, forward_result) = {
                        let mut sock = dest_socket.lock();
                        let pending_id = register_pending_call(pending);
                        let result = write_all(&mut sock, &msg_bytes);
                        if result.is_err() {
                            // A partial stream write makes subsequent framing
                            // unknowable. Tear down both duplicated handles so
                            // this route cannot be reused.
                            let _ = sock.shutdown(ShutdownHow::Both);
                        }
                        (pending_id, result)
                    };
                    if let Err(error) = forward_result {
                        println!(
                            "sbusd: Failed to forward call to client {}: {}",
                            destination_client_id, error
                        );
                        let mut failed_calls = Vec::new();
                        if let Some(pending) = remove_pending_call(pending_id) {
                            failed_calls.push(pending);
                        }
                        failed_calls.extend(quarantine_client(destination_client_id));
                        notify_failed_calls(
                            failed_calls,
                            "org.scarlet.sbus.Unavailable",
                            "Destination service is not accepting messages",
                        );
                        return Ok(());
                    }

                    // Don't send response yet - wait for the actual response from the service
                    // The response will be handled when we receive MethodReturn or MethodError
                    println!("sbusd: Call forwarded, waiting for response");
                    return Ok(()); // Don't send anything to the caller yet
                } else {
                    println!(
                        "sbusd: Destination client {} socket not found",
                        destination_client_id
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

            let pending = take_pending_call(client_id);

            if let Some(pending) = pending {
                println!(
                    "sbusd: Found pending call: caller={}, destination={}",
                    pending.caller_client_id, pending.destination
                );

                if let Some(caller_socket) = get_client_socket(pending.caller_client_id) {
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

            let pending = take_pending_call(client_id);

            if let Some(pending) = pending {
                println!(
                    "sbusd: Found pending call: caller={}, destination={}",
                    pending.caller_client_id, pending.destination
                );

                if let Some(caller_socket) = get_client_socket(pending.caller_client_id) {
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
        Message::Signal { .. } => {
            // Signals are broadcast to every other connected client. The
            // payload already carries the sender/path/interface metadata, so
            // clients can ignore signals they do not subscribe to.
            let bytes = msg.to_bytes()?;
            let destinations: Vec<(usize, Arc<Mutex<Socket>>)> = {
                let clients = CLIENTS.lock();
                clients
                    .iter()
                    .filter(|(destination_id, _)| **destination_id != client_id)
                    .map(|(destination_id, destination_socket)| {
                        (*destination_id, destination_socket.clone())
                    })
                    .collect()
            };
            for (destination_id, destination_socket) in destinations {
                if destination_id == client_id {
                    continue;
                }

                let mut destination_socket = destination_socket.lock();
                if let Err(error) = write_all(&mut destination_socket, &bytes) {
                    println!(
                        "sbusd: Failed to broadcast signal to client {}: {}",
                        destination_id, error
                    );
                }
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

fn get_client_socket(client_id: usize) -> Option<Arc<Mutex<Socket>>> {
    let clients = CLIENTS.lock();
    clients.get(&client_id).cloned()
}

/// Broadcast a ServiceUnregistered signal to every connected client.
///
/// Callers that are blocked waiting on a reply from the vanished service can
/// observe this signal and abort their wait, preventing an indefinite hang
/// when the service process crashes mid-operation (e.g. Files crashing while
/// its picker window is open).
fn broadcast_service_unregistered(bus_name: &str) {
    let mut args = Vec::new();
    args.push(Argument::String(String::from(bus_name)));
    let signal = Message::Signal {
        sender: String::from(SBUS_SELF_BUS_NAME),
        path: String::from(SBUS_SELF_OBJECT_PATH),
        interface: String::from(SBUS_SELF_INTERFACE),
        signal: String::from(SBUS_SERVICE_UNREGISTERED_SIGNAL),
        args,
    };
    let Ok(bytes) = signal.to_bytes() else {
        return;
    };

    let destinations: Vec<(usize, Arc<Mutex<Socket>>)> = {
        let clients = CLIENTS.lock();
        clients
            .iter()
            .map(|(id, sock)| (*id, sock.clone()))
            .collect()
    };
    for (destination_id, destination_socket) in destinations {
        let mut sock = destination_socket.lock();
        if let Err(error) = write_all(&mut sock, &bytes) {
            println!(
                "sbusd: Failed to broadcast ServiceUnregistered({}) to client {}: {}",
                bus_name, destination_id, error
            );
        }
    }
}

fn register_pending_call(pending: PendingCall) -> u64 {
    let pending_id = {
        let mut next_pending_id = NEXT_PENDING_ID.lock();
        let pending_id = *next_pending_id;
        *next_pending_id = next_pending_id.wrapping_add(1);
        if *next_pending_id == 0 {
            *next_pending_id = 1;
        }
        pending_id
    };

    let mut pending_calls = PENDING_CALLS.lock();
    pending_calls.insert(pending_id, pending);
    pending_id
}

fn remove_pending_call(pending_id: u64) -> Option<PendingCall> {
    let mut pending_calls = PENDING_CALLS.lock();
    pending_calls.remove(&pending_id)
}

fn take_pending_call(destination_client_id: usize) -> Option<PendingCall> {
    let mut pending_calls = PENDING_CALLS.lock();
    let pending_id = oldest_pending_id_for_destination(&pending_calls, destination_client_id)?;
    pending_calls.remove(&pending_id)
}

fn quarantine_client(client_id: usize) -> Vec<PendingCall> {
    {
        let mut clients = CLIENTS.lock();
        clients.remove(&client_id);
    }

    let removed_services = {
        let mut services = SERVICES.lock();
        let to_remove: Vec<String> = services
            .iter()
            .filter(|(_, service)| service.client_id == client_id)
            .map(|(bus_name, _)| bus_name.clone())
            .collect();
        for bus_name in &to_remove {
            services.remove(bus_name);
        }
        to_remove
    };
    for bus_name in &removed_services {
        println!(
            "sbusd: Quarantined service {} after client {} write failure",
            bus_name, client_id
        );
    }

    for bus_name in &removed_services {
        broadcast_service_unregistered(bus_name);
    }

    let mut pending_calls = PENDING_CALLS.lock();
    let to_remove: Vec<u64> = pending_calls
        .iter()
        .filter(|(_, pending)| pending.destination_client_id == client_id)
        .map(|(pending_id, _)| *pending_id)
        .collect();
    let mut failed = Vec::new();
    for pending_id in to_remove {
        if let Some(pending) = pending_calls.remove(&pending_id) {
            failed.push(pending);
        }
    }
    failed
}

fn notify_failed_calls(failed_calls: Vec<PendingCall>, error_name: &str, message: &str) {
    for pending in failed_calls {
        let Some(caller_socket) = get_client_socket(pending.caller_client_id) else {
            continue;
        };
        let reply = Message::MethodError {
            serial: 0,
            error_name: error_name.to_string(),
            message: message.to_string(),
        };
        if let Err(error) = send_message(&caller_socket, &reply) {
            println!(
                "sbusd: Failed to notify caller {}: {}",
                pending.caller_client_id, error
            );
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

/// Read the monotonic clock used for write-stall deadlines.
fn monotonic_time_ns() -> u64 {
    use std::syscall::{Syscall, syscall0};

    syscall0(Syscall::MonotonicTime) as u64
}

/// Write all bytes to socket.
fn write_all(socket: &mut Socket, bytes: &[u8]) -> Result<(), &'static str> {
    let mut written = 0;
    let raw_handle = socket.as_raw() as u32;
    let mut stall_deadline_ns = monotonic_time_ns().saturating_add(CLIENT_WRITE_TIMEOUT_NS);

    while written < bytes.len() {
        match socket.write(&bytes[written..]) {
            Ok(0) => {
                println!("[write_all] Write returned 0 (disconnected)");
                let _ = socket.shutdown(ShutdownHow::Both);
                return Err("Failed to send: disconnected");
            }
            Ok(n) => {
                written += n;
                stall_deadline_ns = monotonic_time_ns().saturating_add(CLIENT_WRITE_TIMEOUT_NS);
                println!("[write_all] Wrote {} bytes (total: {})", n, written);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                let now_ns = monotonic_time_ns();
                if now_ns >= stall_deadline_ns {
                    println!(
                        "[write_all] Timed out after {}ms (wrote {} of {} bytes)",
                        CLIENT_WRITE_TIMEOUT_MS,
                        written,
                        bytes.len()
                    );
                    let _ = socket.shutdown(ShutdownHow::Both);
                    return Err("Failed to send: timed out");
                }

                // A readiness wait may return spuriously. Derive the
                // timeout from the monotonic deadline instead of adding
                // the requested poll slice to a synthetic elapsed counter.
                let remaining_ns = stall_deadline_ns.saturating_sub(now_ns);
                let slice_ns = remaining_ns.min(CLIENT_WRITE_POLL_SLICE_NS) as i64;
                let mut poll_handles = [PollHandle::new(raw_handle, POLLOUT)];
                if let Err(code) = poll(&mut poll_handles, slice_ns) {
                    println!("[write_all] Poll error while waiting to write: {}", code);
                    let _ = socket.shutdown(ShutdownHow::Both);
                    return Err("Failed to wait for writable socket");
                }
                if poll_handles[0].revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
                    // Retry once so the socket operation reports the
                    // precise disconnect/broken-pipe condition.
                }
            }
            Err(e) => {
                println!("[write_all] Write error: {:?}", e);
                let _ = socket.shutdown(ShutdownHow::Both);
                return Err("Failed to send");
            }
        }
    }
    Ok(())
}
