//! IPC Server module - handles client connections and messages

use std::println;
use std::socket::Socket;
use std::sync::Mutex;
use std::thread;
use std::vec::Vec;
use sws_protocol as protocol;
use sws_protocol::ClientMessageRef;

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
        message: protocol::ServerMessage,
    ) -> Result<(), &'static str> {
        // TODO: Implement client response mechanism
        // For now, just log
        println!(
            "[IpcServer] Would send message {:?} to client {}",
            message, _client_id
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

    // Evidence-only: log a stack address hint for this thread.
    // If this falls inside the compositor backbuffer range, it indicates overlap risk.
    let stack_marker: u8 = 0;
    let sp_hint = (&stack_marker as *const u8) as usize;
    println!("[AcceptThread] stack marker addr: 0x{:x}", sp_hint);

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

    // Evidence-only: log a stack address hint for this thread.
    let stack_marker: u8 = 0;
    let sp_hint = (&stack_marker as *const u8) as usize;
    println!(
        "[ClientThread {}] stack marker addr: 0x{:x}",
        client_id, sp_hint
    );

    // Per-client window id generator (avoid collision between clients)
    let mut next_window_id: u32 = 100 + (client_id as u32 * 1000);

    loop {
        println!(
            "[ClientThread {}] Waiting for message... (socket handle: {})",
            client_id,
            socket.as_raw()
        );

        let (msg_type, payload) = match protocol::read_frame(&mut socket) {
            Ok(v) => v,
            Err(protocol::ProtocolError::IoDisconnected) => {
                println!("[ClientThread {}] Client disconnected", client_id);
                break;
            }
            Err(e) => {
                println!("[ClientThread {}] Failed to read frame: {:?}", client_id, e);
                break;
            }
        };

        match protocol::parse_client_message(msg_type, &payload) {
            Ok(ClientMessageRef::CreateWindow { width, height }) => {
                // Calculate buffer size (used by client for now)
                let buffer_size = (width as u64)
                    .saturating_mul(height as u64)
                    .saturating_mul(4);

                let window_id = next_window_id;
                next_window_id = next_window_id.saturating_add(1);

                // Reply to client immediately
                send_window_created(&mut socket, window_id, buffer_size);

                // Create window in compositor with the same ID
                push_ipc_event(IpcEvent::CreateWindow {
                    client_id,
                    window_id,
                    width,
                    height,
                });
            }
            Ok(ClientMessageRef::DestroyWindow { window_id }) => {
                println!(
                    "[ClientThread {}] DestroyWindow request for window {}",
                    client_id, window_id
                );
                push_ipc_event(IpcEvent::DestroyWindow {
                    client_id,
                    window_id,
                });
            }
            Ok(ClientMessageRef::UpdateBuffer {
                window_id,
                x,
                y,
                width,
                height,
            }) => {
                // UpdateBuffer (damage notification) - optional
                push_ipc_event(IpcEvent::BufferUpdated {
                    window_id,
                    damage_x: x,
                    damage_y: y,
                    damage_width: width,
                    damage_height: height,
                });
            }
            Ok(_) => {
                // Ignore other messages for now
            }
            Err(e) => {
                println!(
                    "[ClientThread {}] Failed to parse message (type {}): {:?}",
                    client_id, msg_type, e
                );
            }
        }
    }

    println!("[ClientThread {}] Exiting", client_id);
}

/// Send WindowCreated message
fn send_window_created(socket: &mut Socket, window_id: u32, shm_size: u64) {
    if let Err(e) = protocol::write_window_created(socket, window_id, shm_size) {
        println!(
            "[IpcServer] Failed to send WindowCreated: {:?} (window_id={}, shm_size={})",
            e, window_id, shm_size
        );
        return;
    }

    println!(
        "[IpcServer] Sent WindowCreated: window_id={}, shm_size={}",
        window_id, shm_size
    );
}

/// IPC Events that can be sent from clients
#[derive(Debug, Clone)]
pub enum IpcEvent {
    /// Client requested to create a window
    CreateWindow {
        client_id: usize,
        window_id: u32,
        width: u32,
        height: u32,
    },
    /// Client requested to destroy a window
    DestroyWindow { client_id: usize, window_id: u32 },
    /// Client updated their window buffer (damage region only)
    BufferUpdated {
        window_id: u32,
        damage_x: i32,
        damage_y: i32,
        damage_width: u32,
        damage_height: u32,
    },
    /// Client requested window move
    RequestMove { window_id: u32 },
    /// Client moved window
    MoveWindow { window_id: u32, x: i32, y: i32 },
}
