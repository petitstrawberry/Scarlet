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
use shm::ShmManager;
use std::collections::BTreeMap;
use std::env;
use std::handle::capability::memory_mapping::flags;
use std::io::{Read, Write};
use std::ipc::{SharedMemory, permissions};
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

/// SWS window shared memory information
struct WindowShmInfo {
    /// SWS window ID
    window_id: u32,
    /// Shared memory handle for the window's buffer
    shm: SharedMemory,
    /// Mapped address of the SHM
    mapped_addr: usize,
    /// Size of the SHM buffer
    size: usize,
    /// True when SWS renders directly from client buffers.
    external_buffer_attached: bool,
}

struct PendingDamage {
    surface_id: u32,
    rects: Vec<(u32, u32, u32, u32)>,
}

/// Wayland Bridge Server
struct WaylandBridge {
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
    /// Map of SWS window ID -> window SHM information
    window_shm: BTreeMap<u32, WindowShmInfo>,
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
    /// Pending SWS responses that are not input events
    sws_pending: Vec<protocol_sws::ServerMessage>,
    /// Coalesced SWS updates waiting to be sent per window
    pending_damage: BTreeMap<u32, PendingDamage>,
    /// Frame callbacks waiting for the next SWS update flush per surface
    pending_frame_callbacks: BTreeMap<u32, Vec<(u32, u32)>>,
    /// Whether a coalescing delay is pending before the next flush
    flush_deferred: bool,
    /// Minimum interval between EXTENSION_UPDATE_BUFFER flushes
    update_flush_interval: Duration,
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
}

impl WaylandBridge {
    /// Create a new Wayland bridge client state
    fn new_client() -> Result<Self, &'static str> {
        let mut objects = BTreeMap::new();
        // Object ID 1 is always wl_display
        objects.insert(1, String::from("wl_display"));

        let input_event_queue = Arc::new(StdMutex::new(Vec::new()));
        let objects_for_input_thread = Arc::new(StdMutex::new(BTreeMap::new()));
        let pointer_position_for_thread = Arc::new(StdMutex::new((0, 0)));

        Ok(Self {
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
            window_shm: BTreeMap::new(),
            keymap_shm: None,
            keymap_size: 0,
            input_event_queue,
            objects_for_input_thread,
            pointer_position_for_thread,
            focused_surface: None,
            focused_keyboard: None,
            focused_pointer: None,
            sws_rx_buffer: Vec::new(),
            sws_pending: Vec::new(),
            pending_damage: BTreeMap::new(),
            pending_frame_callbacks: BTreeMap::new(),
            flush_deferred: false,
            update_flush_interval: Duration::from_millis(16),
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
        })
    }

    /// Connect to SWS server and register as extension
    fn connect_to_sws(&mut self) -> Result<(), &'static str> {
        bridge_log!("[Bridge] Connecting to SWS at /tmp/sws.sock");

        let sws_socket = Socket::new().map_err(|_| "Failed to create SWS socket")?;

        sws_socket
            .connect("/tmp/sws.sock")
            .map_err(|_| "Failed to connect to SWS")?;

        bridge_log!("[Bridge] Connected to SWS, registering as extension");

        // Send REGISTER_EXTENSION message
        let extension_name = b"wayland_bridge";
        let payload = protocol_sws::payload_register_extension(extension_name);
        let header = protocol_sws::MessageHeader {
            msg_type: protocol_sws::client_msg::REGISTER_EXTENSION,
            payload_size: payload.len() as u32,
        };

        let mut msg_bytes = Vec::new();
        msg_bytes.extend_from_slice(&header.to_le_bytes());
        msg_bytes.extend_from_slice(&payload);

        let mut sws_socket_mut = sws_socket;
        sws_socket_mut
            .write(&msg_bytes)
            .map_err(|_| "Failed to send REGISTER_EXTENSION")?;
        sws_socket_mut
            .set_nonblocking(true)
            .map_err(|_| "Failed to set SWS socket non-blocking")?;

        self.sws_connection = Some(sws_socket_mut);
        if let protocol_sws::ServerMessage::ExtensionRegistered { extension_id } = self
            .wait_for_sws_message(|msg| {
                matches!(msg, protocol_sws::ServerMessage::ExtensionRegistered { .. })
            })?
        {
            self.extension_id = Some(extension_id);
            bridge_log!("[Bridge] Registered as extension with ID: {}", extension_id);
        }
        Ok(())
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
            msg.add_arg(WaylandArg::Fixed(self.pointer_x << 8));
            msg.add_arg(WaylandArg::Fixed(self.pointer_y << 8));
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
            enter.add_arg(WaylandArg::Fixed(self.pointer_x << 8));
            enter.add_arg(WaylandArg::Fixed(self.pointer_y << 8));
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
        const ABS_X: u16 = 0x00;
        const ABS_Y: u16 = 0x01;
        const BTN_MOUSE_MIN: u16 = 0x110;
        const BTN_MOUSE_MAX: u16 = 0x118;
        const BTN_LEFT: u16 = 0x110;

        if let Some(surface_id) = self.surface_id_for_window(window_id) {
            self.queue_focus_events(surface_id);
        }

        match type_ {
            EV_REL => {
                if code == REL_X {
                    self.pointer_x = self.pointer_x.saturating_add(value);
                } else if code == REL_Y {
                    self.pointer_y = self.pointer_y.saturating_add(value);
                }
                self.pending_pointer_motion = true;
                self.pending_pointer_time = time as u32;
                self.pending_pointer_id = self.focused_pointer;
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

    fn poll_sws_messages(&mut self) -> Result<(), &'static str> {
        let sws_conn = match self.sws_connection.as_mut() {
            Some(conn) => conn,
            None => return Ok(()),
        };

        let mut buf = [0u8; 1024];
        loop {
            match sws_conn.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    self.sws_rx_buffer.extend_from_slice(&buf[..n]);
                }
                Err(_) => break,
            }
        }

        loop {
            if self.sws_rx_buffer.len() < protocol_sws::MessageHeader::SIZE {
                break;
            }
            let mut header_bytes = [0u8; protocol_sws::MessageHeader::SIZE];
            header_bytes.copy_from_slice(&self.sws_rx_buffer[..protocol_sws::MessageHeader::SIZE]);
            let header = protocol_sws::MessageHeader::from_le_bytes(header_bytes);
            let frame_len = protocol_sws::MessageHeader::SIZE + header.payload_size as usize;
            if self.sws_rx_buffer.len() < frame_len {
                break;
            }
            let payload = &self.sws_rx_buffer[protocol_sws::MessageHeader::SIZE..frame_len];
            if let Ok(msg) = protocol_sws::parse_server_message(header.msg_type, payload) {
                match msg {
                    protocol_sws::ServerMessage::InputEvent {
                        window_id,
                        time,
                        type_,
                        code,
                        value,
                    } => {
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
                    other => {
                        self.sws_pending.push(other);
                    }
                }
            }
            self.sws_rx_buffer.drain(0..frame_len);
        }

        Ok(())
    }

    fn wait_for_sws_message<F>(
        &mut self,
        mut matches: F,
    ) -> Result<protocol_sws::ServerMessage, &'static str>
    where
        F: FnMut(&protocol_sws::ServerMessage) -> bool,
    {
        loop {
            self.poll_sws_messages()?;
            let mut idx = 0;
            while idx < self.sws_pending.len() {
                if matches(&self.sws_pending[idx]) {
                    return Ok(self.sws_pending.remove(idx));
                }
                idx += 1;
            }
            thread::sleep(Duration::from_millis(1));
        }
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

    /// Create an SWS window for a Wayland surface
    fn create_sws_window_for_surface(&mut self, wl_surface_id: u32) -> Result<(), &'static str> {
        // Check if already mapped
        if self.surface_to_window.contains_key(&wl_surface_id) {
            return Ok(());
        }

        let mut window_id_opt = None;
        {
            let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;

            // Default size for now (800x600)
            let width = 800u32;
            let height = 600u32;

            bridge_log!(
                "[Bridge] Creating SWS window for surface {} ({}x{})",
                wl_surface_id,
                width,
                height
            );

            // Send EXTENSION_CREATE_WINDOW message
            let payload =
                protocol_sws::payload_extension_create_window(wl_surface_id, width, height);
            let header = protocol_sws::MessageHeader {
                msg_type: protocol_sws::client_msg::EXTENSION_CREATE_WINDOW,
                payload_size: payload.len() as u32,
            };

            let mut msg_bytes = Vec::new();
            msg_bytes.extend_from_slice(&header.to_le_bytes());
            msg_bytes.extend_from_slice(&payload);

            sws_conn
                .write(&msg_bytes)
                .map_err(|_| "Failed to send EXTENSION_CREATE_WINDOW")?;
        }
        if let protocol_sws::ServerMessage::WindowCreated {
            window_id,
            shm_size,
        } = self.wait_for_sws_message(|msg| {
            matches!(msg, protocol_sws::ServerMessage::WindowCreated { .. })
        })? {
            bridge_log!(
                "[Bridge] SWS window created: {} for surface {} (shm_size={})",
                window_id,
                wl_surface_id,
                shm_size
            );
            self.surface_to_window.insert(wl_surface_id, window_id);
            window_id_opt = Some(window_id);

            let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;
            if let Ok(shm_handle) = sws_conn.recv_handle() {
                bridge_log!("[Bridge] Received SHM handle for window {}", window_id);
                if let Ok(shm) = SharedMemory::from_handle(shm_handle) {
                    if let Ok(mapper) = shm.as_handle().as_memory_mapping() {
                        if let Ok(mapped_addr) = mapper.mmap(
                            0,
                            shm_size as usize,
                            permissions::READ_WRITE,
                            flags::SHARED,
                            0,
                        ) {
                            bridge_log!(
                                "[Bridge] Mapped window {} SHM at 0x{:x}",
                                window_id,
                                mapped_addr
                            );
                            self.window_shm.insert(
                                window_id,
                                WindowShmInfo {
                                    window_id,
                                    shm,
                                    mapped_addr,
                                    size: shm_size as usize,
                                    external_buffer_attached: false,
                                },
                            );
                        } else {
                            bridge_log!("[Bridge] Failed to map window {} SHM", window_id);
                        }
                    } else {
                        bridge_log!("[Bridge] Window {} SHM doesn't support mapping", window_id);
                    }
                } else {
                    bridge_log!("[Bridge] Received handle is not a shared memory object");
                }
            }
        }

        if let Some(window_id) = window_id_opt {
            let (buffer_id_opt, should_send) = self
                .surface_manager
                .get_surface(wl_surface_id)
                .and_then(|surface| {
                    surface.buffer_id.map(|buffer_id| {
                        (buffer_id, surface.last_attached_buffer != Some(buffer_id))
                    })
                })
                .map(|(buffer_id, should_send)| (Some(buffer_id), should_send))
                .unwrap_or((None, false));

            if let (Some(buffer_id), true) = (buffer_id_opt, should_send)
                && self
                    .send_extension_attach_buffer(wl_surface_id, window_id, buffer_id)
                    .is_ok()
                && let Some(surface) = self.surface_manager.get_surface_mut(wl_surface_id)
            {
                surface.last_attached_buffer = Some(buffer_id);
            }
        }

        Ok(())
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

        let mut window_id_opt = None;
        {
            let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;

            bridge_log!(
                "[Bridge] Creating SWS window for surface {} ({}x{})",
                wl_surface_id,
                width,
                height
            );

            // Send EXTENSION_CREATE_WINDOW message
            let payload =
                protocol_sws::payload_extension_create_window(wl_surface_id, width, height);
            let header = protocol_sws::MessageHeader {
                msg_type: protocol_sws::client_msg::EXTENSION_CREATE_WINDOW,
                payload_size: payload.len() as u32,
            };

            let mut msg_bytes = Vec::new();
            msg_bytes.extend_from_slice(&header.to_le_bytes());
            msg_bytes.extend_from_slice(&payload);

            sws_conn
                .write(&msg_bytes)
                .map_err(|_| "Failed to send EXTENSION_CREATE_WINDOW")?;
        }
        if let protocol_sws::ServerMessage::WindowCreated {
            window_id,
            shm_size,
        } = self.wait_for_sws_message(|msg| {
            matches!(msg, protocol_sws::ServerMessage::WindowCreated { .. })
        })? {
            bridge_log!(
                "[Bridge] SWS window created: {} for surface {} (shm_size={})",
                window_id,
                wl_surface_id,
                shm_size
            );
            self.surface_to_window.insert(wl_surface_id, window_id);
            window_id_opt = Some(window_id);

            let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;
            if let Ok(shm_handle) = sws_conn.recv_handle() {
                bridge_log!("[Bridge] Received SHM handle for window {}", window_id);

                if let Ok(shm) = SharedMemory::from_handle(shm_handle) {
                    if let Ok(mapper) = shm.as_handle().as_memory_mapping() {
                        if let Ok(mapped_addr) = mapper.mmap(
                            0,
                            shm_size as usize,
                            permissions::READ_WRITE,
                            flags::SHARED,
                            0,
                        ) {
                            bridge_log!(
                                "[Bridge] Mapped window {} SHM at 0x{:x}",
                                window_id,
                                mapped_addr
                            );
                            self.window_shm.insert(
                                window_id,
                                WindowShmInfo {
                                    window_id,
                                    shm,
                                    mapped_addr,
                                    size: shm_size as usize,
                                    external_buffer_attached: false,
                                },
                            );
                        } else {
                            bridge_log!("[Bridge] Failed to map window {} SHM", window_id);
                        }
                    } else {
                        bridge_log!("[Bridge] Window {} SHM doesn't support mapping", window_id);
                    }
                } else {
                    bridge_log!("[Bridge] Received handle is not a shared memory object");
                }
            }
        }

        if let Some(window_id) = window_id_opt {
            let (buffer_id_opt, should_send) = self
                .surface_manager
                .get_surface(wl_surface_id)
                .and_then(|surface| {
                    surface.buffer_id.map(|buffer_id| {
                        (buffer_id, surface.last_attached_buffer != Some(buffer_id))
                    })
                })
                .map(|(buffer_id, should_send)| (Some(buffer_id), should_send))
                .unwrap_or((None, false));

            if let (Some(buffer_id), true) = (buffer_id_opt, should_send)
                && self
                    .send_extension_attach_buffer(wl_surface_id, window_id, buffer_id)
                    .is_ok()
                && let Some(surface) = self.surface_manager.get_surface_mut(wl_surface_id)
            {
                surface.last_attached_buffer = Some(buffer_id);
            }
        }

        Ok(())
    }

    fn queue_pending_damage(
        &mut self,
        window_id: u32,
        surface_id: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) {
        let was_empty = self.pending_damage.is_empty();
        let entry = self
            .pending_damage
            .entry(window_id)
            .or_insert(PendingDamage {
                surface_id,
                rects: Vec::new(),
            });

        if entry.surface_id != surface_id {
            entry.surface_id = surface_id;
            entry.rects.clear();
        }

        if was_empty {
            self.flush_deferred = true;
        }

        Self::push_damage_rect(&mut entry.rects, (x, y, width, height));
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
        if damage.is_empty() {
            return Vec::from([(0, 0, surface_width, surface_height)]);
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

    fn flush_pending_updates(&mut self) -> Result<bool, &'static str> {
        if self.pending_damage.is_empty() {
            return Ok(false);
        }

        let window_ids: Vec<u32> = self.pending_damage.keys().copied().collect();
        let mut sent_any = false;

        for window_id in window_ids {
            let Some(pending) = self.pending_damage.remove(&window_id) else {
                continue;
            };
            for (x, y, width, height) in &pending.rects {
                if is_debug_enabled() {
                    bridge_log!(
                        "[Bridge] Updating SWS window {} with damage [{},{} {}x{}]",
                        window_id,
                        x,
                        y,
                        width,
                        height
                    );
                }
                self.send_extension_update_buffer(
                    pending.surface_id,
                    window_id,
                    *x,
                    *y,
                    *width,
                    *height,
                )?;
                sent_any = true;
            }

            if let Some(callbacks) = self.pending_frame_callbacks.remove(&pending.surface_id) {
                let mut callback_msgs = Vec::new();
                for (callback_id, time) in callbacks {
                    let mut msg = WaylandMessage::new(callback_id, protocol::callback_event::DONE);
                    msg.add_arg(WaylandArg::Uint(time));
                    callback_msgs.push(msg);
                }
                self.queue_input_messages(callback_msgs);
            }

            if let Some(surface) = self.surface_manager.get_surface_mut(pending.surface_id)
                && !surface.pending_release.is_empty()
            {
                let mut release_msgs = Vec::new();
                for buffer_id in surface.pending_release.drain(..) {
                    release_msgs.push(WaylandMessage::new(buffer_id, shm::buffer_event::RELEASE));
                }
                self.queue_input_messages(release_msgs);
            }
        }

        Ok(sent_any)
    }

    fn maybe_flush_pending_updates(&mut self) -> Result<bool, &'static str> {
        if self.pending_damage.is_empty() {
            return Ok(false);
        }

        if self.flush_deferred {
            self.flush_deferred = false;
            thread::sleep(self.update_flush_interval);
        }

        self.flush_pending_updates()
    }

    /// Update SWS window buffer when surface commits
    fn update_sws_window(
        &mut self,
        wl_surface_id: u32,
        damage_rect: (u32, u32, u32, u32),
    ) -> Result<(), &'static str> {
        let window_id = *self
            .surface_to_window
            .get(&wl_surface_id)
            .ok_or("Surface not mapped to window")?;

        // Get SWS window SHM info
        if self.window_shm.get(&window_id).is_none() {
            return Err("Window SHM not found");
        }

        let (x, y, width, height) = damage_rect;
        if width == 0 || height == 0 {
            return Ok(());
        }

        self.queue_pending_damage(window_id, wl_surface_id, x, y, width, height);

        // Zero-copy path: SWS uses the client SHM mapping provided via EXTENSION_ATTACH_BUFFER.
        // TODO: If we need a fallback copy path, reintroduce it behind a flag.

        Ok(())
    }

    fn send_extension_update_buffer(
        &mut self,
        wl_surface_id: u32,
        window_id: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<(), &'static str> {
        let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;

        let mut payload = Vec::new();
        payload.extend_from_slice(&wl_surface_id.to_le_bytes());
        payload.extend_from_slice(&window_id.to_le_bytes());
        payload.extend_from_slice(&x.to_le_bytes());
        payload.extend_from_slice(&y.to_le_bytes());
        payload.extend_from_slice(&width.to_le_bytes());
        payload.extend_from_slice(&height.to_le_bytes());

        let header = protocol_sws::MessageHeader {
            msg_type: protocol_sws::client_msg::EXTENSION_UPDATE_BUFFER,
            payload_size: payload.len() as u32,
        };

        let mut msg_bytes = Vec::new();
        msg_bytes.extend_from_slice(&header.to_le_bytes());
        msg_bytes.extend_from_slice(&payload);

        sws_conn
            .write(&msg_bytes)
            .map_err(|_| "Failed to send EXTENSION_UPDATE_BUFFER")?;

        Ok(())
    }

    fn send_request_move_window(&mut self, window_id: u32) -> Result<(), &'static str> {
        let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;

        bridge_log!(
            "[Bridge] Sending REQUEST_MOVE_WINDOW for window {}",
            window_id
        );

        let payload = protocol_sws::payload_request_move_window(window_id);
        let header = protocol_sws::MessageHeader {
            msg_type: protocol_sws::client_msg::REQUEST_MOVE_WINDOW,
            payload_size: payload.len() as u32,
        };

        let mut msg_bytes = Vec::new();
        msg_bytes.extend_from_slice(&header.to_le_bytes());
        msg_bytes.extend_from_slice(&payload);

        sws_conn
            .write(&msg_bytes)
            .map_err(|_| "Failed to send REQUEST_MOVE_WINDOW")?;

        Ok(())
    }

    fn send_minimize_window(&mut self, window_id: u32) -> Result<(), &'static str> {
        let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;

        bridge_log!("[Bridge] Sending MINIMIZE_WINDOW for window {}", window_id);

        let payload = protocol_sws::payload_minimize_window(window_id);
        let header = protocol_sws::MessageHeader {
            msg_type: protocol_sws::client_msg::MINIMIZE_WINDOW,
            payload_size: payload.len() as u32,
        };

        let mut msg_bytes = Vec::new();
        msg_bytes.extend_from_slice(&header.to_le_bytes());
        msg_bytes.extend_from_slice(&payload);

        sws_conn
            .write(&msg_bytes)
            .map_err(|_| "Failed to send MINIMIZE_WINDOW")?;

        Ok(())
    }

    fn send_maximize_window(&mut self, window_id: u32) -> Result<(), &'static str> {
        let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;

        bridge_log!("[Bridge] Sending MAXIMIZE_WINDOW for window {}", window_id);

        let payload = protocol_sws::payload_maximize_window(window_id);
        let header = protocol_sws::MessageHeader {
            msg_type: protocol_sws::client_msg::MAXIMIZE_WINDOW,
            payload_size: payload.len() as u32,
        };

        let mut msg_bytes = Vec::new();
        msg_bytes.extend_from_slice(&header.to_le_bytes());
        msg_bytes.extend_from_slice(&payload);

        sws_conn
            .write(&msg_bytes)
            .map_err(|_| "Failed to send MAXIMIZE_WINDOW")?;

        Ok(())
    }

    fn send_restore_window(&mut self, window_id: u32) -> Result<(), &'static str> {
        let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;

        bridge_log!("[Bridge] Sending RESTORE_WINDOW for window {}", window_id);

        let payload = protocol_sws::payload_restore_window(window_id);
        let header = protocol_sws::MessageHeader {
            msg_type: protocol_sws::client_msg::RESTORE_WINDOW,
            payload_size: payload.len() as u32,
        };

        let mut msg_bytes = Vec::new();
        msg_bytes.extend_from_slice(&header.to_le_bytes());
        msg_bytes.extend_from_slice(&payload);

        sws_conn
            .write(&msg_bytes)
            .map_err(|_| "Failed to send RESTORE_WINDOW")?;

        Ok(())
    }

    fn send_destroy_window(&mut self, window_id: u32) -> Result<(), &'static str> {
        let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;

        bridge_log!("[Bridge] Sending DESTROY_WINDOW for window {}", window_id);

        let payload = protocol_sws::payload_destroy_window(window_id);
        let header = protocol_sws::MessageHeader {
            msg_type: protocol_sws::client_msg::DESTROY_WINDOW,
            payload_size: payload.len() as u32,
        };

        let mut msg_bytes = Vec::new();
        msg_bytes.extend_from_slice(&header.to_le_bytes());
        msg_bytes.extend_from_slice(&payload);

        sws_conn
            .write(&msg_bytes)
            .map_err(|_| "Failed to send DESTROY_WINDOW")?;

        Ok(())
    }

    fn send_extension_attach_buffer(
        &mut self,
        surface_id: u32,
        window_id: u32,
        buffer_id: u32,
    ) -> Result<(), &'static str> {
        let buffer = self
            .shm_manager
            .get_buffer(buffer_id)
            .ok_or("Buffer not found")?;
        let pool = self
            .shm_manager
            .get_pool(buffer.pool_id)
            .ok_or("Pool not found")?;
        let handle = pool.handle.as_ref().ok_or("Pool missing handle")?;

        let width = buffer.width.max(0) as u32;
        let height = buffer.height.max(0) as u32;
        let stride = buffer.stride;
        let format = buffer.format;
        let offset = buffer.offset;
        let mut shm_size = pool.size as u64;
        if stride > 0 && buffer.height > 0 {
            let needed = (offset.max(0) as u64)
                .saturating_add((stride as u64).saturating_mul(buffer.height as u64));
            shm_size = shm_size.max(needed);
        }

        // bridge_log!("[Bridge] === EXTENSION_ATTACH_BUFFER ===");
        // bridge_log!("[Bridge]   surface_id={}, window_id={}, buffer_id={}", surface_id, window_id, buffer_id);
        // bridge_log!("[Bridge]   geometry={}x{} stride={} offset={} format={} shm_size={}",
        //     width, height, stride, offset, format, shm_size);
        // bridge_log!("[Bridge]   client_shm_handle={:?}", handle.as_raw());

        let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;

        let payload = protocol_sws::payload_extension_attach_buffer(
            surface_id, window_id, width, height, offset, stride, format, shm_size,
        );
        let header = protocol_sws::MessageHeader {
            msg_type: protocol_sws::client_msg::EXTENSION_ATTACH_BUFFER,
            payload_size: payload.len() as u32,
        };
        let mut msg_bytes = Vec::new();
        msg_bytes.extend_from_slice(&header.to_le_bytes());
        msg_bytes.extend_from_slice(&payload);

        // bridge_log!("[Bridge] Sending EXTENSION_ATTACH_BUFFER message ({} bytes)", msg_bytes.len());
        sws_conn
            .write(&msg_bytes)
            .map_err(|_| "Failed to send EXTENSION_ATTACH_BUFFER")?;
        // bridge_log!("[Bridge] EXTENSION_ATTACH_BUFFER message sent successfully");

        // bridge_log!("[Bridge] Sending client SHM handle to SWS...");
        sws_conn
            .send_handle(handle)
            .map_err(|_| "Failed to send EXTENSION_ATTACH_BUFFER handle")?;
        // bridge_log!("[Bridge] Client SHM handle sent successfully");
        // bridge_log!("[Bridge] === EXTENSION_ATTACH_BUFFER COMPLETE ===");

        if let Some(window_shm_info) = self.window_shm.get_mut(&window_id) {
            window_shm_info.external_buffer_attached = true;
        }

        Ok(())
    }

    /// Resize an SWS window and update the SHM mapping
    fn resize_sws_window(
        &mut self,
        window_id: u32,
        new_width: u32,
        new_height: u32,
    ) -> Result<(), &'static str> {
        // Calculate new buffer size
        let new_buffer_size = (new_width as u64)
            .saturating_mul(new_height as u64)
            .saturating_mul(4);

        bridge_log!(
            "[Bridge] Resizing window {} to {}x{} ({} bytes)",
            window_id,
            new_width,
            new_height,
            new_buffer_size
        );

        // Send RESIZE_WINDOW message
        let payload = protocol_sws::payload_resize_window(window_id, new_width, new_height);
        let header = protocol_sws::MessageHeader {
            msg_type: protocol_sws::client_msg::RESIZE_WINDOW,
            payload_size: payload.len() as u32,
        };

        let mut msg_bytes = Vec::new();
        msg_bytes.extend_from_slice(&header.to_le_bytes());
        msg_bytes.extend_from_slice(&payload);

        {
            let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;
            sws_conn
                .write(&msg_bytes)
                .map_err(|_| "Failed to send RESIZE_WINDOW")?;
        }

        if let protocol_sws::ServerMessage::WindowResized {
            window_id: resized_window_id,
            shm_size,
            ..
        } = self.wait_for_sws_message(|msg| {
            matches!(msg, protocol_sws::ServerMessage::WindowResized { .. })
        })? {
            bridge_log!(
                "[Bridge] Window {} resized to shm_size={}",
                resized_window_id,
                shm_size
            );

            let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;
            if let Ok(shm_handle) = sws_conn.recv_handle() {
                bridge_log!("[Bridge] Received new SHM handle for window {}", window_id);

                if let Ok(shm) = SharedMemory::from_handle(shm_handle) {
                    if let Ok(mapper) = shm.as_handle().as_memory_mapping() {
                        if let Ok(mapped_addr) = mapper.mmap(
                            0,
                            shm_size as usize,
                            permissions::READ_WRITE,
                            flags::SHARED,
                            0,
                        ) {
                            bridge_log!(
                                "[Bridge] Remapped window {} SHM at 0x{:x}",
                                window_id,
                                mapped_addr
                            );
                            self.window_shm.insert(
                                window_id,
                                WindowShmInfo {
                                    window_id,
                                    shm,
                                    mapped_addr,
                                    size: shm_size as usize,
                                    external_buffer_attached: false,
                                },
                            );
                        } else {
                            bridge_log!("[Bridge] Failed to map resized window {} SHM", window_id);
                        }
                    } else {
                        bridge_log!(
                            "[Bridge] Resized window {} SHM doesn't support mapping",
                            window_id
                        );
                    }
                } else {
                    bridge_log!("[Bridge] Received handle is not a shared memory object");
                }
            }
        }

        Ok(())
    }

    /// Handle a client connection
    fn handle_client(&mut self, mut client: Socket) -> Result<(), &'static str> {
        bridge_log!("[Bridge] New client connected");

        client
            .set_nonblocking(true)
            .map_err(|_| "Failed to set client socket non-blocking")?;

        let mut buffer: Vec<u8> = Vec::new();
        let mut idle_backoff_ms = 1u64;

        loop {
            let mut got_data = false;
            loop {
                let mut read_buf = [0u8; 4096];
                match client.read(&mut read_buf) {
                    Ok(0) => {
                        bridge_log!("[Bridge] Client disconnected");
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
                    Err(_) => {
                        bridge_log!("[Bridge] Error reading from client");
                        return Ok(());
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

                    // Handle the message
                    let responses = self.handle_message(
                        &header,
                        &buffer[offset + 8..offset + msg_size],
                        &mut client,
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
                            if is_keyboard && let Some(shm) = self.keymap_shm.as_ref() {
                                match client.send_handle_and_data(shm.as_handle(), &response_bytes)
                                {
                                    Ok(()) => {
                                        if is_debug_enabled() {
                                            bridge_log!(
                                                "[Bridge] KEYMAP sent with handle successfully"
                                            );
                                        }
                                        continue;
                                    }
                                    Err(e) => {
                                        bridge_log!(
                                            "[Bridge] Failed to send KEYMAP with handle: {:?}, falling back",
                                            e
                                        );
                                    }
                                }
                            }
                        }
                        client
                            .write(&response_bytes)
                            .map_err(|_| "Failed to send response")?;
                    }

                    offset += msg_size;
                }

                if offset > 0 {
                    buffer.drain(0..offset);
                }
            }

            // Check for input events from SWS (from shared connection)
            let _ = self.poll_sws_messages();
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
            let mut bytes_written = 0;
            while bytes_written < encoded_input_events.len() {
                match client.write(&encoded_input_events[bytes_written..]) {
                    Ok(0) => {
                        bridge_log!("[Bridge] Failed to forward input events: short write");
                        break;
                    }
                    Ok(n) => bytes_written += n,
                    Err(e) => {
                        bridge_log!("[Bridge] Failed to forward input events: {:?}", e);
                        break;
                    }
                }
            }

            let sent_updates = self.maybe_flush_pending_updates()?;

            if !got_data && !had_input_events && !sent_updates {
                thread::sleep(Duration::from_millis(idle_backoff_ms));
                idle_backoff_ms = (idle_backoff_ms * 2).min(8);
            } else {
                idle_backoff_ms = 1;
            }
        }
    }

    /// Handle a Wayland protocol message
    fn handle_message(
        &mut self,
        header: &MessageHeader,
        payload: &[u8],
        client: &mut Socket,
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

        match interface.as_str() {
            "wl_display" => self.handle_display_message(opcode, payload),
            "wl_registry" => self.handle_registry_message(object_id, opcode, payload),
            "wl_compositor" => self.handle_compositor_message(opcode, payload),
            "wl_surface" => self.handle_surface_message(object_id, opcode, payload),
            "wl_shm" => self.handle_shm_message(opcode, payload, client),
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

                    // Send done event for the callback
                    let mut msg = WaylandMessage::new(callback_id, 0); // wl_callback.done
                    let serial = self.allocate_serial();
                    msg.add_arg(WaylandArg::Uint(serial)); // serial
                    let mut msgs = Vec::new();
                    msgs.push(msg);
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
                                    mode.add_arg(WaylandArg::Int(800)); // width
                                    mode.add_arg(WaylandArg::Int(600)); // height
                                    mode.add_arg(WaylandArg::Int(60000)); // refresh mHz
                                    msgs.push(mode);

                                    let mut scale =
                                        WaylandMessage::new(new_id, protocol::output_event::SCALE);
                                    scale.add_arg(WaylandArg::Int(1));
                                    msgs.push(scale);

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
                self.surface_manager.destroy_surface(surface_id);
                self.objects.remove(&surface_id);
                // Remove from surface_to_window mapping
                if let Some(window_id) = self.surface_to_window.remove(&surface_id) {
                    self.pending_damage.remove(&window_id);
                    let _ = self.send_destroy_window(window_id);
                }
                Ok(Vec::new())
            }
            protocol::surface_request::ATTACH => {
                if is_debug_enabled() {
                    bridge_log!("[Bridge] wl_surface.attach on surface {}", surface_id);
                }
                if payload.len() >= 12 {
                    let buffer_id = Self::parse_u32(payload, 0).unwrap_or(0);
                    let _x = Self::parse_i32(payload, 4).unwrap_or(0);
                    let _y = Self::parse_i32(payload, 8).unwrap_or(0);
                    let mut should_send_attach = false;
                    let mut skip_window = false;

                    if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                        if surface.role == Some(surface::SurfaceRole::Cursor) {
                            skip_window = true;
                        }
                        if buffer_id == 0 {
                            surface.buffer_id = None;
                            surface.last_attached_buffer = None;
                        } else {
                            let old_width = surface.width;
                            let old_height = surface.height;
                            surface.attach(buffer_id);
                            if surface.last_attached_buffer != Some(buffer_id) {
                                should_send_attach = true;
                            }
                            if let Some(buffer) = self.shm_manager.get_buffer(buffer_id) {
                                let buffer_width = buffer.width.max(0) as u32;
                                let buffer_height = buffer.height.max(0) as u32;
                                surface.width = buffer_width;
                                surface.height = buffer_height;

                                if !skip_window {
                                    // Check if window already exists
                                    if let Some(&window_id) =
                                        self.surface_to_window.get(&surface_id)
                                    {
                                        // Window exists, check if resize is needed
                                        if buffer_width != old_width || buffer_height != old_height
                                        {
                                            bridge_log!(
                                                "[Bridge] Buffer size {}x{} differs from surface {}x{}, resizing window",
                                                buffer_width,
                                                buffer_height,
                                                old_width,
                                                old_height
                                            );
                                            if let Err(e) = self.resize_sws_window(
                                                window_id,
                                                buffer_width,
                                                buffer_height,
                                            ) {
                                                bridge_log!(
                                                    "[Bridge] Failed to resize window: {}",
                                                    e
                                                );
                                            }
                                        }
                                    } else {
                                        // Window doesn't exist yet, create it with buffer size
                                        bridge_log!(
                                            "[Bridge] No window yet, creating with buffer size {}x{}",
                                            buffer_width,
                                            buffer_height
                                        );
                                        if let Err(e) = self.create_sws_window_with_size(
                                            surface_id,
                                            buffer_width,
                                            buffer_height,
                                        ) {
                                            bridge_log!("[Bridge] Failed to create window: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if buffer_id != 0 && !skip_window {
                        if let Some(&window_id) = self.surface_to_window.get(&surface_id) {
                            // bridge_log!("[Bridge] Sending attach for surface {} buffer {} window {}", surface_id, buffer_id, window_id);
                            if should_send_attach {
                                if let Err(e) = self
                                    .send_extension_attach_buffer(surface_id, window_id, buffer_id)
                                {
                                    bridge_log!("[Bridge] Failed to send attach buffer: {}", e);
                                } else if let Some(surface) =
                                    self.surface_manager.get_surface_mut(surface_id)
                                {
                                    surface.last_attached_buffer = Some(buffer_id);
                                }
                            }
                        } else {
                            bridge_log!("[Bridge] No window ID found for surface {}", surface_id);
                        }
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
                        surface.add_damage(x, y, width, height);
                    }
                }
                Ok(Vec::new())
            }
            protocol::surface_request::COMMIT => {
                if is_debug_enabled() {
                    bridge_log!("[Bridge] wl_surface.commit on surface {}", surface_id);
                }
                let mut release_buffers = Vec::new();
                let mut callback_msg = None;
                let mut should_update = false;
                let mut buffer_present = false;
                let mut surface_size = (0u32, 0u32);
                let mut damage_rects = Vec::new();
                let mut callback_serial = None;
                let serial_for_callback = self.allocate_serial();
                let mut configure_msgs = Vec::new();
                let mut configure_state = None;
                let mut defer_callback_until_update = false;

                if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                    if let Some(cb_id) = surface.take_pending_callback() {
                        callback_serial = Some((cb_id, serial_for_callback));
                    }
                    should_update = matches!(
                        surface.role,
                        Some(surface::SurfaceRole::XdgToplevel)
                            | Some(surface::SurfaceRole::XdgPopup)
                    );
                    buffer_present = surface.buffer_id.is_some();
                    surface_size = (surface.width.max(1), surface.height.max(1));
                    damage_rects =
                        Self::compute_damage_rects(&surface.damage, surface.width, surface.height);
                    surface.commit();
                    let current_buffer = surface.buffer_id;
                    if let Some(prev_buffer) = surface.swap_committed_buffer(current_buffer)
                        && current_buffer != Some(prev_buffer)
                        && self.objects.get(&prev_buffer).is_some()
                    {
                        release_buffers.push(prev_buffer);
                    }
                    if !release_buffers.is_empty() {
                        surface.pending_release.append(&mut release_buffers);
                    }
                }

                if !buffer_present
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
                    if let Some(xdg_surface) =
                        self.xdg_shell_manager.get_xdg_surface_mut(xdg_surface_id)
                    {
                        xdg_surface.last_configure_serial = Some(serial);
                    }

                    let mut toplevel_configure =
                        WaylandMessage::new(toplevel_id, xdg_shell::xdg_toplevel_event::CONFIGURE);
                    toplevel_configure.add_arg(WaylandArg::Int(0));
                    toplevel_configure.add_arg(WaylandArg::Int(0));
                    toplevel_configure.add_arg(WaylandArg::Array(Vec::new()));

                    let mut surface_configure = WaylandMessage::new(
                        xdg_surface_id,
                        xdg_shell::xdg_surface_event::CONFIGURE,
                    );
                    surface_configure.add_arg(WaylandArg::Uint(serial));

                    configure_msgs.push(toplevel_configure);
                    configure_msgs.push(surface_configure);
                }

                if should_update && buffer_present {
                    if !self.surface_to_window.contains_key(&surface_id) {
                        let _ = self.create_sws_window_with_size(
                            surface_id,
                            surface_size.0,
                            surface_size.1,
                        );
                    }
                    if self.surface_to_window.contains_key(&surface_id) {
                        for damage_rect in damage_rects {
                            match self.update_sws_window(surface_id, damage_rect) {
                                Ok(()) => defer_callback_until_update = true,
                                Err(e) => {
                                    bridge_log!("[Bridge] Failed to update SWS window: {}", e);
                                }
                            }
                        }
                    }
                    if self.focused_surface.is_none() {
                        self.queue_focus_events(surface_id);
                    }
                }

                if let Some((cb_id, time)) = callback_serial {
                    if defer_callback_until_update {
                        self.pending_frame_callbacks
                            .entry(surface_id)
                            .or_insert_with(Vec::new)
                            .push((cb_id, time));
                    } else {
                        let mut msg = WaylandMessage::new(cb_id, protocol::callback_event::DONE);
                        msg.add_arg(WaylandArg::Uint(time));
                        callback_msg = Some(msg);
                    }
                }

                let mut msgs = Vec::new();
                if let Some(msg) = callback_msg {
                    msgs.push(msg);
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
        client: &mut Socket,
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            shm::shm_request::CREATE_POOL => {
                bridge_log!("[Bridge] wl_shm.create_pool");
                // Payload: new_id (u32) + size (i32) = 8 bytes
                // FD is passed via handle transfer (Socket::recv_handle)
                if payload.len() >= 8 {
                    let pool_id = Self::parse_u32(payload, 0).unwrap_or(0);
                    let size = Self::parse_i32(payload, 4).unwrap_or(0);
                    bridge_log!("[Bridge] Created pool ID: {} size: {}", pool_id, size);
                    self.add_object(pool_id, String::from("wl_shm_pool"));
                    let handle_result = client.recv_handle();
                    let handle = match handle_result {
                        Ok(h) => {
                            bridge_log!("[Bridge] Received SHM handle for pool {}", pool_id);
                            Some(h)
                        }
                        Err(e) => {
                            bridge_log!("[Bridge] Failed to receive SHM handle: {:?}", e);
                            None
                        }
                    };
                    self.shm_manager.create_pool(pool_id, handle, size);
                }
                Ok(Vec::new())
            }
            _ => {
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
                    self.add_object(buffer_id, String::from("wl_buffer"));
                    self.shm_manager
                        .create_buffer(buffer_id, pool_id, offset, width, height, stride, format)?;
                }
                Ok(Vec::new())
            }
            shm::shm_pool_request::DESTROY => {
                bridge_log!("[Bridge] wl_shm_pool.destroy");
                self.shm_manager.destroy_pool(pool_id);
                Ok(Vec::new())
            }
            shm::shm_pool_request::RESIZE => {
                bridge_log!("[Bridge] wl_shm_pool.resize");
                if payload.len() >= 4 {
                    let new_size =
                        i32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    self.shm_manager.resize_pool(pool_id, new_size)?;
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
                self.shm_manager.destroy_buffer(buffer_id);
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
                        if let Some(surface) = self.surface_manager.get_surface_mut(wl_surface_id) {
                            surface.set_role(surface::SurfaceRole::XdgToplevel);
                        }
                    }
                    return Ok(Vec::new());
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_surface_request::GET_POPUP => {
                bridge_log!("[Bridge] xdg_surface.get_popup (ignored)");
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
                if let Some(window_id) = self.window_id_for_toplevel(xdg_toplevel_id) {
                    let _ = self.send_maximize_window(window_id);
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::UNSET_MAXIMIZED => {
                bridge_log!("[Bridge] xdg_toplevel.unset_maximized");
                if let Some(window_id) = self.window_id_for_toplevel(xdg_toplevel_id) {
                    let _ = self.send_restore_window(window_id);
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::SET_FULLSCREEN => {
                bridge_log!("[Bridge] xdg_toplevel.set_fullscreen");
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::UNSET_FULLSCREEN => {
                bridge_log!("[Bridge] xdg_toplevel.unset_fullscreen");
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
                    self.input_manager.create_keyboard(keyboard_id, seat_id);

                    // Store keyboard as focused
                    self.focused_keyboard = Some(keyboard_id);

                    // Send keymap event
                    let size = self.ensure_keymap()?;
                    let mut keymap_msg =
                        WaylandMessage::new(keyboard_id, input::keyboard_event::KEYMAP);
                    keymap_msg.add_arg(WaylandArg::Uint(1)); // XKB_V1 format
                    keymap_msg.add_arg(WaylandArg::FdPlaceholder); // FD placeholder
                    keymap_msg.add_arg(WaylandArg::Uint(size)); // size

                    let mut msgs = Vec::new();
                    msgs.push(keymap_msg);

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

                        if header.msg_type == protocol_sws::server_msg::EXTENSION_INPUT_EVENT
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
    bridge_log!("=== Wayland Bridge Server ===");
    bridge_log!("Starting Wayland to SWS bridge...");

    let socket_path = "/tmp/wayland-0";

    let server_socket = match create_server_socket(socket_path) {
        Ok(sock) => sock,
        Err(e) => {
            bridge_log!("[Bridge] Failed to initialize: {}", e);
            return 1;
        }
    };

    bridge_log!("[Bridge] Listening on {}", socket_path);
    bridge_log!("[Bridge] Clients can connect with WAYLAND_DISPLAY=wayland-0");

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
                thread::spawn(move || {
                    let mut bridge = match WaylandBridge::new_client() {
                        Ok(b) => b,
                        Err(e) => {
                            bridge_log!("[Bridge] Failed to init client state: {}", e);
                            return;
                        }
                    };

                    if let Err(e) = bridge.connect_to_sws() {
                        bridge_log!("[Bridge] Failed to connect to SWS: {}", e);
                        bridge_log!("[Bridge] Make sure SWS is running at /tmp/sws.sock");
                        return;
                    }

                    if enable_input && let Err(e) = bridge.spawn_input_thread() {
                        bridge_log!("[Bridge] Failed to spawn input thread: {}", e);
                        return;
                    }

                    if let Err(e) = bridge.handle_client(client) {
                        bridge_log!("[Bridge] Error handling client: {}", e);
                    }
                });
            }
            Err(e) => {
                bridge_log!("[Bridge] Error accepting connection: {:?}", e);
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}
