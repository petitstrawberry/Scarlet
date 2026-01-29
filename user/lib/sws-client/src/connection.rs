//! Connection management for SWS client

use crate::error::Error;
use crate::event::{Event, InputEvent};
use crate::surface::Surface;
use crate::TransientFlags;
use crate::WindowSizeLimits;
use scarlet_std::collections::BTreeMap;
use scarlet_std::ipc::SharedMemory;
use scarlet_std::println;
use scarlet_std::socket::Socket;
use scarlet_std::string::String;
use scarlet_std::vec::Vec;
use sws_protocol::{self as protocol, ServerMessage};

/// Window list entry
#[derive(Debug, Clone)]
pub struct WindowListEntry {
    pub window_id: u32,
    pub app_id: String,
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
            Ok(0) => {
                // println!("[sws-client] read_exact: EOF (connection closed)");
                return Err(Error::Disconnected);
            }
            Ok(n) => {
                filled += n;
            }
            Err(e) => {
                if e.kind() != scarlet_std::io::ErrorKind::WouldBlock {
                    println!("[sws-client] read_exact: error (not WouldBlock): {:?}", e);
                }
                if e.kind() == scarlet_std::io::ErrorKind::WouldBlock {
                    return Err(Error::WouldBlock);
                }
                return Err(Error::IoError);
            }
        }
    }
    Ok(())
}

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

    fn poll(
        &mut self,
        socket: &mut Socket,
        out_payload: &mut Vec<u8>,
    ) -> Result<Option<u32>, Error> {
        use scarlet_std::io::Read;

        loop {
            if !self.header_parsed {
                match socket.read(&mut self.header[self.header_filled..]) {
                    Ok(0) => return Err(Error::Disconnected),
                    Ok(n) => {
                        self.header_filled += n;
                        if self.header_filled < self.header.len() {
                            continue;
                        }
                        let header = protocol::MessageHeader::from_le_bytes(self.header);
                        let payload_len = header.payload_size as usize;
                        if payload_len > protocol::MAX_PAYLOAD_SIZE {
                            return Err(Error::ProtocolError);
                        }
                        self.msg_type = header.msg_type;
                        self.payload_len = payload_len;
                        self.payload.clear();
                        if payload_len > 0 {
                            self.payload.resize(payload_len, 0);
                        }
                        self.payload_filled = 0;
                        self.header_parsed = true;
                        if payload_len == 0 {
                            out_payload.clear();
                            let msg_type = self.msg_type;
                            self.reset();
                            return Ok(Some(msg_type));
                        }
                    }
                    Err(e) => {
                        if e.kind() == scarlet_std::io::ErrorKind::WouldBlock {
                            return Ok(None);
                        }
                        return Err(Error::IoError);
                    }
                }
            }

            if self.header_parsed {
                match socket.read(&mut self.payload[self.payload_filled..]) {
                    Ok(0) => return Err(Error::Disconnected),
                    Ok(n) => {
                        self.payload_filled += n;
                        if self.payload_filled < self.payload_len {
                            continue;
                        }
                        out_payload.clear();
                        out_payload.extend_from_slice(&self.payload);
                        let msg_type = self.msg_type;
                        self.reset();
                        return Ok(Some(msg_type));
                    }
                    Err(e) => {
                        if e.kind() == scarlet_std::io::ErrorKind::WouldBlock {
                            return Ok(None);
                        }
                        return Err(Error::IoError);
                    }
                }
            }
        }
    }
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
    frame_reader: FrameReader,
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
            frame_reader: FrameReader::new(),
        })
    }

    /// Create a new surface (window)
    ///
    /// This sends a CreateWindow request and waits for the response.
    /// The returned Surface can be drawn to immediately.
    /// Default window type is NORMAL (0).
    pub fn create_surface(&mut self, app_id: &str, app_name: &str, menu_titles: &str, width: u32, height: u32) -> Result<u32, Error> {
        self.create_surface_with_type_and_resizable(app_id, app_name, menu_titles, width, height, 0, true)
    }

    /// Create a new surface (window) with specific window type
    ///
    /// This sends a CreateWindow request and waits for the response.
    /// The returned Surface can be drawn to immediately.
    pub fn create_surface_with_type(&mut self, app_id: &str, app_name: &str, menu_titles: &str, width: u32, height: u32, window_type: u32) -> Result<u32, Error> {
        self.create_surface_with_type_and_resizable(app_id, app_name, menu_titles, width, height, window_type, true)
    }

    /// Create a new surface (window) with specific window type and resizable flag
    ///
    /// This sends a CreateWindow request and waits for the response.
    /// The returned Surface can be drawn to immediately.
    pub fn create_surface_with_type_and_resizable(
        &mut self,
        app_id: &str,
        app_name: &str,
        menu_titles: &str,
        width: u32,
        height: u32,
        window_type: u32,
        resizable: bool,
    ) -> Result<u32, Error> {
        let focus_on_create = true;
        let active_on_focus = window_type == 0;
        self.create_surface_with_type_and_policies(
            app_id,
            app_name,
            menu_titles,
            width,
            height,
            window_type,
            resizable,
            focus_on_create,
            active_on_focus,
        )
    }

    /// Create a new surface (window) with explicit focus/active policies
    pub fn create_surface_with_type_and_policies(
        &mut self,
        app_id: &str,
        app_name: &str,
        menu_titles: &str,
        width: u32,
        height: u32,
        window_type: u32,
        resizable: bool,
        focus_on_create: bool,
        active_on_focus: bool,
    ) -> Result<u32, Error> {
        // Send CreateWindow request
        let payload = protocol::payload_create_window(
            app_id.as_bytes(),
            app_name.as_bytes(),
            menu_titles.as_bytes(),
            width,
            height,
            window_type,
            resizable,
            focus_on_create,
            active_on_focus,
        );
        // println!("[sws-client] Creating surface: payload size {}", payload.len());
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

    /// Create a new surface (window) with explicit focus/active policies and initial position.
    pub fn create_surface_with_type_and_policies_at(
        &mut self,
        app_id: &str,
        app_name: &str,
        menu_titles: &str,
        width: u32,
        height: u32,
        window_type: u32,
        resizable: bool,
        focus_on_create: bool,
        active_on_focus: bool,
        x: i32,
        y: i32,
    ) -> Result<u32, Error> {
        let payload = protocol::payload_create_window_with_position(
            app_id.as_bytes(),
            app_name.as_bytes(),
            menu_titles.as_bytes(),
            width,
            height,
            window_type,
            resizable,
            focus_on_create,
            active_on_focus,
            x,
            y,
        );
        // println!("[sws-client] Creating surface: payload size {}", payload.len());
        write_frame(
            &mut self.socket,
            protocol::client_msg::CREATE_WINDOW,
            &payload,
        )
        .map_err(|_| Error::SendFailed)?;

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

        let shm_handle = self
            .socket
            .recv_handle()
            .map_err(|_| Error::ShmHandleFailed)?;

        let shm =
            SharedMemory::from_handle(shm_handle).map_err(|_| Error::ShmHandleFailed)?;

        self.socket
            .set_nonblocking(true)
            .map_err(|_| Error::SocketConfig)?;

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

    /// Update menu titles for a window (format: "menu1|menu2|menu3").
    pub fn set_window_menu_titles(
        &mut self,
        surface_id: u32,
        menu_titles: &str,
    ) -> Result<(), Error> {
        if self.surfaces.get(&surface_id).is_none() {
            return Err(Error::SurfaceNotFound);
        }

        let payload = protocol::payload_set_window_menu_titles(surface_id, menu_titles.as_bytes());
        write_frame(
            &mut self.socket,
            protocol::client_msg::SET_WINDOW_MENU_TITLES,
            &payload,
        )
        .map_err(|_| Error::SendFailed)
    }

    /// Notify the server that a menu item was activated for a window.
    pub fn activate_menu_item(
        &mut self,
        window_id: u32,
        menu_item_id: &str,
    ) -> Result<(), Error> {
        let payload = protocol::payload_activate_menu_item(window_id, menu_item_id.as_bytes());
        write_frame(
            &mut self.socket,
            protocol::client_msg::ACTIVATE_MENU_ITEM,
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

    /// Focus and raise a window to the top of the Z-order.
    ///
    /// This only works for surfaces created by this client connection.
    /// For focusing windows created by other clients, use `focus_window_any`.
    pub fn focus_window(&mut self, surface_id: u32) -> Result<(), Error> {
        if self.surfaces.get(&surface_id).is_none() {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_focus_window(surface_id);
        write_frame(
            &mut self.socket,
            protocol::client_msg::FOCUS_WINDOW,
            &payload,
        )
        .map_err(|_| Error::SendFailed)
    }

    /// Focus and raise any window (including those created by other clients).
    ///
    /// Unlike `focus_window`, this does not check if the surface exists locally.
    /// This is useful for system services like stemd that need to focus windows
    /// created by other applications.
    ///
    /// The server will return an error if the window_id does not exist.
    pub fn focus_window_any(&mut self, window_id: u32) -> Result<(), Error> {
        let payload = protocol::payload_focus_window(window_id);
        write_frame(
            &mut self.socket,
            protocol::client_msg::FOCUS_WINDOW,
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

    /// Set whether window content contains alpha channel (semi-transparent pixels).
    ///
    /// This is separate from window opacity - this controls whether pixel alpha
    /// values in the window buffer should be respected during composition.
    ///
    /// - false: Window content is fully opaque, use fast copy path (default)
    /// - true: Window content has semi-transparent pixels, use alpha blending
    pub fn set_window_has_alpha_content(
        &mut self,
        surface_id: u32,
        has_alpha: bool,
    ) -> Result<(), Error> {
        if self.surfaces.get(&surface_id).is_none() {
            return Err(Error::SurfaceNotFound);
        }
        let payload = protocol::payload_set_window_has_alpha_content(surface_id, has_alpha);
        write_frame(
            &mut self.socket,
            protocol::client_msg::SET_WINDOW_HAS_ALPHA_CONTENT,
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
            match self.frame_reader.poll(&mut self.socket, &mut self.read_payload) {
                Ok(Some(msg_type)) => {
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
                            ServerMessage::FocusChanged {
                                window_id,
                                app_id,
                                app_id_len,
                                app_name,
                                app_name_len,
                                title,
                                title_len,
                                menu_titles,
                                menu_titles_len,
                            } => {
                                // Convert fixed-size buffers to String
                                let app_id_str = String::from_utf8_lossy(&app_id[..app_id_len as usize]).into_owned();
                                let app_name_str = String::from_utf8_lossy(&app_name[..app_name_len as usize]).into_owned();
                                let title_str = String::from_utf8_lossy(&title[..title_len as usize]).into_owned();
                                let menu_titles_str = String::from_utf8_lossy(&menu_titles[..menu_titles_len as usize]).into_owned();
                                self.pending_events.push(Event::FocusChanged {
                                    window_id,
                                    app_id: app_id_str,
                                    app_name: app_name_str,
                                    title: title_str,
                                    menu_titles: menu_titles_str,
                                });
                                count += 1;
                            }
                            ServerMessage::ActiveAppChanged {
                                window_id,
                                app_id,
                                app_id_len,
                                app_name,
                                app_name_len,
                                title,
                                title_len,
                                menu_titles,
                                menu_titles_len,
                            } => {
                                // Convert fixed-size buffers to String
                                let app_id_str = String::from_utf8_lossy(&app_id[..app_id_len as usize]).into_owned();
                                let app_name_str = String::from_utf8_lossy(&app_name[..app_name_len as usize]).into_owned();
                                let title_str = String::from_utf8_lossy(&title[..title_len as usize]).into_owned();
                                let menu_titles_str = String::from_utf8_lossy(&menu_titles[..menu_titles_len as usize]).into_owned();
                                self.pending_events.push(Event::ActiveAppChanged {
                                    window_id,
                                    app_id: app_id_str,
                                    app_name: app_name_str,
                                    title: title_str,
                                    menu_titles: menu_titles_str,
                                });
                                count += 1;
                            }
                            ServerMessage::MenuItemActivated {
                                window_id,
                                menu_item_id,
                                menu_item_id_len,
                            } => {
                                let menu_item_id_str = String::from_utf8_lossy(
                                    &menu_item_id[..menu_item_id_len as usize],
                                )
                                .into_owned();
                                self.pending_events.push(Event::MenuItemActivated {
                                    window_id,
                                    menu_item_id: menu_item_id_str,
                                });
                                count += 1;
                            }
                            _ => {}
                        }
                    }
                }
                Ok(None) => break,
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

        let ev = self.pending_events[self.pending_head].clone();
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
                        app_id: w.app_id,
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
