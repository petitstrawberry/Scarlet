//! IPC Server module - handles client connections and messages

use std::collections::BTreeMap;
use std::handle::capability::memory_mapping::flags as mmap_flags;
use std::ipc::{SharedMemory, permissions};
use std::println;
use std::socket::Socket;
use std::sync::Mutex;
use std::thread::{self, sleep, yield_now};
use std::vec::Vec;
use sws_protocol as protocol;
use sws_protocol::ClientMessageRef;

#[derive(Debug, Clone, Copy, Default)]
struct WindowSizeLimits {
    min_width: u32,
    min_height: u32,
    max_width: u32,
    max_height: u32,
}

impl WindowSizeLimits {
    fn clamp(&self, width: u32, height: u32) -> (u32, u32) {
        let mut w = width.max(1);
        let mut h = height.max(1);

        if self.min_width != 0 {
            w = w.max(self.min_width.max(1));
        }
        if self.min_height != 0 {
            h = h.max(self.min_height.max(1));
        }

        let effective_max_width = if self.max_width == 0 {
            0
        } else if self.min_width != 0 {
            self.max_width.max(self.min_width.max(1))
        } else {
            self.max_width.max(1)
        };
        let effective_max_height = if self.max_height == 0 {
            0
        } else if self.min_height != 0 {
            self.max_height.max(self.min_height.max(1))
        } else {
            self.max_height.max(1)
        };

        if effective_max_width != 0 {
            w = w.min(effective_max_width);
        }
        if effective_max_height != 0 {
            h = h.min(effective_max_height);
        }

        (w, h)
    }
}

#[derive(Debug)]
enum FrameIoError {
    WouldBlock,
    Disconnected,
    Io,
    Protocol,
}

/// Non-blocking framed-message reader.
///
/// With non-blocking sockets, reads can return `WouldBlock` after consuming
/// *some* bytes. If we drop that partial progress and restart from a fresh
/// header read, the stream becomes desynchronized and subsequent frames are
/// mis-parsed (e.g., intermittent 0x0 damage rectangles).
struct FrameReader {
    header: [u8; protocol::MessageHeader::SIZE],
    header_filled: usize,
    header_parsed: bool,

    msg_type: u32,
    payload_len: usize,
    payload: Vec<u8>,
    payload_filled: usize,
}

impl FrameReader {
    fn new() -> Self {
        Self {
            header: [0u8; protocol::MessageHeader::SIZE],
            header_filled: 0,
            header_parsed: false,
            msg_type: 0,
            payload_len: 0,
            payload: Vec::new(),
            payload_filled: 0,
        }
    }

    fn reset(&mut self) {
        self.header_filled = 0;
        self.header_parsed = false;
        self.msg_type = 0;
        self.payload_len = 0;
        self.payload.clear();
        self.payload_filled = 0;
    }

    /// Poll for the next complete frame.
    ///
    /// - `Ok(Some((msg_type, payload)))` when a full frame is assembled
    /// - `Ok(None)` when no complete frame is available yet
    /// - `Err(..)` on disconnect / I/O / protocol error
    fn poll(&mut self, socket: &mut Socket) -> Result<Option<(u32, Vec<u8>)>, FrameIoError> {
        use std::io::Read;

        // Read header.
        while self.header_filled < self.header.len() {
            match socket.read(&mut self.header[self.header_filled..]) {
                Ok(0) => return Err(FrameIoError::Disconnected),
                Ok(n) => self.header_filled += n,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        return Ok(None);
                    }
                    return Err(FrameIoError::Io);
                }
            }
        }

        // Parse header once.
        if !self.header_parsed {
            let header = protocol::MessageHeader::from_le_bytes(self.header);
            let payload_len = header.payload_size as usize;
            if payload_len > protocol::MAX_PAYLOAD_SIZE {
                self.reset();
                return Err(FrameIoError::Protocol);
            }
            self.msg_type = header.msg_type;
            self.payload_len = payload_len;
            if payload_len > 0 {
                self.payload.resize(payload_len, 0);
            }
            self.header_parsed = true;
        }

        // Read payload.
        while self.payload_filled < self.payload_len {
            match socket.read(&mut self.payload[self.payload_filled..]) {
                Ok(0) => return Err(FrameIoError::Disconnected),
                Ok(n) => self.payload_filled += n,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        return Ok(None);
                    }
                    return Err(FrameIoError::Io);
                }
            }
        }

        // Complete.
        let msg_type = self.msg_type;
        let payload = core::mem::take(&mut self.payload);
        self.reset();
        Ok(Some((msg_type, payload)))
    }
}

fn write_all(socket: &mut Socket, buf: &[u8]) -> Result<(), FrameIoError> {
    use std::io::Write;

    let mut written = 0;
    while written < buf.len() {
        match socket.write(&buf[written..]) {
            Ok(0) => return Err(FrameIoError::Disconnected),
            Ok(n) => written += n,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    // Non-blocking socket; give the scheduler a chance and retry.
                    sleep(core::time::Duration::from_millis(1));
                    continue;
                }
                return Err(FrameIoError::Io);
            }
        }
    }
    Ok(())
}

fn write_frame(socket: &mut Socket, msg_type: u32, payload: &[u8]) -> Result<(), FrameIoError> {
    use std::io::Write;

    // Write header + payload directly to avoid allocating a temporary Vec.
    let header = protocol::MessageHeader {
        msg_type,
        payload_size: payload.len() as u32,
    };
    let header_bytes = header.to_le_bytes();
    write_all(socket, &header_bytes)?;
    if !payload.is_empty() {
        write_all(socket, payload)?;
    }
    socket.flush().map_err(|_| FrameIoError::Io)?;
    Ok(())
}

/// Input event to be sent to a client
#[derive(Debug, Clone)]
pub struct PendingInputEvent {
    pub time: u64,
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

/// Global event queue for IPC events
static EVENT_QUEUE: Mutex<Vec<IpcEvent>> = Mutex::new(Vec::new());

/// Global pending input events: BTreeMap for O(log n) lookup
static PENDING_INPUT_EVENTS: Mutex<BTreeMap<u32, Vec<PendingInputEvent>>> =
    Mutex::new(BTreeMap::new());

/// Pending server->client frames to be delivered to a specific window.
#[derive(Debug, Clone)]
pub struct PendingServerFrame {
    pub msg_type: u32,
    pub payload: Vec<u8>,
}

static PENDING_SERVER_FRAMES: Mutex<BTreeMap<u32, Vec<PendingServerFrame>>> =
    Mutex::new(BTreeMap::new());

/// Pending server->client responses for specific clients (by client_id)
/// This is used for responses to clients that don't have windows (like stemd)
static PENDING_CLIENT_RESPONSES: Mutex<BTreeMap<usize, Vec<PendingServerFrame>>> =
    Mutex::new(BTreeMap::new());

/// Add an event to the global queue
pub fn push_ipc_event(event: IpcEvent) {
    let mut queue = EVENT_QUEUE.lock();
    queue.push(event);
}

/// Get all pending events from the queue
pub fn pop_all_ipc_events() -> Vec<IpcEvent> {
    let mut queue = EVENT_QUEUE.lock();
    core::mem::take(&mut *queue)
}

/// Register a window for input event routing
fn register_window(window_id: u32, _client_id: usize) {
    {
        let mut pending = PENDING_INPUT_EVENTS.lock();
        pending.entry(window_id).or_insert_with(Vec::new);
    }

    {
        let mut pending = PENDING_SERVER_FRAMES.lock();
        pending.entry(window_id).or_insert_with(Vec::new);
    }
}

/// Unregister a window
fn unregister_window(window_id: u32) {
    {
        let mut pending = PENDING_INPUT_EVENTS.lock();
        pending.remove(&window_id);
    }

    {
        let mut pending = PENDING_SERVER_FRAMES.lock();
        pending.remove(&window_id);
    }
}

/// Queue an input event for a specific window (O(log n) lookup)
pub fn send_input_to_window(window_id: u32, time: u64, type_: u16, code: u16, value: i32) {
    let mut pending = PENDING_INPUT_EVENTS.lock();

    if let Some(events) = pending.get_mut(&window_id) {
        events.push(PendingInputEvent {
            time,
            type_,
            code,
            value,
        });
    }
}

/// Queue a server->client protocol message for a specific window.
pub fn send_message_to_window(window_id: u32, msg_type: u32, payload: Vec<u8>) {
    let mut pending = PENDING_SERVER_FRAMES.lock();
    match pending.get_mut(&window_id) {
        Some(frames) => frames.push(PendingServerFrame { msg_type, payload }),
        None => {
            println!(
                "[IpcServer] Warning: server message queued for unregistered window {} (msg_type={}); creating queue",
                window_id, msg_type
            );
            let mut frames = Vec::new();
            frames.push(PendingServerFrame { msg_type, payload });
            pending.insert(window_id, frames);
        }
    }
}

/// Queue a server->client protocol message for a specific client (by client_id).
/// This is used for responses to clients that don't have windows (like stemd).
pub fn send_message_to_client(client_id: usize, msg_type: u32, payload: Vec<u8>) {
    let mut pending = PENDING_CLIENT_RESPONSES.lock();
    match pending.get_mut(&client_id) {
        Some(frames) => frames.push(PendingServerFrame { msg_type, payload }),
        None => {
            println!(
                "[IpcServer] Sending message to client {} (msg_type={}, payload_len={})",
                client_id, msg_type, payload.len()
            );
            let mut frames = Vec::new();
            frames.push(PendingServerFrame { msg_type, payload });
            pending.insert(client_id, frames);
        }
    }
}

fn pop_pending_server_frames(window_id: u32) -> Vec<PendingServerFrame> {
    let mut pending = PENDING_SERVER_FRAMES.lock();
    if let Some(frames) = pending.get_mut(&window_id) {
        if frames.is_empty() {
            Vec::new() // Already empty, no reallocation needed
        } else {
            core::mem::take(frames)
        }
    } else {
        Vec::new()
    }
}

/// Get pending input events for a window (called by client thread, O(log n) lookup)
fn pop_pending_input_events(window_id: u32) -> Vec<PendingInputEvent> {
    let mut pending = PENDING_INPUT_EVENTS.lock();

    if let Some(events) = pending.get_mut(&window_id) {
        if events.is_empty() {
            Vec::new() // Already empty, no reallocation needed
        } else {
            core::mem::take(events)
        }
    } else {
        Vec::new()
    }
}

/// Pop pending server responses for a specific client (by client_id)
fn pop_pending_client_responses(client_id: usize) -> Vec<PendingServerFrame> {
    let mut pending = PENDING_CLIENT_RESPONSES.lock();
    if let Some(frames) = pending.get_mut(&client_id) {
        if frames.is_empty() {
            Vec::new()
        } else {
            let frames = core::mem::take(frames);
            println!(
                "[IpcServer] Popping {} pending responses for client {}",
                frames.len(),
                client_id
            );
            frames
        }
    } else {
        Vec::new()
    }
}

/// IPC Server - manages Socket VFS connections
pub struct IpcServer {
    socket_path: &'static str,
    accept_thread_started: bool,
}

impl IpcServer {
    /// Create a new IPC server
    pub fn new(socket_path: &'static str) -> Result<Self, &'static str> {
        println!("[IpcServer] Initializing at {}", socket_path);

        Ok(Self {
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
    #[allow(dead_code)]
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

    // Enable non-blocking mode for event-driven I/O
    if let Err(e) = socket.set_nonblocking(true) {
        println!(
            "[ClientThread {}] Failed to set non-blocking mode: {:?}",
            client_id, e
        );
        return;
    }
    println!("[ClientThread {}] Enabled non-blocking mode", client_id);

    // Evidence-only: log a stack address hint for this thread.
    let stack_marker: u8 = 0;
    let sp_hint = (&stack_marker as *const u8) as usize;
    println!(
        "[ClientThread {}] stack marker addr: 0x{:x}",
        client_id, sp_hint
    );

    // Per-client window id generator (avoid collision between clients)
    let mut next_window_id: u32 = 100 + (client_id as u32 * 1000);
    let mut managed_windows: Vec<u32> = Vec::new();
    let mut window_size_limits: BTreeMap<u32, WindowSizeLimits> = BTreeMap::new();

    // Debug: loop counter for periodic logging
    let mut loop_count: u64 = 0;

    let mut frame_reader = FrameReader::new();

    println!("[ClientThread {}] Entering main loop", client_id);

    'main: loop {
        loop_count += 1;
        let _should_log = loop_count % 100 == 0; // Log every 100 iterations (more frequent)

        // Send any pending input events for this client's windows
        let mut has_events = false;
        let mut _total_events = 0;

        // First, check for pending responses addressed directly to this client
        // (for clients that don't have windows, like stemd)
        let client_responses = pop_pending_client_responses(client_id);
        for frame in client_responses {
            println!(
                "[ClientThread {}] Sending client response (msg_type={}, payload_len={})",
                client_id, frame.msg_type, frame.payload.len()
            );
            if let Err(e) = write_frame(&mut socket, frame.msg_type, &frame.payload) {
                println!(
                    "[ClientThread {}] Failed to send client response: {:?}",
                    client_id, e
                );
                break 'main;
            }
            has_events = true;
        }

        for &window_id in &managed_windows {
            // Send queued server->client control messages for this window.
            let frames = pop_pending_server_frames(window_id);
            for frame in frames {
                if let Err(e) = write_frame(&mut socket, frame.msg_type, &frame.payload) {
                    println!(
                        "[ClientThread {}] Failed to send server message to window {}: {:?}",
                        client_id, window_id, e
                    );
                    break 'main;
                }
                has_events = true;
            }

            let events = pop_pending_input_events(window_id);
            if !events.is_empty() {
                has_events = true;
                _total_events += events.len();
                // println!(
                //     "[ClientThread {}] Loop #{}: Found {} events for window {}",
                //     client_id,
                //     loop_count,
                //     events.len(),
                //     window_id
                // );
                for event in events {
                    let payload = protocol::payload_input_event(
                        window_id,
                        event.time,
                        event.type_,
                        event.code,
                        event.value,
                    );
                    if let Err(e) =
                        write_frame(&mut socket, protocol::server_msg::INPUT_EVENT, &payload)
                    {
                        println!(
                            "[ClientThread {}] Failed to send input event to window {}: {:?}",
                            client_id, window_id, e
                        );
                        break 'main;
                    } else {
                        // println!(
                        //     "[ClientThread {}] Sent event: type={} code={} value={}",
                        //     client_id, event.type_, event.code, event.value
                        // );
                    }
                }
            }
        }

        // // Debug: log event queue status periodically
        // if should_log && !managed_windows.is_empty() {
        //     println!(
        //         "[ClientThread {}] Loop #{}: checked {} windows, found {} events",
        //         client_id,
        //         loop_count,
        //         managed_windows.len(),
        //         total_events
        //     );
        // }

        // If we sent events, loop back immediately to check for more
        // This ensures rapid delivery during input bursts
        if has_events {
            continue;
        }

        // // Always log before first read attempt or periodically
        // if loop_count <= 5 || loop_count % 100 == 0 {
        //     println!(
        //         "[ClientThread {}] Loop #{}: about to call read_frame (socket handle: {}, windows: {})",
        //         client_id,
        //         loop_count,
        //         socket.as_raw(),
        //         managed_windows.len()
        //     );
        // }

        let (msg_type, payload) = match frame_reader.poll(&mut socket) {
            Ok(Some(v)) => {
                // println!(
                //     "[ClientThread {}] Loop #{}: read_frame SUCCESS (msg_type={})",
                //     client_id, loop_count, v.0
                // );
                v
            }
            Ok(None) => {
                // No complete frame available yet; keep looping so we can deliver input events.
                // Yield to avoid a tight busy loop when idle.
                yield_now();
                continue;
            }
            Err(FrameIoError::Disconnected) => {
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
                // Calculate buffer size
                let buffer_size = (width as u64)
                    .saturating_mul(height as u64)
                    .saturating_mul(4);

                let window_id = next_window_id;
                next_window_id = next_window_id.saturating_add(1);

                // Create shared memory region for this window
                println!(
                    "[ClientThread {}] Creating SHM for window {} ({}x{} = {} bytes)",
                    client_id, window_id, width, height, buffer_size
                );

                match SharedMemory::create(buffer_size as usize, permissions::READ_WRITE) {
                    Ok(shm) => {
                        // Map SHM into server's address space for compositor access
                        let shm_mapped_addr = match shm.as_handle().as_memory_mapping() {
                            Ok(mapper) => {
                                match mapper.mmap(
                                    0,
                                    buffer_size as usize,
                                    permissions::READ_WRITE,
                                    mmap_flags::SHARED,
                                    0,
                                ) {
                                    Ok(addr) => {
                                        println!(
                                            "[ClientThread {}] SHM mapped at 0x{:x}",
                                            client_id, addr
                                        );

                                        // Zero-initialize the SHM for deterministic behavior
                                        unsafe {
                                            let ptr = addr as *mut u8;
                                            for i in 0..buffer_size as usize {
                                                *ptr.add(i) = 0;
                                            }
                                        }
                                        println!(
                                            "[ClientThread {}] SHM zero-initialized",
                                            client_id
                                        );

                                        // Sample first few bytes to verify
                                        unsafe {
                                            let ptr = addr as *const u8;
                                            println!(
                                                "[ClientThread {}] SHM first 16 bytes: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                                                client_id,
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

                                        Some(addr)
                                    }
                                    Err(_) => {
                                        println!("[ClientThread {}] Failed to mmap SHM", client_id);
                                        None
                                    }
                                }
                            }
                            Err(_) => {
                                println!(
                                    "[ClientThread {}] SHM does not support mapping",
                                    client_id
                                );
                                None
                            }
                        };

                        // Reply to client with window created message
                        send_window_created(&mut socket, window_id, buffer_size);

                        // Send SHM handle out-of-band
                        println!(
                            "[ClientThread {}] Sending SHM handle for window {}",
                            client_id, window_id
                        );
                        if let Err(e) = socket.send_handle(shm.as_handle()) {
                            println!(
                                "[ClientThread {}] Failed to send SHM handle: {:?}",
                                client_id, e
                            );
                            continue;
                        }
                        println!("[ClientThread {}] SHM handle sent successfully", client_id);

                        // Register window for input event routing
                        register_window(window_id, client_id);

                        // Track this window for input event polling
                        managed_windows.push(window_id);

                        // Notify compositor to create window entry with SHM ownership
                        push_ipc_event(IpcEvent::CreateWindow {
                            client_id,
                            window_id,
                            width,
                            height,
                            shm: Some(shm),
                            shm_mapped_addr,
                            shm_size: buffer_size as usize,
                        });
                    }
                    Err(e) => {
                        println!("[ClientThread {}] Failed to create SHM: {:?}", client_id, e);
                        // Send error response (optional)
                        continue;
                    }
                }
            }
            Ok(ClientMessageRef::DestroyWindow { window_id }) => {
                println!(
                    "[ClientThread {}] DestroyWindow request for window {}",
                    client_id, window_id
                );

                // Unregister window from input routing
                unregister_window(window_id);

                window_size_limits.remove(&window_id);

                // Remove from managed windows
                managed_windows.retain(|&id| id != window_id);

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
            Ok(ClientMessageRef::RequestMoveWindow { window_id }) => {
                push_ipc_event(IpcEvent::RequestMove { window_id });
            }
            Ok(ClientMessageRef::MoveWindow { window_id, x, y }) => {
                push_ipc_event(IpcEvent::MoveWindow { window_id, x, y });
            }
            Ok(ClientMessageRef::SetWindowParent {
                window_id,
                parent_id,
            }) => {
                push_ipc_event(IpcEvent::SetWindowParent {
                    window_id,
                    parent_id,
                });
            }
            Ok(ClientMessageRef::SetWindowTransientFlags { window_id, flags }) => {
                push_ipc_event(IpcEvent::SetWindowTransientFlags { window_id, flags });
            }
            Ok(ClientMessageRef::SetWindowSizeLimits {
                window_id,
                min_width,
                min_height,
                max_width,
                max_height,
            }) => {
                println!(
                    "[ClientThread {}] SetWindowSizeLimits: window_id={} min={}x{} max={}x{}",
                    client_id, window_id, min_width, min_height, max_width, max_height
                );

                window_size_limits.insert(
                    window_id,
                    WindowSizeLimits {
                        min_width,
                        min_height,
                        max_width,
                        max_height,
                    },
                );

                push_ipc_event(IpcEvent::SetWindowSizeLimits {
                    window_id,
                    min_width,
                    min_height,
                    max_width,
                    max_height,
                });
            }
            Ok(ClientMessageRef::ResizeWindow {
                window_id,
                width,
                height,
            }) => {
                let (width, height) = match window_size_limits.get(&window_id) {
                    Some(limits) => {
                        let (w, h) = limits.clamp(width, height);
                        if w != width || h != height {
                            println!(
                                "[ClientThread {}] ResizeWindow clamped: window_id={} {}x{} -> {}x{}",
                                client_id, window_id, width, height, w, h
                            );
                        }
                        (w, h)
                    }
                    None => (width.max(1), height.max(1)),
                };

                let buffer_size = (width as u64)
                    .saturating_mul(height as u64)
                    .saturating_mul(4);

                println!(
                    "[ClientThread {}] ResizeWindow: window_id={} {}x{} ({} bytes)",
                    client_id, window_id, width, height, buffer_size
                );

                match SharedMemory::create(buffer_size as usize, permissions::READ_WRITE) {
                    Ok(shm) => {
                        // Map for compositor
                        let mapper = match shm.as_handle().as_memory_mapping() {
                            Ok(m) => m,
                            Err(_) => {
                                println!(
                                    "[ClientThread {}] ResizeWindow: SHM mapping unsupported",
                                    client_id
                                );
                                continue;
                            }
                        };

                        let mapped_addr = match mapper.mmap(
                            0,
                            buffer_size as usize,
                            permissions::READ_WRITE,
                            mmap_flags::SHARED,
                            0,
                        ) {
                            Ok(a) => a,
                            Err(_) => {
                                println!("[ClientThread {}] ResizeWindow: mmap failed", client_id);
                                continue;
                            }
                        };

                        // Reply to client with WINDOW_RESIZED + SHM handle.
                        let payload =
                            protocol::payload_window_resized(window_id, buffer_size, width, height);
                        if let Err(e) =
                            write_frame(&mut socket, protocol::server_msg::WINDOW_RESIZED, &payload)
                        {
                            println!(
                                "[ClientThread {}] ResizeWindow: failed to send WINDOW_RESIZED: {:?}",
                                client_id, e
                            );
                            continue;
                        }

                        if let Err(e) = socket.send_handle(shm.as_handle()) {
                            println!(
                                "[ClientThread {}] ResizeWindow: failed to send SHM handle: {:?}",
                                client_id, e
                            );
                            continue;
                        }

                        push_ipc_event(IpcEvent::ResizeWindow {
                            window_id,
                            width,
                            height,
                            shm: Some(shm),
                            shm_mapped_addr: Some(mapped_addr),
                            shm_size: buffer_size as usize,
                        });
                    }
                    Err(_) => {
                        println!(
                            "[ClientThread {}] ResizeWindow: failed to create SHM",
                            client_id
                        );
                    }
                }
            }
            Ok(ClientMessageRef::MinimizeWindow { window_id }) => {
                println!(
                    "[ClientThread {}] MinimizeWindow: window_id={}",
                    client_id, window_id
                );
                push_ipc_event(IpcEvent::MinimizeWindow { window_id });
            }
            Ok(ClientMessageRef::MaximizeWindow { window_id }) => {
                println!(
                    "[ClientThread {}] MaximizeWindow: window_id={}",
                    client_id, window_id
                );
                push_ipc_event(IpcEvent::MaximizeWindow { window_id });
            }
            Ok(ClientMessageRef::RestoreWindow { window_id }) => {
                println!(
                    "[ClientThread {}] RestoreWindow: window_id={}",
                    client_id, window_id
                );
                push_ipc_event(IpcEvent::RestoreWindow { window_id });
            }
            Ok(ClientMessageRef::FocusWindow { window_id }) => {
                println!(
                    "[ClientThread {}] FocusWindow: window_id={}",
                    client_id, window_id
                );
                push_ipc_event(IpcEvent::FocusWindow { window_id });
            }
            Ok(ClientMessageRef::SetWindowType {
                window_id,
                window_type,
            }) => {
                println!(
                    "[ClientThread {}] SetWindowType: window_id={} type={}",
                    client_id, window_id, window_type
                );
                push_ipc_event(IpcEvent::SetWindowType {
                    window_id,
                    window_type,
                });
            }
            Ok(ClientMessageRef::SetWindowOpacity { window_id, opacity }) => {
                println!(
                    "[ClientThread {}] SetWindowOpacity: window_id={} opacity={}",
                    client_id, window_id, opacity
                );
                push_ipc_event(IpcEvent::SetWindowOpacity { window_id, opacity });
            }
            Ok(ClientMessageRef::SetWorkarea {
                x,
                y,
                width,
                height,
            }) => {
                println!(
                    "[ClientThread {}] SetWorkarea: x={}, y={}, width={}, height={}",
                    client_id, x, y, width, height
                );
                push_ipc_event(IpcEvent::SetWorkarea {
                    x,
                    y,
                    width,
                    height,
                });
            }
            Ok(ClientMessageRef::SetWindowResizable {
                window_id,
                resizable,
            }) => {
                println!(
                    "[ClientThread {}] SetWindowResizable: window_id={} resizable={}",
                    client_id, window_id, resizable
                );
                push_ipc_event(IpcEvent::SetWindowResizable {
                    window_id,
                    resizable,
                });
            }
            Ok(ClientMessageRef::GetScreenSize {}) => {
                println!(
                    "[ClientThread {}] GetScreenSize: requesting screen size",
                    client_id
                );
                // Send response directly with default screen size
                // TODO: Get actual screen size from compositor
                let screen_width = 1024; // Default fallback
                let screen_height = 768;
                let payload = protocol::payload_screen_size(screen_width, screen_height);
                if let Err(e) =
                    write_frame(&mut socket, protocol::server_msg::SCREEN_SIZE, &payload)
                {
                    println!(
                        "[ClientThread {}] Failed to send SCREEN_SIZE response: {:?}",
                        client_id, e
                    );
                } else {
                    println!(
                        "[ClientThread {}] Sent SCREEN_SIZE: {}x{}",
                        client_id, screen_width, screen_height
                    );
                }
            }
            Ok(ClientMessageRef::GetWindowList {}) => {
                println!(
                    "[ClientThread {}] GetWindowList: requesting window list",
                    client_id
                );
                push_ipc_event(IpcEvent::GetWindowList { client_id });
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
        // sleep(std::time::Duration::from_millis(16));
        yield_now();
    }

    // Cleanup: ensure per-window routing queues don't leak when the client disappears.
    // Also notify the compositor so orphaned windows don't stick around.
    for window_id in managed_windows.drain(..) {
        unregister_window(window_id);
        window_size_limits.remove(&window_id);
        push_ipc_event(IpcEvent::DestroyWindow {
            client_id,
            window_id,
        });
    }

    println!("[ClientThread {}] Exiting", client_id);
}

/// Send WindowCreated message
fn send_window_created(socket: &mut Socket, window_id: u32, shm_size: u64) {
    let payload = protocol::payload_window_created(window_id, shm_size);
    if let Err(e) = write_frame(socket, protocol::server_msg::WINDOW_CREATED, &payload) {
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
#[derive(Debug)]
pub enum IpcEvent {
    /// Client requested to create a window
    CreateWindow {
        client_id: usize,
        window_id: u32,
        width: u32,
        height: u32,
        /// Shared memory for the window buffer (server-allocated)
        shm: Option<SharedMemory>,
        shm_mapped_addr: Option<usize>,
        /// Size of the SHM mapping in bytes.
        shm_size: usize,
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

    /// Set (or clear) parent window relationship
    ///
    /// `parent_id == 0` means "no parent".
    SetWindowParent { window_id: u32, parent_id: u32 },

    /// Set transient behavior flags for a window (bitset).
    SetWindowTransientFlags { window_id: u32, flags: u32 },

    /// Set min/max size constraints for a window.
    SetWindowSizeLimits {
        window_id: u32,
        min_width: u32,
        min_height: u32,
        max_width: u32,
        max_height: u32,
    },

    /// Resize a window and replace its SHM buffer.
    ResizeWindow {
        window_id: u32,
        width: u32,
        height: u32,
        shm: Option<SharedMemory>,
        shm_mapped_addr: Option<usize>,
        /// Size of the SHM mapping in bytes.
        shm_size: usize,
    },

    /// Minimize a window
    MinimizeWindow { window_id: u32 },

    /// Maximize a window
    MaximizeWindow { window_id: u32 },

    /// Restore a window from minimized or maximized state
    RestoreWindow { window_id: u32 },

    /// Focus and raise a window
    FocusWindow { window_id: u32 },

    /// Set window type for Z-order management
    SetWindowType { window_id: u32, window_type: u32 },

    /// Set window opacity
    SetWindowOpacity { window_id: u32, opacity: u8 },

    /// Set the workarea (usable screen area) for the window manager
    SetWorkarea {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },

    /// Set whether a window can be resized by the user via interactive resize
    SetWindowResizable { window_id: u32, resizable: bool },

    /// Get the screen size
    GetScreenSize { client_id: usize },

    /// Get list of all windows
    GetWindowList { client_id: usize },
}
