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
use std::handle::capability::memory_mapping::{self, flags};
use std::io::{Read, Write};
use std::ipc::{SharedMemory, permissions};
use std::println;
use std::socket::Socket;
use std::string::{String, ToString};
use std::sync::{Arc, Mutex as StdMutex};
use std::thread;
use std::time::Duration;
use std::vec::Vec;
use surface::SurfaceManager;
use sws_protocol as protocol_sws;
use xdg_shell::XdgShellManager;

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

// Note: We can't define macros here that use the above functions directly
// due to macro hygiene rules. We'll use conditional compilation and
// replace println! calls with if statements checking the log level.

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
}

/// Wayland Bridge Server
struct WaylandBridge {
    /// Server socket listening for Wayland clients
    server_socket: Socket,
    /// Registry of global interfaces
    registry: Registry,
    /// Surface manager
    surface_manager: SurfaceManager,
    /// XDG Shell manager
    xdg_shell_manager: XdgShellManager,
    /// Shared memory manager for client SHM pools
    shm_manager: ShmManager,
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
    /// Currently focused surface (for keyboard enter/leave events)
    focused_surface: Option<u32>,
    /// Last focused keyboard (for sending leave events)
    focused_keyboard: Option<u32>,
}

impl WaylandBridge {
    /// Create a new Wayland bridge
    fn new(socket_path: &str) -> Result<Self, &'static str> {
        println!("[Bridge] Creating server socket at {}", socket_path);

        // Create server socket
        let server_socket = Socket::new().map_err(|_| "Failed to create socket")?;

        // Bind to socket path
        server_socket
            .bind(socket_path)
            .map_err(|_| "Failed to bind socket")?;

        // Listen for connections
        server_socket
            .listen(5)
            .map_err(|_| "Failed to listen on socket")?;

        println!("[Bridge] Server socket ready");

        let mut objects = BTreeMap::new();
        // Object ID 1 is always wl_display
        objects.insert(1, String::from("wl_display"));

        let input_event_queue = Arc::new(StdMutex::new(Vec::new()));
        let objects_for_input_thread = Arc::new(StdMutex::new(BTreeMap::new()));

        Ok(Self {
            server_socket,
            registry: Registry::new(),
            surface_manager: SurfaceManager::new(),
            xdg_shell_manager: XdgShellManager::new(),
            shm_manager: ShmManager::new(),
            input_manager: InputManager::new(),
            sws_connection: None,
            extension_id: None,
            next_serial: 1,
            objects,
            surface_to_window: BTreeMap::new(),
            window_shm: BTreeMap::new(),
            keymap_shm: None,
            keymap_size: 0,
            input_event_queue,
            objects_for_input_thread,
            focused_surface: None,
            focused_keyboard: None,
        })
    }

    /// Connect to SWS server and register as extension
    fn connect_to_sws(&mut self) -> Result<(), &'static str> {
        println!("[Bridge] Connecting to SWS at /tmp/sws.sock");

        let sws_socket = Socket::new().map_err(|_| "Failed to create SWS socket")?;

        sws_socket
            .connect("/tmp/sws.sock")
            .map_err(|_| "Failed to connect to SWS")?;

        println!("[Bridge] Connected to SWS, registering as extension");

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

        // Read response
        let mut response_buf = [0u8; 1024];
        let n = sws_socket_mut
            .read(&mut response_buf)
            .map_err(|_| "Failed to read EXTENSION_REGISTERED response")?;

        if n >= 12 {
            let mut header_bytes = [0u8; 8];
            header_bytes.copy_from_slice(&response_buf[0..8]);
            let resp_header = protocol_sws::MessageHeader::from_le_bytes(header_bytes);
            if resp_header.msg_type == protocol_sws::server_msg::EXTENSION_REGISTERED {
                let extension_id = u32::from_le_bytes([
                    response_buf[8],
                    response_buf[9],
                    response_buf[10],
                    response_buf[11],
                ]);
                self.extension_id = Some(extension_id);
                println!("[Bridge] Registered as extension with ID: {}", extension_id);
            }
        }

        self.sws_connection = Some(sws_socket_mut);
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

            println!(
                "[Bridge] Creating SWS window for surface {} ({}x{})",
                wl_surface_id, width, height
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

            // Read WINDOW_CREATED response
            let mut response_buf = [0u8; 1024];
            let n = sws_conn
                .read(&mut response_buf)
                .map_err(|_| "Failed to read WINDOW_CREATED response")?;

            if n >= 20 {
                let mut header_bytes = [0u8; 8];
                header_bytes.copy_from_slice(&response_buf[0..8]);
                let resp_header = protocol_sws::MessageHeader::from_le_bytes(header_bytes);
                if resp_header.msg_type == protocol_sws::server_msg::WINDOW_CREATED {
                    let window_id = u32::from_le_bytes([
                        response_buf[8],
                        response_buf[9],
                        response_buf[10],
                        response_buf[11],
                    ]);
                    let shm_size = u64::from_le_bytes([
                        response_buf[12],
                        response_buf[13],
                        response_buf[14],
                        response_buf[15],
                        response_buf[16],
                        response_buf[17],
                        response_buf[18],
                        response_buf[19],
                    ]) as usize;

                    println!(
                        "[Bridge] SWS window created: {} for surface {} (shm_size={})",
                        window_id, wl_surface_id, shm_size
                    );
                    self.surface_to_window.insert(wl_surface_id, window_id);
                    window_id_opt = Some(window_id);

                    // Receive SHM handle and store it
                    if let Ok(shm_handle) = sws_conn.recv_handle() {
                        println!("[Bridge] Received SHM handle for window {}", window_id);
                        // Wrap the handle in SharedMemory
                        if let Ok(shm) = SharedMemory::from_handle(shm_handle) {
                            // Map the SHM for read/write access
                            if let Ok(mapper) = shm.as_handle().as_memory_mapping() {
                                if let Ok(mapped_addr) = mapper.mmap(
                                    0,
                                    shm_size,
                                    permissions::READ_WRITE,
                                    flags::SHARED,
                                    0,
                                ) {
                                    println!(
                                        "[Bridge] Mapped window {} SHM at 0x{:x}",
                                        window_id, mapped_addr
                                    );
                                    self.window_shm.insert(
                                        window_id,
                                        WindowShmInfo {
                                            window_id,
                                            shm,
                                            mapped_addr,
                                            size: shm_size,
                                        },
                                    );
                                } else {
                                    println!("[Bridge] Failed to map window {} SHM", window_id);
                                }
                            } else {
                                println!(
                                    "[Bridge] Window {} SHM doesn't support mapping",
                                    window_id
                                );
                            }
                        } else {
                            println!("[Bridge] Received handle is not a shared memory object");
                        }
                    }
                }
            }
        }

        if let Some(window_id) = window_id_opt {
            if let Some(surface) = self.surface_manager.get_surface(wl_surface_id) {
                if let Some(buffer_id) = surface.buffer_id {
                    let _ = self.send_extension_attach_buffer(wl_surface_id, window_id, buffer_id);
                }
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

            println!(
                "[Bridge] Creating SWS window for surface {} ({}x{})",
                wl_surface_id, width, height
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

            // Read WINDOW_CREATED response
            let mut response_buf = [0u8; 1024];
            let n = sws_conn
                .read(&mut response_buf)
                .map_err(|_| "Failed to read WINDOW_CREATED response")?;

            if n >= 20 {
                let mut header_bytes = [0u8; 8];
                header_bytes.copy_from_slice(&response_buf[0..8]);
                let resp_header = protocol_sws::MessageHeader::from_le_bytes(header_bytes);
                if resp_header.msg_type == protocol_sws::server_msg::WINDOW_CREATED {
                    let window_id = u32::from_le_bytes([
                        response_buf[8],
                        response_buf[9],
                        response_buf[10],
                        response_buf[11],
                    ]);
                    let shm_size = u64::from_le_bytes([
                        response_buf[12],
                        response_buf[13],
                        response_buf[14],
                        response_buf[15],
                        response_buf[16],
                        response_buf[17],
                        response_buf[18],
                        response_buf[19],
                    ]) as usize;

                    println!(
                        "[Bridge] SWS window created: {} for surface {} (shm_size={})",
                        window_id, wl_surface_id, shm_size
                    );
                    self.surface_to_window.insert(wl_surface_id, window_id);
                    window_id_opt = Some(window_id);

                    // Receive SHM handle and store it
                    if let Ok(shm_handle) = sws_conn.recv_handle() {
                        println!("[Bridge] Received SHM handle for window {}", window_id);

                        if let Ok(shm) = SharedMemory::from_handle(shm_handle) {
                            if let Ok(mapper) = shm.as_handle().as_memory_mapping() {
                                if let Ok(mapped_addr) = mapper.mmap(
                                    0,
                                    shm_size,
                                    permissions::READ_WRITE,
                                    flags::SHARED,
                                    0,
                                ) {
                                    println!(
                                        "[Bridge] Mapped window {} SHM at 0x{:x}",
                                        window_id, mapped_addr
                                    );
                                    self.window_shm.insert(
                                        window_id,
                                        WindowShmInfo {
                                            window_id,
                                            shm,
                                            mapped_addr,
                                            size: shm_size,
                                        },
                                    );
                                } else {
                                    println!("[Bridge] Failed to map window {} SHM", window_id);
                                }
                            } else {
                                println!(
                                    "[Bridge] Window {} SHM doesn't support mapping",
                                    window_id
                                );
                            }
                        } else {
                            println!("[Bridge] Received handle is not a shared memory object");
                        }
                    }
                }
            }
        }

        if let Some(window_id) = window_id_opt {
            if let Some(surface) = self.surface_manager.get_surface(wl_surface_id) {
                if let Some(buffer_id) = surface.buffer_id {
                    let _ = self.send_extension_attach_buffer(wl_surface_id, window_id, buffer_id);
                }
            }
        }

        Ok(())
    }

    /// Update SWS window buffer when surface commits
    fn update_sws_window(&mut self, wl_surface_id: u32) -> Result<(), &'static str> {
        let window_id = self
            .surface_to_window
            .get(&wl_surface_id)
            .ok_or("Surface not mapped to window")?;

        let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;

        // Get surface and buffer info
        let surface = self
            .surface_manager
            .get_surface(wl_surface_id)
            .ok_or("Surface not found")?;

        let buffer_id = surface.buffer_id.ok_or("No buffer attached")?;
        let buffer = self
            .shm_manager
            .get_buffer(buffer_id)
            .ok_or("Buffer not found")?;
        let pool = self
            .shm_manager
            .get_pool(buffer.pool_id)
            .ok_or("Pool not found")?;
        let client_shm_handle = pool.handle.as_ref().ok_or("Pool missing handle")?;

        // Get SWS window SHM info
        let window_shm_info = self
            .window_shm
            .get(&window_id)
            .ok_or("Window SHM not found")?;

        // Use full surface damage for simplicity (or first damage rect if available)
        let (x, y, width, height) = if let Some(&(dx, dy, dw, dh)) = surface.damage.first() {
            (dx, dy, dw as u32, dh as u32)
        } else {
            (0, 0, surface.width, surface.height)
        };

        if is_debug_enabled() {
            println!(
                "[Bridge] Updating SWS window {} with damage [{},{} {}x{}]",
                window_id, x, y, width, height
            );
        }

        // Fallback: Use pixel copy for buffers that weren't zero-copied
        // Map client's SHM pool to read pixel data
        if let Ok(client_mapper) = client_shm_handle.as_memory_mapping() {
            let pool_size = pool.size.max(
                (buffer.offset.abs() as usize)
                    + (buffer.stride.abs() as usize).saturating_mul(buffer.height.max(0) as usize),
            );
            if let Ok(client_addr) =
                client_mapper.mmap(0, pool_size, permissions::READ, flags::SHARED, 0)
            {
                // println!("[Bridge] Mapped client SHM at 0x{:x}", client_addr);

                // Copy pixel data from client SHM to SWS window SHM
                let src_width = buffer.width.max(0) as usize;
                let src_height = buffer.height.max(0) as usize;
                let src_stride = buffer.stride.max(0) as usize;
                let src_offset = buffer.offset.max(0) as usize;

                let dst_stride = (surface.width.max(0) as u32 * 4) as usize;

                // println!(
                //     "[Bridge] Copying pixels: {}x{} stride={} src_offset={} dst_stride={}",
                //     src_width, src_height, src_stride, src_offset, dst_stride
                // );

                // Copy row by row
                unsafe {
                    for row in 0..src_height.min(height as usize) {
                        let src_row_offset = src_offset + row * src_stride;
                        let dst_row_offset = (y as usize + row) * dst_stride + (x as usize * 4);

                        let src_start = client_addr + src_row_offset;
                        let dst_start = window_shm_info.mapped_addr + dst_row_offset;

                        let bytes_to_copy = (src_width * 4)
                            .min((window_shm_info.size.saturating_sub(dst_row_offset)));

                        // Copy pixel data
                        core::ptr::copy_nonoverlapping(
                            src_start as *const u8,
                            dst_start as *mut u8,
                            bytes_to_copy.min(src_stride),
                        );
                    }
                }

                // println!("[Bridge] Pixel copy complete");
            } else {
                println!("[Bridge] Failed to map client SHM");
            }
        } else {
            println!("[Bridge] Client SHM handle doesn't support memory mapping");
        }

        // Send EXTENSION_UPDATE_BUFFER message
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

        // println!("[Bridge] === EXTENSION_ATTACH_BUFFER ===");
        // println!("[Bridge]   surface_id={}, window_id={}, buffer_id={}", surface_id, window_id, buffer_id);
        // println!("[Bridge]   geometry={}x{} stride={} offset={} format={} shm_size={}",
        //     width, height, stride, offset, format, shm_size);
        // println!("[Bridge]   client_shm_handle={:?}", handle.as_raw());

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

        // println!("[Bridge] Sending EXTENSION_ATTACH_BUFFER message ({} bytes)", msg_bytes.len());
        sws_conn
            .write(&msg_bytes)
            .map_err(|_| "Failed to send EXTENSION_ATTACH_BUFFER")?;
        // println!("[Bridge] EXTENSION_ATTACH_BUFFER message sent successfully");

        // println!("[Bridge] Sending client SHM handle to SWS...");
        sws_conn
            .send_handle(handle)
            .map_err(|_| "Failed to send EXTENSION_ATTACH_BUFFER handle")?;
        // println!("[Bridge] Client SHM handle sent successfully");
        // println!("[Bridge] === EXTENSION_ATTACH_BUFFER COMPLETE ===");

        Ok(())
    }

    /// Resize an SWS window and update the SHM mapping
    fn resize_sws_window(
        &mut self,
        window_id: u32,
        new_width: u32,
        new_height: u32,
    ) -> Result<(), &'static str> {
        let sws_conn = self.sws_connection.as_mut().ok_or("Not connected to SWS")?;

        // Calculate new buffer size
        let new_buffer_size = (new_width as u64)
            .saturating_mul(new_height as u64)
            .saturating_mul(4);

        println!(
            "[Bridge] Resizing window {} to {}x{} ({} bytes)",
            window_id, new_width, new_height, new_buffer_size
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

        sws_conn
            .write(&msg_bytes)
            .map_err(|_| "Failed to send RESIZE_WINDOW")?;

        // Read WINDOW_RESIZED response
        let mut response_buf = [0u8; 1024];
        let n = sws_conn
            .read(&mut response_buf)
            .map_err(|_| "Failed to read WINDOW_RESIZED response")?;

        if n >= 28 {
            let mut header_bytes = [0u8; 8];
            header_bytes.copy_from_slice(&response_buf[0..8]);
            let resp_header = protocol_sws::MessageHeader::from_le_bytes(header_bytes);

            if resp_header.msg_type == protocol_sws::server_msg::WINDOW_RESIZED {
                let resized_window_id = u32::from_le_bytes([
                    response_buf[8],
                    response_buf[9],
                    response_buf[10],
                    response_buf[11],
                ]);
                let shm_size = u64::from_le_bytes([
                    response_buf[12],
                    response_buf[13],
                    response_buf[14],
                    response_buf[15],
                    response_buf[16],
                    response_buf[17],
                    response_buf[18],
                    response_buf[19],
                ]) as usize;
                let _resized_width = u32::from_le_bytes([
                    response_buf[20],
                    response_buf[21],
                    response_buf[22],
                    response_buf[23],
                ]);
                let _resized_height = u32::from_le_bytes([
                    response_buf[24],
                    response_buf[25],
                    response_buf[26],
                    response_buf[27],
                ]);

                println!(
                    "[Bridge] Window {} resized to shm_size={}",
                    resized_window_id, shm_size
                );

                // Receive new SHM handle
                if let Ok(shm_handle) = sws_conn.recv_handle() {
                    println!("[Bridge] Received new SHM handle for window {}", window_id);

                    if let Ok(shm) = SharedMemory::from_handle(shm_handle) {
                        if let Ok(mapper) = shm.as_handle().as_memory_mapping() {
                            if let Ok(mapped_addr) =
                                mapper.mmap(0, shm_size, permissions::READ_WRITE, flags::SHARED, 0)
                            {
                                println!(
                                    "[Bridge] Remapped window {} SHM at 0x{:x}",
                                    window_id, mapped_addr
                                );
                                self.window_shm.insert(
                                    window_id,
                                    WindowShmInfo {
                                        window_id,
                                        shm,
                                        mapped_addr,
                                        size: shm_size,
                                    },
                                );
                            } else {
                                println!("[Bridge] Failed to map resized window {} SHM", window_id);
                            }
                        } else {
                            println!(
                                "[Bridge] Resized window {} SHM doesn't support mapping",
                                window_id
                            );
                        }
                    } else {
                        println!("[Bridge] Received handle is not a shared memory object");
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle a client connection
    fn handle_client(&mut self, mut client: Socket) -> Result<(), &'static str> {
        println!("[Bridge] New client connected");

        let mut buffer: Vec<u8> = Vec::new();

        loop {
            let mut read_buf = [0u8; 4096];
            // Read message from client
            let n = match client.read(&mut read_buf) {
                Ok(0) => {
                    println!("[Bridge] Client disconnected");
                    break;
                }
                Ok(n) => n,
                Err(_) => {
                    println!("[Bridge] Error reading from client");
                    break;
                }
            };

            if is_debug_enabled() {
                println!("[Bridge] Received {} bytes from client", n);
            }
            buffer.extend_from_slice(&read_buf[..n]);

            // Parse and handle messages
            let mut offset = 0;
            while offset + 8 <= buffer.len() {
                let header_bytes = &buffer[offset..offset + 8];
                let mut header_array = [0u8; 8];
                header_array.copy_from_slice(header_bytes);
                let header = MessageHeader::from_bytes(&header_array);

                let msg_size = header.size() as usize;
                if offset + msg_size > buffer.len() {
                    if is_debug_enabled() {
                        println!("[Bridge] Incomplete message, waiting for more data");
                    }
                    break;
                }

                if is_debug_enabled() {
                    println!(
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
                    if is_debug_enabled() {
                        println!(
                            "[Bridge] Sending response: obj={} opcode={} size={}",
                            response.header.object_id,
                            response.header.opcode(),
                            response_bytes.len()
                        );
                    }
                    // Only send KEYMAP with handle for wl_keyboard objects
                    if response.header.opcode() == input::keyboard_event::KEYMAP {
                        // Verify this is actually a wl_keyboard object
                        if let Some(interface) = self.objects.get(&response.header.object_id) {
                            if interface == "wl_keyboard" {
                                if is_debug_enabled() {
                                    println!(
                                        "[Bridge] KEYMAP event for wl_keyboard, sending with handle"
                                    );
                                }
                                if let Some(shm) = self.keymap_shm.as_ref() {
                                    match client
                                        .send_handle_and_data(shm.as_handle(), &response_bytes)
                                    {
                                        Ok(()) => {
                                            if is_debug_enabled() {
                                                println!(
                                                    "[Bridge] KEYMAP sent successfully with handle"
                                                );
                                            }
                                            continue; // Already sent, skip regular write
                                        }
                                        Err(e) => {
                                            println!(
                                                "[Bridge] Failed to send KEYMAP with handle: {:?}, falling back to regular write",
                                                e
                                            );
                                            // Fall through to regular write
                                        }
                                    }
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

            // Check for input events from SWS (from background thread)
            let mut input_events = Vec::new();
            {
                let mut queue = self.input_event_queue.lock();
                input_events.extend(queue.drain(..));
            }
            for input_msg in input_events {
                let msg_bytes = input_msg.encode();
                println!(
                    "[Bridge] Forwarding input event: obj={} opcode={}",
                    input_msg.header.object_id,
                    input_msg.header.opcode()
                );
                if let Err(e) = client.write(&msg_bytes) {
                    println!("[Bridge] Failed to forward input event: {:?}", e);
                }
            }
        }

        Ok(())
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
        let interface = self
            .objects
            .get(&object_id)
            .ok_or("Unknown object ID")?
            .clone();

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
            "xdg_wm_base" => self.handle_xdg_wm_base_message(opcode, payload),
            "xdg_surface" => self.handle_xdg_surface_message(object_id, opcode, payload),
            "xdg_toplevel" => self.handle_xdg_toplevel_message(object_id, opcode, payload),
            _ => {
                println!("[Bridge] Unhandled interface: {}", interface);
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
                println!("[Bridge] wl_display.sync");
                // Parse callback ID from payload
                if payload.len() >= 4 {
                    let callback_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    println!("[Bridge] Sync callback ID: {}", callback_id);
                    self.objects
                        .insert(callback_id, String::from("wl_callback"));

                    // Send done event for the callback
                    let mut msg = WaylandMessage::new(callback_id, 0); // wl_callback.done
                    msg.add_arg(WaylandArg::Uint(0)); // serial
                    let mut msgs = Vec::new();
                    msgs.push(msg);
                    return Ok(msgs);
                }
                Ok(Vec::new())
            }
            protocol::display_request::GET_REGISTRY => {
                println!("[Bridge] wl_display.get_registry");
                // Parse registry ID from payload
                if payload.len() >= 4 {
                    let registry_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    println!("[Bridge] Registry ID: {}", registry_id);
                    self.objects
                        .insert(registry_id, String::from("wl_registry"));

                    return Ok(self.registry.get_global_events(registry_id));
                }
                Ok(Vec::new())
            }
            _ => {
                println!("[Bridge] Unknown wl_display opcode: {}", opcode);
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
                println!("[Bridge] wl_registry.bind");
                // Parse: name (u32), interface (string), version (u32), id (u32)
                if let Some(name) = Self::parse_u32(payload, 0) {
                    println!("[Bridge] Binding global name: {}", name);

                    if let Some((interface_name, offset)) = Self::parse_string(payload, 4) {
                        let version = Self::parse_u32(payload, offset).unwrap_or(0);
                        let new_id = Self::parse_u32(payload, offset + 4).unwrap_or(0);
                        println!(
                            "[Bridge] Bind interface={} version={} new_id={}",
                            interface_name, version, new_id
                        );

                        if let Some(global) = self.registry.get_global(name) {
                            if global.interface == interface_name && new_id != 0 {
                                self.add_object(new_id, interface_name.clone());

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

                                if interface_name == "wl_seat" {
                                    self.input_manager.create_seat(new_id, "seat0");
                                    let mut msgs = Vec::new();
                                    let mut caps = WaylandMessage::new(
                                        new_id,
                                        input::seat_event::CAPABILITIES,
                                    );
                                    caps.add_arg(WaylandArg::Uint(
                                        input::seat_capabilities::POINTER
                                            | input::seat_capabilities::KEYBOARD,
                                    ));
                                    msgs.push(caps);
                                    // NOTE: NAME event is optional, don't send it
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
                                println!(
                                    "[Bridge] Bind mismatch: requested={}, advertised={}",
                                    interface_name, global.interface
                                );
                            }
                        } else {
                            println!("[Bridge] Unknown global name {}", name);
                        }
                    }
                }
                Ok(Vec::new())
            }
            _ => {
                println!("[Bridge] Unknown wl_registry opcode: {}", opcode);
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
                println!("[Bridge] wl_compositor.create_surface");
                if payload.len() >= 4 {
                    let surface_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    println!("[Bridge] Created surface ID: {}", surface_id);
                    self.add_object(surface_id, String::from("wl_surface"));
                    self.surface_manager.create_surface(surface_id);
                }
                Ok(Vec::new())
            }
            _ => {
                println!("[Bridge] Unknown wl_compositor opcode: {}", opcode);
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
            protocol::surface_request::ATTACH => {
                if is_debug_enabled() {
                    println!("[Bridge] wl_surface.attach on surface {}", surface_id);
                }
                if payload.len() >= 12 {
                    let buffer_id = Self::parse_u32(payload, 0).unwrap_or(0);
                    let _x = Self::parse_i32(payload, 4).unwrap_or(0);
                    let _y = Self::parse_i32(payload, 8).unwrap_or(0);

                    if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                        if buffer_id == 0 {
                            surface.buffer_id = None;
                        } else {
                            surface.attach(buffer_id);
                            if let Some(buffer) = self.shm_manager.get_buffer(buffer_id) {
                                let buffer_width = buffer.width.max(0) as u32;
                                let buffer_height = buffer.height.max(0) as u32;
                                surface.width = buffer_width;
                                surface.height = buffer_height;

                                // Check if window already exists
                                if let Some(&window_id) = self.surface_to_window.get(&surface_id) {
                                    // Window exists, check if resize is needed
                                    let old_width = surface.width;
                                    let old_height = surface.height;

                                    if buffer_width != old_width || buffer_height != old_height {
                                        println!(
                                            "[Bridge] Buffer size {}x{} differs from surface {}x{}, resizing window",
                                            buffer_width, buffer_height, old_width, old_height
                                        );
                                        if let Err(e) = self.resize_sws_window(
                                            window_id,
                                            buffer_width,
                                            buffer_height,
                                        ) {
                                            println!("[Bridge] Failed to resize window: {}", e);
                                        }
                                    }
                                } else {
                                    // Window doesn't exist yet, create it with buffer size
                                    println!(
                                        "[Bridge] No window yet, creating with buffer size {}x{}",
                                        buffer_width, buffer_height
                                    );
                                    if let Err(e) = self.create_sws_window_with_size(
                                        surface_id,
                                        buffer_width,
                                        buffer_height,
                                    ) {
                                        println!("[Bridge] Failed to create window: {}", e);
                                    }
                                }
                            }
                        }
                    }

                    if buffer_id != 0 {
                        if let Some(&window_id) = self.surface_to_window.get(&surface_id) {
                            // println!("[Bridge] Sending attach for surface {} buffer {} window {}", surface_id, buffer_id, window_id);
                            if let Err(e) =
                                self.send_extension_attach_buffer(surface_id, window_id, buffer_id)
                            {
                                println!("[Bridge] Failed to send attach buffer: {}", e);
                            }
                        } else {
                            println!("[Bridge] No window ID found for surface {}", surface_id);
                        }
                    }
                }
                Ok(Vec::new())
            }
            protocol::surface_request::DAMAGE => {
                if is_debug_enabled() {
                    println!("[Bridge] wl_surface.damage on surface {}", surface_id);
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
                    println!("[Bridge] wl_surface.commit on surface {}", surface_id);
                }
                let mut release_msg = None;
                let mut callback_msg = None;

                // Check if there's a pending frame callback
                if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                    let callback_id = surface.take_pending_callback();
                    if let Some(cb_id) = callback_id {
                        // Send wl_callback.done event when the surface is committed
                        let time = self.allocate_serial();
                        let mut msg = WaylandMessage::new(cb_id, protocol::callback_event::DONE);
                        msg.add_arg(WaylandArg::Uint(time));
                        callback_msg = Some(msg);
                    }
                }

                let has_role = self
                    .surface_manager
                    .get_surface(surface_id)
                    .map(|surface| surface.role.is_some())
                    .unwrap_or(false);
                if has_role {
                    if let Err(e) = self.update_sws_window(surface_id) {
                        println!("[Bridge] Failed to update SWS window: {}", e);
                    }
                }
                if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                    let buffer_id = surface.buffer_id;
                    surface.commit();
                    if let Some(buf_id) = buffer_id {
                        if self.objects.get(&buf_id).is_some() {
                            let msg = WaylandMessage::new(buf_id, shm::buffer_event::RELEASE);
                            release_msg = Some(msg);
                        }
                    }
                }

                // Collect all messages to send
                let mut msgs = Vec::new();
                if let Some(msg) = release_msg {
                    msgs.push(msg);
                }
                if let Some(msg) = callback_msg {
                    msgs.push(msg);
                }
                Ok(msgs)
            }
            protocol::surface_request::FRAME => {
                if is_debug_enabled() {
                    println!("[Bridge] wl_surface.frame on surface {}", surface_id);
                }
                if payload.len() >= 4 {
                    let callback_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    if is_debug_enabled() {
                        println!("[Bridge] Callback ID: {}", callback_id);
                    }
                    self.add_object(callback_id, String::from("wl_callback"));

                    // Store the callback to be sent when the surface is committed
                    if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                        surface.set_pending_callback(callback_id);
                    }
                }
                Ok(Vec::new())
            }
            _ => {
                println!("[Bridge] Unknown wl_surface opcode: {}", opcode);
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
                println!("[Bridge] wl_shm.create_pool");
                // Payload: new_id (u32) + size (i32) = 8 bytes
                // FD is passed via handle transfer (Socket::recv_handle)
                if payload.len() >= 8 {
                    let pool_id = Self::parse_u32(payload, 0).unwrap_or(0);
                    let size = Self::parse_i32(payload, 4).unwrap_or(0);
                    println!("[Bridge] Created pool ID: {} size: {}", pool_id, size);
                    self.add_object(pool_id, String::from("wl_shm_pool"));
                    let handle_result = client.recv_handle();
                    let handle = match handle_result {
                        Ok(h) => {
                            println!("[Bridge] Received SHM handle for pool {}", pool_id);
                            Some(h)
                        }
                        Err(e) => {
                            println!("[Bridge] Failed to receive SHM handle: {:?}", e);
                            None
                        }
                    };
                    self.shm_manager.create_pool(pool_id, handle, size);
                }
                Ok(Vec::new())
            }
            _ => {
                println!("[Bridge] Unknown wl_shm opcode: {}", opcode);
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
                println!("[Bridge] wl_shm_pool.create_buffer");
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

                    println!(
                        "[Bridge] Buffer: {}x{} stride:{} format:{}",
                        width, height, stride, format
                    );
                    self.add_object(buffer_id, String::from("wl_buffer"));
                    self.shm_manager
                        .create_buffer(buffer_id, pool_id, offset, width, height, stride, format)?;
                }
                Ok(Vec::new())
            }
            shm::shm_pool_request::DESTROY => {
                println!("[Bridge] wl_shm_pool.destroy");
                self.shm_manager.destroy_pool(pool_id);
                Ok(Vec::new())
            }
            shm::shm_pool_request::RESIZE => {
                println!("[Bridge] wl_shm_pool.resize");
                if payload.len() >= 4 {
                    let new_size =
                        i32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    self.shm_manager.resize_pool(pool_id, new_size)?;
                }
                Ok(Vec::new())
            }
            _ => {
                println!("[Bridge] Unknown wl_shm_pool opcode: {}", opcode);
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
                println!("[Bridge] wl_buffer.destroy");
                self.shm_manager.destroy_buffer(buffer_id);
                Ok(Vec::new())
            }
            _ => {
                println!("[Bridge] Unknown wl_buffer opcode: {}", opcode);
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
                println!("[Bridge] xdg_wm_base.get_xdg_surface");
                if payload.len() >= 8 {
                    let xdg_surface_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let wl_surface_id =
                        u32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
                    println!(
                        "[Bridge] XDG surface ID: {} for wl_surface: {}",
                        xdg_surface_id, wl_surface_id
                    );
                    self.objects
                        .insert(xdg_surface_id, String::from("xdg_surface"));
                    self.xdg_shell_manager
                        .create_xdg_surface(xdg_surface_id, wl_surface_id);
                }
                Ok(Vec::new())
            }
            xdg_shell::wm_base_request::PONG => {
                println!("[Bridge] xdg_wm_base.pong");
                Ok(Vec::new())
            }
            _ => {
                println!("[Bridge] Unknown xdg_wm_base opcode: {}", opcode);
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
            xdg_shell::xdg_surface_request::GET_TOPLEVEL => {
                println!("[Bridge] xdg_surface.get_toplevel");
                if payload.len() >= 4 {
                    let xdg_toplevel_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    println!("[Bridge] XDG toplevel ID: {}", xdg_toplevel_id);
                    self.objects
                        .insert(xdg_toplevel_id, String::from("xdg_toplevel"));
                    self.xdg_shell_manager
                        .create_toplevel(xdg_surface_id, xdg_toplevel_id)?;

                    // Get wl_surface_id for focus management
                    let mut wl_surface_id_opt = None;
                    // Set surface role to toplevel
                    if let Some(xdg_surface) =
                        self.xdg_shell_manager.get_xdg_surface(xdg_surface_id)
                    {
                        let wl_surface_id = xdg_surface.wl_surface_id;
                        wl_surface_id_opt = Some(wl_surface_id);
                        if let Some(surface) = self.surface_manager.get_surface_mut(wl_surface_id) {
                            surface.set_role(surface::SurfaceRole::XdgToplevel);
                        }
                    }

                    let serial = self.allocate_serial();
                    let mut toplevel_configure = WaylandMessage::new(
                        xdg_toplevel_id,
                        xdg_shell::xdg_toplevel_event::CONFIGURE,
                    );
                    toplevel_configure.add_arg(WaylandArg::Int(0));
                    toplevel_configure.add_arg(WaylandArg::Int(0));
                    toplevel_configure.add_arg(WaylandArg::Array(Vec::new()));

                    let mut surface_configure = WaylandMessage::new(
                        xdg_surface_id,
                        xdg_shell::xdg_surface_event::CONFIGURE,
                    );
                    surface_configure.add_arg(WaylandArg::Uint(serial));

                    let mut msgs = Vec::new();
                    msgs.push(toplevel_configure);
                    msgs.push(surface_configure);

                    // Set this surface as focused and send keyboard.enter if keyboard exists
                    if let Some(wl_surface_id) = wl_surface_id_opt {
                        self.focused_surface = Some(wl_surface_id);
                        if let Some(keyboard_id) = self.focused_keyboard {
                            let serial = self.allocate_serial();
                            let mut enter_msg =
                                WaylandMessage::new(keyboard_id, input::keyboard_event::ENTER);
                            enter_msg.add_arg(WaylandArg::Uint(serial));
                            enter_msg.add_arg(WaylandArg::Object(wl_surface_id));
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
                    }

                    return Ok(msgs);
                }
                Ok(Vec::new())
            }
            xdg_shell::xdg_surface_request::ACK_CONFIGURE => {
                println!("[Bridge] xdg_surface.ack_configure");
                Ok(Vec::new())
            }
            _ => {
                println!("[Bridge] Unknown xdg_surface opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Handle xdg_toplevel messages
    fn handle_xdg_toplevel_message(
        &mut self,
        _xdg_toplevel_id: u32,
        opcode: u16,
        _payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        match opcode {
            xdg_shell::xdg_toplevel_request::SET_TITLE => {
                println!("[Bridge] xdg_toplevel.set_title");
                // Parse title from payload (string format)
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::SET_APP_ID => {
                println!("[Bridge] xdg_toplevel.set_app_id");
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::MOVE => {
                println!("[Bridge] xdg_toplevel.move");
                Ok(Vec::new())
            }
            xdg_shell::xdg_toplevel_request::RESIZE => {
                println!("[Bridge] xdg_toplevel.resize");
                Ok(Vec::new())
            }
            _ => {
                println!("[Bridge] Unknown xdg_toplevel opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Run the bridge server
    fn run(&mut self) -> Result<(), &'static str> {
        println!("[Bridge] Waiting for connections...");

        loop {
            // Accept a connection
            match self.server_socket.accept() {
                Ok(client) => {
                    println!("[Bridge] Accepted connection");
                    // Handle this client (blocking for now)
                    if let Err(e) = self.handle_client(client) {
                        println!("[Bridge] Error handling client: {}", e);
                    }
                }
                Err(e) => {
                    println!("[Bridge] Error accepting connection: {:?}", e);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
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
                println!("[Bridge] wl_seat.get_pointer");
                if payload.len() >= 4 {
                    let pointer_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    println!("[Bridge] Pointer ID: {}", pointer_id);
                    self.add_object(pointer_id, String::from("wl_pointer"));
                    self.input_manager.create_pointer(pointer_id, seat_id);
                }
                Ok(Vec::new())
            }
            input::seat_request::GET_KEYBOARD => {
                println!("[Bridge] wl_seat.get_keyboard");
                if payload.len() >= 4 {
                    let keyboard_id =
                        u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    println!("[Bridge] Keyboard ID: {}", keyboard_id);
                    self.add_object(keyboard_id, String::from("wl_keyboard"));
                    self.input_manager.create_keyboard(keyboard_id, seat_id);

                    // Store keyboard as focused
                    self.focused_keyboard = Some(keyboard_id);

                    // Send keymap event
                    let size = self.ensure_keymap()?;
                    let mut keymap_msg = WaylandMessage::new(keyboard_id, input::keyboard_event::KEYMAP);
                    keymap_msg.add_arg(WaylandArg::Uint(1)); // XKB_V1 format
                    keymap_msg.add_arg(WaylandArg::Fd(0)); // FD (out-of-band handle)
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
                println!("[Bridge] wl_seat.release");
                Ok(Vec::new())
            }
            _ => {
                println!("[Bridge] Unknown wl_seat opcode: {}", opcode);
                Ok(Vec::new())
            }
        }
    }

    /// Handle wl_pointer messages
    fn handle_pointer_message(
        &mut self,
        _pointer_id: u32,
        opcode: u16,
        _payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        println!("[Bridge] wl_pointer opcode: {}", opcode);
        // Pointer events are sent from SWS, not received from client
        Ok(Vec::new())
    }

    /// Handle wl_keyboard messages
    fn handle_keyboard_message(
        &mut self,
        _keyboard_id: u32,
        opcode: u16,
        _payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        println!("[Bridge] wl_keyboard opcode: {}", opcode);
        // Keyboard events are sent from SWS, not received from client
        Ok(Vec::new())
    }

    /// Handle wl_output messages
    fn handle_output_message(
        &mut self,
        _output_id: u32,
        opcode: u16,
        _payload: &[u8],
    ) -> Result<Vec<WaylandMessage>, &'static str> {
        println!("[Bridge] wl_output opcode: {}", opcode);
        Ok(Vec::new())
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

        println!("[Bridge] Spawning input event thread...");

        thread::spawn(move || {
            println!("[Input Thread] Started, connecting to SWS...");

            // Create separate connection to SWS for input events
            let sws_socket = match Socket::new() {
                Ok(s) => s,
                Err(_) => {
                    println!("[Input Thread] Failed to create socket");
                    return;
                }
            };

            if let Err(_) = sws_socket.connect("/tmp/sws.sock") {
                println!("[Input Thread] Failed to connect to SWS");
                return;
            }

            println!("[Input Thread] Connected to SWS, listening for input events");

            let mut sws = sws_socket;
            let mut buf = [0u8; 1024];

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

                            println!(
                                "[Input Thread] Received event: type={} code={} value={}",
                                type_, code, value
                            );

                            // Get current objects list
                            let objects_guard = objects_clone.lock();
                            let mut messages = Vec::new();

                            // Convert SWS event to Wayland input events
                            // Type 1 = keyboard, Type 2 = mouse
                            if type_ == 1 {
                                // Keyboard event - forward to all wl_keyboard objects
                                for (id, interface) in objects_guard.iter() {
                                    if interface == "wl_keyboard" {
                                        let mut msg =
                                            WaylandMessage::new(*id, input::keyboard_event::KEY);
                                        msg.add_arg(WaylandArg::Uint(0)); // serial
                                        msg.add_arg(WaylandArg::Uint(time as u32));
                                        msg.add_arg(WaylandArg::Uint(code as u32));
                                        msg.add_arg(WaylandArg::Uint(value as u32)); // state
                                        messages.push(msg);
                                    }
                                }
                            } else if type_ == 2 {
                                // Mouse event - forward to all wl_pointer objects
                                for (id, interface) in objects_guard.iter() {
                                    if interface == "wl_pointer" {
                                        if code == 0 {
                                            // Motion event
                                            let mut msg = WaylandMessage::new(
                                                *id,
                                                input::pointer_event::MOTION,
                                            );
                                            msg.add_arg(WaylandArg::Uint(time as u32));
                                            msg.add_arg(WaylandArg::Int(value)); // x (simplified)
                                            msg.add_arg(WaylandArg::Int(value)); // y (simplified)
                                            messages.push(msg);
                                        } else {
                                            // Button event
                                            let mut msg = WaylandMessage::new(
                                                *id,
                                                input::pointer_event::BUTTON,
                                            );
                                            msg.add_arg(WaylandArg::Uint(0)); // serial
                                            msg.add_arg(WaylandArg::Uint(time as u32));
                                            msg.add_arg(WaylandArg::Uint(code as u32));
                                            msg.add_arg(WaylandArg::Uint(value as u32)); // state
                                            messages.push(msg);
                                        }
                                    }
                                }
                            }

                            // Add messages to shared queue
                            if !messages.is_empty() {
                                let mut queue = input_queue.lock();
                                queue.extend(messages);
                            }
                        }
                    }
                    Ok(_) => {
                        // No data or incomplete message
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => {
                        // Error - wait a bit before retrying
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        });

        Ok(())
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("=== Wayland Bridge Server ===");
    println!("Starting Wayland to SWS bridge...");

    let socket_path = "/tmp/wayland-0";

    let mut bridge = match WaylandBridge::new(socket_path) {
        Ok(b) => b,
        Err(e) => {
            println!("[Bridge] Failed to initialize: {}", e);
            return 1;
        }
    };

    println!("[Bridge] Listening on {}", socket_path);

    // Connect to SWS and register as extension
    if let Err(e) = bridge.connect_to_sws() {
        println!("[Bridge] Failed to connect to SWS: {}", e);
        println!("[Bridge] Make sure SWS is running at /tmp/sws.sock");
        return 1;
    }

    println!("[Bridge] Connected to SWS successfully");

    // Spawn input event listener thread
    if let Err(e) = bridge.spawn_input_thread() {
        println!("[Bridge] Failed to spawn input thread: {}", e);
        return 1;
    }

    println!("[Bridge] Clients can connect with WAYLAND_DISPLAY=wayland-0");

    if let Err(e) = bridge.run() {
        println!("[Bridge] Error: {}", e);
        return 1;
    }

    0
}
