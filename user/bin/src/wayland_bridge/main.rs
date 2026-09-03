//! Wayland Bridge Server
//!
//! A Wayland protocol proxy server that bridges Wayland clients to the
//! Scarlet Window System (SWS). This allows running Wayland applications
//! on Scarlet OS by translating Wayland protocol messages to SWS IPC.
//!
//! Architecture:
//! - Wayland Client <-> UNIX Socket <-> Wayland Bridge <-> SWS Socket <-> SWS Server
//!
//! The bridge listens on a UNIX domain socket (e.g., /tmp/wayland-0) and
//! accepts Wayland protocol connections. It maintains a connection to SWS
//! and translates Wayland protocol messages to SWS protocol messages.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

mod input;
mod protocol;
mod region;
mod registry;
mod shm;
mod surface;
mod xdg_shell;

use input::InputManager;
use protocol::{MessageHeader, WaylandArg, WaylandMessage};
use registry::Registry;
use scarlet_os::time::monotonic_time_ns;
use shm::ShmManager;
use std::collections::BTreeMap;
use std::env;
use std::handle::Handle;
use std::io::{Read, Write};
use std::ipc::{SharedMemory, permissions};
use std::poll::{POLLERR, POLLHUP, POLLIN, POLLNVAL, POLLOUT, PollHandle, poll};
use std::socket::Socket;
use std::string::String;
use std::sync::{Arc, Mutex as StdMutex};
use std::thread;
use std::time::Duration;
use std::vec::Vec;
use surface::SurfaceManager;
use sws_protocol as protocol_sws;
use xdg_shell::XdgShellManager;

const MAX_PENDING_DAMAGE_RECTS: usize = 8;
const DAMAGE_MERGE_AREA_FACTOR: u64 = 2;
const MAX_WAYLAND_RECORD_SIZE: usize = 1024 * 1024 + MessageHeader::SIZE;
const SOCKET_FAILURE_EVENTS: u16 = POLLERR | POLLHUP | POLLNVAL;
const SWS_RESPONSE_TIMEOUT_NS: u64 = 5_000_000_000;

fn should_log_resource_count(count: u64) -> bool {
    count <= 4 || count.is_multiple_of(64)
}

fn locally_releasable_buffer(
    uses_sws_scene: bool,
    buffer_attached: bool,
    buffer_id: Option<u32>,
) -> Option<u32> {
    if uses_sws_scene || !buffer_attached {
        None
    } else {
        buffer_id
    }
}

fn take_message_handle<T>(
    interface: Option<&str>,
    opcode: u16,
    pending_handles: &mut Vec<T>,
) -> Option<T> {
    let expects_handle = interface == Some("wl_shm") && opcode == shm::shm_request::CREATE_POOL;
    if expects_handle && !pending_handles.is_empty() {
        Some(pending_handles.remove(0))
    } else {
        None
    }
}

/// Log level for the Wayland bridge
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LogLevel {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

/// Get the current log level from environment variable
fn get_log_level() -> LogLevel {
    use core::sync::atomic::{AtomicU8, Ordering};

    static LOG_LEVEL_CACHE: AtomicU8 = AtomicU8::new(u8::MAX);

    let cached = LOG_LEVEL_CACHE.load(Ordering::Relaxed);
    if cached != u8::MAX {
        return unsafe { core::mem::transmute::<u8, LogLevel>(cached) };
    }

    let level = match env::var("WAYLAND_BRIDGE_LOG") {
        Some(val) => match val.as_str() {
            "0" | "error" | "ERROR" => LogLevel::Error,
            "1" | "warn" | "WARN" => LogLevel::Warn,
            "2" | "info" | "INFO" => LogLevel::Info,
            "3" | "debug" | "DEBUG" => LogLevel::Debug,
            _ => LogLevel::Info,
        },
        None => LogLevel::Info,
    };

    LOG_LEVEL_CACHE.store(level as u8, Ordering::Relaxed);
    level
}

/// Check if debug logging is enabled
fn is_debug_enabled() -> bool {
    get_log_level() >= LogLevel::Debug
}

/// Check if info logging is enabled
fn is_info_enabled() -> bool {
    get_log_level() >= LogLevel::Info
}

/// Check if warn logging is enabled
fn is_warn_enabled() -> bool {
    get_log_level() >= LogLevel::Warn
}

fn log_moves_only() -> bool {
    static LOG_MOVES_ONLY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOG_MOVES_ONLY.get_or_init(|| {
        std::env::var("WAYLAND_BRIDGE_LOG_MOVES_ONLY")
            .map(|value| value != "0")
            .unwrap_or(false)
    })
}

fn log_suppress_moves() -> bool {
    static LOG_SUPPRESS_MOVES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOG_SUPPRESS_MOVES.get_or_init(|| {
        std::env::var("WAYLAND_BRIDGE_LOG_SUPPRESS_MOVES")
            .map(|value| value != "0")
            .unwrap_or(false)
    })
}

fn log_suppress_forwarding() -> bool {
    static LOG_SUPPRESS_FORWARDING: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOG_SUPPRESS_FORWARDING.get_or_init(|| {
        std::env::var("WAYLAND_BRIDGE_LOG_SUPPRESS_FORWARDING")
            .map(|value| value != "0")
            .unwrap_or(false)
    })
}

fn write_bridge_stderr(arguments: core::fmt::Arguments<'_>) {
    static STDERR_LOCK: StdMutex<()> = StdMutex::new(());
    let _guard = STDERR_LOCK.lock();
    let message = std::format!("{}\n", arguments);
    let _ = std::io::stderr().write_all(message.as_bytes());
}

macro_rules! bridge_log {
    ($($arg:tt)*) => {
        if is_debug_enabled() {
            let msg = ::std::format!($($arg)*);
            let is_move_msg =
                msg.contains("xdg_toplevel.move") || msg.contains("REQUEST_MOVE_WINDOW");
            let is_forwarding_msg = msg.contains("Forwarding input event");
            if log_moves_only() {
                if is_move_msg {
                    ::std::println!("{}", msg);
                }
            } else if log_suppress_moves() {
                if !is_move_msg && !(log_suppress_forwarding() && is_forwarding_msg) {
                    ::std::println!("{}", msg);
                }
            } else if log_suppress_forwarding() {
                if !is_forwarding_msg {
                    ::std::println!("{}", msg);
                }
            } else {
                ::std::println!("{}", msg);
            }
        }
    };
}

macro_rules! bridge_info {
    ($($arg:tt)*) => {
        if is_info_enabled() {
            ::std::println!($($arg)*);
        }
    };
}

macro_rules! bridge_warn {
    ($($arg:tt)*) => {
        if is_warn_enabled() {
            write_bridge_stderr(format_args!($($arg)*));
        }
    };
}

macro_rules! bridge_error {
    ($($arg:tt)*) => {
        write_bridge_stderr(format_args!($($arg)*));
    };
}

fn wait_for_socket_event(
    socket: &Socket,
    requested_events: u16,
    failure: &'static str,
) -> Result<(), &'static str> {
    let mut handle = PollHandle::new(
        socket.as_raw() as u32,
        requested_events | SOCKET_FAILURE_EVENTS,
    );
    loop {
        let ready = poll(core::slice::from_mut(&mut handle), -1).map_err(|_| failure)?;
        if ready == 0 {
            continue;
        }
        if handle.revents & requested_events != 0 {
            return Ok(());
        }
        if handle.revents & SOCKET_FAILURE_EVENTS != 0 {
            return Err(failure);
        }
    }
}

fn wait_for_socket_event_until(
    socket: &Socket,
    requested_events: u16,
    deadline_ns: u64,
    timeout: &'static str,
    failure: &'static str,
) -> Result<(), &'static str> {
    let mut handle = PollHandle::new(
        socket.as_raw() as u32,
        requested_events | SOCKET_FAILURE_EVENTS,
    );
    loop {
        let now_ns = monotonic_time_ns();
        if now_ns >= deadline_ns {
            return Err(timeout);
        }
        let remaining_ns = deadline_ns.saturating_sub(now_ns).min(i64::MAX as u64) as i64;
        let ready = poll(core::slice::from_mut(&mut handle), remaining_ns).map_err(|_| failure)?;
        if ready == 0 {
            return Err(timeout);
        }
        if handle.revents & requested_events != 0 {
            return Ok(());
        }
        if handle.revents & SOCKET_FAILURE_EVENTS != 0 {
            return Err(failure);
        }
    }
}

fn write_all_nonblocking(
    socket: &mut Socket,
    bytes: &[u8],
    failure: &'static str,
) -> Result<(), &'static str> {
    let mut written = 0;
    while written < bytes.len() {
        match socket.write(&bytes[written..]) {
            Ok(0) => return Err(failure),
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                wait_for_socket_event(socket, POLLOUT, failure)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(failure),
        }
    }
    Ok(())
}

fn send_handle_and_data_nonblocking(
    socket: &Socket,
    handle: &Handle,
    bytes: &[u8],
    failure: &'static str,
) -> Result<(), &'static str> {
    loop {
        match socket.send_handle_and_data(handle, bytes) {
            Ok(()) => return Ok(()),
            Err(std::socket::SocketError::WouldBlock) => {
                wait_for_socket_event(socket, POLLOUT, failure)?;
            }
            Err(_) => return Err(failure),
        }
    }
}

fn create_server_socket(socket_path: &str) -> Result<Socket, &'static str> {
    bridge_log!("[Bridge] Creating server socket at {}", socket_path);

    let server_socket = Socket::new().map_err(|_| "Failed to create socket")?;
    server_socket
        .bind(socket_path)
        .map_err(|_| "Failed to bind socket")?;
    server_socket
        .listen(5)
        .map_err(|_| "Failed to listen on socket")?;

    bridge_log!("[Bridge] Server socket ready");
    Ok(server_socket)
}

/// Mapping of Wayland surface ID to SWS window ID
#[derive(Debug, Clone, Copy)]
struct SurfaceWindowMapping {
    wl_surface_id: u32,
    sws_window_id: u32,
}

/// Latest committed surface state waiting for the next SWS presentation slot.
#[derive(Debug)]
struct PendingSurfaceCommit {
    surface_id: u32,
    window_id: u32,
    wayland_buffer_id: Option<u32>,
    sws_buffer_id: Option<u32>,
    damage_rects: Vec<(u32, u32, u32, u32)>,
}

/// Wayland Bridge Server
struct WaylandBridge {
    /// Stable identifier used to correlate this Wayland connection in logs.
    client_id: u32,
    /// Number of Wayland surfaces created by this client.
    surface_count: u64,
    /// Number of completed wl_display.sync round trips for this client.
    display_sync_count: u64,
    /// Number of Wayland SHM pools created by this client.
    shm_pool_count: u64,
    /// Number of Wayland SHM buffers created by this client.
    shm_buffer_count: u64,
    /// Number of surface commits processed for this client.
    surface_commit_count: u64,
    /// Number of buffers released without involving an SWS window.
    local_buffer_release_count: u64,
    /// Number of reusable buffer commits forwarded to SWS.
    sws_buffer_commit_count: u64,
    /// Number of reusable buffer releases returned by SWS.
    sws_buffer_release_count: u64,
    /// Number of compositor presentation callbacks returned by SWS.
    sws_frame_done_count: u64,
    /// Registry of global interfaces
    registry: Registry,
    /// Surface manager
    surface_manager: SurfaceManager,
    /// XDG Shell manager
    xdg_shell_manager: XdgShellManager,
    /// Shared memory manager for client SHM pools
    shm_manager: ShmManager,
    /// Region manager
    region_manager: region::RegionManager,
    /// Input manager
    input_manager: InputManager,
    /// Connection to SWS server
    sws_connection: Option<Socket>,
    /// Extension ID assigned by SWS
    extension_id: Option<u32>,
    /// Next serial for configure events
    next_serial: u32,
    /// Map of object ID -> interface name
    objects: BTreeMap<u32, String>,
    /// Map of object ID -> interface version
    object_versions: BTreeMap<u32, u32>,
    /// Map of Wayland surface ID -> SWS window ID
    surface_to_window: BTreeMap<u32, u32>,
    /// Cached keymap SHM for wl_keyboard.keymap
    keymap_shm: Option<SharedMemory>,
    keymap_size: u32,
    /// Input event queue from SWS (shared between threads)
    input_event_queue: Arc<StdMutex<Vec<WaylandMessage>>>,
    /// Objects map clone for input thread
    objects_for_input_thread: Arc<StdMutex<BTreeMap<u32, String>>>,
    /// Pointer position shared with input thread
    pointer_position_for_thread: Arc<StdMutex<(i32, i32)>>,
    /// Currently focused surface (for keyboard enter/leave events)
    focused_surface: Option<u32>,
    /// Last focused keyboard (for sending leave events)
    focused_keyboard: Option<u32>,
    /// Last focused pointer (for sending leave events)
    focused_pointer: Option<u32>,
    /// Incoming buffer for SWS frames
    sws_rx_buffer: Vec<u8>,
    /// Scratch buffer for one SWS handle-and-data record
    sws_handle_record: Vec<u8>,
    /// Pending SWS responses keyed by their non-zero request ID.
    sws_pending: Vec<(u8, protocol_sws::ServerMessage, Option<Handle>)>,
    /// Next non-zero request ID for synchronous SWS requests.
    next_sws_request_id: u8,
    /// Wayland frame callbacks committed for each surface.
    pending_frame_callbacks: BTreeMap<u32, Vec<u32>>,
    /// SWS frame callback token -> Wayland surface.
    frame_request_tokens: BTreeMap<u64, u32>,
    /// Wayland callbacks assigned to each in-flight SWS frame request.
    frame_callbacks_by_token: BTreeMap<u64, Vec<u32>>,
    /// One outstanding SWS frame request per Wayland surface.
    surface_frame_request_outstanding: BTreeMap<u32, u64>,
    /// Latest surface state received while an earlier frame is being presented.
    pending_surface_commits: BTreeMap<u32, PendingSurfaceCommit>,
    /// Buffer resource most recently submitted to SWS for each surface.
    submitted_surface_buffers: BTreeMap<u32, Option<u32>>,
    /// Next non-zero SWS frame callback token.
    next_frame_request_token: u64,
    /// Next non-zero commit serial for reusable extension buffers.
    next_extension_commit_serial: u64,
    /// Next connection-scoped SWS resource ID, independent of Wayland ID reuse.
    next_extension_resource_id: u32,
    /// Pointer position (surface-local, in pixels)
    pointer_x: i32,
    pointer_y: i32,
    /// Pending pointer events waiting for the SWS EV_SYN packet boundary
    pending_pointer_messages: Vec<WaylandMessage>,
    pending_pointer_motion: bool,
    pending_pointer_time: u32,
    pending_pointer_id: Option<u32>,
    /// Current cursor surface (if set via wl_pointer.set_cursor)
    cursor_surface_id: Option<u32>,
    /// Track pointer left button state for xdg_toplevel.move timing
    left_button_down: bool,
    /// Last left-button press serial (from wl_pointer.button)
    last_left_button_serial: Option<u32>,
    /// Last left-button press time
    last_left_button_time: Option<u32>,
    /// Output scale factor (integer, from SWS output_scale_milli).
    /// Advertised via wl_output.scale so Wayland clients render at full
    /// physical resolution under HiDPI.
    output_scale: i32,
}

impl WaylandBridge {
    /// Create a new Wayland bridge client state
    fn new_client(client_id: u32) -> Result<Self, &'static str> {
        let mut objects = BTreeMap::new();
        // Object ID 1 is always wl_display
        objects.insert(1, String::from("wl_display"));

        let input_event_queue = Arc::new(StdMutex::new(Vec::new()));
        let objects_for_input_thread = Arc::new(StdMutex::new(BTreeMap::new()));
        let pointer_position_for_thread = Arc::new(StdMutex::new((0, 0)));
        let mut sws_handle_record = Vec::new();
        sws_handle_record.resize(
            protocol_sws::MessageHeader::SIZE + protocol_sws::MAX_PAYLOAD_SIZE,
            0,
        );

        Ok(Self {
            client_id,
            surface_count: 0,
            display_sync_count: 0,
            shm_pool_count: 0,
            shm_buffer_count: 0,
            surface_commit_count: 0,
            local_buffer_release_count: 0,
            sws_buffer_commit_count: 0,
            sws_buffer_release_count: 0,
            sws_frame_done_count: 0,
            registry: Registry::new(),
            surface_manager: SurfaceManager::new(),
            xdg_shell_manager: XdgShellManager::new(),
            shm_manager: ShmManager::new(),
            region_manager: region::RegionManager::new(),
            input_manager: InputManager::new(),
            sws_connection: None,
            extension_id: None,
            next_serial: 1,
            objects,
            object_versions: BTreeMap::new(),
            surface_to_window: BTreeMap::new(),
            keymap_shm: None,
            keymap_size: 0,
            input_event_queue,
            objects_for_input_thread,
            pointer_position_for_thread,
            focused_surface: None,
            focused_keyboard: None,
            focused_pointer: None,
            sws_rx_buffer: Vec::new(),
            sws_handle_record,
            sws_pending: Vec::new(),
            next_sws_request_id: 1,
            pending_frame_callbacks: BTreeMap::new(),
            frame_request_tokens: BTreeMap::new(),
            frame_callbacks_by_token: BTreeMap::new(),
            surface_frame_request_outstanding: BTreeMap::new(),
            pending_surface_commits: BTreeMap::new(),
            submitted_surface_buffers: BTreeMap::new(),
            next_frame_request_token: 1,
            next_extension_commit_serial: 1,
            next_extension_resource_id: 1,
            pointer_x: 0,
            pointer_y: 0,
            pending_pointer_messages: Vec::new(),
            pending_pointer_motion: false,
            pending_pointer_time: 0,
            pending_pointer_id: None,
            cursor_surface_id: None,
            left_button_down: false,
            last_left_button_serial: None,
            last_left_button_time: None,
            output_scale: 1,
        })
    }

    fn wait_for_activity(&self, client: &Socket) -> Result<(), &'static str> {
        let sws = self.sws_connection.as_ref().ok_or("Not connected to SWS")?;
        // Keep the client first. Until Scarlet poll supports registering one
        // waiter with the complete set, its multi-handle fallback anchors the
        // wait to the first selectable and periodically rescans the others.
        let mut handles = [
            PollHandle::new(client.as_raw() as u32, POLLIN | SOCKET_FAILURE_EVENTS),
            PollHandle::new(sws.as_raw() as u32, POLLIN | SOCKET_FAILURE_EVENTS),
        ];
        loop {
            let ready = poll(&mut handles, -1).map_err(|_| "Failed to wait for bridge sockets")?;
            if ready == 0 {
                continue;
            }
            if handles[0].revents & POLLNVAL != 0 {
                return Err("Wayland client socket became invalid");
            }
            if handles[1].revents & SOCKET_FAILURE_EVENTS != 0 && handles[1].revents & POLLIN == 0 {
                return Err("SWS connection closed while waiting for events");
            }
            return Ok(());
        }
    }

    fn reset_initial_sws_connection(&mut self) {
        self.sws_connection = None;
        self.extension_id = None;
        self.output_scale = 1;
        self.sws_rx_buffer.clear();
        self.sws_pending.clear();
        self.next_sws_request_id = 1;
    }

    /// Connect to SWS server and register as extension.
    ///
    /// The initial handshake is safe to retry because no Wayland object or SWS
    /// window has been consumed yet. Later resource operations remain
    /// connection-fatal rather than attempting to replay partially committed
    /// protocol state.
    fn connect_to_sws(&mut self) -> Result<(), &'static str> {
        let mut last_error = "Failed to initialize SWS connection";
        for attempt in 1..=2 {
            self.reset_initial_sws_connection();
            bridge_info!(
                "[wayland-bridge] client={} SWS handshake attempt={}",
                self.client_id,
                attempt
            );
            match self.connect_to_sws_once() {
                Ok(()) => return Ok(()),
                Err(error) => {
                    last_error = error;
                    self.reset_initial_sws_connection();
                    if attempt < 2 {
                        bridge_warn!(
                            "[wayland-bridge] client={} SWS handshake attempt={} failed: {}; retrying",
                            self.client_id,
                            attempt,
                            error
                        );
                    }
                }
            }
        }
        Err(last_error)
    }

    fn connect_to_sws_once(&mut self) -> Result<(), &'static str> {
        bridge_log!("[Bridge] Connecting to SWS at /tmp/sws.sock");

        let sws_socket = Socket::new().map_err(|_| "Failed to create SWS socket")?;

        sws_socket
            .connect("/tmp/sws.sock")
            .map_err(|_| "Failed to connect to SWS")?;

        bridge_info!(
            "[wayland-bridge] client={} SWS socket connected",
            self.client_id
        );

        bridge_log!("[Bridge] Connected to SWS, registering as extension");

        sws_socket
            .set_nonblocking(true)
            .map_err(|_| "Failed to set SWS socket non-blocking")?;

        self.sws_connection = Some(sws_socket);
        let extension_name = b"wayland_bridge";
        let payload = protocol_sws::payload_register_extension(extension_name);
        let request_id =
            self.send_sws_request(protocol_sws::client_msg::REGISTER_EXTENSION, &payload)?;
        bridge_info!(
            "[wayland-bridge] client={} waiting for SWS extension registration request={}",
            self.client_id,
            request_id
        );
        if let protocol_sws::ServerMessage::ExtensionRegistered { extension_id } = self
            .wait_for_sws_message(request_id, |msg| {
                matches!(msg, protocol_sws::ServerMessage::ExtensionRegistered { .. })
            })?
        {
            self.extension_id = Some(extension_id);
            bridge_log!("[Bridge] Registered as extension with ID: {}", extension_id);
            bridge_info!(
                "[wayland-bridge] client={} SWS extension registered id={}",
                self.client_id,
                extension_id
            );
        }

        self.query_output_scale()?;

        bridge_info!(
            "[wayland-bridge] client={} SWS extension={} output_scale={}",
            self.client_id,
            self.extension_id.unwrap_or(0),
            self.output_scale
        );

        Ok(())
    }

    /// Query the current output scale from SWS and store it as an integer.
    ///
    /// Converts SWS milli-units (1000 = 1.0x, 2000 = 2.0x) to the nearest
    /// integer, matching the `wl_output.scale` requirement of being a
    /// positive integer.
    fn query_output_scale(&mut self) -> Result<(), &'static str> {
        let request_id = self.send_sws_request(protocol_sws::client_msg::GET_OUTPUT_SCALE, &[])?;
        bridge_info!(
            "[wayland-bridge] client={} waiting for SWS output scale request={}",
            self.client_id,
            request_id
        );

        if let protocol_sws::ServerMessage::OutputScale { scale_milli } = self
            .wait_for_sws_message(request_id, |msg| {
                matches!(msg, protocol_sws::ServerMessage::OutputScale { .. })
            })?
        {
            let scale = ((scale_milli + 500) / 1000).max(1) as i32;
            bridge_log!(
                "[Bridge] Output scale: milli={} -> integer {}",
                scale_milli,
                scale
            );
            self.output_scale = scale;
        }

        Ok(())
    }

    /// Convert SWS physical pixel coordinate to Wayland surface-local
    /// (logical) coordinate using the focused surface's buffer_scale.
    fn physical_to_logical_x(&self, x: i32) -> i32 {
        let scale = self.focused_surface_scale();
        if scale > 0 { x / scale } else { x }
    }

    fn physical_to_logical_y(&self, y: i32) -> i32 {
        let scale = self.focused_surface_scale();
        if scale > 0 { y / scale } else { y }
    }

    fn focused_surface_scale(&self) -> i32 {
        self.focused_surface
            .and_then(|sid| self.surface_manager.get_surface(sid))
            .map(|s| s.buffer_scale.max(1))
            .unwrap_or(1)
    }

    fn allocate_serial(&mut self) -> u32 {
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1);
        serial
    }

    /// Add an object to the objects map and sync to input thread
    fn add_object(&mut self, id: u32, interface: String) {
        self.objects.insert(id, interface.clone());
        // Update input thread's objects map
        let mut objects = self.objects_for_input_thread.lock();
        objects.insert(id, interface);
    }

    fn remove_object(&mut self, id: u32) {
        self.objects.remove(&id);
        self.object_versions.remove(&id);
        self.objects_for_input_thread.lock().remove(&id);
    }

    fn append_callback_done(
        &mut self,
        messages: &mut Vec<WaylandMessage>,
        callback_id: u32,
        time: u32,
    ) {
        let mut done = WaylandMessage::new(callback_id, protocol::callback_event::DONE);
        done.add_arg(WaylandArg::Uint(time));
        messages.push(done);

        let mut delete_id = WaylandMessage::new(1, protocol::display_event::DELETE_ID);
        delete_id.add_arg(WaylandArg::Uint(callback_id));
        messages.push(delete_id);
        self.remove_object(callback_id);
    }

    fn append_buffer_release(&mut self, messages: &mut Vec<WaylandMessage>, buffer_id: u32) {
        if self
            .objects
            .get(&buffer_id)
            .is_some_and(|interface| interface == "wl_buffer")
        {
            messages.push(WaylandMessage::new(buffer_id, shm::buffer_event::RELEASE));
            self.local_buffer_release_count = self.local_buffer_release_count.saturating_add(1);
        }
    }

    fn discard_callbacks(&mut self, callbacks: Vec<u32>) {
        let mut messages = Vec::new();
        for callback_id in callbacks {
            let mut delete_id = WaylandMessage::new(1, protocol::display_event::DELETE_ID);
            delete_id.add_arg(WaylandArg::Uint(callback_id));
            messages.push(delete_id);
            self.remove_object(callback_id);
        }
        self.queue_input_messages(messages);
    }

    fn surface_id_for_window(&self, window_id: u32) -> Option<u32> {
        self.surface_to_window
            .iter()
            .find_map(|(surface_id, &win_id)| {
                if win_id == window_id {
                    Some(*surface_id)
                } else {
                    None
                }
            })
    }

    fn window_id_for_toplevel(&mut self, xdg_toplevel_id: u32) -> Option<u32> {
        self.xdg_shell_manager
            .get_toplevel_mut(xdg_toplevel_id)
            .and_then(|(_, wl_surface_id)| self.surface_to_window.get(&wl_surface_id).copied())
    }

    fn xdg_toplevel_state_bytes(maximized: bool, fullscreen: bool) -> Vec<u8> {
        let mut states = Vec::new();
        if fullscreen {
            states.extend_from_slice(&xdg_shell::xdg_toplevel_state::FULLSCREEN.to_ne_bytes());
        } else if maximized {
            states.extend_from_slice(&xdg_shell::xdg_toplevel_state::MAXIMIZED.to_ne_bytes());
        }
        states
    }

    fn update_xdg_window_state(&mut self, window_id: u32, state_flags: u32) {
        let Some(wl_surface_id) = self.surface_id_for_window(window_id) else {
            return;
        };
        let Some(xdg_surface) = self
            .xdg_shell_manager
            .get_xdg_surface_by_wl_surface_mut(wl_surface_id)
        else {
            return;
        };
        let Some(toplevel) = xdg_surface.toplevel.as_mut() else {
            return;
        };

        toplevel.fullscreen = (state_flags & protocol_sws::window_state::FULLSCREEN) != 0;
        toplevel.maximized = (state_flags & protocol_sws::window_state::MAXIMIZED) != 0;
    }

    fn queue_xdg_window_configure(&mut self, window_id: u32, width: u32, height: u32) {
        let Some(wl_surface_id) = self.surface_id_for_window(window_id) else {
            return;
        };
        let Some((xdg_surface_id, toplevel_id, maximized, fullscreen)) = self
            .xdg_shell_manager
            .get_xdg_surface_ids_by_wl_surface(wl_surface_id)
            .and_then(|(xdg_surface_id, toplevel_id)| {
                let toplevel_id = toplevel_id?;
                let toplevel = self
                    .xdg_shell_manager
                    .get_xdg_surface(xdg_surface_id)?
                    .toplevel
                    .as_ref()?;
                Some((
                    xdg_surface_id,
                    toplevel_id,
                    toplevel.maximized,
                    toplevel.fullscreen,
                ))
            })
        else {
            return;
        };

        let scale = self
            .surface_manager
            .get_surface(wl_surface_id)
            .map(|surface| surface.buffer_scale.max(1) as u32)
            .unwrap_or(1);
        let logical_width = width.div_ceil(scale).max(1);
        let logical_height = height.div_ceil(scale).max(1);
        let serial = self.allocate_serial();
        if let Some(xdg_surface) = self.xdg_shell_manager.get_xdg_surface_mut(xdg_surface_id) {
            xdg_surface.last_configure_serial = Some(serial);
        }

        let mut toplevel_configure =
            WaylandMessage::new(toplevel_id, xdg_shell::xdg_toplevel_event::CONFIGURE);
        toplevel_configure.add_arg(WaylandArg::Int(logical_width as i32));
        toplevel_configure.add_arg(WaylandArg::Int(logical_height as i32));
        toplevel_configure.add_arg(WaylandArg::Array(Self::xdg_toplevel_state_bytes(
            maximized, fullscreen,
        )));

        let mut surface_configure =
            WaylandMessage::new(xdg_surface_id, xdg_shell::xdg_surface_event::CONFIGURE);
        surface_configure.add_arg(WaylandArg::Uint(serial));
        let mut messages = Vec::new();
        messages.push(toplevel_configure);
        messages.push(surface_configure);
        self.queue_input_messages(messages);
    }

    fn apply_xdg_toplevel_state_to_sws(&mut self, wl_surface_id: u32, window_id: u32) {
        let Some((maximized, fullscreen)) = self
            .xdg_shell_manager
            .get_xdg_surface_ids_by_wl_surface(wl_surface_id)
            .and_then(|(xdg_surface_id, _)| {
                self.xdg_shell_manager
                    .get_xdg_surface(xdg_surface_id)?
                    .toplevel
                    .as_ref()
                    .map(|toplevel| (toplevel.maximized, toplevel.fullscreen))
            })
        else {
            return;
        };

        if maximized {
            let _ = self.send_maximize_window(window_id);
        }
        if fullscreen {
            let _ = self.send_set_fullscreen(window_id);
        }
    }

    fn queue_input_messages(&self, messages: Vec<WaylandMessage>) {
        if messages.is_empty() {
            return;
        }
        let mut queue = self.input_event_queue.lock();
        queue.extend(messages);
    }

    fn queue_pending_pointer_motion(&mut self) {
        if !self.pending_pointer_motion {
            return;
        }

        if let Some(pointer_id) = self.focused_pointer {
            let mut msg = WaylandMessage::new(pointer_id, input::pointer_event::MOTION);
            msg.add_arg(WaylandArg::Uint(self.pending_pointer_time));
            msg.add_arg(WaylandArg::Fixed(
                self.physical_to_logical_x(self.pointer_x) << 8,
            ));
            msg.add_arg(WaylandArg::Fixed(
                self.physical_to_logical_y(self.pointer_y) << 8,
            ));
            self.pending_pointer_messages.push(msg);
            self.pending_pointer_id = Some(pointer_id);
        }

        self.pending_pointer_motion = false;
    }

    fn flush_pending_pointer_messages(&mut self) {
        self.queue_pending_pointer_motion();

        if self.pending_pointer_messages.is_empty() {
            self.pending_pointer_id = None;
            return;
        }

        if let Some(pointer_id) = self.pending_pointer_id
            && self.pointer_frame_supported(pointer_id)
        {
            self.pending_pointer_messages
                .push(WaylandMessage::new(pointer_id, input::pointer_event::FRAME));
        }

        let messages = core::mem::take(&mut self.pending_pointer_messages);
        self.pending_pointer_id = None;
        self.queue_input_messages(messages);
    }

    fn pointer_frame_supported(&self, pointer_id: u32) -> bool {
        let seat_id = match self.input_manager.pointer_seat_id(pointer_id) {
            Some(id) => id,
            None => return false,
        };
        self.object_versions.get(&seat_id).copied().unwrap_or(1) >= 5
    }

    fn queue_focus_events(&mut self, surface_id: u32) {
        if self.focused_surface == Some(surface_id) {
            return;
        }

        let old_surface = self.focused_surface;
        self.focused_surface = Some(surface_id);

        let mut messages = Vec::new();

        if let Some(pointer_id) = self.focused_pointer {
            if let Some(old_id) = old_surface {
                let serial = self.allocate_serial();
                let mut leave = WaylandMessage::new(pointer_id, input::pointer_event::LEAVE);
                leave.add_arg(WaylandArg::Uint(serial));
                leave.add_arg(WaylandArg::Object(old_id));
                messages.push(leave);
            }
            let serial = self.allocate_serial();
            let mut enter = WaylandMessage::new(pointer_id, input::pointer_event::ENTER);
            enter.add_arg(WaylandArg::Uint(serial));
            enter.add_arg(WaylandArg::Object(surface_id));
            enter.add_arg(WaylandArg::Fixed(
                self.physical_to_logical_x(self.pointer_x) << 8,
            ));
            enter.add_arg(WaylandArg::Fixed(
                self.physical_to_logical_y(self.pointer_y) << 8,
            ));
            messages.push(enter);
        }

        if let Some(keyboard_id) = self.focused_keyboard {
            if let Some(old_id) = old_surface {
                let serial = self.allocate_serial();
                let mut leave = WaylandMessage::new(keyboard_id, input::keyboard_event::LEAVE);
                leave.add_arg(WaylandArg::Uint(serial));
                leave.add_arg(WaylandArg::Object(old_id));
                messages.push(leave);
            }
            let serial = self.allocate_serial();
            let mut enter = WaylandMessage::new(keyboard_id, input::keyboard_event::ENTER);
            enter.add_arg(WaylandArg::Uint(serial));
            enter.add_arg(WaylandArg::Object(surface_id));
            enter.add_arg(WaylandArg::Array(Vec::new()));
            messages.push(enter);
        }

        self.queue_input_messages(messages);
    }

    fn handle_sws_input_event(
        &mut self,
        window_id: u32,
        time: u64,
        type_: u16,
        code: u16,
        value: i32,
    ) {
        const EV_KEY: u16 = 0x01;
        const EV_REL: u16 = 0x02;
        const EV_ABS: u16 = 0x03;
        const EV_SYN: u16 = 0x00;
        const REL_X: u16 = 0x00;
        const REL_Y: u16 = 0x01;
        const REL_HWHEEL: u16 = 0x06;
        const REL_WHEEL: u16 = 0x08;
        const ABS_X: u16 = 0x00;
        const ABS_Y: u16 = 0x01;
        const BTN_MOUSE_MIN: u16 = 0x110;
        const BTN_MOUSE_MAX: u16 = 0x118;
        const BTN_LEFT: u16 = 0x110;
        /// wl_pointer axis types
        const WL_POINTER_AXIS_VERTICAL_SCROLL: u32 = 0;
        const WL_POINTER_AXIS_HORIZONTAL_SCROLL: u32 = 1;

        if let Some(surface_id) = self.surface_id_for_window(window_id) {
            self.queue_focus_events(surface_id);
        }

        match type_ {
            EV_REL => {
                if code == REL_X {
                    self.pointer_x = self.pointer_x.saturating_add(value);
                    self.pending_pointer_motion = true;
                    self.pending_pointer_time = time as u32;
                    self.pending_pointer_id = self.focused_pointer;
                } else if code == REL_Y {
                    self.pointer_y = self.pointer_y.saturating_add(value);
                    self.pending_pointer_motion = true;
                    self.pending_pointer_time = time as u32;
                    self.pending_pointer_id = self.focused_pointer;
                } else if code == REL_WHEEL {
                    if let Some(pointer_id) = self.focused_pointer {
                        self.queue_pending_pointer_motion();
                        let mut msg = WaylandMessage::new(pointer_id, input::pointer_event::AXIS);
                        msg.add_arg(WaylandArg::Uint(time as u32));
                        msg.add_arg(WaylandArg::Uint(WL_POINTER_AXIS_VERTICAL_SCROLL));
                        let axis_value = (value as i64 * 256) as i32;
                        msg.add_arg(WaylandArg::Fixed(axis_value));
                        self.pending_pointer_messages.push(msg);
                        self.pending_pointer_id = Some(pointer_id);
                    }
                } else if code == REL_HWHEEL {
                    if let Some(pointer_id) = self.focused_pointer {
                        self.queue_pending_pointer_motion();
                        let mut msg = WaylandMessage::new(pointer_id, input::pointer_event::AXIS);
                        msg.add_arg(WaylandArg::Uint(time as u32));
                        msg.add_arg(WaylandArg::Uint(WL_POINTER_AXIS_HORIZONTAL_SCROLL));
                        let axis_value = (value as i64 * 256) as i32;
                        msg.add_arg(WaylandArg::Fixed(axis_value));
                        self.pending_pointer_messages.push(msg);
                        self.pending_pointer_id = Some(pointer_id);
                    }
                }
            }
            EV_ABS => {
                if code == ABS_X {
                    self.pointer_x = value;
                } else if code == ABS_Y {
                    self.pointer_y = value;
                }
                self.pending_pointer_motion = true;
                self.pending_pointer_time = time as u32;
                self.pending_pointer_id = self.focused_pointer;
            }
            EV_KEY => {
                if (BTN_MOUSE_MIN..=BTN_MOUSE_MAX).contains(&code) {
                    if code == BTN_LEFT {
                        self.left_button_down = value != 0;
                    }
                    if let Some(pointer_id) = self.focused_pointer {
                        self.queue_pending_pointer_motion();
                        let serial = self.allocate_serial();
                        if code == BTN_LEFT && value != 0 {
                            self.last_left_button_serial = Some(serial);
                            self.last_left_button_time = Some(time as u32);
                        }
                        let mut msg = WaylandMessage::new(pointer_id, input::pointer_event::BUTTON);
                        msg.add_arg(WaylandArg::Uint(serial));
                        msg.add_arg(WaylandArg::Uint(time as u32));
                        msg.add_arg(WaylandArg::Uint(code as u32));
                        msg.add_arg(WaylandArg::Uint(if value != 0 {
                            input::pointer_button_state::PRESSED
                        } else {
                            input::pointer_button_state::RELEASED
                        }));
                        self.pending_pointer_messages.push(msg);
                        self.pending_pointer_id = Some(pointer_id);
                    }
                } else if let Some(keyboard_id) = self.focused_keyboard {
                    let mut msg = WaylandMessage::new(keyboard_id, input::keyboard_event::KEY);
                    msg.add_arg(WaylandArg::Uint(self.allocate_serial()));
                    msg.add_arg(WaylandArg::Uint(time as u32));
                    msg.add_arg(WaylandArg::Uint(code as u32));
                    msg.add_arg(WaylandArg::Uint(value as u32));
                    let mut messages = Vec::new();
                    messages.push(msg);
                    self.queue_input_messages(messages);
                }
            }
            EV_SYN => self.flush_pending_pointer_messages(),
            _ => {}
        }
    }

    fn allocate_sws_request_id(&mut self) -> Result<u8, &'static str> {
        for _ in 0..u8::MAX {
            let request_id = self.next_sws_request_id.max(1);
            self.next_sws_request_id = request_id.wrapping_add(1).max(1);
            if !self
                .sws_pending
                .iter()
                .any(|(pending_id, _, _)| *pending_id == request_id)
            {
                return Ok(request_id);
            }
        }
        Err("SWS request IDs exhausted")
    }

    fn send_sws_request(&mut self, msg_type: u32, payload: &[u8]) -> Result<u8, &'static str> {
        if payload.len() > protocol_sws::MAX_PAYLOAD_SIZE {
            return Err("SWS request payload is too large");
        }
        let request_id = self.allocate_sws_request_id()?;
        let header =
            protocol_sws::MessageHeader::request(msg_type, request_id, payload.len() as u32);
        let mut frame = Vec::with_capacity(protocol_sws::MessageHeader::SIZE + payload.len());
        frame.extend_from_slice(&header.to_le_bytes());
        frame.extend_from_slice(payload);
        let connection = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;
        write_all_nonblocking(connection, &frame, "Failed to send SWS request")?;
        connection
            .flush()
            .map_err(|_| "Failed to flush SWS request")?;
        Ok(request_id)
    }

    fn send_sws_handle_request(
        &mut self,
        msg_type: u32,
        payload: &[u8],
        handle: &Handle,
    ) -> Result<u8, &'static str> {
        if payload.len() > protocol_sws::MAX_PAYLOAD_SIZE {
            return Err("SWS handle request payload is too large");
        }
        let request_id = self.allocate_sws_request_id()?;
        let header =
            protocol_sws::MessageHeader::request(msg_type, request_id, payload.len() as u32);
        let mut frame = Vec::with_capacity(protocol_sws::MessageHeader::SIZE + payload.len());
        frame.extend_from_slice(&header.to_le_bytes());
        frame.extend_from_slice(payload);
        let connection = self.sws_connection.as_ref().ok_or("Not connected to SWS")?;
        send_handle_and_data_nonblocking(
            connection,
            handle,
            &frame,
            "Failed to send SWS handle request",
        )?;
        Ok(request_id)
    }

    fn send_sws_async_message(
        &mut self,
        msg_type: u32,
        payload: &[u8],
    ) -> Result<(), &'static str> {
        if payload.len() > protocol_sws::MAX_PAYLOAD_SIZE {
            return Err("SWS asynchronous payload is too large");
        }
        let header = protocol_sws::MessageHeader::new(msg_type, payload.len() as u32);
        let mut frame = Vec::with_capacity(protocol_sws::MessageHeader::SIZE + payload.len());
        frame.extend_from_slice(&header.to_le_bytes());
        frame.extend_from_slice(payload);
        let connection = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;
        write_all_nonblocking(
            connection,
            &frame,
            "Failed to send asynchronous SWS message",
        )?;
        connection
            .flush()
            .map_err(|_| "Failed to flush asynchronous SWS message")
    }

    fn allocate_frame_request_token(&mut self) -> u64 {
        loop {
            let token = self.next_frame_request_token.max(1);
            self.next_frame_request_token = token.wrapping_add(1).max(1);
            if !self.frame_request_tokens.contains_key(&token) {
                return token;
            }
        }
    }

    fn allocate_extension_commit_serial(&mut self) -> u64 {
        let serial = self.next_extension_commit_serial.max(1);
        self.next_extension_commit_serial = serial.wrapping_add(1).max(1);
        serial
    }

    fn allocate_extension_resource_id(&mut self) -> u32 {
        let resource_id = self.next_extension_resource_id.max(1);
        self.next_extension_resource_id = resource_id.wrapping_add(1).max(1);
        resource_id
    }

    fn ensure_sws_frame_request(
        &mut self,
        surface_id: u32,
        pace_without_callbacks: bool,
    ) -> Result<(), &'static str> {
        if self
            .surface_frame_request_outstanding
            .contains_key(&surface_id)
        {
            return Ok(());
        }
        if !pace_without_callbacks
            && !self
                .pending_frame_callbacks
                .get(&surface_id)
                .is_some_and(|callbacks| !callbacks.is_empty())
        {
            return Ok(());
        }
        let Some(&window_id) = self.surface_to_window.get(&surface_id) else {
            return Ok(());
        };
        let token = self.allocate_frame_request_token();
        let payload = protocol_sws::payload_request_frame(window_id, token);
        let callbacks = self
            .pending_frame_callbacks
            .remove(&surface_id)
            .unwrap_or_default();
        if let Err(error) =
            self.send_sws_async_message(protocol_sws::client_msg::REQUEST_FRAME, &payload)
        {
            self.pending_frame_callbacks
                .entry(surface_id)
                .or_insert_with(Vec::new)
                .extend(callbacks);
            return Err(error);
        }
        self.frame_request_tokens.insert(token, surface_id);
        self.frame_callbacks_by_token.insert(token, callbacks);
        self.surface_frame_request_outstanding
            .insert(surface_id, token);
        Ok(())
    }

    fn complete_sws_frame_request(
        &mut self,
        window_id: u32,
        token: u64,
        presentation_time_ns: u64,
    ) -> Result<(), &'static str> {
        let Some(surface_id) = self.frame_request_tokens.remove(&token) else {
            bridge_log!(
                "[Bridge] Ignoring unknown SWS frame callback token {}",
                token
            );
            return Ok(());
        };
        let callbacks = self
            .frame_callbacks_by_token
            .remove(&token)
            .unwrap_or_default();
        if self.surface_frame_request_outstanding.get(&surface_id) != Some(&token) {
            bridge_log!(
                "[Bridge] Ignoring stale SWS frame callback token {} for surface {}",
                token,
                surface_id
            );
            self.discard_callbacks(callbacks);
            return Ok(());
        }
        self.surface_frame_request_outstanding.remove(&surface_id);
        if self.surface_to_window.get(&surface_id) != Some(&window_id) {
            bridge_log!(
                "[Bridge] Ignoring SWS frame callback token {} with mismatched window {}",
                token,
                window_id
            );
            self.discard_callbacks(callbacks);
            return Ok(());
        }

        self.sws_frame_done_count = self.sws_frame_done_count.saturating_add(1);
        if should_log_resource_count(self.sws_frame_done_count) {
            bridge_info!(
                "[wayland-bridge] client={} surface={} SWS frame_done={} token={}",
                self.client_id,
                surface_id,
                self.sws_frame_done_count,
                token
            );
        }

        let time_ms = (presentation_time_ns / 1_000_000) as u32;
        let mut messages = Vec::new();
        for callback_id in callbacks {
            self.append_callback_done(&mut messages, callback_id, time_ms);
        }
        self.queue_input_messages(messages);

        if !self.flush_pending_surface_commit(surface_id)? {
            // A pending buffer may have been forced to SWS before its
            // wl_buffer object was destroyed. Its callbacks still need the
            // next presentation token even though no queued commit remains.
            self.ensure_sws_frame_request(surface_id, false)?;
        }
        Ok(())
    }

    fn cancel_surface_frame_request(&mut self, surface_id: u32) {
        if let Some(token) = self.surface_frame_request_outstanding.remove(&surface_id) {
            self.frame_request_tokens.remove(&token);
            if let Some(callbacks) = self.frame_callbacks_by_token.remove(&token) {
                self.discard_callbacks(callbacks);
            }
        }
        if let Some(callbacks) = self.pending_frame_callbacks.remove(&surface_id) {
            self.discard_callbacks(callbacks);
        }
    }

    fn route_sws_message(
        &mut self,
        header: protocol_sws::MessageHeader,
        message: protocol_sws::ServerMessage,
        handle: Option<Handle>,
    ) -> Result<(), &'static str> {
        if header.is_response() {
            if header.request_id == 0 {
                return Err("SWS response used reserved request ID zero");
            }
            self.sws_pending.push((header.request_id, message, handle));
            return Ok(());
        }
        if header.request_id != 0 || handle.is_some() {
            return Err("Invalid unsolicited SWS frame routing");
        }
        match message {
            protocol_sws::ServerMessage::InputEvent {
                window_id,
                time,
                type_,
                code,
                value,
            } => {
                if handle.is_some() {
                    return Err("Unexpected handle attached to SWS input event");
                }
                self.handle_sws_input_event(window_id, time, type_, code, value);
            }
            protocol_sws::ServerMessage::ExtensionInputEvent {
                external_client_id,
                window_id,
                time,
                type_,
                code,
                value,
            } => {
                if handle.is_some() {
                    return Err("Unexpected handle attached to SWS extension input event");
                }
                if let Some(surface_id) = self.surface_id_for_window(window_id)
                    && external_client_id != surface_id
                {
                    bridge_log!(
                        "[Bridge] EXTENSION_INPUT_EVENT client mismatch: window={} external_client_id={} surface_id={}",
                        window_id,
                        external_client_id,
                        surface_id
                    );
                }
                self.handle_sws_input_event(window_id, time, type_, code, value);
            }
            protocol_sws::ServerMessage::OutputScaleChanged { scale_milli } => {
                self.output_scale = ((scale_milli + 500) / 1000).max(1) as i32;
            }
            protocol_sws::ServerMessage::WindowStateChanged {
                window_id,
                state_flags,
            } => {
                self.update_xdg_window_state(window_id, state_flags);
            }
            protocol_sws::ServerMessage::FrameDone {
                window_id,
                callback_id,
                presentation_time_ns,
            } => {
                self.complete_sws_frame_request(window_id, callback_id, presentation_time_ns)?;
            }
            protocol_sws::ServerMessage::ExtensionBufferReleased {
                buffer_id,
                commit_serial,
            } => {
                self.sws_buffer_release_count = self.sws_buffer_release_count.saturating_add(1);
                if should_log_resource_count(self.sws_buffer_release_count) {
                    bridge_info!(
                        "[wayland-bridge] client={} SWS buffer_release={} resource={} commit_serial={}",
                        self.client_id,
                        self.sws_buffer_release_count,
                        buffer_id,
                        commit_serial
                    );
                }
                if let Some(wayland_buffer_id) = self
                    .shm_manager
                    .get_buffer_by_sws_id(buffer_id)
                    .map(|buffer| buffer.buffer_id)
                    && self
                        .objects
                        .get(&wayland_buffer_id)
                        .is_some_and(|interface| interface == "wl_buffer")
                {
                    bridge_log!(
                        "[Bridge] SWS released resource {} (wl_buffer {}) retained by commit {}",
                        buffer_id,
                        wayland_buffer_id,
                        commit_serial
                    );
                    self.queue_input_messages(Vec::from([WaylandMessage::new(
                        wayland_buffer_id,
                        shm::buffer_event::RELEASE,
                    )]));
                }
            }
            protocol_sws::ServerMessage::WindowConfigure {
                window_id,
                width,
                height,
            } => {
                self.queue_xdg_window_configure(window_id, width, height);
            }
            protocol_sws::ServerMessage::Error { code } => {
                bridge_error!(
                    "[wayland-bridge] client={} SWS asynchronous request failed code={}",
                    self.client_id,
                    code
                );
                return Err("SWS rejected an asynchronous bridge request");
            }
            _ => {}
        }
        Ok(())
    }

    fn poll_sws_messages(&mut self) -> Result<(), &'static str> {
        if self.sws_connection.is_none() {
            return Ok(());
        }

        loop {
            let result = self
                .sws_connection
                .as_ref()
                .ok_or("Not connected to SWS")?
                .recv_handle_and_data(&mut self.sws_handle_record);
            match result {
                Ok((handle, bytes_read)) => {
                    if bytes_read < protocol_sws::MessageHeader::SIZE {
                        return Err("Truncated SWS handle record");
                    }
                    let mut header_bytes = [0u8; protocol_sws::MessageHeader::SIZE];
                    header_bytes.copy_from_slice(
                        &self.sws_handle_record[..protocol_sws::MessageHeader::SIZE],
                    );
                    let header = protocol_sws::MessageHeader::from_le_bytes(header_bytes);
                    let frame_len =
                        protocol_sws::MessageHeader::SIZE + header.payload_size as usize;
                    if frame_len != bytes_read
                        || header.payload_size as usize > protocol_sws::MAX_PAYLOAD_SIZE
                    {
                        return Err("Invalid SWS handle record length");
                    }
                    let message = protocol_sws::parse_server_message(
                        header.msg_type_u32(),
                        &self.sws_handle_record[protocol_sws::MessageHeader::SIZE..frame_len],
                    )
                    .map_err(|_| "Invalid SWS handle record")?;
                    self.route_sws_message(header, message, Some(handle))?;
                }
                Err(std::socket::SocketError::ReceiveBufferTooSmall { required_len }) => {
                    self.sws_handle_record.resize(required_len, 0);
                }
                Err(std::socket::SocketError::WouldBlock) => break,
                Err(_) => return Err("Failed to receive SWS handle record"),
            }
        }

        let mut buf = [0u8; 1024];
        loop {
            let read_result = self
                .sws_connection
                .as_mut()
                .ok_or("Not connected to SWS")?
                .read(&mut buf);
            match read_result {
                Ok(0) => return Err("SWS connection closed"),
                Ok(n) => {
                    self.sws_rx_buffer.extend_from_slice(&buf[..n]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err("Failed to read SWS message stream"),
            }
        }

        loop {
            if self.sws_rx_buffer.len() < protocol_sws::MessageHeader::SIZE {
                break;
            }
            let mut header_bytes = [0u8; protocol_sws::MessageHeader::SIZE];
            header_bytes.copy_from_slice(&self.sws_rx_buffer[..protocol_sws::MessageHeader::SIZE]);
            let header = protocol_sws::MessageHeader::from_le_bytes(header_bytes);
            if header.payload_size as usize > protocol_sws::MAX_PAYLOAD_SIZE {
                return Err("SWS frame payload is too large");
            }
            let frame_len = protocol_sws::MessageHeader::SIZE
                .checked_add(header.payload_size as usize)
                .ok_or("Invalid SWS frame length")?;
            if self.sws_rx_buffer.len() < frame_len {
                break;
            }
            let payload = &self.sws_rx_buffer[protocol_sws::MessageHeader::SIZE..frame_len];
            let message = protocol_sws::parse_server_message(header.msg_type_u32(), payload)
                .map_err(|_| "Invalid SWS frame")?;
            self.route_sws_message(header, message, None)?;
            self.sws_rx_buffer.drain(0..frame_len);
        }

        Ok(())
    }

    fn wait_for_sws_entry(
        &mut self,
        request_id: u8,
    ) -> Result<(protocol_sws::ServerMessage, Option<Handle>), &'static str> {
        if request_id == 0 {
            return Err("Cannot wait for reserved SWS request ID zero");
        }
        let deadline_ns = monotonic_time_ns().saturating_add(SWS_RESPONSE_TIMEOUT_NS);
        loop {
            self.poll_sws_messages()?;
            let mut idx = 0;
            while idx < self.sws_pending.len() {
                if self.sws_pending[idx].0 == request_id {
                    let (_, message, handle) = self.sws_pending.remove(idx);
                    return Ok((message, handle));
                }
                idx += 1;
            }
            let connection = self.sws_connection.as_ref().ok_or("Not connected to SWS")?;
            wait_for_socket_event_until(
                connection,
                POLLIN,
                deadline_ns,
                "Timed out waiting for SWS response",
                "SWS connection closed while waiting",
            )?;
        }
    }

    fn wait_for_sws_message<F>(
        &mut self,
        request_id: u8,
        mut matches: F,
    ) -> Result<protocol_sws::ServerMessage, &'static str>
    where
        F: FnMut(&protocol_sws::ServerMessage) -> bool,
    {
        let (message, handle) = self.wait_for_sws_entry(request_id)?;
        if handle.is_some() {
            return Err("Unexpected handle attached to SWS response");
        }
        if let protocol_sws::ServerMessage::Error { code } = &message {
            bridge_error!(
                "[wayland-bridge] client={} SWS rejected request={} code={}",
                self.client_id,
                request_id,
                code
            );
            return Err("SWS rejected request");
        }
        if !matches(&message) {
            return Err("Unexpected SWS response");
        }
        Ok(message)
    }

    fn wait_for_sws_message_with_handle<F>(
        &mut self,
        request_id: u8,
        mut matches: F,
    ) -> Result<(protocol_sws::ServerMessage, Handle), &'static str>
    where
        F: FnMut(&protocol_sws::ServerMessage) -> bool,
    {
        let (message, handle) = self.wait_for_sws_entry(request_id)?;
        if let protocol_sws::ServerMessage::Error { code } = &message {
            bridge_error!(
                "[wayland-bridge] client={} SWS rejected handle request={} code={}",
                self.client_id,
                request_id,
                code
            );
            return Err("SWS rejected handle request");
        }
        if !matches(&message) {
            return Err("Unexpected SWS response");
        }
        Ok((message, handle.ok_or("Missing handle on SWS response")?))
    }

    fn get_object_version(&self, id: u32) -> Option<u32> {
        self.object_versions.get(&id).copied()
    }

    fn ensure_keymap(&mut self) -> Result<u32, &'static str> {
        if self.keymap_shm.is_some() {
            return Ok(self.keymap_size);
        }

        let keymap = b"xkb_keymap {\n\
            xkb_keycodes \"(unnamed)\" { minimum = 8; maximum = 255; };\n\
            xkb_types \"(unnamed)\" { type \"ONE_LEVEL\" { level_name[1] = \"Any\"; }; };\n\
            xkb_compatibility \"(unnamed)\" { };\n\
            xkb_symbols \"(unnamed)\" { };\n\
            xkb_geometry \"(unnamed)\" { };\n\
        };";
        let size = keymap.len() + 1;

        let shm = SharedMemory::create(size, permissions::READ_WRITE)
            .map_err(|_| "Failed to create keymap SHM")?;
        let mapper = shm
            .as_handle()
            .as_memory_mapping()
            .map_err(|_| "Keymap SHM mapping unsupported")?;
        let addr = mapper
            .mmap(
                0,
                size,
                permissions::READ_WRITE,
                std::handle::capability::memory_mapping::flags::SHARED,
                0,
            )
            .map_err(|_| "Failed to mmap keymap SHM")?;
        unsafe {
            let ptr = addr as *mut u8;
            core::ptr::copy_nonoverlapping(keymap.as_ptr(), ptr, keymap.len());
            *ptr.add(keymap.len()) = 0;
        }

        self.keymap_size = size as u32;
        self.keymap_shm = Some(shm);
        Ok(self.keymap_size)
    }

    fn parse_u32(payload: &[u8], offset: usize) -> Option<u32> {
        if payload.len() < offset + 4 {
            return None;
        }
        Some(u32::from_ne_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]))
    }

    fn parse_i32(payload: &[u8], offset: usize) -> Option<i32> {
        if payload.len() < offset + 4 {
            return None;
        }
        Some(i32::from_ne_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]))
    }

    fn parse_string(payload: &[u8], offset: usize) -> Option<(String, usize)> {
        let len = Self::parse_u32(payload, offset)? as usize;
        let start = offset + 4;
        let end = start.checked_add(len)?;
        if payload.len() < end {
            return None;
        }
        let bytes = if len == 0 {
            &[]
        } else {
            &payload[start..end - 1]
        };
        let s = String::from_utf8_lossy(bytes).into_owned();
        let padded = (len + 3) & !3;
        Some((s, start + padded))
    }

    /// Create an SWS window with specific dimensions
    fn create_sws_window_with_size(
        &mut self,
        wl_surface_id: u32,
        width: u32,
        height: u32,
    ) -> Result<(), &'static str> {
        // Check if already mapped
        if self.surface_to_window.contains_key(&wl_surface_id) {
            return Ok(());
        }

        bridge_log!(
            "[Bridge] Creating SWS window for surface {} ({}x{})",
            wl_surface_id,
            width,
            height
        );

        let payload = protocol_sws::payload_extension_create_window(wl_surface_id, width, height);
        let request_id =
            self.send_sws_request(protocol_sws::client_msg::EXTENSION_CREATE_WINDOW, &payload)?;
        bridge_info!(
            "[wayland-bridge] client={} surface={} waiting for SWS window request={} size={}x{}",
            self.client_id,
            wl_surface_id,
            request_id,
            width,
            height
        );
        let (create_response, _initial_shm_handle) = self
            .wait_for_sws_message_with_handle(request_id, |msg| {
                matches!(msg, protocol_sws::ServerMessage::WindowCreated { .. })
            })?;
        if let protocol_sws::ServerMessage::WindowCreated { window_id, .. } = create_response {
            bridge_log!(
                "[Bridge] SWS window created: {} for surface {}",
                window_id,
                wl_surface_id
            );
            self.surface_to_window.insert(wl_surface_id, window_id);
            bridge_info!(
                "[wayland-bridge] client={} surface={} SWS window={} created",
                self.client_id,
                wl_surface_id,
                window_id
            );
            if let Some(surface) = self.surface_manager.get_surface_mut(wl_surface_id) {
                surface.sws_window_id = Some(window_id);
            }
            self.apply_xdg_toplevel_state_to_sws(wl_surface_id, window_id);
        }

        Ok(())
    }

    fn rect_area(rect: (u32, u32, u32, u32)) -> u64 {
        u64::from(rect.2).saturating_mul(u64::from(rect.3))
    }

    fn union_damage_rect(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
        let ax1 = a.0.saturating_add(a.2);
        let ay1 = a.1.saturating_add(a.3);
        let bx1 = b.0.saturating_add(b.2);
        let by1 = b.1.saturating_add(b.3);
        let x0 = a.0.min(b.0);
        let y0 = a.1.min(b.1);
        let x1 = ax1.max(bx1);
        let y1 = ay1.max(by1);
        (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
    }

    fn should_merge_damage(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> bool {
        let union = Self::union_damage_rect(a, b);
        let separate_area = Self::rect_area(a).saturating_add(Self::rect_area(b));
        let union_area = Self::rect_area(union);
        union_area <= separate_area.saturating_mul(DAMAGE_MERGE_AREA_FACTOR)
    }

    fn push_damage_rect(rects: &mut Vec<(u32, u32, u32, u32)>, rect: (u32, u32, u32, u32)) {
        if rect.2 == 0 || rect.3 == 0 {
            return;
        }

        for existing in rects.iter_mut() {
            if Self::should_merge_damage(*existing, rect) {
                *existing = Self::union_damage_rect(*existing, rect);
                return;
            }
        }

        if rects.len() < MAX_PENDING_DAMAGE_RECTS {
            rects.push(rect);
            return;
        }

        let mut best_index = 0;
        let mut best_extra_area = u64::MAX;
        for (idx, existing) in rects.iter().enumerate() {
            let union = Self::union_damage_rect(*existing, rect);
            let extra_area = Self::rect_area(union).saturating_sub(Self::rect_area(*existing));
            if extra_area < best_extra_area {
                best_index = idx;
                best_extra_area = extra_area;
            }
        }
        rects[best_index] = Self::union_damage_rect(rects[best_index], rect);
    }

    fn compute_damage_rects(
        damage: &[(i32, i32, i32, i32)],
        surface_width: u32,
        surface_height: u32,
    ) -> Vec<(u32, u32, u32, u32)> {
        if surface_width == 0 || surface_height == 0 {
            return Vec::new();
        }
        let mut rects = Vec::new();
        for &(dx, dy, dw, dh) in damage {
            if dw <= 0 || dh <= 0 {
                continue;
            }
            let rx0 = dx;
            let ry0 = dy;
            let rx1 = dx.saturating_add(dw);
            let ry1 = dy.saturating_add(dh);

            let cx0 = rx0.max(0).min(surface_width as i32);
            let cy0 = ry0.max(0).min(surface_height as i32);
            let cx1 = rx1.max(0).min(surface_width as i32);
            let cy1 = ry1.max(0).min(surface_height as i32);

            if cx1 <= cx0 || cy1 <= cy0 {
                continue;
            }
            Self::push_damage_rect(
                &mut rects,
                (
                    cx0 as u32,
                    cy0 as u32,
                    (cx1 - cx0) as u32,
                    (cy1 - cy0) as u32,
                ),
            );
        }

        rects
    }

    fn submitted_surface_buffer(&self, surface_id: u32) -> Option<u32> {
        self.submitted_surface_buffers
            .get(&surface_id)
            .copied()
            .flatten()
    }

    fn coalesce_pending_surface_commit(&mut self, mut newer: PendingSurfaceCommit) -> Option<u32> {
        let surface_id = newer.surface_id;
        let submitted_buffer = self.submitted_surface_buffer(surface_id);
        let Some(previous) = self.pending_surface_commits.remove(&surface_id) else {
            self.pending_surface_commits.insert(surface_id, newer);
            return None;
        };

        let locally_releasable = (previous.sws_buffer_id != newer.sws_buffer_id
            && previous.sws_buffer_id != submitted_buffer)
            .then_some(previous.wayland_buffer_id)
            .flatten();
        for rect in previous.damage_rects {
            Self::push_damage_rect(&mut newer.damage_rects, rect);
        }
        self.pending_surface_commits.insert(surface_id, newer);
        locally_releasable
    }

    fn submit_surface_commit(
        &mut self,
        mut pending: PendingSurfaceCommit,
    ) -> Result<bool, &'static str> {
        let submitted_buffer = self.submitted_surface_buffer(pending.surface_id);
        let buffer_changed = submitted_buffer != pending.sws_buffer_id;
        let callbacks_pending = self
            .pending_frame_callbacks
            .get(&pending.surface_id)
            .is_some_and(|callbacks| !callbacks.is_empty());

        if !buffer_changed && pending.damage_rects.is_empty() && !callbacks_pending {
            return Ok(false);
        }

        if pending.sws_buffer_id.is_some()
            && pending.damage_rects.is_empty()
            && (buffer_changed || callbacks_pending)
        {
            // Buffer selection and callback-only commits still need one
            // presentation boundary, but unchanged surface contents do not
            // justify uploading the complete backing store.
            pending.damage_rects.push((0, 0, 1, 1));
        }

        self.commit_extension_buffer(
            pending.surface_id,
            pending.window_id,
            pending.sws_buffer_id,
            buffer_changed,
            &pending.damage_rects,
        )?;
        self.submitted_surface_buffers
            .insert(pending.surface_id, pending.sws_buffer_id);
        self.ensure_sws_frame_request(pending.surface_id, true)?;
        Ok(true)
    }

    fn queue_or_submit_surface_commit(
        &mut self,
        pending: PendingSurfaceCommit,
        messages: &mut Vec<WaylandMessage>,
    ) -> Result<(), &'static str> {
        let callbacks_pending = self
            .pending_frame_callbacks
            .get(&pending.surface_id)
            .is_some_and(|callbacks| !callbacks.is_empty());
        let needs_submission = self.submitted_surface_buffer(pending.surface_id)
            != pending.sws_buffer_id
            || !pending.damage_rects.is_empty()
            || callbacks_pending;
        if !needs_submission {
            return Ok(());
        }

        if self
            .surface_frame_request_outstanding
            .contains_key(&pending.surface_id)
        {
            if let Some(buffer_id) = self.coalesce_pending_surface_commit(pending) {
                self.append_buffer_release(messages, buffer_id);
            }
            return Ok(());
        }

        self.submit_surface_commit(pending)?;
        Ok(())
    }

    fn flush_pending_surface_commit(&mut self, surface_id: u32) -> Result<bool, &'static str> {
        let Some(pending) = self.pending_surface_commits.remove(&surface_id) else {
            return Ok(false);
        };
        self.submit_surface_commit(pending)
    }

    fn flush_pending_surface_commits_using_buffer(
        &mut self,
        sws_buffer_id: u32,
    ) -> Result<(), &'static str> {
        let surface_ids: Vec<u32> = self
            .pending_surface_commits
            .iter()
            .filter_map(|(&surface_id, pending)| {
                (pending.sws_buffer_id == Some(sws_buffer_id)).then_some(surface_id)
            })
            .collect();
        for surface_id in surface_ids {
            self.flush_pending_surface_commit(surface_id)?;
        }
        Ok(())
    }

    fn discard_pending_surface_commit(&mut self, surface_id: u32) -> Option<u32> {
        let pending = self.pending_surface_commits.remove(&surface_id)?;
        (pending.sws_buffer_id != self.submitted_surface_buffer(surface_id))
            .then_some(pending.wayland_buffer_id)
            .flatten()
    }

    fn send_request_move_window(&mut self, window_id: u32) -> Result<(), &'static str> {
        bridge_log!(
            "[Bridge] Sending REQUEST_MOVE_WINDOW for window {}",
            window_id
        );

        let payload = protocol_sws::payload_request_move_window(window_id);
        self.send_sws_async_message(protocol_sws::client_msg::REQUEST_MOVE_WINDOW, &payload)
    }

    fn send_minimize_window(&mut self, window_id: u32) -> Result<(), &'static str> {
        bridge_log!("[Bridge] Sending MINIMIZE_WINDOW for window {}", window_id);

        let payload = protocol_sws::payload_minimize_window(window_id);
        self.send_sws_async_message(protocol_sws::client_msg::MINIMIZE_WINDOW, &payload)
    }

    fn send_maximize_window(&mut self, window_id: u32) -> Result<(), &'static str> {
        bridge_log!("[Bridge] Sending MAXIMIZE_WINDOW for window {}", window_id);

        let payload = protocol_sws::payload_maximize_window(window_id);
        self.send_sws_async_message(protocol_sws::client_msg::MAXIMIZE_WINDOW, &payload)
    }

    fn send_restore_window(&mut self, window_id: u32) -> Result<(), &'static str> {
        bridge_log!("[Bridge] Sending RESTORE_WINDOW for window {}", window_id);

        let payload = protocol_sws::payload_restore_window(window_id);
        self.send_sws_async_message(protocol_sws::client_msg::RESTORE_WINDOW, &payload)
    }

    fn send_set_fullscreen(&mut self, window_id: u32) -> Result<(), &'static str> {
        let payload = protocol_sws::payload_set_fullscreen(window_id);
        self.send_sws_async_message(protocol_sws::client_msg::SET_FULLSCREEN, &payload)
    }

    fn send_unset_fullscreen(&mut self, window_id: u32) -> Result<(), &'static str> {
        let payload = protocol_sws::payload_unset_fullscreen(window_id);
        self.send_sws_async_message(protocol_sws::client_msg::UNSET_FULLSCREEN, &payload)
    }

    fn send_destroy_window(&mut self, window_id: u32) -> Result<(), &'static str> {
        bridge_log!("[Bridge] Sending DESTROY_WINDOW for window {}", window_id);

        let payload = protocol_sws::payload_destroy_window(window_id);
        self.send_sws_async_message(protocol_sws::client_msg::DESTROY_WINDOW, &payload)
    }

    fn register_extension_shm_pool(
        &mut self,
        pool_id: u32,
        size: usize,
        handle: &Handle,
    ) -> Result<(), &'static str> {
        let payload = protocol_sws::payload_extension_register_shm_pool(pool_id, size as u64);
        let request_id = self.send_sws_handle_request(
            protocol_sws::client_msg::EXTENSION_REGISTER_SHM_POOL,
            &payload,
            handle,
        )?;
        let log_resource = should_log_resource_count(self.shm_pool_count);
        if log_resource {
            bridge_info!(
                "[wayland-bridge] client={} waiting for SWS SHM pool request={} pool={} pools={} size={}",
                self.client_id,
                request_id,
                pool_id,
                self.shm_pool_count,
                size
            );
        }
        let response = self.wait_for_sws_message(request_id, |message| {
            matches!(
                message,
                protocol_sws::ServerMessage::ExtensionShmPoolRegistered { .. }
            )
        })?;
        match response {
            protocol_sws::ServerMessage::ExtensionShmPoolRegistered {
                pool_id: registered_id,
                size: registered_size,
            } if registered_id == pool_id && registered_size == size as u64 => {
                if log_resource {
                    bridge_info!(
                        "[wayland-bridge] client={} SWS SHM pool={} registered pools={} size={}",
                        self.client_id,
                        pool_id,
                        self.shm_pool_count,
                        size
                    );
                }
                Ok(())
            }
            _ => Err("SWS registered an unexpected extension SHM pool"),
        }
    }

    fn resize_extension_shm_pool(&mut self, pool_id: u32, size: usize) -> Result<(), &'static str> {
        let payload = protocol_sws::payload_extension_resize_shm_pool(pool_id, size as u64);
        let request_id = self.send_sws_request(
            protocol_sws::client_msg::EXTENSION_RESIZE_SHM_POOL,
            &payload,
        )?;
        let response = self.wait_for_sws_message(request_id, |message| {
            matches!(
                message,
                protocol_sws::ServerMessage::ExtensionShmPoolResized { .. }
            )
        })?;
        match response {
            protocol_sws::ServerMessage::ExtensionShmPoolResized {
                pool_id: resized_id,
                size: resized_size,
            } if resized_id == pool_id && resized_size == size as u64 => Ok(()),
            _ => Err("SWS resized an unexpected extension SHM pool"),
        }
    }

    fn destroy_extension_shm_pool(&mut self, pool_id: u32) -> Result<(), &'static str> {
        let payload = protocol_sws::payload_extension_destroy_shm_pool(pool_id);
        self.send_sws_async_message(
            protocol_sws::client_msg::EXTENSION_DESTROY_SHM_POOL,
            &payload,
        )
    }

    fn define_extension_shm_buffer(&mut self, buffer_id: u32) -> Result<(), &'static str> {
        let buffer = self
            .shm_manager
            .get_buffer(buffer_id)
            .ok_or("Wayland SHM buffer not found")?;
        let sws_pool_id = self
            .shm_manager
            .get_pool(buffer.pool_id)
            .ok_or("Wayland SHM pool not found")?
            .sws_pool_id;
        let payload = protocol_sws::payload_extension_define_buffer(
            buffer.sws_buffer_id,
            sws_pool_id,
            buffer.offset as u64,
            buffer.width as u32,
            buffer.height as u32,
            buffer.stride as u32,
            buffer.format,
        );
        self.send_sws_async_message(protocol_sws::client_msg::EXTENSION_DEFINE_BUFFER, &payload)
    }

    fn destroy_extension_buffer(&mut self, buffer_id: u32) -> Result<(), &'static str> {
        let payload = protocol_sws::payload_extension_destroy_buffer(buffer_id);
        self.send_sws_async_message(protocol_sws::client_msg::EXTENSION_DESTROY_BUFFER, &payload)
    }

    fn commit_extension_buffer(
        &mut self,
        surface_id: u32,
        window_id: u32,
        sws_buffer_id: Option<u32>,
        buffer_changed: bool,
        damage_rects: &[(u32, u32, u32, u32)],
    ) -> Result<(), &'static str> {
        let serial = self.allocate_extension_commit_serial();
        let damage: Vec<protocol_sws::ExtensionDamageRect> = damage_rects
            .iter()
            .map(|&(x, y, width, height)| {
                protocol_sws::ExtensionDamageRect::new(x as i32, y as i32, width, height)
            })
            .collect();
        let payload = protocol_sws::payload_extension_commit_buffer(
            surface_id,
            window_id,
            sws_buffer_id.unwrap_or(0),
            buffer_changed,
            serial,
            &damage,
        )
        .map_err(|_| "Invalid reusable extension-buffer commit")?;
        self.send_sws_async_message(protocol_sws::client_msg::EXTENSION_COMMIT_BUFFER, &payload)?;
        self.sws_buffer_commit_count = self.sws_buffer_commit_count.saturating_add(1);
        if should_log_resource_count(self.sws_buffer_commit_count) {
            bridge_info!(
                "[wayland-bridge] client={} surface={} window={} SWS buffer_commit={} resource={} serial={} changed={} damage_rects={}",
                self.client_id,
                surface_id,
                window_id,
                self.sws_buffer_commit_count,
                sws_buffer_id.unwrap_or(0),
                serial,
                buffer_changed,
                damage_rects.len()
            );
        }
        Ok(())
    }

    /// Handle a client connection
    fn handle_client(&mut self, mut client: Socket) -> Result<(), &'static str> {
        bridge_info!("[wayland-bridge] client={} connected", self.client_id);

        client
            .set_nonblocking(true)
            .map_err(|_| "Failed to set client socket non-blocking")?;

        let mut buffer: Vec<u8> = Vec::new();
        let mut record_buffer = Vec::new();
        record_buffer.resize(MAX_WAYLAND_RECORD_SIZE, 0);
        let mut received_handles: Vec<Handle> = Vec::new();

        loop {
            let mut got_data = false;
            loop {
                match client.recv_handle_and_data(&mut record_buffer) {
                    Ok((handle, bytes_read)) => {
                        got_data = true;
                        received_handles.push(handle);
                        buffer.extend_from_slice(&record_buffer[..bytes_read]);
                        continue;
                    }
                    Err(std::socket::SocketError::ReceiveBufferTooSmall { required_len }) => {
                        record_buffer.resize(required_len, 0);
                        continue;
                    }
                    Err(std::socket::SocketError::WouldBlock) => {}
                    Err(_) => {
                        return Err("Failed to receive Wayland handle-and-data record");
                    }
                }

                match client.recv_handle() {
                    Ok(handle) => {
                        got_data = true;
                        received_handles.push(handle);
                        continue;
                    }
                    Err(std::socket::SocketError::WouldBlock) => {}
                    Err(_) => {
                        return Err("Failed to receive Wayland handle");
                    }
                }

                let mut read_buf = [0u8; 4096];
                match client.read(&mut read_buf) {
                    Ok(0) => {
                        bridge_info!(
                            "[wayland-bridge] client={} disconnected windows={} surfaces={} syncs={} pools={} buffers={} commits={} local_releases={} sws_commits={} sws_releases={} frame_done={}",
                            self.client_id,
                            self.surface_to_window.len(),
                            self.surface_count,
                            self.display_sync_count,
                            self.shm_pool_count,
                            self.shm_buffer_count,
                            self.surface_commit_count,
                            self.local_buffer_release_count,
                            self.sws_buffer_commit_count,
                            self.sws_buffer_release_count,
                            self.sws_frame_done_count
                        );
                        return Ok(());
                    }
                    Ok(n) => {
                        got_data = true;
                        if is_debug_enabled() {
                            bridge_log!("[Bridge] Received {} bytes from client", n);
                        }
                        buffer.extend_from_slice(&read_buf[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        return Err("Failed to read Wayland client stream");
                    }
                }
            }

            if got_data {
                // Parse and handle messages
                let mut offset = 0;
                while offset + 8 <= buffer.len() {
                    let header_bytes = &buffer[offset..offset + 8];
                    let mut header_array = [0u8; 8];
                    header_array.copy_from_slice(header_bytes);
                    let header = MessageHeader::from_bytes(&header_array);

                    let msg_size = header.size() as usize;
                    if msg_size < MessageHeader::SIZE || msg_size % 4 != 0 {
                        bridge_log!(
                            "[Bridge] Invalid Wayland message header at offset {}: object_id={} opcode={} size={} bytes={:02x?}",
                            offset,
                            header.object_id,
                            header.opcode(),
                            msg_size,
                            header_bytes
                        );
                        return Err("Invalid Wayland message header");
                    }
                    if offset + msg_size > buffer.len() {
                        if is_debug_enabled() {
                            bridge_log!("[Bridge] Incomplete message, waiting for more data");
                        }
                        break;
                    }

                    if is_debug_enabled() {
                        bridge_log!(
                            "[Bridge] Message: object_id={} opcode={} size={}",
                            header.object_id,
                            header.opcode(),
                            msg_size
                        );
                    }

                    // SCM_RIGHTS handles are ordered independently from the byte
                    // stream. libwayland may batch requests before wl_shm.create_pool
                    // in the same sendmsg(), so consume a handle only when the
                    // protocol signature for the current request requires one.
                    let interface = self
                        .objects
                        .get(&header.object_id)
                        .map(|name| name.as_str());
                    let attached_handle =
                        take_message_handle(interface, header.opcode(), &mut received_handles);

                    // Handle the message
                    let responses = self.handle_message(
                        &header,
                        &buffer[offset + 8..offset + msg_size],
                        attached_handle,
                    )?;
                    for response in responses {
                        let response_bytes = response.encode();
                        // Always log responses for debugging
                        bridge_log!(
                            "[Bridge] Sending response: obj={} opcode={} size={}",
                            response.header.object_id,
                            response.header.opcode(),
                            response_bytes.len()
                        );
                        // Only wl_keyboard.keymap requires an FD/handle transfer.
                        if response.header.opcode() == input::keyboard_event::KEYMAP {
                            let is_keyboard = self
                                .objects
                                .get(&response.header.object_id)
                                .map(|iface| iface == "wl_keyboard")
                                .unwrap_or(false);
                            if is_keyboard {
                                let shm = self
                                    .keymap_shm
                                    .as_ref()
                                    .ok_or("Missing keymap shared memory")?;
                                send_handle_and_data_nonblocking(
                                    &client,
                                    shm.as_handle(),
                                    &response_bytes,
                                    "Failed to send KEYMAP handle record",
                                )?;
                                bridge_info!(
                                    "[wayland-bridge] client={} wl_keyboard={} keymap sent size={}",
                                    self.client_id,
                                    response.header.object_id,
                                    self.keymap_size
                                );
                                continue;
                            }
                        }
                        write_all_nonblocking(
                            &mut client,
                            &response_bytes,
                            "Failed to send Wayland response",
                        )?;
                    }

                    offset += msg_size;
                }

                if offset > 0 {
                    buffer.drain(0..offset);
                }
            }

            // Check for input, configure, release, and frame events from SWS.
            // Losing this connection terminates only this client worker; the
            // server accept loop remains available for subsequent clients.
            self.poll_sws_messages()?;
            let mut input_events = Vec::new();
            {
                let mut queue = self.input_event_queue.lock();
                input_events.extend(queue.drain(..));
            }
            let had_input_events = !input_events.is_empty();
            let mut encoded_input_events = Vec::new();
            for input_msg in input_events {
                let msg_bytes = input_msg.encode();
                let should_log_input = match self.objects.get(&input_msg.header.object_id) {
                    Some(interface) if interface == "wl_pointer" => matches!(
                        input_msg.header.opcode(),
                        input::pointer_event::ENTER
                            | input::pointer_event::LEAVE
                            | input::pointer_event::MOTION
                            | input::pointer_event::BUTTON
                            | input::pointer_event::FRAME
                    ),
                    Some(interface) if interface == "wl_keyboard" => matches!(
                        input_msg.header.opcode(),
                        input::keyboard_event::ENTER | input::keyboard_event::LEAVE
                    ),
                    _ => input_msg.header.object_id == 0,
                };
                if should_log_input {
                    bridge_log!(
                        "[Bridge] Forwarding input event: obj={} opcode={} size={} bytes={:02x?}",
                        input_msg.header.object_id,
                        input_msg.header.opcode(),
                        msg_bytes.len(),
                        &msg_bytes[..msg_bytes.len().min(32)]
                    );
                }
                encoded_input_events.extend_from_slice(&msg_bytes);
            }
            if !encoded_input_events.is_empty() {
                write_all_nonblocking(
                    &mut client,
                    &encoded_input_events,
                    "Failed to forward Wayland events",
                )?;
            }

            if !got_data && !had_input_events {
                self.wait_for_activity(&client)?;
            }
        }
    }

    /// Handle a Wayland protocol message
    fn handle_message(
        &mut self,
        header: &MessageHeader,
        payload: &[u8],
        attached_handle: Option<Handle>,
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        let object_id = header.object_id;
        let opcode = header.opcode();

        // Get the interface for this object
        let interface = match self.objects.get(&object_id) {
            Some(interface) => interface.clone(),
            None => {
                bridge_log!("[Bridge] Unknown object ID: {}", object_id);
                return Ok(Vec::new());
            }
        };

        if attached_handle.is_some()
            && (interface.as_str() != "wl_shm" || opcode != shm::shm_request::CREATE_POOL)
        {
            return Err("Unexpected handle attached to Wayland message");
        }

        match interface.as_str() {
            "wl_display" => self.handle_display_message(opcode, payload),
            "wl_registry" => self.handle_registry_message(object_id, opcode, payload),
            "wl_compositor" => self.handle_compositor_message(opcode, payload),
            "wl_surface" => self.handle_surface_message(object_id, opcode, payload),
            "wl_shm" => self.handle_shm_message(opcode, payload, attached_handle),
            "wl_shm_pool" => self.handle_shm_pool_message(object_id, opcode, payload),
            "wl_buffer" => self.handle_buffer_message(object_id, opcode, payload),
            "wl_seat" => self.handle_seat_message(object_id, opcode, payload),
            "wl_pointer" => self.handle_pointer_message(object_id, opcode, payload),
            "wl_keyboard" => self.handle_keyboard_message(object_id, opcode, payload),
            "wl_output" => self.handle_output_message(object_id, opcode, payload),
            "wl_data_device_manager" => self.handle_data_device_manager_message(opcode, payload),
            "wl_data_device" => self.handle_data_device_message(object_id, opcode, payload),
            "wl_data_source" => self.handle_data_source_message(object_id, opcode, payload),
            "wl_region" => self.handle_region_message(object_id, opcode, payload),
            "xdg_wm_base" => self.handle_xdg_wm_base_message(opcode, payload),
            "xdg_surface" => self.handle_xdg_surface_message(object_id, opcode, payload),
            "xdg_toplevel" => self.handle_xdg_toplevel_message(object_id, opcode, payload),
            "xdg_toplevel_dead" => Ok(Vec::new()),
            "xdg_surface_dead" => Ok(Vec::new()),
            _ => {
                bridge_log!("[Bridge] Unhandled interface: {}", interface);
                Ok(Vec::new())
            }
        }
    }

    /// Handle wl_display messages
    fn handle_display_message(
        &mut self,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            protocol::display_request::SYNC => {
                bridge_log!("[Bridge] wl_display.sync");
                // Parse callback ID from payload
                if payload.len() >= 4 {
                    let callback_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    bridge_log!("[Bridge] Sync callback ID: {}", callback_id);
                    self.objects
                        .insert(callback_id, String::from("wl_callback"));

                    let serial = self.allocate_serial();
                    let mut msgs = Vec::new();
                    self.append_callback_done(&mut msgs, callback_id, serial);
                    self.display_sync_count = self.display_sync_count.saturating_add(1);
                    if should_log_resource_count(self.display_sync_count) {
                        bridge_info!(
                            "[wayland-bridge] client={} completed wl_display.sync={} callback={}",
                            self.client_id,
                            self.display_sync_count,
                            callback_id
                        );
                    }
                    return Ok(msgs);
                }
                Ok(Vec::new())
            }
            protocol::display_request::GET_REGISTRY => {
                bridge_log!("[Bridge] wl_display.get_registry");
                // Parse registry ID from payload
                if payload.len() >= 4 {
                    let registry_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    bridge_info!(
                        "[wayland-bridge] client={} Wayland registry requested object={}",
                        self.client_id,
                        registry_id
                    );
                    bridge_log!("[Bridge] Registry ID: {}", registry_id);
                    self.objects
                        .insert(registry_id, String::from("wl_registry"));

                    return Ok(self.registry.get_global_events(registry_id));
                }
                Ok(Vec::new())
            }
            _ => {
                bridge_log!("[Bridge] Unknown wl_display opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Handle wl_registry messages
    fn handle_registry_message(
        &mut self,
        _registry_id: u32,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            protocol::registry_request::BIND => {
                bridge_log!("[Bridge] wl_registry.bind");
                // Parse: name (u32), interface (string), version (u32), id (u32)
                if let Some(name) = Self::parse_u32(payload, 0) {
                    bridge_log!("[Bridge] Binding global name: {}", name);

                    if let Some((interface_name, offset)) = Self::parse_string(payload, 4) {
                        let version = Self::parse_u32(payload, offset).unwrap_or(0);
                        let new_id = Self::parse_u32(payload, offset + 4).unwrap_or(0);
                        bridge_log!(
                            "[Bridge] Bind interface={} version={} new_id={}",
                            interface_name,
                            version,
                            new_id
                        );

                        if let Some(global) = self.registry.get_global(name) {
                            bridge_log!(
                                "[Bridge] Global info: interface='{}' requested='{}' match={}",
                                global.interface,
                                interface_name,
                                global.interface == interface_name
                            );
                            if global.interface == interface_name && new_id != 0 {
                                self.add_object(new_id, interface_name.clone());
                                self.object_versions.insert(new_id, version);
                                bridge_info!(
                                    "[wayland-bridge] client={} bound {} version={} object={}",
                                    self.client_id,
                                    interface_name,
                                    version,
                                    new_id
                                );

                                if interface_name == "wl_shm" {
                                    let mut msgs = Vec::new();
                                    let mut fmt_argb =
                                        WaylandMessage::new(new_id, shm::shm_event::FORMAT);
                                    fmt_argb.add_arg(WaylandArg::Uint(shm::shm_format::ARGB8888));
                                    msgs.push(fmt_argb);

                                    let mut fmt_xrgb =
                                        WaylandMessage::new(new_id, shm::shm_event::FORMAT);
                                    fmt_xrgb.add_arg(WaylandArg::Uint(shm::shm_format::XRGB8888));
                                    msgs.push(fmt_xrgb);
                                    return Ok(msgs);
                                }

                                if interface_name == "wl_data_device_manager" {
                                    // Just accept bind, no events needed
                                    return Ok(Vec::new());
                                }

                                if interface_name == "wl_seat" {
                                    self.input_manager.create_seat(new_id, "seat0");
                                    let mut msgs = Vec::new();

                                    // Send CAPABILITIES event
                                    let mut caps = WaylandMessage::new(
                                        new_id,
                                        input::seat_event::CAPABILITIES,
                                    );
                                    caps.add_arg(WaylandArg::Uint(
                                        input::seat_capabilities::POINTER
                                            | input::seat_capabilities::KEYBOARD,
                                    ));
                                    msgs.push(caps);

                                    if version >= 2 {
                                        let mut name_msg =
                                            WaylandMessage::new(new_id, input::seat_event::NAME);
                                        name_msg.add_arg(WaylandArg::String(b"seat0".to_vec()));
                                        msgs.push(name_msg);
                                    }
                                    return Ok(msgs);
                                }

                                if interface_name == "wl_output" {
                                    let scale = self.output_scale;
                                    bridge_log!(
                                        "[Bridge] Advertising wl_output.scale = {} to client",
                                        scale
                                    );

                                    let mut msgs = Vec::new();
                                    let mut geom = WaylandMessage::new(
                                        new_id,
                                        protocol::output_event::GEOMETRY,
                                    );
                                    geom.add_arg(WaylandArg::Int(0)); // x
                                    geom.add_arg(WaylandArg::Int(0)); // y
                                    geom.add_arg(WaylandArg::Int(320)); // phys width mm
                                    geom.add_arg(WaylandArg::Int(200)); // phys height mm
                                    geom.add_arg(WaylandArg::Int(0)); // subpixel
                                    geom.add_arg(WaylandArg::String(b"Scarlet".to_vec()));
                                    geom.add_arg(WaylandArg::String(b"Virtual".to_vec()));
                                    geom.add_arg(WaylandArg::Int(0)); // transform
                                    msgs.push(geom);

                                    let mut mode =
                                        WaylandMessage::new(new_id, protocol::output_event::MODE);
                                    mode.add_arg(WaylandArg::Uint(1)); // current
                                    mode.add_arg(WaylandArg::Int(800 * scale)); // width (physical)
                                    mode.add_arg(WaylandArg::Int(600 * scale)); // height (physical)
                                    mode.add_arg(WaylandArg::Int(60000)); // refresh mHz
                                    msgs.push(mode);

                                    let mut scale_msg =
                                        WaylandMessage::new(new_id, protocol::output_event::SCALE);
                                    scale_msg.add_arg(WaylandArg::Int(scale));
                                    msgs.push(scale_msg);

                                    let done =
                                        WaylandMessage::new(new_id, protocol::output_event::DONE);
                                    msgs.push(done);
                                    return Ok(msgs);
                                }
                            } else {
                                bridge_log!(
                                    "[Bridge] Bind mismatch: requested={}, advertised={}",
                                    interface_name,
                                    global.interface
                                );
                            }
                        } else {
                            bridge_log!("[Bridge] Unknown global name {}", name);
                        }
                    }
                }
                Ok(Vec::new())
            }
            _ => {
                bridge_log!("[Bridge] Unknown wl_registry opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Handle wl_compositor messages
    fn handle_compositor_message(
        &mut self,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            protocol::compositor_request::CREATE_SURFACE => {
                bridge_log!("[Bridge] wl_compositor.create_surface");
                if payload.len() >= 4 {
                    let surface_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    bridge_log!("[Bridge] Created surface ID: {}", surface_id);
                    self.add_object(surface_id, String::from("wl_surface"));
                    self.surface_manager.create_surface(surface_id);
                    self.surface_count = self.surface_count.saturating_add(1);
                    if should_log_resource_count(self.surface_count) {
                        bridge_info!(
                            "[wayland-bridge] client={} created wl_surface={} surfaces={}",
                            self.client_id,
                            surface_id,
                            self.surface_count
                        );
                    }
                }
                Ok(Vec::new())
            }
            protocol::compositor_request::CREATE_REGION => {
                bridge_log!("[Bridge] wl_compositor.create_region");
                if payload.len() >= 4 {
                    let region_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    bridge_log!("[Bridge] Created region ID: {}", region_id);
                    self.add_object(region_id, String::from("wl_region"));
                    self.region_manager.create_region_with_id(region_id);
                }
                Ok(Vec::new())
            }
            _ => {
                bridge_log!("[Bridge] Unknown wl_compositor opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Handle wl_surface messages
    fn handle_surface_message(
        &mut self,
        surface_id: u32,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            protocol::surface_request::DESTROY => {
                bridge_log!("[Bridge] wl_surface.destroy: {}", surface_id);
                let mut messages = Vec::new();
                if let Some(buffer_id) = self.discard_pending_surface_commit(surface_id) {
                    self.append_buffer_release(&mut messages, buffer_id);
                }
                self.cancel_surface_frame_request(surface_id);
                self.submitted_surface_buffers.remove(&surface_id);
                self.surface_manager.destroy_surface(surface_id);
                self.remove_object(surface_id);
                // Remove from surface_to_window mapping
                if let Some(window_id) = self.surface_to_window.remove(&surface_id) {
                    let _ = self.send_destroy_window(window_id);
                }
                Ok(messages)
            }
            protocol::surface_request::ATTACH => {
                if is_debug_enabled() {
                    bridge_log!("[Bridge] wl_surface.attach on surface {}", surface_id);
                }
                if payload.len() >= 12 {
                    let buffer_id = Self::parse_u32(payload, 0).unwrap_or(0);
                    let _x = Self::parse_i32(payload, 4).unwrap_or(0);
                    let _y = Self::parse_i32(payload, 8).unwrap_or(0);
                    let pending_buffer = (buffer_id != 0).then_some(buffer_id);
                    if let Some(buffer_id) = pending_buffer
                        && self.shm_manager.get_buffer(buffer_id).is_none()
                    {
                        return Err("wl_surface.attach referenced an unknown SHM buffer");
                    }
                    if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                        surface.attach(pending_buffer);
                    }
                }
                Ok(Vec::new())
            }
            protocol::surface_request::DAMAGE => {
                if is_debug_enabled() {
                    bridge_log!("[Bridge] wl_surface.damage on surface {}", surface_id);
                }
                if payload.len() >= 16 {
                    let x = Self::parse_i32(payload, 0).unwrap_or(0);
                    let y = Self::parse_i32(payload, 4).unwrap_or(0);
                    let width = Self::parse_i32(payload, 8).unwrap_or(0);
                    let height = Self::parse_i32(payload, 12).unwrap_or(0);
                    if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                        let scale = surface.buffer_scale.max(1);
                        surface.add_damage(
                            x.saturating_mul(scale),
                            y.saturating_mul(scale),
                            width.saturating_mul(scale),
                            height.saturating_mul(scale),
                        );
                    }
                }
                Ok(Vec::new())
            }
            protocol::surface_request::DAMAGE_BUFFER => {
                if is_debug_enabled() {
                    bridge_log!(
                        "[Bridge] wl_surface.damage_buffer on surface {}",
                        surface_id
                    );
                }
                if payload.len() >= 16 {
                    let x = Self::parse_i32(payload, 0).unwrap_or(0);
                    let y = Self::parse_i32(payload, 4).unwrap_or(0);
                    let width = Self::parse_i32(payload, 8).unwrap_or(0);
                    let height = Self::parse_i32(payload, 12).unwrap_or(0);
                    if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                        surface.add_damage(x, y, width, height);
                    }
                }
                Ok(Vec::new())
            }
            protocol::surface_request::COMMIT => {
                if is_debug_enabled() {
                    bridge_log!("[Bridge] wl_surface.commit on surface {}", surface_id);
                }
                self.surface_commit_count = self.surface_commit_count.saturating_add(1);
                let (
                    should_update,
                    buffer_attached,
                    buffer_changed,
                    buffer_id,
                    surface_role,
                    pending_damage,
                    frame_callbacks,
                ) = {
                    let surface = self
                        .surface_manager
                        .get_surface_mut(surface_id)
                        .ok_or("wl_surface.commit referenced an unknown surface")?;
                    let frame_callbacks = surface.take_pending_callbacks();
                    let should_update = matches!(
                        surface.role,
                        Some(surface::SurfaceRole::XdgToplevel)
                            | Some(surface::SurfaceRole::XdgPopup)
                    );
                    let pending_damage = surface.damage.clone();
                    let buffer_commit = surface.commit_buffer();
                    surface.commit();
                    (
                        should_update,
                        buffer_commit.attached,
                        buffer_commit.changed,
                        buffer_commit.buffer_id,
                        surface.role,
                        pending_damage,
                        frame_callbacks,
                    )
                };
                if should_log_resource_count(self.surface_commit_count) {
                    bridge_info!(
                        "[wayland-bridge] client={} surface={} commit={} role={:?} attach={} changed={} buffer={:?}",
                        self.client_id,
                        surface_id,
                        self.surface_commit_count,
                        surface_role,
                        buffer_attached,
                        buffer_changed,
                        buffer_id
                    );
                }
                let surface_size = buffer_id
                    .and_then(|buffer_id| self.shm_manager.get_buffer(buffer_id))
                    .map(|buffer| (buffer.width as u32, buffer.height as u32));
                let sws_buffer_id = match buffer_id {
                    Some(buffer_id) => Some(
                        self.shm_manager
                            .get_buffer(buffer_id)
                            .ok_or("Committed Wayland SHM buffer no longer exists")?
                            .sws_buffer_id,
                    ),
                    None => None,
                };
                if let Some((width, height)) = surface_size
                    && let Some(surface) = self.surface_manager.get_surface_mut(surface_id)
                {
                    surface.width = width;
                    surface.height = height;
                }
                let damage_rects = surface_size
                    .map(|(width, height)| {
                        Self::compute_damage_rects(&pending_damage, width, height)
                    })
                    .unwrap_or_default();
                let mut configure_msgs = Vec::new();
                let mut configure_state = None;

                if buffer_id.is_none()
                    && let Some((xdg_surface_id, toplevel_id_opt)) = self
                        .xdg_shell_manager
                        .get_xdg_surface_ids_by_wl_surface(surface_id)
                    && let Some(toplevel_id) = toplevel_id_opt
                {
                    let needs_configure = self
                        .xdg_shell_manager
                        .get_xdg_surface(xdg_surface_id)
                        .map(|surface| surface.last_configure_serial.is_none())
                        .unwrap_or(false);
                    if needs_configure {
                        let serial = self.allocate_serial();
                        configure_state = Some((xdg_surface_id, toplevel_id, serial));
                    }
                }

                if let Some((xdg_surface_id, toplevel_id, serial)) = configure_state {
                    bridge_info!(
                        "[wayland-bridge] client={} surface={} initial xdg configure serial={}",
                        self.client_id,
                        surface_id,
                        serial
                    );
                    let (maximized, fullscreen) = self
                        .xdg_shell_manager
                        .get_xdg_surface(xdg_surface_id)
                        .and_then(|surface| surface.toplevel.as_ref())
                        .map(|toplevel| (toplevel.maximized, toplevel.fullscreen))
                        .unwrap_or((false, false));
                    if let Some(xdg_surface) =
                        self.xdg_shell_manager.get_xdg_surface_mut(xdg_surface_id)
                    {
                        xdg_surface.last_configure_serial = Some(serial);
                    }

                    let mut toplevel_configure =
                        WaylandMessage::new(toplevel_id, xdg_shell::xdg_toplevel_event::CONFIGURE);
                    toplevel_configure.add_arg(WaylandArg::Int(0));
                    toplevel_configure.add_arg(WaylandArg::Int(0));
                    toplevel_configure.add_arg(WaylandArg::Array(Self::xdg_toplevel_state_bytes(
                        maximized, fullscreen,
                    )));

                    let mut surface_configure = WaylandMessage::new(
                        xdg_surface_id,
                        xdg_shell::xdg_surface_event::CONFIGURE,
                    );
                    surface_configure.add_arg(WaylandArg::Uint(serial));

                    configure_msgs.push(toplevel_configure);
                    configure_msgs.push(surface_configure);
                }

                let mut msgs = Vec::new();
                let mut mapped_window = None;
                if should_update {
                    if let Some((width, height)) = surface_size
                        && !self.surface_to_window.contains_key(&surface_id)
                    {
                        self.create_sws_window_with_size(surface_id, width, height)?;
                    }
                    mapped_window = self.surface_to_window.get(&surface_id).copied();
                    if !frame_callbacks.is_empty() && mapped_window.is_some() {
                        self.pending_frame_callbacks
                            .entry(surface_id)
                            .or_insert_with(Vec::new)
                            .extend(frame_callbacks.iter().copied());
                    }
                    if let Some(window_id) = mapped_window {
                        self.queue_or_submit_surface_commit(
                            PendingSurfaceCommit {
                                surface_id,
                                window_id,
                                wayland_buffer_id: buffer_id,
                                sws_buffer_id,
                                damage_rects,
                            },
                            &mut msgs,
                        )?;
                    }
                    if self.focused_surface.is_none() && mapped_window.is_some() {
                        self.queue_focus_events(surface_id);
                    }
                }

                if let Some(buffer_id) =
                    locally_releasable_buffer(mapped_window.is_some(), buffer_attached, buffer_id)
                {
                    // Cursor and role-less surfaces are not sampled by SWS.
                    // Once the commit is consumed, the bridge has no remaining
                    // use for their pixels and must release the client buffer.
                    self.append_buffer_release(&mut msgs, buffer_id);
                }
                if !frame_callbacks.is_empty() && mapped_window.is_none() {
                    // Cursor, role-less, and not-yet-mapped surfaces have no
                    // SWS presentation boundary to pace against.
                    let time = self.allocate_serial();
                    for callback_id in frame_callbacks {
                        self.append_callback_done(&mut msgs, callback_id, time);
                    }
                }

                if !configure_msgs.is_empty() {
                    msgs.extend(configure_msgs);
                }
                Ok(msgs)
            }
            protocol::surface_request::FRAME => {
                if is_debug_enabled() {
                    bridge_log!("[Bridge] wl_surface.frame on surface {}", surface_id);
                }
                if payload.len() >= 4 {
                    let callback_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    if is_debug_enabled() {
                        bridge_log!("[Bridge] Callback ID: {}", callback_id);
                    }
                    self.add_object(callback_id, String::from("wl_callback"));

                    // Store the callback to be sent when the surface is committed
                    if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                        surface.set_pending_callback(callback_id);
                    }
                }
                Ok(Vec::new())
            }
            protocol::surface_request::SET_OPAQUE_REGION => {
                if payload.len() >= 4 {
                    let region_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    if is_debug_enabled() {
                        bridge_log!(
                            "[Bridge] wl_surface.set_opaque_region: surface={} region={}",
                            surface_id,
                            region_id
                        );
                    }
                    let region_opt = if region_id == 0 {
                        None
                    } else {
                        Some(region_id)
                    };
                    self.surface_manager
                        .set_opaque_region(surface_id, region_opt);
                }
                Ok(Vec::new())
            }
            protocol::surface_request::SET_INPUT_REGION => {
                if payload.len() >= 4 {
                    let region_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    if is_debug_enabled() {
                        bridge_log!(
                            "[Bridge] wl_surface.set_input_region: surface={} region={}",
                            surface_id,
                            region_id
                        );
                    }
                    let region_opt = if region_id == 0 {
                        None
                    } else {
                        Some(region_id)
                    };
                    self.surface_manager
                        .set_input_region(surface_id, region_opt);
                }
                Ok(Vec::new())
            }
            protocol::surface_request::SET_BUFFER_SCALE => {
                if payload.len() >= 4 {
                    let scale =
                        i32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    bridge_log!(
                        "[Bridge] wl_surface.set_buffer_scale: surface={} scale={}",
                        surface_id,
                        scale
                    );
                    if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                        surface.set_buffer_scale(scale);
                    }
                }
                Ok(Vec::new())
            }
            protocol::surface_request::SET_BUFFER_TRANSFORM => {
                if payload.len() >= 4 {
                    let transform =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    bridge_log!(
                        "[Bridge] wl_surface.set_buffer_transform: surface={} transform={}",
                        surface_id,
                        transform
                    );
                    if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                        surface.set_buffer_transform(transform as i32);
                    }
                }
                Ok(Vec::new())
            }
            _ => {
                bridge_log!("[Bridge] Unknown wl_surface opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Handle wl_shm messages
    fn handle_shm_message(
        &mut self,
        opcode: u16,
        payload: &[u8],
        attached_handle: Option<Handle>,
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            shm::shm_request::CREATE_POOL => {
                bridge_log!("[Bridge] wl_shm.create_pool");
                // Payload: new_id (u32) + size (i32) = 8 bytes
                // SCM_RIGHTS descriptors are matched to FD-bearing requests in
                // protocol order, even when one sendmsg batches several requests.
                if payload.len() < 8 {
                    return Err("Invalid wl_shm.create_pool payload");
                }
                let pool_id = Self::parse_u32(payload, 0).unwrap_or(0);
                let size = Self::parse_i32(payload, 4).unwrap_or(0);
                let handle = attached_handle.ok_or("Missing wl_shm.create_pool handle")?;
                if size <= 0 {
                    return Err("wl_shm.create_pool requires a positive size");
                }
                let sws_pool_id = self.allocate_extension_resource_id();
                self.shm_pool_count = self.shm_pool_count.saturating_add(1);
                bridge_log!("[Bridge] Created pool ID: {} size: {}", pool_id, size);
                bridge_log!("[Bridge] Received SHM handle for pool {}", pool_id);
                self.register_extension_shm_pool(sws_pool_id, size as usize, &handle)?;
                if let Err(error) =
                    self.shm_manager
                        .create_pool(pool_id, sws_pool_id, Some(handle), size)
                {
                    let _ = self.destroy_extension_shm_pool(sws_pool_id);
                    return Err(error);
                }
                self.add_object(pool_id, String::from("wl_shm_pool"));
                Ok(Vec::new())
            }
            _ => {
                if attached_handle.is_some() {
                    return Err("Unexpected handle attached to wl_shm request");
                }
                bridge_log!("[Bridge] Unknown wl_shm opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Handle wl_shm_pool messages
    fn handle_shm_pool_message(
        &mut self,
        pool_id: u32,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            shm::shm_pool_request::CREATE_BUFFER => {
                if payload.len() >= 24 {
                    let buffer_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let offset =
                        i32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
                    let width =
                        i32::from_ne_bytes([payload[8], payload[9], payload[10], payload[11]]);
                    let height =
                        i32::from_ne_bytes([payload[12], payload[13], payload[14], payload[15]]);
                    let stride =
                        i32::from_ne_bytes([payload[16], payload[17], payload[18], payload[19]]);
                    let format =
                        u32::from_ne_bytes([payload[20], payload[21], payload[22], payload[23]]);

                    bridge_log!(
                        "[Bridge] Buffer: {}x{} stride:{} format:{}",
                        width,
                        height,
                        stride,
                        format
                    );
                    let sws_buffer_id = self.allocate_extension_resource_id();
                    self.shm_manager.create_buffer(
                        buffer_id,
                        sws_buffer_id,
                        pool_id,
                        offset,
                        width,
                        height,
                        stride,
                        format,
                    )?;
                    self.define_extension_shm_buffer(buffer_id)?;
                    self.add_object(buffer_id, String::from("wl_buffer"));
                    self.shm_buffer_count = self.shm_buffer_count.saturating_add(1);
                    if should_log_resource_count(self.shm_buffer_count) {
                        bridge_info!(
                            "[wayland-bridge] client={} created wl_buffer={} buffers={} pool={} size={}x{} stride={} format={}",
                            self.client_id,
                            buffer_id,
                            self.shm_buffer_count,
                            pool_id,
                            width,
                            height,
                            stride,
                            format
                        );
                    }
                }
                Ok(Vec::new())
            }
            shm::shm_pool_request::DESTROY => {
                bridge_log!("[Bridge] wl_shm_pool.destroy");
                let sws_pool_id = self
                    .shm_manager
                    .get_pool(pool_id)
                    .ok_or("Wayland SHM pool not found")?
                    .sws_pool_id;
                self.destroy_extension_shm_pool(sws_pool_id)?;
                self.shm_manager.destroy_pool(pool_id);
                self.remove_object(pool_id);
                Ok(Vec::new())
            }
            shm::shm_pool_request::RESIZE => {
                bridge_log!("[Bridge] wl_shm_pool.resize");
                if payload.len() >= 4 {
                    let new_size =
                        i32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    self.shm_manager.resize_pool(pool_id, new_size)?;
                    let (sws_pool_id, pool_size) = self
                        .shm_manager
                        .get_pool(pool_id)
                        .map(|pool| (pool.sws_pool_id, pool.size))
                        .ok_or("Wayland SHM pool disappeared during resize")?;
                    self.resize_extension_shm_pool(sws_pool_id, pool_size)?;
                }
                Ok(Vec::new())
            }
            _ => {
                bridge_log!("[Bridge] Unknown wl_shm_pool opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Handle wl_buffer messages
    fn handle_buffer_message(
        &mut self,
        buffer_id: u32,
        opcode: u16,
        _payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            shm::buffer_request::DESTROY => {
                bridge_log!("[Bridge] wl_buffer.destroy");
                let sws_buffer_id = self
                    .shm_manager
                    .get_buffer(buffer_id)
                    .ok_or("Wayland SHM buffer not found")?
                    .sws_buffer_id;
                // A client may destroy the protocol object immediately after
                // committing it. Publish any coalesced use before retiring the
                // reusable SWS resource so the compositor observes the same
                // commit-before-destroy ordering as the Wayland stream.
                self.flush_pending_surface_commits_using_buffer(sws_buffer_id)?;
                self.destroy_extension_buffer(sws_buffer_id)?;
                self.shm_manager.destroy_buffer(buffer_id);
                self.remove_object(buffer_id);
                Ok(Vec::new())
            }
            _ => {
                bridge_log!("[Bridge] Unknown wl_buffer opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Handle xdg_wm_base messages
    fn handle_xdg_wm_base_message(
        &mut self,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            xdg_shell::wm_base_request::GET_XDG_SURFACE => {
                bridge_log!("[Bridge] xdg_wm_base.get_xdg_surface");
                if payload.len() >= 8 {
                    let xdg_surface_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let wl_surface_id =
                        u32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
                    bridge_log!(
                        "[Bridge] XDG surface ID: {} for wl_surface: {}",
                        xdg_surface_id,
                        wl_surface_id
                    );
                    self.objects
                        .insert(xdg_surface_id, String::from("xdg_surface"));
                    self.xdg_shell_manager
                        .create_xdg_surface(xdg_surface_id, wl_surface_id);
                }
                Ok(Vec::new())
            }
            xdg_shell::wm_base_request::PONG => {
                bridge_log!("[Bridge] xdg_wm_base.pong");
                Ok(Vec::new())
            }
            _ => {
                bridge_log!("[Bridge] Unknown xdg_wm_base opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Handle xdg_surface messages
    fn handle_xdg_surface_message(
        &mut self,
        xdg_surface_id: u32,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            xdg_shell::xdg_surface_request::DESTROY => {
                bridge_log!("[Bridge] xdg_surface.destroy");
                self.xdg_shell_manager.destroy_xdg_surface(xdg_surface_id);
                self.objects
                    .insert(xdg_surface_id, String::from("xdg_surface_dead"));
                Ok(Vec::new())
            }
            xdg_shell::xdg_surface_request::GET_TOPLEVEL => {
                bridge_log!("[Bridge] xdg_surface.get_toplevel");
                if payload.len() >= 4 {
                    let xdg_toplevel_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    bridge_log!("[Bridge] XDG toplevel ID: {}", xdg_toplevel_id);
                    self.objects
                        .insert(xdg_toplevel_id, String::from("xdg_toplevel"));
                    self.xdg_shell_manager
                        .create_toplevel(xdg_surface_id, xdg_toplevel_id)?;

                    // Set surface role to toplevel
                    if let Some(xdg_surface) =
                        self.xdg_shell_manager.get_xdg_surface(xdg_surface_id)
                    {
                        let wl_surface_id = xdg_surface.wl_surface_id;
                        bridge_info!(
                            "[wayland-bridge] client={} xdg_toplevel={} surface={}",
                            self.client_id,
                            xdg_toplevel_id,
                            wl_surface_id
                        );
                        if let Some(surface) = self.surface_manager.get_surface_mut(wl_surface_id) {
                            surface.set_role(surface::SurfaceRole::XdgToplevel);
                        }
                    }
                    return Ok(Vec::new());
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_surface_request::GET_POPUP => {
                bridge_warn!(
                    "[wayland-bridge] client={} requested unsupported xdg_popup on xdg_surface={}",
                    self.client_id,
                    xdg_surface_id
                );
                Ok(Vec::new())
            }
            xdg_shell::xdg_surface_request::SET_WINDOW_GEOMETRY => {
                bridge_log!("[Bridge] xdg_surface.set_window_geometry");
                Ok(Vec::new())
            }
            xdg_shell::xdg_surface_request::ACK_CONFIGURE => {
                bridge_log!("[Bridge] xdg_surface.ack_configure");
                if payload.len() >= 4 {
                    let serial =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    if let Some(surface) =
                        self.xdg_shell_manager.get_xdg_surface_mut(xdg_surface_id)
                    {
                        surface.last_ack_serial = Some(serial);
                    }
                }
                Ok(Vec::new())
            }
            _ => {
                bridge_log!("[Bridge] Unknown xdg_surface opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Handle xdg_toplevel messages
    fn handle_xdg_toplevel_message(
        &mut self,
        xdg_toplevel_id: u32,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            xdg_shell::xdg_toplevel_request::DESTROY => {
                bridge_log!("[Bridge] xdg_toplevel.destroy");
                if let Some(wl_surface_id) = self.xdg_shell_manager.clear_toplevel(xdg_toplevel_id)
                    && let Some(surface) = self.surface_manager.get_surface_mut(wl_surface_id)
                {
                    surface.role = None;
                }
                self.objects
                    .insert(xdg_toplevel_id, String::from("xdg_toplevel_dead"));
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::SET_PARENT => {
                bridge_log!("[Bridge] xdg_toplevel.set_parent");
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::SET_TITLE => {
                bridge_log!("[Bridge] xdg_toplevel.set_title");
                if let Some((title, _)) = Self::parse_string(payload, 0)
                    && let Some((toplevel, _)) =
                        self.xdg_shell_manager.get_toplevel_mut(xdg_toplevel_id)
                {
                    toplevel.title = Some(title);
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::SET_APP_ID => {
                bridge_log!("[Bridge] xdg_toplevel.set_app_id");
                if let Some((app_id, _)) = Self::parse_string(payload, 0)
                    && let Some((toplevel, _)) =
                        self.xdg_shell_manager.get_toplevel_mut(xdg_toplevel_id)
                {
                    toplevel.app_id = Some(app_id);
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::SET_MAX_SIZE => {
                bridge_log!("[Bridge] xdg_toplevel.set_max_size");
                let width = Self::parse_i32(payload, 0).unwrap_or(0);
                let height = Self::parse_i32(payload, 4).unwrap_or(0);
                if let Some((toplevel, _)) =
                    self.xdg_shell_manager.get_toplevel_mut(xdg_toplevel_id)
                {
                    toplevel.max_size = Some((width, height));
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::SET_MIN_SIZE => {
                bridge_log!("[Bridge] xdg_toplevel.set_min_size");
                let width = Self::parse_i32(payload, 0).unwrap_or(0);
                let height = Self::parse_i32(payload, 4).unwrap_or(0);
                if let Some((toplevel, _)) =
                    self.xdg_shell_manager.get_toplevel_mut(xdg_toplevel_id)
                {
                    toplevel.min_size = Some((width, height));
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::MOVE => {
                let serial = Self::parse_u32(payload, 4).unwrap_or(0);
                bridge_log!(
                    "[Bridge] xdg_toplevel.move: toplevel={} serial={}",
                    xdg_toplevel_id,
                    serial
                );
                let window_id = self.window_id_for_toplevel(xdg_toplevel_id);
                if let Some(window_id) = window_id {
                    bridge_log!("[Bridge] xdg_toplevel.move mapped to window {}", window_id);
                    let _ = self.send_request_move_window(window_id);
                } else {
                    bridge_log!(
                        "[Bridge] xdg_toplevel.move missing window mapping for toplevel {}",
                        xdg_toplevel_id
                    );
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::RESIZE => {
                bridge_log!("[Bridge] xdg_toplevel.resize");
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::SHOW_WINDOW_MENU => {
                bridge_log!("[Bridge] xdg_toplevel.show_window_menu");
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::SET_MAXIMIZED => {
                bridge_log!("[Bridge] xdg_toplevel.set_maximized");
                let wl_surface_id = self
                    .xdg_shell_manager
                    .get_toplevel_mut(xdg_toplevel_id)
                    .map(|(toplevel, wl_surface_id)| {
                        toplevel.maximized = true;
                        wl_surface_id
                    });
                if let Some(window_id) = wl_surface_id
                    .and_then(|surface_id| self.surface_to_window.get(&surface_id).copied())
                {
                    let _ = self.send_maximize_window(window_id);
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::UNSET_MAXIMIZED => {
                bridge_log!("[Bridge] xdg_toplevel.unset_maximized");
                let wl_surface_id = self
                    .xdg_shell_manager
                    .get_toplevel_mut(xdg_toplevel_id)
                    .map(|(toplevel, wl_surface_id)| {
                        toplevel.maximized = false;
                        wl_surface_id
                    });
                if let Some(window_id) = wl_surface_id
                    .and_then(|surface_id| self.surface_to_window.get(&surface_id).copied())
                {
                    let _ = self.send_restore_window(window_id);
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::SET_FULLSCREEN => {
                bridge_log!("[Bridge] xdg_toplevel.set_fullscreen");
                let wl_surface_id = self
                    .xdg_shell_manager
                    .get_toplevel_mut(xdg_toplevel_id)
                    .map(|(toplevel, wl_surface_id)| {
                        toplevel.fullscreen = true;
                        wl_surface_id
                    });
                if let Some(window_id) = wl_surface_id
                    .and_then(|surface_id| self.surface_to_window.get(&surface_id).copied())
                {
                    let _ = self.send_set_fullscreen(window_id);
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::UNSET_FULLSCREEN => {
                bridge_log!("[Bridge] xdg_toplevel.unset_fullscreen");
                let state = self
                    .xdg_shell_manager
                    .get_toplevel_mut(xdg_toplevel_id)
                    .map(|(toplevel, wl_surface_id)| {
                        toplevel.fullscreen = false;
                        (wl_surface_id, toplevel.maximized)
                    });
                if let Some((window_id, restore_maximized)) =
                    state.and_then(|(surface_id, maximized)| {
                        self.surface_to_window
                            .get(&surface_id)
                            .copied()
                            .map(|window_id| (window_id, maximized))
                    })
                {
                    let _ = self.send_unset_fullscreen(window_id);
                    if restore_maximized {
                        let _ = self.send_maximize_window(window_id);
                    } else {
                        let _ = self.send_restore_window(window_id);
                    }
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::SET_MINIMIZED => {
                bridge_log!("[Bridge] xdg_toplevel.set_minimized");
                if let Some(window_id) = self.window_id_for_toplevel(xdg_toplevel_id) {
                    let _ = self.send_minimize_window(window_id);
                }
                Ok(Vec::new())
            }
            _ => {
                bridge_log!("[Bridge] Unknown xdg_toplevel opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Handle wl_seat messages
    fn handle_seat_message(
        &mut self,
        seat_id: u32,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            input::seat_request::GET_POINTER => {
                bridge_log!("[Bridge] wl_seat.get_pointer");
                if payload.len() >= 4 {
                    let pointer_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    bridge_log!("[Bridge] Pointer ID: {}", pointer_id);
                    self.add_object(pointer_id, String::from("wl_pointer"));
                    self.input_manager.create_pointer(pointer_id, seat_id);
                    bridge_info!(
                        "[wayland-bridge] client={} created wl_pointer={} seat={}",
                        self.client_id,
                        pointer_id,
                        seat_id
                    );

                    // Store pointer as focused
                    self.focused_pointer = Some(pointer_id);

                    let mut msgs = Vec::new();

                    // If there's a focused surface, send enter event
                    if let Some(surface_id) = self.focused_surface {
                        let serial = self.allocate_serial();
                        let mut enter_msg =
                            WaylandMessage::new(pointer_id, input::pointer_event::ENTER);
                        enter_msg.add_arg(WaylandArg::Uint(serial));
                        enter_msg.add_arg(WaylandArg::Object(surface_id));
                        enter_msg.add_arg(WaylandArg::Fixed(0)); // surface_x
                        enter_msg.add_arg(WaylandArg::Fixed(0)); // surface_y
                        msgs.push(enter_msg);
                    }

                    return Ok(msgs);
                }
                Ok(Vec::new())
            }
            input::seat_request::GET_KEYBOARD => {
                bridge_log!("[Bridge] wl_seat.get_keyboard");
                if payload.len() >= 4 {
                    let keyboard_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    bridge_log!("[Bridge] Keyboard ID: {}", keyboard_id);
                    self.add_object(keyboard_id, String::from("wl_keyboard"));
                    let keyboard_version = self.object_versions.get(&seat_id).copied().unwrap_or(1);
                    self.object_versions.insert(keyboard_id, keyboard_version);
                    self.input_manager.create_keyboard(keyboard_id, seat_id);
                    bridge_info!(
                        "[wayland-bridge] client={} created wl_keyboard={} seat={} version={}",
                        self.client_id,
                        keyboard_id,
                        seat_id,
                        keyboard_version
                    );

                    // Store keyboard as focused
                    self.focused_keyboard = Some(keyboard_id);

                    // Send keymap event
                    let size = self.ensure_keymap()?;
                    let mut keymap_msg =
                        WaylandMessage::new(keyboard_id, input::keyboard_event::KEYMAP);
                    keymap_msg.add_arg(WaylandArg::Uint(1)); // XKB_V1 format
                    keymap_msg.add_arg(WaylandArg::FdPlaceholder); // SCM_RIGHTS only; no wire bytes
                    keymap_msg.add_arg(WaylandArg::Uint(size)); // size

                    let mut msgs = Vec::new();
                    msgs.push(keymap_msg);

                    // wl_keyboard v4+ clients own key-repeat timing after the
                    // compositor advertises repeat_info. SWS therefore sends
                    // only the physical press/release pair to this bridge.
                    if keyboard_version >= 4 {
                        let mut repeat_info =
                            WaylandMessage::new(keyboard_id, input::keyboard_event::REPEAT_INFO);
                        repeat_info.add_arg(WaylandArg::Int(20));
                        repeat_info.add_arg(WaylandArg::Int(500));
                        msgs.push(repeat_info);
                    }

                    // If there's a focused surface, send enter and modifiers events
                    if let Some(surface_id) = self.focused_surface {
                        let serial = self.allocate_serial();
                        let mut enter_msg =
                            WaylandMessage::new(keyboard_id, input::keyboard_event::ENTER);
                        enter_msg.add_arg(WaylandArg::Uint(serial));
                        enter_msg.add_arg(WaylandArg::Object(surface_id));
                        enter_msg.add_arg(WaylandArg::Array(Vec::new())); // keys array
                        msgs.push(enter_msg);

                        // Send modifiers event
                        let mut modifiers_msg =
                            WaylandMessage::new(keyboard_id, input::keyboard_event::MODIFIERS);
                        modifiers_msg.add_arg(WaylandArg::Uint(serial));
                        modifiers_msg.add_arg(WaylandArg::Uint(0)); // mods_depressed
                        modifiers_msg.add_arg(WaylandArg::Uint(0)); // mods_latched
                        modifiers_msg.add_arg(WaylandArg::Uint(0)); // mods_locked
                        modifiers_msg.add_arg(WaylandArg::Uint(0)); // group
                        msgs.push(modifiers_msg);
                    }

                    return Ok(msgs);
                }
                Ok(Vec::new())
            }
            input::seat_request::RELEASE => {
                bridge_log!("[Bridge] wl_seat.release");
                Ok(Vec::new())
            }
            _ => {
                bridge_log!("[Bridge] Unknown wl_seat opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Handle wl_pointer messages
    fn handle_pointer_message(
        &mut self,
        _pointer_id: u32,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        bridge_log!("[Bridge] wl_pointer opcode: {}", opcode);
        match opcode {
            0 => {
                // wl_pointer.set_cursor(serial, surface, hotspot_x, hotspot_y)
                let serial = Self::parse_u32(payload, 0).unwrap_or(0);
                let surface_id = Self::parse_u32(payload, 4).unwrap_or(0);
                let hotspot_x = Self::parse_i32(payload, 8).unwrap_or(0);
                let hotspot_y = Self::parse_i32(payload, 12).unwrap_or(0);
                bridge_log!(
                    "[Bridge] wl_pointer.set_cursor serial={} surface={} hotspot=({}, {})",
                    serial,
                    surface_id,
                    hotspot_x,
                    hotspot_y
                );
                if surface_id == 0 {
                    self.cursor_surface_id = None;
                } else {
                    if let Some(prev) = self.cursor_surface_id
                        && prev != surface_id
                        && let Some(surface) = self.surface_manager.get_surface_mut(prev)
                        && surface.role == Some(surface::SurfaceRole::Cursor)
                    {
                        surface.role = None;
                    }
                    self.cursor_surface_id = Some(surface_id);
                    if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                        surface.set_role(surface::SurfaceRole::Cursor);
                    }
                }
                Ok(Vec::new())
            }
            _ => {
                // Pointer events are sent from SWS, not received from client
                Ok(Vec::new())
            }
        }
    }

    /// Handle wl_keyboard messages
    fn handle_keyboard_message(
        &mut self,
        _keyboard_id: u32,
        opcode: u16,
        _payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        bridge_log!("[Bridge] wl_keyboard opcode: {}", opcode);
        // Keyboard events are sent from SWS, not received from client
        Ok(Vec::new())
    }

    /// Handle wl_data_device_manager messages.
    ///
    /// GTK binds this global even when the application does not actively use
    /// clipboard or drag-and-drop.  Scarlet does not provide selection data yet,
    /// but registering the requested objects keeps later no-op requests from
    /// being dropped as unknown object IDs.
    fn handle_data_device_manager_message(
        &mut self,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            protocol::data_device_manager_request::CREATE_DATA_SOURCE => {
                bridge_log!("[Bridge] wl_data_device_manager.create_data_source");
                if let Some(source_id) = Self::parse_u32(payload, 0) {
                    self.add_object(source_id, String::from("wl_data_source"));
                }
                Ok(Vec::new())
            }
            protocol::data_device_manager_request::GET_DATA_DEVICE => {
                bridge_log!("[Bridge] wl_data_device_manager.get_data_device");
                if let Some(device_id) = Self::parse_u32(payload, 0) {
                    self.add_object(device_id, String::from("wl_data_device"));
                }
                Ok(Vec::new())
            }
            _ => {
                bridge_log!("[Bridge] Unknown wl_data_device_manager opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    fn handle_data_device_message(
        &mut self,
        data_device_id: u32,
        opcode: u16,
        _payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            protocol::data_device_request::RELEASE => {
                bridge_log!("[Bridge] wl_data_device.release");
                self.objects.remove(&data_device_id);
            }
            protocol::data_device_request::START_DRAG => {
                bridge_log!("[Bridge] wl_data_device.start_drag (ignored)");
            }
            protocol::data_device_request::SET_SELECTION => {
                bridge_log!("[Bridge] wl_data_device.set_selection (ignored)");
            }
            _ => bridge_log!("[Bridge] Unknown wl_data_device opcode: {}", opcode),
        }
        Ok(Vec::new())
    }

    fn handle_data_source_message(
        &mut self,
        data_source_id: u32,
        opcode: u16,
        _payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            protocol::data_source_request::DESTROY => {
                bridge_log!("[Bridge] wl_data_source.destroy");
                self.objects.remove(&data_source_id);
            }
            protocol::data_source_request::OFFER => {
                bridge_log!("[Bridge] wl_data_source.offer (ignored)");
            }
            protocol::data_source_request::SET_ACTIONS => {
                bridge_log!("[Bridge] wl_data_source.set_actions (ignored)");
            }
            _ => bridge_log!("[Bridge] Unknown wl_data_source opcode: {}", opcode),
        }
        Ok(Vec::new())
    }

    /// Handle wl_output messages
    fn handle_output_message(
        &mut self,
        _output_id: u32,
        opcode: u16,
        _payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        bridge_log!("[Bridge] wl_output opcode: {}", opcode);
        Ok(Vec::new())
    }

    /// Handle wl_region messages
    fn handle_region_message(
        &mut self,
        region_id: u32,
        opcode: u16,
        payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            0 => {
                // wl_region.destroy
                bridge_log!("[Bridge] wl_region.destroy: {}", region_id);
                self.region_manager.destroy_region(region_id);
                self.objects.remove(&region_id);
                Ok(Vec::new())
            }
            1 => {
                // wl_region.add
                if payload.len() >= 16 {
                    let x = i32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let y = i32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
                    let width =
                        i32::from_ne_bytes([payload[8], payload[9], payload[10], payload[11]]);
                    let height =
                        i32::from_ne_bytes([payload[12], payload[13], payload[14], payload[15]]);
                    if is_debug_enabled() {
                        bridge_log!(
                            "[Bridge] wl_region.add: region={} x={} y={} w={} h={}",
                            region_id,
                            x,
                            y,
                            width,
                            height
                        );
                    }
                    self.region_manager
                        .add_to_region(region_id, x, y, width, height);
                }
                Ok(Vec::new())
            }
            2 => {
                // wl_region.subtract
                if payload.len() >= 16 {
                    let x = i32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let y = i32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
                    let width =
                        i32::from_ne_bytes([payload[8], payload[9], payload[10], payload[11]]);
                    let height =
                        i32::from_ne_bytes([payload[12], payload[13], payload[14], payload[15]]);
                    if is_debug_enabled() {
                        bridge_log!(
                            "[Bridge] wl_region.subtract: region={} x={} y={} w={} h={}",
                            region_id,
                            x,
                            y,
                            width,
                            height
                        );
                    }
                    self.region_manager
                        .subtract_from_region(region_id, x, y, width, height);
                }
                Ok(Vec::new())
            }
            _ => {
                bridge_log!("[Bridge] Unknown wl_region opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Process input events from SWS and forward to Wayland clients
    /// This should be called periodically or in a separate thread
    /// NOTE: This is now a no-op placeholder - input event forwarding is disabled
    fn process_sws_input_events(&mut self) -> Result<Vec<WaylandMessage>, &'static str> {
        // Disabled for now - was causing blocking issues
        // TODO: Implement proper non-blocking I/O or use poll/epoll
        Ok(Vec::new())
    }

    /// Spawn a background thread to listen for SWS input events
    fn spawn_input_thread(&self) -> Result<(), &'static str> {
        let input_queue = self.input_event_queue.clone();
        let objects_clone = self.objects_for_input_thread.clone();
        let pointer_pos = self.pointer_position_for_thread.clone();

        bridge_log!("[Bridge] Spawning input event thread...");

        thread::spawn(move || {
            bridge_log!("[Input Thread] Started, connecting to SWS...");

            // Serial counter for input events (must be non-zero for GTK)
            let mut next_serial: u32 = 1;

            // Create separate connection to SWS for input events
            let sws_socket = match Socket::new() {
                Ok(s) => s,
                Err(_) => {
                    bridge_log!("[Input Thread] Failed to create socket");
                    return;
                }
            };

            if sws_socket.connect("/tmp/sws.sock").is_err() {
                bridge_log!("[Input Thread] Failed to connect to SWS");
                return;
            }

            bridge_log!("[Input Thread] Connected to SWS, listening for input events");

            let mut sws = sws_socket;
            let mut buf = [0u8; 1024];
            let mut current_x: i32 = 0;
            let mut current_y: i32 = 0;

            loop {
                match sws.read(&mut buf) {
                    Ok(n) if n >= 8 => {
                        let mut header_bytes = [0u8; 8];
                        header_bytes.copy_from_slice(&buf[0..8]);
                        let header = protocol_sws::MessageHeader::from_le_bytes(header_bytes);

                        if header.msg_type_u32() == protocol_sws::server_msg::EXTENSION_INPUT_EVENT
                            && n >= 32
                        {
                            // Parse EXTENSION_INPUT_EVENT
                            let _external_client_id =
                                u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
                            let _window_id =
                                u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
                            let time = u64::from_le_bytes([
                                buf[16], buf[17], buf[18], buf[19], buf[20], buf[21], buf[22],
                                buf[23],
                            ]);
                            let type_ = u16::from_le_bytes([buf[24], buf[25]]);
                            let code = u16::from_le_bytes([buf[26], buf[27]]);
                            let value = i32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]);

                            bridge_log!(
                                "[Input Thread] Received EXTENSION_INPUT_EVENT: type={} code={} value={}",
                                type_,
                                code,
                                value
                            );

                            // Get current objects list
                            let objects_guard = objects_clone.lock();
                            let mut messages = Vec::new();

                            // Convert SWS event to Wayland input events
                            // Type 1 = keyboard (EV_KEY), Type 3 = mouse absolute (EV_ABS)
                            if type_ == 1 {
                                // Keyboard event - forward to all wl_keyboard objects
                                for (id, interface) in objects_guard.iter() {
                                    if interface == "wl_keyboard" {
                                        let mut msg =
                                            WaylandMessage::new(*id, input::keyboard_event::KEY);
                                        msg.add_arg(WaylandArg::Uint(next_serial));
                                        next_serial = next_serial.wrapping_add(1); // serial
                                        msg.add_arg(WaylandArg::Uint(time as u32));
                                        msg.add_arg(WaylandArg::Uint(code as u32));
                                        msg.add_arg(WaylandArg::Uint(value as u32)); // state
                                        messages.push(msg);
                                        bridge_log!(
                                            "[Input Thread] Queued keyboard key event: id={}, code={}, state={}",
                                            id,
                                            code,
                                            value
                                        );
                                    }
                                }
                            } else if type_ == 3 {
                                // EV_ABS - absolute position or button
                                if code == 0 {
                                    // ABS_X
                                    current_x = value;
                                    bridge_log!("[Input Thread] Updated X position: {}", current_x);
                                } else if code == 1 {
                                    // ABS_Y
                                    current_y = value;
                                    bridge_log!("[Input Thread] Updated Y position: {}", current_y);
                                } else if (0x100..=0x104).contains(&code) {
                                    // Mouse buttons (BTN_LEFT, BTN_RIGHT, etc.)
                                    for (id, interface) in objects_guard.iter() {
                                        if interface == "wl_pointer" {
                                            // Update shared pointer position
                                            {
                                                let mut pos = pointer_pos.lock();
                                                pos.0 = current_x;
                                                pos.1 = current_y;
                                            }

                                            // Send motion event first (required before button)
                                            let mut motion_msg = WaylandMessage::new(
                                                *id,
                                                input::pointer_event::MOTION,
                                            );
                                            motion_msg.add_arg(WaylandArg::Uint(time as u32));
                                            motion_msg.add_arg(WaylandArg::Fixed(
                                                (current_x as f64 * 256.0) as i32,
                                            )); // wl_fixed_t
                                            motion_msg.add_arg(WaylandArg::Fixed(
                                                (current_y as f64 * 256.0) as i32,
                                            ));
                                            messages.push(motion_msg);

                                            // Send button event
                                            let button_code = code - 0x100 + 272; // Convert to Linux input button code
                                            let mut msg = WaylandMessage::new(
                                                *id,
                                                input::pointer_event::BUTTON,
                                            );
                                            msg.add_arg(WaylandArg::Uint(next_serial));
                                            next_serial = next_serial.wrapping_add(1); // serial
                                            msg.add_arg(WaylandArg::Uint(time as u32));
                                            msg.add_arg(WaylandArg::Uint(button_code as u32));
                                            msg.add_arg(WaylandArg::Uint(value as u32)); // state (0=up, 1=down)
                                            messages.push(msg);

                                            // End pointer event frame
                                            let frame_msg = WaylandMessage::new(
                                                *id,
                                                input::pointer_event::FRAME,
                                            );
                                            messages.push(frame_msg);
                                            bridge_log!(
                                                "[Input Thread] Queued pointer button event: id={}, button={}, state={}, x={}, y={}",
                                                id,
                                                button_code,
                                                value,
                                                current_x,
                                                current_y
                                            );
                                        }
                                    }
                                }
                            }

                            // Add messages to shared queue
                            let msg_count = messages.len();
                            if msg_count > 0 {
                                let mut queue = input_queue.lock();
                                queue.extend(messages);
                                bridge_log!("[Input Thread] Added {} messages to queue", msg_count);
                            }
                        }
                    }
                    Ok(_) => {
                        // No data or incomplete message
                        thread::sleep(Duration::from_millis(16));
                    }
                    Err(_) => {
                        // Error - wait before retrying
                        thread::sleep(Duration::from_millis(16));
                    }
                }
            }
        });

        Ok(())
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    bridge_info!("[wayland-bridge] starting");

    let socket_path = "/tmp/wayland-0";

    let server_socket = match create_server_socket(socket_path) {
        Ok(sock) => sock,
        Err(e) => {
            bridge_error!("[wayland-bridge] failed to initialize: {}", e);
            return 1;
        }
    };

    bridge_info!("[wayland-bridge] listening on {}", socket_path);

    let use_input_thread = env::var("WAYLAND_BRIDGE_INPUT_THREAD")
        .map(|val| val == "1")
        .unwrap_or(false);
    if !use_input_thread {
        bridge_log!("[Bridge] Input thread disabled (set WAYLAND_BRIDGE_INPUT_THREAD=1 to enable)");
    }

    loop {
        match server_socket.accept() {
            Ok(client) => {
                bridge_log!("[Bridge] Accepted connection");
                let enable_input = use_input_thread;
                static NEXT_CLIENT_ID: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(1);
                let client_id = NEXT_CLIENT_ID
                    .fetch_add(1, core::sync::atomic::Ordering::Relaxed)
                    .max(1);
                thread::spawn(move || {
                    let mut bridge = match WaylandBridge::new_client(client_id) {
                        Ok(b) => b,
                        Err(e) => {
                            bridge_error!(
                                "[wayland-bridge] client={} state initialization failed: {}",
                                client_id,
                                e
                            );
                            return;
                        }
                    };

                    if let Err(e) = bridge.connect_to_sws() {
                        bridge_error!(
                            "[wayland-bridge] client={} failed to connect to SWS: {}",
                            client_id,
                            e
                        );
                        return;
                    }

                    if enable_input && let Err(e) = bridge.spawn_input_thread() {
                        bridge_error!(
                            "[wayland-bridge] client={} failed to spawn input thread: {}",
                            client_id,
                            e
                        );
                        return;
                    }

                    if let Err(e) = bridge.handle_client(client) {
                        bridge_error!(
                            "[wayland-bridge] client={} worker terminated windows={}: {}",
                            client_id,
                            bridge.surface_to_window.len(),
                            e
                        );
                    }
                });
            }
            Err(e) => {
                bridge_warn!("[wayland-bridge] accept failed: {:?}", e);
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PendingSurfaceCommit, WaylandBridge, locally_releasable_buffer, shm, take_message_handle,
    };

    #[test]
    fn empty_wayland_damage_does_not_become_a_full_surface_upload() {
        assert!(WaylandBridge::compute_damage_rects(&[], 1920, 1080).is_empty());
    }

    #[test]
    fn pending_surface_commits_keep_latest_buffer_and_union_damage() {
        let mut bridge = WaylandBridge::new_client(1).expect("bridge state should initialize");
        bridge.submitted_surface_buffers.insert(7, Some(100));

        assert_eq!(
            bridge.coalesce_pending_surface_commit(PendingSurfaceCommit {
                surface_id: 7,
                window_id: 70,
                wayland_buffer_id: Some(20),
                sws_buffer_id: Some(200),
                damage_rects: std::vec![(10, 10, 20, 20)],
            }),
            None
        );
        assert_eq!(
            bridge.coalesce_pending_surface_commit(PendingSurfaceCommit {
                surface_id: 7,
                window_id: 70,
                wayland_buffer_id: Some(30),
                sws_buffer_id: Some(300),
                damage_rects: std::vec![(25, 25, 20, 20)],
            }),
            Some(20)
        );

        let pending = bridge
            .pending_surface_commits
            .get(&7)
            .expect("coalesced commit should remain queued");
        assert_eq!(pending.sws_buffer_id, Some(300));
        assert_eq!(pending.wayland_buffer_id, Some(30));
        assert_eq!(pending.damage_rects, std::vec![(10, 10, 35, 35)]);
    }

    #[test]
    fn only_attached_buffers_outside_the_sws_scene_release_locally() {
        assert_eq!(locally_releasable_buffer(false, true, Some(17)), Some(17));
        assert_eq!(locally_releasable_buffer(false, false, Some(17)), None);
        assert_eq!(locally_releasable_buffer(false, true, None), None);
        assert_eq!(locally_releasable_buffer(true, true, Some(17)), None);
    }

    #[test]
    fn batched_handle_waits_for_wl_shm_create_pool() {
        let mut handles = std::vec![41u32];

        assert_eq!(
            take_message_handle(Some("xdg_surface"), 4, &mut handles),
            None
        );
        assert_eq!(handles.len(), 1);
        assert_eq!(
            take_message_handle(Some("wl_shm"), shm::shm_request::CREATE_POOL, &mut handles,),
            Some(41)
        );
        assert!(handles.is_empty());
    }

    #[test]
    fn multiple_handles_are_consumed_in_ancillary_order() {
        let mut handles = std::vec![7u32, 9u32];

        assert_eq!(
            take_message_handle(Some("wl_shm"), shm::shm_request::CREATE_POOL, &mut handles,),
            Some(7)
        );
        assert_eq!(
            take_message_handle(Some("wl_shm"), shm::shm_request::CREATE_POOL, &mut handles,),
            Some(9)
        );
        assert!(handles.is_empty());
    }
}
