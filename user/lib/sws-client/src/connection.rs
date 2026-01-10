//! Connection management for SWS client

use crate::error::Error;
use crate::event::{Event, InputEvent};
use crate::surface::Surface;
use scarlet_std::collections::BTreeMap;
use scarlet_std::ipc::SharedMemory;
use scarlet_std::socket::Socket;
use scarlet_std::vec::Vec;
use sws_protocol::{self as protocol, ServerMessage};

fn read_exact(socket: &mut Socket, buf: &mut [u8]) -> Result<(), Error> {
    use scarlet_std::io::Read;

    let mut filled = 0;
    while filled < buf.len() {
        match socket.read(&mut buf[filled..]) {
            Ok(0) => return Err(Error::Disconnected),
            Ok(n) => filled += n,
            Err(e) => {
                if e.kind() == scarlet_std::io::ErrorKind::WouldBlock {
                    return Err(Error::WouldBlock);
                }
                return Err(Error::IoError);
            }
        }
    }
    Ok(())
}

fn write_all(socket: &mut Socket, buf: &[u8]) -> Result<(), Error> {
    use scarlet_std::io::Write;

    let mut written = 0;
    while written < buf.len() {
        match socket.write(&buf[written..]) {
            Ok(0) => return Err(Error::Disconnected),
            Ok(n) => written += n,
            Err(_) => return Err(Error::IoError),
        }
    }
    Ok(())
}

fn read_frame(socket: &mut Socket) -> Result<(u32, Vec<u8>), Error> {
    let mut header_bytes = [0u8; protocol::MessageHeader::SIZE];
    read_exact(socket, &mut header_bytes)?;
    let header = protocol::MessageHeader::from_le_bytes(header_bytes);

    let payload_len = header.payload_size as usize;
    if payload_len > protocol::MAX_PAYLOAD_SIZE {
        return Err(Error::ProtocolError);
    }

    let mut payload = Vec::new();
    if payload_len > 0 {
        payload.resize(payload_len, 0);
        read_exact(socket, &mut payload)?;
    }

    Ok((header.msg_type, payload))
}

fn write_frame(socket: &mut Socket, msg_type: u32, payload: &[u8]) -> Result<(), Error> {
    use scarlet_std::io::Write;

    let frame = protocol::encode_frame(msg_type, payload);
    write_all(socket, &frame)?;
    socket.flush().map_err(|_| Error::IoError)?;
    Ok(())
}

/// Connection to the Scarlet Window Server
///
/// This manages the socket connection, surfaces, and event dispatch.
/// Only one Connection should exist per application.
pub struct Connection {
    socket: Socket,
    surfaces: BTreeMap<u32, Surface>,
    pending_events: Vec<Event>,
    pending_head: usize,
}

impl Connection {
    /// Connect to SWS at the default socket path (/tmp/sws.sock)
    pub fn connect_default() -> Result<Self, Error> {
        Self::connect("/tmp/sws.sock")
    }

    /// Connect to SWS at the specified socket path
    pub fn connect(socket_path: &str) -> Result<Self, Error> {
        let socket = Socket::new().map_err(|_| Error::SocketCreation)?;
        socket
            .connect(socket_path)
            .map_err(|_| Error::ConnectionFailed)?;

        // Set socket to non-blocking mode once at connection time
        socket
            .set_nonblocking(true)
            .map_err(|_| Error::SocketConfig)?;

        Ok(Self {
            socket,
            surfaces: BTreeMap::new(),
            pending_events: Vec::new(),
            pending_head: 0,
        })
    }

    /// Create a new surface (window)
    ///
    /// This sends a CreateWindow request and waits for the response.
    /// The returned Surface can be drawn to immediately.
    pub fn create_surface(&mut self, width: u32, height: u32) -> Result<u32, Error> {
        // Send CreateWindow request
        let payload = protocol::payload_create_window(width, height);
        write_frame(
            &mut self.socket,
            protocol::client_msg::CREATE_WINDOW,
            &payload,
        )
        .map_err(|_| Error::SendFailed)?;

        // Block until we receive the response
        // Temporarily set blocking mode for synchronous create
        self.socket
            .set_nonblocking(false)
            .map_err(|_| Error::SocketConfig)?;

        let (msg_type, payload) = read_frame(&mut self.socket).map_err(|_| Error::ReceiveFailed)?;

        let response =
            protocol::parse_server_message(msg_type, &payload).map_err(|_| Error::InvalidResponse)?;

        let (surface_id, _shm_size) = match response {
            ServerMessage::WindowCreated {
                window_id,
                shm_size,
            } => (window_id, shm_size),
            _ => return Err(Error::InvalidResponse),
        };

        // Receive SHM handle (out-of-band)
        let shm_handle = self
            .socket
            .recv_handle()
            .map_err(|_| Error::ShmHandleFailed)?;

        let shm =
            SharedMemory::from_handle(shm_handle).map_err(|_| Error::ShmHandleFailed)?;

        // Restore non-blocking mode
        self.socket
            .set_nonblocking(true)
            .map_err(|_| Error::SocketConfig)?;

        // Create surface object
        let surface = Surface::new(surface_id, width, height, shm)?;
        self.surfaces.insert(surface_id, surface);

        Ok(surface_id)
    }

    /// Destroy a surface
    pub fn destroy_surface(&mut self, surface_id: u32) -> Result<(), Error> {
        if self.surfaces.remove(&surface_id).is_none() {
            return Err(Error::SurfaceNotFound);
        }

        let payload = protocol::payload_destroy_window(surface_id);
        write_frame(
            &mut self.socket,
            protocol::client_msg::DESTROY_WINDOW,
            &payload,
        )
        .map_err(|_| Error::SendFailed)?;

        Ok(())
    }

    /// Get a reference to a surface
    pub fn surface(&self, surface_id: u32) -> Option<&Surface> {
        self.surfaces.get(&surface_id)
    }

    /// Get a mutable reference to a surface
    pub fn surface_mut(&mut self, surface_id: u32) -> Option<&mut Surface> {
        self.surfaces.get_mut(&surface_id)
    }

    /// Commit surface changes to the server
    ///
    /// This notifies the server that the surface buffer has been updated.
    pub fn commit(&mut self, surface_id: u32) -> Result<(), Error> {
        let surface = self.surfaces.get_mut(&surface_id).ok_or(Error::SurfaceNotFound)?;

        if surface.is_dirty() {
            let payload = protocol::payload_update_buffer(
                surface_id,
                0,
                0,
                surface.width(),
                surface.height(),
            );
            write_frame(
                &mut self.socket,
                protocol::client_msg::UPDATE_BUFFER,
                &payload,
            )
            .map_err(|_| Error::SendFailed)?;

            surface.clear_dirty();
        }

        Ok(())
    }

    /// Flush pending writes to the socket
    pub fn flush(&mut self) -> Result<(), Error> {
        use scarlet_std::io::Write;
        self.socket.flush().map_err(|_| Error::IoError)
    }

    /// Request that the window manager begins an interactive move for this surface.
    ///
    /// The server is expected to track pointer movement and update the window position
    /// until the primary button is released.
    pub fn request_move_window(&mut self, surface_id: u32) -> Result<(), Error> {
        let payload = protocol::payload_request_move_window(surface_id);
        write_frame(
            &mut self.socket,
            protocol::client_msg::REQUEST_MOVE_WINDOW,
            &payload,
        )
        .map_err(|_| Error::SendFailed)
    }

    /// Set the window position (absolute) for this surface.
    pub fn move_window(&mut self, surface_id: u32, x: i32, y: i32) -> Result<(), Error> {
        let payload = protocol::payload_move_window(surface_id, x, y);
        write_frame(&mut self.socket, protocol::client_msg::MOVE_WINDOW, &payload)
            .map_err(|_| Error::SendFailed)
    }

    /// Dispatch pending events (non-blocking)
    ///
    /// Reads all available events from the socket and stores them.
    /// Returns the number of events read.
    pub fn dispatch(&mut self) -> Result<usize, Error> {
        let mut count = 0;

        // Opportunistically compact the queue if we've consumed a lot.
        // This avoids unbounded growth when clients mix `poll_event()` and `dispatch()`.
        if self.pending_head > 0 && self.pending_head * 2 >= self.pending_events.len() {
            self.pending_events.drain(..self.pending_head);
            self.pending_head = 0;
        }

        loop {
            match read_frame(&mut self.socket) {
                Ok((msg_type, payload)) => {
                    if let Ok(msg) = protocol::parse_server_message(msg_type, &payload) {
                        match msg {
                            ServerMessage::InputEvent {
                                time,
                                type_,
                                code,
                                value,
                            } => {
                                self.pending_events.push(Event::Input(InputEvent {
                                    time,
                                    type_,
                                    code,
                                    value,
                                }));
                                count += 1;
                            }
                            ServerMessage::WindowDestroyed { window_id } => {
                                self.surfaces.remove(&window_id);
                                self.pending_events
                                    .push(Event::SurfaceDestroyed { surface_id: window_id });
                                count += 1;
                            }
                            ServerMessage::Error { code } => {
                                self.pending_events.push(Event::Error { code });
                                count += 1;
                            }
                            _ => {}
                        }
                    }
                }
                Err(Error::WouldBlock) => break,
                Err(Error::Disconnected) => return Err(Error::Disconnected),
                Err(_) => return Err(Error::IoError),
            }
        }

        Ok(count)
    }

    /// Pop the next pending event
    pub fn poll_event(&mut self) -> Option<Event> {
        if self.pending_head >= self.pending_events.len() {
            self.pending_events.clear();
            self.pending_head = 0;
            return None;
        }

        let ev = self.pending_events[self.pending_head];
        self.pending_head += 1;

        if self.pending_head >= self.pending_events.len() {
            self.pending_events.clear();
            self.pending_head = 0;
        }

        Some(ev)
    }

    /// Drain all pending events
    pub fn drain_events(&mut self) -> Vec<Event> {
        if self.pending_head == 0 {
            core::mem::take(&mut self.pending_events)
        } else {
            let v = self.pending_events[self.pending_head..].to_vec();
            self.pending_events.clear();
            self.pending_head = 0;
            v
        }
    }

    /// Check if there are pending events
    pub fn has_events(&self) -> bool {
        self.pending_head < self.pending_events.len()
    }
}
