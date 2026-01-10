//! Connection management for SWS client

use crate::error::Error;
use crate::event::{Event, InputEvent};
use crate::surface::Surface;
use scarlet_std::collections::BTreeMap;
use scarlet_std::ipc::SharedMemory;
use scarlet_std::socket::Socket;
use scarlet_std::vec::Vec;
use sws_protocol::{self as protocol, ServerMessage};

/// Connection to the Scarlet Window Server
///
/// This manages the socket connection, surfaces, and event dispatch.
/// Only one Connection should exist per application.
pub struct Connection {
    socket: Socket,
    surfaces: BTreeMap<u32, Surface>,
    pending_events: Vec<Event>,
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
        })
    }

    /// Create a new surface (window)
    ///
    /// This sends a CreateWindow request and waits for the response.
    /// The returned Surface can be drawn to immediately.
    pub fn create_surface(&mut self, width: u32, height: u32) -> Result<u32, Error> {
        // Send CreateWindow request
        protocol::write_create_window(&mut self.socket, width, height)
            .map_err(|_| Error::SendFailed)?;

        // Block until we receive the response
        // Temporarily set blocking mode for synchronous create
        self.socket
            .set_nonblocking(false)
            .map_err(|_| Error::SocketConfig)?;

        let (msg_type, payload) =
            protocol::read_frame(&mut self.socket).map_err(|_| Error::ReceiveFailed)?;

        let response =
            protocol::parse_server_message(msg_type, &payload).map_err(|_| Error::InvalidResponse)?;

        let (surface_id, _shm_size) = match response {
            ServerMessage::WindowCreated {
                window_id,
                shm_size,
            } => (window_id, shm_size),
            _ => return Err(Error::InvalidResponse),
        };

        // Receive SHM handle
        let shm_handle =
            protocol::recv_shm_handle(&self.socket).map_err(|_| Error::ShmHandleFailed)?;

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

        protocol::write_destroy_window(&mut self.socket, surface_id)
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
            protocol::write_update_buffer(
                &mut self.socket,
                surface_id,
                0,
                0,
                surface.width(),
                surface.height(),
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

    /// Dispatch pending events (non-blocking)
    ///
    /// Reads all available events from the socket and stores them.
    /// Returns the number of events read.
    pub fn dispatch(&mut self) -> Result<usize, Error> {
        let mut count = 0;

        loop {
            match protocol::read_frame(&mut self.socket) {
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
                Err(protocol::ProtocolError::IoWouldBlock) => break,
                Err(protocol::ProtocolError::IoDisconnected) => return Err(Error::Disconnected),
                Err(_) => return Err(Error::IoError),
            }
        }

        Ok(count)
    }

    /// Pop the next pending event
    pub fn poll_event(&mut self) -> Option<Event> {
        if self.pending_events.is_empty() {
            None
        } else {
            Some(self.pending_events.remove(0))
        }
    }

    /// Drain all pending events
    pub fn drain_events(&mut self) -> Vec<Event> {
        core::mem::take(&mut self.pending_events)
    }

    /// Check if there are pending events
    pub fn has_events(&self) -> bool {
        !self.pending_events.is_empty()
    }
}
