//! IPC Server module - handles client connections and messages

use super::protocol::{MessageHeader, ServerMessage};
use std::io::{Read, Write};
use std::println;
use std::socket::Socket;
use std::sync::Mutex;
use std::thread;
use std::vec::Vec;

/// Global event queue for IPC events
static EVENT_QUEUE: Mutex<Vec<IpcEvent>> = Mutex::new(Vec::new());

/// Add an event to the global queue
pub fn push_ipc_event(event: IpcEvent) {
    let mut queue = EVENT_QUEUE.lock();
    queue.push(event);
}

/// Get all pending events from the queue
pub fn pop_all_ipc_events() -> Vec<IpcEvent> {
    let mut queue = EVENT_QUEUE.lock();
    let events = queue.clone();
    queue.clear();
    events
}

/// IPC Server - manages Socket VFS connections
pub struct IpcServer {
    socket: Option<Socket>,
    socket_path: &'static str,
    accept_thread_started: bool,
}

impl IpcServer {
    /// Create a new IPC server
    pub fn new(socket_path: &'static str) -> Result<Self, &'static str> {
        println!("[IpcServer] Initializing at {}", socket_path);

        Ok(Self {
            socket: None,
            socket_path,
            accept_thread_started: false,
        })
    }

    /// Start listening for connections in a separate thread
    pub fn listen(&mut self) -> Result<(), &'static str> {
        if self.accept_thread_started {
            return Ok(());
        }

        println!("[IpcServer] Creating socket at {}", self.socket_path);

        // Create and setup socket
        let server_socket = Socket::new().map_err(|e| {
            println!("[IpcServer] Socket::new() failed: {:?}", e);
            "Failed to create socket"
        })?;

        println!(
            "[IpcServer] Socket created (handle {})",
            server_socket.as_raw()
        );

        server_socket.bind(self.socket_path).map_err(|e| {
            println!("[IpcServer] bind() failed: {:?}", e);
            "Failed to bind socket"
        })?;

        println!("[IpcServer] Socket bound to {}", self.socket_path);

        server_socket.listen(10).map_err(|e| {
            println!("[IpcServer] listen() failed: {:?}", e);
            "Failed to listen"
        })?;

        let server_handle = server_socket.as_raw();
        println!("[IpcServer] Socket listening (handle {})", server_handle);

        // Move socket to accept thread
        // HandleTable is cloned but Arc<SocketObject> is shared
        thread::spawn(move || {
            accept_thread_main(server_socket);
        });

        self.accept_thread_started = true;
        println!("[IpcServer] Accept thread started");
        Ok(())
    }

    /// Get all pending IPC events (non-blocking)
    pub fn process_messages(&mut self) -> Result<Vec<IpcEvent>, &'static str> {
        Ok(pop_all_ipc_events())
    }

    /// Send a message to a specific client (not yet implemented for multi-threaded)
    pub fn send_to_client(
        &mut self,
        _client_id: usize,
        message: ServerMessage,
    ) -> Result<(), &'static str> {
        // TODO: Implement client response mechanism
        // For now, just log
        println!(
            "[IpcServer] Would send message type {} to client {}",
            message.type_id(),
            _client_id
        );
        Ok(())
    }
}

/// Accept thread main function
fn accept_thread_main(server_socket: Socket) {
    println!(
        "[AcceptThread] Starting with socket handle {}",
        server_socket.as_raw()
    );

    let mut client_id_counter: usize = 0;

    loop {
        // Accept new client (blocking)
        println!(
            "[AcceptThread] Calling accept() on handle {}...",
            server_socket.as_raw()
        );
        match server_socket.accept() {
            Ok(client_socket) => {
                let client_id = client_id_counter;
                client_id_counter += 1;

                println!(
                    "[AcceptThread] Accepted client {} (socket handle: {})",
                    client_id,
                    client_socket.as_raw()
                );

                // Spawn client handler thread
                thread::spawn(move || {
                    client_thread_main(client_id, client_socket);
                });
            }
            Err(e) => {
                println!("[AcceptThread] Accept failed: {:?}", e);
                // TODO: Kernel accept() should block instead of returning WouldBlock
                // For now, break on error to avoid busy loop
                println!("[AcceptThread] Exiting due to accept error");
                break;
            }
        }
    }

    println!("[AcceptThread] Thread exiting");
}

/// Client thread main function
fn client_thread_main(client_id: usize, mut socket: Socket) {
    println!(
        "[ClientThread {}] Started (socket handle: {})",
        client_id,
        socket.as_raw()
    );

    loop {
        // Read message header (blocking per client)
        let mut header_buf = [0u8; 8];

        println!(
            "[ClientThread {}] Waiting for message... (socket handle: {})",
            client_id,
            socket.as_raw()
        );
        println!("[ClientThread {}] Calling read()...", client_id);
        match socket.read(&mut header_buf) {
            Ok(n) if n == 8 => {
                println!("[ClientThread {}] Read {} byte header", client_id, n);
                // Parse header
                let msg_type = u32::from_le_bytes([
                    header_buf[0],
                    header_buf[1],
                    header_buf[2],
                    header_buf[3],
                ]);
                let payload_size = u32::from_le_bytes([
                    header_buf[4],
                    header_buf[5],
                    header_buf[6],
                    header_buf[7],
                ]);

                println!(
                    "[ClientThread {}] msg_type={}, payload_size={}",
                    client_id, msg_type, payload_size
                );

                // Read payload if needed
                let mut payload = Vec::new();
                if payload_size > 0 {
                    payload.resize(payload_size as usize, 0);
                    match socket.read(&mut payload) {
                        Ok(n) if n == payload_size as usize => {
                            println!("[ClientThread {}] Read {} byte payload", client_id, n);
                        }
                        Ok(n) => {
                            println!(
                                "[ClientThread {}] Incomplete payload read: {} of {}",
                                client_id, n, payload_size
                            );
                            continue;
                        }
                        Err(e) => {
                            println!("[ClientThread {}] Payload read error: {:?}", client_id, e);
                            continue;
                        }
                    }
                }

                // Parse message and push to global queue
                if let Some(event) = parse_message(client_id, msg_type, &payload) {
                    println!("[ClientThread {}] Pushing event to queue", client_id);
                    push_ipc_event(event);

                    // Send response immediately (simplified - just ACK for now)
                    if msg_type == 1 {
                        // CreateWindow
                        if payload.len() >= 8 {
                            let width = u32::from_le_bytes([
                                payload[0], payload[1], payload[2], payload[3],
                            ]);
                            let height = u32::from_le_bytes([
                                payload[4], payload[5], payload[6], payload[7],
                            ]);

                            // Calculate buffer size
                            let buffer_size = (width * height * 4) as usize;

                            // Send WindowCreated response
                            let window_id = client_id as u32 + 100; // Temporary ID
                            send_window_created(&mut socket, window_id, buffer_size);
                            
                            // Trigger compositor redraw
                            push_ipc_event(IpcEvent::WindowCreated { window_id, width, height });
                        }
                    } else if msg_type == 2 {
                        // DestroyWindow
                        if payload.len() >= 4 {
                            let window_id = u32::from_le_bytes([
                                payload[0], payload[1], payload[2], payload[3],
                            ]);
                            println!("[ClientThread {}] DestroyWindow request for window {}", client_id, window_id);
                            
                            // Trigger compositor to destroy window
                            push_ipc_event(IpcEvent::WindowDestroyed { window_id });
                        }
                    } else if msg_type == 3 {
                        // BufferUpdated - includes full buffer data
                        if payload.len() >= 4 {
                            let window_id = u32::from_le_bytes([
                                payload[0], payload[1], payload[2], payload[3],
                            ]);
                            let buffer_data = &payload[4..];
                            println!(
                                "[ClientThread {}] BufferUpdated for window {} ({} bytes)",
                                client_id, window_id, buffer_data.len()
                            );
                            
                            // Clone buffer data
                            let mut buffer = Vec::new();
                            buffer.extend_from_slice(buffer_data);
                            
                            // Trigger compositor to update window buffer with new data
                            push_ipc_event(IpcEvent::ClientBufferUpdate { window_id, buffer });
                        }
                    }
                }
            }
            Ok(0) => {
                // Connection closed
                println!("[ClientThread {}] Client disconnected", client_id);
                break;
            }
            Ok(n) => {
                println!(
                    "[ClientThread {}] Unexpected read size: {} (expected 8)",
                    client_id, n
                );
                break;
            }
            Err(e) => {
                // Error or no data
                // For blocking socket, this means error
                println!("[ClientThread {}] Read error: {:?}", client_id, e);
                break;
            }
        }
    }

    println!("[ClientThread {}] Exiting", client_id);
}

/// Send WindowCreated message
fn send_window_created(socket: &mut Socket, window_id: u32, shm_size: usize) {
    let mut payload = Vec::new();
    payload.extend_from_slice(&window_id.to_le_bytes());
    payload.extend_from_slice(&shm_size.to_le_bytes());

    let header = MessageHeader {
        msg_type: 10, // WindowCreated
        payload_size: payload.len() as u32,
    };

    let mut header_buf = Vec::new();
    header_buf.extend_from_slice(&header.msg_type.to_le_bytes());
    header_buf.extend_from_slice(&header.payload_size.to_le_bytes());

    let _ = socket.write(&header_buf);
    let _ = socket.write(&payload);

    println!(
        "[IpcServer] Sent WindowCreated: window_id={}, shm_size={}",
        window_id, shm_size
    );
}

/// Parse message and create IpcEvent
fn parse_message(client_id: usize, msg_type: u32, payload: &[u8]) -> Option<IpcEvent> {
    match msg_type {
        1 => {
            // CreateWindow
            if payload.len() >= 8 {
                let width = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                let height = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
                Some(IpcEvent::CreateWindow {
                    client_id,
                    width,
                    height,
                })
            } else {
                None
            }
        }
        2 => {
            // DestroyWindow
            if payload.len() >= 4 {
                let window_id =
                    u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                Some(IpcEvent::DestroyWindow {
                    client_id,
                    window_id,
                })
            } else {
                None
            }
        }
        4 => {
            // UpdateBuffer
            if payload.len() >= 20 {
                let window_id =
                    u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                let x = i32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
                let y = i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
                let width =
                    u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
                let height =
                    u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
                Some(IpcEvent::BufferUpdated {
                    window_id,
                    damage_x: x,
                    damage_y: y,
                    damage_width: width,
                    damage_height: height,
                })
            } else {
                None
            }
        }
        5 => {
            // RequestMove
            if payload.len() >= 4 {
                let window_id =
                    u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                Some(IpcEvent::RequestMove { window_id })
            } else {
                None
            }
        }
        6 => {
            // MoveWindow
            if payload.len() >= 12 {
                let window_id =
                    u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                let x = i32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
                let y = i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
                Some(IpcEvent::MoveWindow { window_id, x, y })
            } else {
                None
            }
        }
        _ => {
            println!("[IpcServer] Unknown message type: {}", msg_type);
            None
        }
    }
}

/// IPC Events that can be sent from clients
#[derive(Debug, Clone)]
pub enum IpcEvent {
    /// Client requested to create a window
    CreateWindow {
        client_id: usize,
        width: u32,
        height: u32,
    },
    /// Window was created via IPC (for compositor notification)
    WindowCreated {
        window_id: u32,
        width: u32,
        height: u32,
    },
    /// Client requested to destroy a window
    DestroyWindow { client_id: usize, window_id: u32 },
    /// Window was destroyed (for compositor notification)
    WindowDestroyed { window_id: u32 },
    /// Client updated their window buffer (damage region only)
    BufferUpdated {
        window_id: u32,
        damage_x: i32,
        damage_y: i32,
        damage_width: u32,
        damage_height: u32,
    },
    /// Client sent new buffer data (full buffer update from IPC)
    ClientBufferUpdate {
        window_id: u32,
        buffer: Vec<u8>,
    },
    /// Client requested window move
    RequestMove { window_id: u32 },
    /// Client moved window
    MoveWindow { window_id: u32, x: i32, y: i32 },
}
