//! Local IPC transport for the SWS remote capture and virtual-input API.

use core::sync::atomic::{AtomicUsize, Ordering};
use scarlet_os::handle::Handle;
use scarlet_os::poll::{POLLERR, POLLHUP, POLLIN, POLLNVAL, POLLOUT, PollHandle, poll};
use scarlet_os::socket::{Socket, SocketError};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::println;
use std::sync::Mutex;
use std::thread;
use std::vec::Vec;
use sws_remote_protocol::{
    ClientMessage, MessageHeader, ServerMessage, decode_client_message, decode_header,
    encode_server_message,
};

const CLIENT_POLL_TIMEOUT_NS: i64 = 8_000_000;
const MAX_OUTBOUND_BYTES: usize = 1024 * 1024;
const MAX_WRITE_BYTES_PER_TICK: usize = 64 * 1024;
const MAX_WRITE_CHUNK: usize = 16 * 1024;

static NEXT_CLIENT_ID: AtomicUsize = AtomicUsize::new(1);
static EVENT_QUEUE: Mutex<Vec<RemoteEvent>> = Mutex::new(Vec::new());
static OUTBOUND: Mutex<BTreeMap<usize, Vec<ServerMessage>>> = Mutex::new(BTreeMap::new());

/// Work delivered from a remote IPC connection to the compositor thread.
#[derive(Debug)]
pub(crate) enum RemoteEvent {
    /// Create the connection's capture session.
    CreateCapture {
        /// Transport connection identifier.
        client_id: usize,
        /// Requested SWS output identifier.
        output_id: u32,
    },
    /// Register one client-owned capture buffer and its transferred handle.
    RegisterBuffer {
        /// Transport connection identifier.
        client_id: usize,
        /// Connection-local buffer identifier.
        buffer_id: u32,
        /// Buffer width.
        width: u32,
        /// Buffer height.
        height: u32,
        /// Bytes between rows.
        stride: u32,
        /// Capture pixel format.
        format: sws_remote_protocol::CaptureFormat,
        /// Transferred shared-memory handle.
        handle: Handle,
    },
    /// Request one on-demand frame copy.
    RequestFrame {
        /// Transport connection identifier.
        client_id: usize,
        /// Destination capture buffer.
        buffer_id: u32,
    },
    /// Inject one transport-neutral virtual-input message.
    Input {
        /// Transport connection identifier.
        client_id: usize,
        /// Key or pointer message.
        message: ClientMessage,
    },
    /// Release any session state owned by a disconnected connection.
    Disconnected {
        /// Transport connection identifier.
        client_id: usize,
    },
}

/// SWS local remote-service listener.
pub(crate) struct RemoteServer {
    socket_path: &'static str,
    socket: Option<Socket>,
    started: bool,
}

impl RemoteServer {
    /// Construct a remote-service listener.
    ///
    /// # Arguments
    ///
    /// * `socket_path` - Local Scarlet socket path.
    ///
    /// # Returns
    ///
    /// An inactive listener.
    pub(crate) fn new(socket_path: &'static str) -> Self {
        Self {
            socket_path,
            socket: None,
            started: false,
        }
    }

    /// Bind the remote-service socket without starting worker threads.
    ///
    /// # Returns
    ///
    /// Success after the server owns its socket address.
    pub(crate) fn bind(&mut self) -> Result<(), &'static str> {
        if self.socket.is_some() || self.started {
            return Ok(());
        }
        let socket = Socket::new().map_err(|_| "Failed to create SWS remote socket")?;
        socket
            .bind(self.socket_path)
            .map_err(|_| "Failed to bind SWS remote socket")?;
        socket
            .listen(4)
            .map_err(|_| "Failed to listen on SWS remote socket")?;
        self.socket = Some(socket);
        println!("[SwsRemote] Listening at {}", self.socket_path);
        Ok(())
    }

    /// Start the remote-service accept thread.
    ///
    /// The socket is bound first when bind has not been called.
    ///
    /// # Returns
    ///
    /// Success after the listener thread has been created.
    pub(crate) fn listen(&mut self) -> Result<(), &'static str> {
        if self.started {
            return Ok(());
        }
        self.bind()?;
        let socket = self
            .socket
            .take()
            .ok_or("SWS remote socket was not bound")?;
        thread::Builder::new()
            .spawn(move || accept_loop(socket))
            .map_err(|_| "Failed to start SWS remote accept thread")?;
        self.started = true;
        Ok(())
    }

    /// Drain work queued for the compositor thread.
    ///
    /// # Returns
    ///
    /// Remote events in receive order.
    pub(crate) fn process_messages(&mut self) -> Vec<RemoteEvent> {
        pop_all_events()
    }
}

/// Return whether remote work is waiting for the compositor.
///
/// # Returns
///
/// `true` when [`RemoteServer::process_messages`] will return at least one event.
pub(crate) fn has_pending_events() -> bool {
    !EVENT_QUEUE
        .lock()
        .expect("SWS remote mutex poisoned")
        .is_empty()
}

/// Queue one server event for a connected remote client.
///
/// # Arguments
///
/// * `client_id` - Destination transport connection.
/// * `message` - Protocol message to deliver.
pub(crate) fn send_to_client(client_id: usize, message: ServerMessage) {
    let mut outbound = OUTBOUND.lock().expect("SWS remote mutex poisoned");
    let Some(messages) = outbound.get_mut(&client_id) else {
        return;
    };
    if let ServerMessage::FrameAvailable { .. } = message
        && let Some(existing) = messages.last_mut()
        && matches!(existing, ServerMessage::FrameAvailable { .. })
    {
        *existing = message;
        return;
    }
    messages.push(message);
}

fn push_event(event: RemoteEvent) {
    let mut events = EVENT_QUEUE.lock().expect("SWS remote mutex poisoned");
    let should_wake = events.is_empty();
    events.push(event);
    drop(events);
    if should_wake {
        super::super::ipc::wake_compositor();
    }
}

fn pop_all_events() -> Vec<RemoteEvent> {
    let mut events = EVENT_QUEUE.lock().expect("SWS remote mutex poisoned");
    core::mem::take(&mut *events)
}

fn take_outbound(client_id: usize) -> Vec<ServerMessage> {
    let mut outbound = OUTBOUND.lock().expect("SWS remote mutex poisoned");
    outbound
        .get_mut(&client_id)
        .map(core::mem::take)
        .unwrap_or_default()
}

fn accept_loop(server_socket: Socket) {
    loop {
        match server_socket.accept() {
            Ok(socket) => {
                let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
                OUTBOUND
                    .lock()
                    .expect("SWS remote mutex poisoned")
                    .insert(client_id, Vec::new());
                if thread::Builder::new()
                    .spawn(move || client_loop(client_id, socket))
                    .is_err()
                {
                    OUTBOUND
                        .lock()
                        .expect("SWS remote mutex poisoned")
                        .remove(&client_id);
                }
            }
            Err(SocketError::WouldBlock) => {
                thread::sleep(core::time::Duration::from_millis(10));
            }
            Err(_) => {
                thread::sleep(core::time::Duration::from_millis(10));
            }
        }
    }
}

fn client_loop(client_id: usize, mut socket: Socket) {
    if socket.set_nonblocking(true).is_err() {
        disconnect(client_id);
        return;
    }
    println!("[SwsRemote] Client {} connected", client_id);
    let mut reader = FrameReader::new();
    let mut atomic_frame = Vec::new();
    let mut writer = FrameWriter::new();

    'connection: loop {
        for message in take_outbound(client_id) {
            if writer.enqueue(encode_server_message(&message)).is_err() {
                break 'connection;
            }
        }
        if writer.flush(&mut socket).is_err() {
            break;
        }

        let atomic = match poll_handle_frame(&socket, &mut atomic_frame) {
            Ok(frame) => frame,
            Err(_) => break,
        };
        let frame = match atomic {
            Some((header, payload, handle)) => Some((header, payload, Some(handle))),
            None => match reader.poll(&mut socket) {
                Ok(Some((header, payload))) => Some((header, payload, None)),
                Ok(None) => None,
                Err(_) => break,
            },
        };

        if let Some((header, payload, handle)) = frame {
            let message = match decode_client_message(header, &payload) {
                Ok(message) => message,
                Err(_) => break,
            };
            if message.requires_handle() != handle.is_some() {
                break;
            }
            match (message, handle) {
                (ClientMessage::CreateCapture { output_id }, None) => {
                    push_event(RemoteEvent::CreateCapture {
                        client_id,
                        output_id,
                    });
                }
                (
                    ClientMessage::RegisterBuffer {
                        buffer_id,
                        width,
                        height,
                        stride,
                        format,
                    },
                    Some(handle),
                ) => push_event(RemoteEvent::RegisterBuffer {
                    client_id,
                    buffer_id,
                    width,
                    height,
                    stride,
                    format,
                    handle,
                }),
                (ClientMessage::RequestFrame { buffer_id }, None) => {
                    push_event(RemoteEvent::RequestFrame {
                        client_id,
                        buffer_id,
                    });
                }
                (message, None) => push_event(RemoteEvent::Input { client_id, message }),
                _ => break,
            }
            continue;
        }

        let interests = POLLIN | if writer.has_pending() { POLLOUT } else { 0 };
        let mut handle = PollHandle::new(socket.as_raw() as u32, interests);
        let Ok(_) = poll(core::slice::from_mut(&mut handle), CLIENT_POLL_TIMEOUT_NS) else {
            thread::sleep(core::time::Duration::from_millis(10));
            continue;
        };
        if handle.revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
            break;
        }
    }

    disconnect(client_id);
}

fn disconnect(client_id: usize) {
    OUTBOUND
        .lock()
        .expect("SWS remote mutex poisoned")
        .remove(&client_id);
    push_event(RemoteEvent::Disconnected { client_id });
    println!("[SwsRemote] Client {} disconnected", client_id);
}

struct FrameReader {
    header: [u8; MessageHeader::SIZE],
    header_filled: usize,
    parsed_header: Option<MessageHeader>,
    payload: Vec<u8>,
    payload_filled: usize,
}

impl FrameReader {
    fn new() -> Self {
        Self {
            header: [0; MessageHeader::SIZE],
            header_filled: 0,
            parsed_header: None,
            payload: Vec::new(),
            payload_filled: 0,
        }
    }

    fn reset(&mut self) {
        self.header_filled = 0;
        self.parsed_header = None;
        self.payload.clear();
        self.payload_filled = 0;
    }

    fn poll(
        &mut self,
        socket: &mut Socket,
    ) -> Result<Option<(MessageHeader, Vec<u8>)>, TransportError> {
        while self.header_filled < MessageHeader::SIZE {
            match socket.read(&mut self.header[self.header_filled..]) {
                Ok(0) => return Err(TransportError),
                Ok(count) => self.header_filled += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
                Err(_) => return Err(TransportError),
            }
        }
        if self.parsed_header.is_none() {
            let header = decode_header(&self.header).map_err(|_| TransportError)?;
            self.payload.resize(header.payload_size as usize, 0);
            self.parsed_header = Some(header);
        }
        while self.payload_filled < self.payload.len() {
            match socket.read(&mut self.payload[self.payload_filled..]) {
                Ok(0) => return Err(TransportError),
                Ok(count) => self.payload_filled += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
                Err(_) => return Err(TransportError),
            }
        }
        let header = self.parsed_header.ok_or(TransportError)?;
        let payload = core::mem::take(&mut self.payload);
        self.reset();
        Ok(Some((header, payload)))
    }
}

struct PendingFrame {
    bytes: Vec<u8>,
    offset: usize,
}

struct FrameWriter {
    frames: Vec<PendingFrame>,
    head: usize,
    pending_bytes: usize,
}

impl FrameWriter {
    fn new() -> Self {
        Self {
            frames: Vec::new(),
            head: 0,
            pending_bytes: 0,
        }
    }

    fn has_pending(&self) -> bool {
        self.head < self.frames.len()
    }

    fn enqueue(&mut self, bytes: Vec<u8>) -> Result<(), TransportError> {
        if self.pending_bytes.saturating_add(bytes.len()) > MAX_OUTBOUND_BYTES {
            return Err(TransportError);
        }
        self.pending_bytes += bytes.len();
        self.frames.push(PendingFrame { bytes, offset: 0 });
        Ok(())
    }

    fn flush(&mut self, socket: &mut Socket) -> Result<(), TransportError> {
        let mut written = 0;
        while written < MAX_WRITE_BYTES_PER_TICK {
            let Some(frame) = self.frames.get_mut(self.head) else {
                break;
            };
            let count = (frame.bytes.len() - frame.offset)
                .min(MAX_WRITE_CHUNK)
                .min(MAX_WRITE_BYTES_PER_TICK - written);
            if count == 0 {
                self.head += 1;
                continue;
            }
            match socket.write(&frame.bytes[frame.offset..frame.offset + count]) {
                Ok(0) => return Err(TransportError),
                Ok(count) => {
                    frame.offset += count;
                    written += count;
                    self.pending_bytes = self.pending_bytes.saturating_sub(count);
                    if frame.offset == frame.bytes.len() {
                        self.head += 1;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => return Err(TransportError),
            }
        }
        if self.head > 0 && self.head.saturating_mul(2) >= self.frames.len() {
            self.frames.drain(..self.head);
            self.head = 0;
        }
        Ok(())
    }
}

fn poll_handle_frame(
    socket: &Socket,
    frame: &mut Vec<u8>,
) -> Result<Option<(MessageHeader, Vec<u8>, Handle)>, TransportError> {
    let mut probe = [];
    let required_length = match socket.recv_handle_and_data(&mut probe) {
        Err(SocketError::ReceiveBufferTooSmall { required_len }) => required_len,
        Err(SocketError::WouldBlock) => return Ok(None),
        _ => return Err(TransportError),
    };
    if !(MessageHeader::SIZE..=MessageHeader::SIZE + sws_remote_protocol::MAX_PAYLOAD_SIZE)
        .contains(&required_length)
    {
        return Err(TransportError);
    }
    frame.clear();
    frame.resize(required_length, 0);
    let (handle, received) = socket
        .recv_handle_and_data(frame)
        .map_err(|_| TransportError)?;
    if received != required_length {
        return Err(TransportError);
    }
    let header = decode_header(frame).map_err(|_| TransportError)?;
    if MessageHeader::SIZE + header.payload_size as usize != frame.len() {
        return Err(TransportError);
    }
    Ok(Some((
        header,
        frame[MessageHeader::SIZE..].to_vec(),
        handle,
    )))
}

#[derive(Debug)]
struct TransportError;
