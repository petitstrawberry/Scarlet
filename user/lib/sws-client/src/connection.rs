//! Connection management for SWS client

use crate::error::Error;
use crate::event::{Event, InputEvent};
use crate::surface::Surface;
use crate::TransientFlags;
use crate::WindowSizeLimits;
use scarlet_std::collections::BTreeMap;
use scarlet_std::ipc::SharedMemory;
use scarlet_std::socket::Socket;
use scarlet_std::string::String;
use scarlet_std::vec::Vec;
use sws_protocol::{self as protocol, ServerMessage};

/// Window list entry
#[derive(Debug, Clone)]
pub struct WindowListEntry {
    pub window_id: u32,
    pub title: String,
    pub window_type: u32,
    pub visible: bool,
    pub focused: bool,
    pub minimized: bool,
}

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
            Err(e) => {
                if e.kind() == scarlet_std::io::ErrorKind::WouldBlock {
                    // Socket is non-blocking; retry a bit later.
                    let _ = scarlet_std::thread::sleep(core::time::Duration::from_millis(1));
                    continue;
                }
                return Err(Error::IoError);
            }
        }
    }
    Ok(())
}

fn read_frame_into(socket: &mut Socket, payload: &mut Vec<u8>) -> Result<u32, Error> {
    let mut header_bytes = [0u8; protocol::MessageHeader::SIZE];
    read_exact(socket, &mut header_bytes)?;
    let header = protocol::MessageHeader::from_le_bytes(header_bytes);

    let payload_len = header.payload_size as usize;
    if payload_len > protocol::MAX_PAYLOAD_SIZE {
        return Err(Error::ProtocolError);
    }

    payload.clear();
    if payload_len > 0 {
        payload.resize(payload_len, 0);
        read_exact(socket, payload)?;
    }

    Ok(header.msg_type)
}

fn write_frame(socket: &mut Socket, msg_type: u32, payload: &[u8]) -> Result<(), Error> {
    use scarlet_std::io::Write;

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
    read_payload: Vec<u8>,
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
            read_payload: Vec::new(),
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

        let msg_type =
            read_frame_into(&mut self.socket, &mut self.read_payload).map_err(|_| Error::ReceiveFailed)?;

        let response = protocol::parse_server_message(msg_type, &self.read_payload)
            .map_err(|_| Error::InvalidResponse)?;

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

    /// Set per-window size constraints.
    ///
    /// All values are in pixels. `0` means "unset".
    pub fn set_window_size_limits(
        &mut self,
        surface_id: u32,
        limits: WindowSizeLimits,
    ) -> Result<(), Error> {
        self.set_window_size_limits_raw(
            surface_id,
            limits.min_width,
            limits.min_height,
            limits.max_width,
            limits.max_height,
        )
    }

    /// Set per-window size constraints (raw values).
    ///
    /// Prefer [`set_window_size_limits`] with [`WindowSizeLimits`].
    pub fn set_window_size_limits_raw(
        &mut self,
        surface_id: u32,
        min_width: u32,
        min_height: u32,
        max_width: u32,
        max_height: u32,
    ) -> Result<(), Error> {
        if self.surfaces.get(&surface_id).is_none() {
            return Err(Error::SurfaceNotFound);
        }

        let payload = protocol::payload_set_window_size_limits(
            surface_id,
            min_width,
            min_height,
            max_width,
            max_height,
        );
        write_frame(
            &mut self.socket,
            protocol::client_msg::SET_WINDOW_SIZE_LIMITS,
            &payload,
        )
        .map_err(|_| Error::SendFailed)
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

    /// Commit a specific region of the surface to the server
    ///
    /// This is more efficient than `commit()` when only a small region changed.
    pub fn commit_region(&mut self, surface_id: u32, x: u32, y: u32, width: u32, height: u32) -> Result<(), Error> {
        let surface = self.surfaces.get_mut(&surface_id).ok_or(Error::SurfaceNotFound)?;

        // Clamp region to surface bounds
        let sw = surface.width();
        let sh = surface.height();
        let x = x.min(sw);
        let y = y.min(sh);
        let width = width.min(sw.saturating_sub(x));
        let height = height.min(sh.saturating_sub(y));

        if width == 0 || height == 0 {
            return Ok(());
        }

        let payload = protocol::payload_update_buffer(surface_id, x as i32, y as i32, width, height);
        write_frame(
            &mut self.socket,
            protocol::client_msg::UPDATE_BUFFER,
            &payload,
        )
        .map_err(|_| Error::SendFailed)?;

        surface.clear_dirty();
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

    /// Set (or clear) the logical parent of a window.
    ///
    /// Use this for transient dialogs/popups so the compositor can keep the child
    /// stacked above its parent and move it together during interactive drags.
    ///
    /// `parent_surface_id == None` clears the parent.
    pub fn set_window_parent(
        &mut self,
        surface_id: u32,
        parent_surface_id: Option<u32>,
    ) -> Result<(), Error> {
        let parent_id = parent_surface_id.unwrap_or(0);
        let payload = protocol::payload_set_window_parent(surface_id, parent_id);
        write_frame(
            &mut self.socket,
            protocol::client_msg::SET_WINDOW_PARENT,
            &payload,
        )
        .map_err(|_| Error::SendFailed)
    }

    /// Configure transient behavior flags for a window.
    pub fn set_window_transient_flags(
        &mut self,
        surface_id: u32,
        flags: TransientFlags,
    ) -> Result<(), Error> {
        self.set_window_transient_flags_raw(surface_id, flags.bits())
    }

    /// Configure transient behavior flags for a window (raw bits).
    ///
    /// Prefer [`set_window_transient_flags`] with [`TransientFlags`].
    pub fn set_window_transient_flags_raw(
        &mut self,
        surface_id: u32,
        flags: u32,
    ) -> Result<(), Error> {
        let payload = protocol::payload_set_window_transient_flags(surface_id, flags);
        write_frame(
            &mut self.socket,
            protocol::client_msg::SET_WINDOW_TRANSIENT_FLAGS,
            &payload,
        )
        .map_err(|_| Error::SendFailed)
    }

    /// Minimize a window (hide it; buffer size remains unchanged).
    pub fn minimize_window(&mut self, surface_id: u32) -> Result<(), Error> {
        if self.surfaces.get(&surface_id).is_none() {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_minimize_window(surface_id);
        write_frame(
            &mut self.socket,
            protocol::client_msg::MINIMIZE_WINDOW,
            &payload,
        )
        .map_err(|_| Error::SendFailed)
    }

    /// Maximize a window.
    ///
    /// The server may respond with `WINDOW_CONFIGURE` to request a buffer resize.
    pub fn maximize_window(&mut self, surface_id: u32) -> Result<(), Error> {
        if self.surfaces.get(&surface_id).is_none() {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_maximize_window(surface_id);
        write_frame(
            &mut self.socket,
            protocol::client_msg::MAXIMIZE_WINDOW,
            &payload,
        )
        .map_err(|_| Error::SendFailed)
    }

    /// Restore a window from minimized or maximized state.
    pub fn restore_window(&mut self, surface_id: u32) -> Result<(), Error> {
        if self.surfaces.get(&surface_id).is_none() {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_restore_window(surface_id);
        write_frame(
            &mut self.socket,
            protocol::client_msg::RESTORE_WINDOW,
            &payload,
        )
        .map_err(|_| Error::SendFailed)
    }

    /// Set the window type used for Z-order management.
    ///
    /// The `window_type` argument selects one of the window type constants defined
    /// by the SWS protocol:
    ///
    /// - `NORMAL = 0`: Standard application window.
    /// - `ALWAYS_ON_TOP = 1`: Stays above `NORMAL` and `TASKBAR` windows.
    /// - `TASKBAR = 2`: Taskbar or dock-style window, above `DESKTOP` but
    ///   below `ALWAYS_ON_TOP`.
    /// - `DESKTOP = 3`: Desktop background window, at the bottom of the
    ///   stacking order.
    ///
    /// Higher-priority types (for example `ALWAYS_ON_TOP`) are kept above
    /// lower-priority types in the global Z-order. See
    /// [`sws_protocol::window_types`] for the available constants.
    pub fn set_window_type(&mut self, surface_id: u32, window_type: u32) -> Result<(), Error> {
        if self.surfaces.get(&surface_id).is_none() {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_set_window_type(surface_id, window_type);
        write_frame(
            &mut self.socket,
            protocol::client_msg::SET_WINDOW_TYPE,
            &payload,
        )
        .map_err(|_| Error::SendFailed)
    }

    /// Set per-window opacity (0 = fully transparent, 255 = fully opaque).
    pub fn set_window_opacity(&mut self, surface_id: u32, opacity: u8) -> Result<(), Error> {
        if self.surfaces.get(&surface_id).is_none() {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_set_window_opacity(surface_id, opacity);
        write_frame(
            &mut self.socket,
            protocol::client_msg::SET_WINDOW_OPACITY,
            &payload,
        )
        .map_err(|_| Error::SendFailed)
    }

    /// Set the workarea (usable screen area) for the window manager.
    ///
    /// This informs the window manager about the area where normal windows
    /// should be placed, typically excluding the area occupied by the taskbar.
    pub fn set_workarea(&mut self, x: i32, y: i32, width: u32, height: u32) -> Result<(), Error> {
        let payload = protocol::payload_set_workarea(x, y, width, height);
        write_frame(
            &mut self.socket,
            protocol::client_msg::SET_WORKAREA,
            &payload,
        )
        .map_err(|_| Error::SendFailed)
    }

    /// Set whether a window can be resized by the user via interactive resize.
    pub fn set_window_resizable(&mut self, surface_id: u32, resizable: bool) -> Result<(), Error> {
        if self.surfaces.get(&surface_id).is_none() {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_set_window_resizable(surface_id, resizable);
        write_frame(
            &mut self.socket,
            protocol::client_msg::SET_WINDOW_RESIZABLE,
            &payload,
        )
        .map_err(|_| Error::SendFailed)
    }

    /// Resize a surface.
    ///
    /// This is a synchronous request: it waits for `WINDOW_RESIZED` and a new SHM handle,
    /// then updates the local surface mapping.
    pub fn resize_window(&mut self, surface_id: u32, width: u32, height: u32) -> Result<(), Error> {
        if self.surfaces.get(&surface_id).is_none() {
            return Err(Error::SurfaceNotFound);
        }

        let payload = protocol::payload_resize_window(surface_id, width, height);
        write_frame(&mut self.socket, protocol::client_msg::RESIZE_WINDOW, &payload)
            .map_err(|_| Error::SendFailed)?;

        // Block until we receive WINDOW_RESIZED + SHM handle.
        self.socket
            .set_nonblocking(false)
            .map_err(|_| Error::SocketConfig)?;

        let msg_type =
            read_frame_into(&mut self.socket, &mut self.read_payload).map_err(|_| Error::ReceiveFailed)?;
        let response = protocol::parse_server_message(msg_type, &self.read_payload)
            .map_err(|_| Error::InvalidResponse)?;

        let (window_id, _shm_size, new_w, new_h) = match response {
            ServerMessage::WindowResized {
                window_id,
                shm_size,
                width,
                height,
            } => (window_id, shm_size, width, height),
            _ => {
                self.socket
                    .set_nonblocking(true)
                    .map_err(|_| Error::SocketConfig)?;
                return Err(Error::InvalidResponse);
            }
        };

        if window_id != surface_id {
            self.socket
                .set_nonblocking(true)
                .map_err(|_| Error::SocketConfig)?;
            return Err(Error::InvalidResponse);
        }

        let shm_handle = self
            .socket
            .recv_handle()
            .map_err(|_| Error::ShmHandleFailed)?;
        let shm = SharedMemory::from_handle(shm_handle).map_err(|_| Error::ShmHandleFailed)?;

        self.socket
            .set_nonblocking(true)
            .map_err(|_| Error::SocketConfig)?;

        if let Some(surface) = self.surfaces.get_mut(&surface_id) {
            surface.remap(new_w, new_h, shm)?;
            Ok(())
        } else {
            Err(Error::SurfaceNotFound)
        }
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
            match read_frame_into(&mut self.socket, &mut self.read_payload) {
                Ok(msg_type) => {
                    if let Ok(msg) = protocol::parse_server_message(msg_type, &self.read_payload) {
                        match msg {
                            ServerMessage::InputEvent {
                                window_id,
                                time,
                                type_,
                                code,
                                value,
                            } => {
                                self.pending_events.push(Event::Input(InputEvent {
                                    surface_id: window_id,
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
                            ServerMessage::WindowResized { window_id, .. } => {
                                // Resizes are handled synchronously by `resize_window()`.
                                // Ignore here to keep `dispatch()` non-blocking.
                                let _ = window_id;
                            }
                            ServerMessage::WindowConfigure {
                                window_id,
                                width,
                                height,
                            } => {
                                self.pending_events.push(Event::SurfaceConfigure {
                                    surface_id: window_id,
                                    width,
                                    height,
                                });
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

    /// Get the screen size.
    ///
    /// This is a synchronous request: it blocks until the server responds with SCREEN_SIZE.
    pub fn get_screen_size(&mut self) -> Result<(u32, u32), Error> {
        // Send GET_SCREEN_SIZE request (no payload)
        write_frame(&mut self.socket, protocol::client_msg::GET_SCREEN_SIZE, &[])
            .map_err(|_| Error::SendFailed)?;

        // Switch to blocking mode for synchronous response
        self.socket
            .set_nonblocking(false)
            .map_err(|_| Error::SocketConfig)?;

        // Wait for SCREEN_SIZE response
        let msg_type =
            read_frame_into(&mut self.socket, &mut self.read_payload).map_err(|_| Error::ReceiveFailed)?;
        let response = protocol::parse_server_message(msg_type, &self.read_payload)
            .map_err(|_| Error::InvalidResponse)?;

        match response {
            ServerMessage::ScreenSize { width, height } => {
                // Restore non-blocking mode
                let _ = self.socket.set_nonblocking(true);
                Ok((width, height))
            }
            _ => {
                // Restore non-blocking mode
                let _ = self.socket.set_nonblocking(true);
                Err(Error::InvalidResponse)
            }
        }
    }

    /// Get the list of all windows.
    ///
    /// This is a synchronous request: it blocks until the server responds with WINDOW_LIST.
    pub fn get_window_list(&mut self) -> Result<Vec<WindowListEntry>, Error> {
        // Send GET_WINDOW_LIST request (no payload)
        write_frame(&mut self.socket, protocol::client_msg::GET_WINDOW_LIST, &[])
            .map_err(|_| Error::SendFailed)?;

        // Switch to blocking mode for synchronous response
        self.socket
            .set_nonblocking(false)
            .map_err(|_| Error::SocketConfig)?;

        // Wait for WINDOW_LIST response
        let msg_type =
            read_frame_into(&mut self.socket, &mut self.read_payload).map_err(|_| Error::ReceiveFailed)?;
        let response = protocol::parse_server_message(msg_type, &self.read_payload)
            .map_err(|_| Error::InvalidResponse)?;

        match response {
            ServerMessage::WindowList => {
                // Use protocol library's parser
                let windows = protocol::parse_window_list_payload(&self.read_payload)
                    .map_err(|_| Error::InvalidResponse)?;

                // Restore non-blocking mode
                let _ = self.socket.set_nonblocking(true);

                // Convert protocol::WindowListEntry to sws_client::WindowListEntry
                Ok(windows
                    .into_iter()
                    .map(|w| WindowListEntry {
                        window_id: w.window_id,
                        title: w.title,
                        window_type: w.window_type,
                        visible: w.visible,
                        focused: w.focused,
                        minimized: w.minimized,
                    })
                    .collect())
            }
            _ => {
                // Restore non-blocking mode
                let _ = self.socket.set_nonblocking(true);
                Err(Error::InvalidResponse)
            }
        }
    }

    /// Check if there are pending events
    pub fn has_events(&self) -> bool {
        self.pending_head < self.pending_events.len()
    }
}
