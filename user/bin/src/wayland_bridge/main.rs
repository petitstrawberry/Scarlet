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

mod protocol;
mod registry;
mod shm;
mod surface;
mod xdg_shell;

use protocol::{WaylandArg, WaylandMessage, MessageHeader};
use registry::Registry;
use shm::ShmManager;
use surface::SurfaceManager;
use xdg_shell::XdgShellManager;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::println;
use std::socket::Socket;
use std::string::String;
use std::vec::Vec;

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
    /// Shared memory manager
    shm_manager: ShmManager,
    /// Connection to SWS server
    sws_connection: Option<Socket>,
    /// Next object ID to allocate
    next_object_id: u32,
    /// Map of object ID -> interface name
    objects: BTreeMap<u32, String>,
}

impl WaylandBridge {
    /// Create a new Wayland bridge
    fn new(socket_path: &str) -> Result<Self, &'static str> {
        println!("[Bridge] Creating server socket at {}", socket_path);

        // Create server socket
        let server_socket = Socket::new()
            .map_err(|_| "Failed to create socket")?;

        // Bind to socket path
        server_socket.bind(socket_path)
            .map_err(|_| "Failed to bind socket")?;

        // Listen for connections
        server_socket.listen(5)
            .map_err(|_| "Failed to listen on socket")?;

        println!("[Bridge] Server socket ready");

        let mut objects = BTreeMap::new();
        // Object ID 1 is always wl_display
        objects.insert(1, String::from("wl_display"));

        Ok(Self {
            server_socket,
            registry: Registry::new(),
            surface_manager: SurfaceManager::new(),
            xdg_shell_manager: XdgShellManager::new(),
            shm_manager: ShmManager::new(),
            sws_connection: None,
            next_object_id: 2,
            objects,
        })
    }

    /// Allocate a new object ID
    fn allocate_object_id(&mut self, interface: &str) -> u32 {
        let id = self.next_object_id;
        self.next_object_id += 1;
        self.objects.insert(id, String::from(interface));
        id
    }

    /// Handle a client connection
    fn handle_client(&mut self, mut client: Socket) -> Result<(), &'static str> {
        println!("[Bridge] New client connected");

        let mut buffer = Vec::new();
        buffer.resize(4096, 0);

        loop {
            // Read message from client
            let n = match client.read(&mut buffer) {
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

            println!("[Bridge] Received {} bytes from client", n);

            // Parse and handle messages
            let mut offset = 0;
            while offset + 8 <= n {
                let header_bytes = &buffer[offset..offset + 8];
                let mut header_array = [0u8; 8];
                header_array.copy_from_slice(header_bytes);
                let header = MessageHeader::from_bytes(&header_array);

                let msg_size = header.size() as usize;
                if offset + msg_size > n {
                    println!("[Bridge] Incomplete message, waiting for more data");
                    break;
                }

                println!(
                    "[Bridge] Message: object_id={} opcode={} size={}",
                    header.object_id,
                    header.opcode(),
                    msg_size
                );

                // Handle the message
                if let Some(response) = self.handle_message(&header, &buffer[offset + 8..offset + msg_size])? {
                    let response_bytes = response.encode();
                    client.write(&response_bytes)
                        .map_err(|_| "Failed to send response")?;
                }

                offset += msg_size;
            }
        }

        Ok(())
    }

    /// Handle a Wayland protocol message
    fn handle_message(&mut self, header: &MessageHeader, payload: &[u8]) -> Result<Option<WaylandMessage>, &'static str> {
        let object_id = header.object_id;
        let opcode = header.opcode();

        // Get the interface for this object
        let interface = self.objects.get(&object_id)
            .ok_or("Unknown object ID")?;

        match interface.as_str() {
            "wl_display" => self.handle_display_message(opcode, payload),
            "wl_registry" => self.handle_registry_message(object_id, opcode, payload),
            "wl_compositor" => self.handle_compositor_message(opcode, payload),
            "wl_surface" => self.handle_surface_message(object_id, opcode, payload),
            "wl_shm" => self.handle_shm_message(opcode, payload),
            "wl_shm_pool" => self.handle_shm_pool_message(object_id, opcode, payload),
            "wl_buffer" => self.handle_buffer_message(object_id, opcode, payload),
            "xdg_wm_base" => self.handle_xdg_wm_base_message(opcode, payload),
            "xdg_surface" => self.handle_xdg_surface_message(object_id, opcode, payload),
            "xdg_toplevel" => self.handle_xdg_toplevel_message(object_id, opcode, payload),
            _ => {
                println!("[Bridge] Unhandled interface: {}", interface);
                Ok(None)
            }
        }
    }

    /// Handle wl_display messages
    fn handle_display_message(&mut self, opcode: u16, payload: &[u8]) -> Result<Option<WaylandMessage>, &'static str> {
        match opcode {
            protocol::display_request::SYNC => {
                println!("[Bridge] wl_display.sync");
                // Parse callback ID from payload
                if payload.len() >= 4 {
                    let callback_id = u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    println!("[Bridge] Sync callback ID: {}", callback_id);
                    self.objects.insert(callback_id, String::from("wl_callback"));
                    
                    // Send done event for the callback
                    let mut msg = WaylandMessage::new(callback_id, 0); // wl_callback.done
                    msg.add_arg(WaylandArg::Uint(0)); // serial
                    return Ok(Some(msg));
                }
                Ok(None)
            }
            protocol::display_request::GET_REGISTRY => {
                println!("[Bridge] wl_display.get_registry");
                // Parse registry ID from payload
                if payload.len() >= 4 {
                    let registry_id = u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    println!("[Bridge] Registry ID: {}", registry_id);
                    self.objects.insert(registry_id, String::from("wl_registry"));
                    
                    // Note: In a complete implementation, we would queue all global events
                    // and send them after this handler returns. For now, we just create
                    // the registry object. The client will typically call sync after
                    // get_registry, and we'll send globals in response to bind requests.
                }
                Ok(None)
            }
            _ => {
                println!("[Bridge] Unknown wl_display opcode: {}", opcode);
                Ok(None)
            }
        }
    }

    /// Handle wl_registry messages
    fn handle_registry_message(&mut self, _registry_id: u32, opcode: u16, payload: &[u8]) -> Result<Option<WaylandMessage>, &'static str> {
        match opcode {
            protocol::registry_request::BIND => {
                println!("[Bridge] wl_registry.bind");
                // Parse: name (u32), interface (string), version (u32), id (u32)
                if payload.len() >= 4 {
                    let name = u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    println!("[Bridge] Binding global name: {}", name);
                    
                    // Get interface name before allocating object ID
                    if let Some(global) = self.registry.get_global(name) {
                        let interface_name = global.interface.clone();
                        println!("[Bridge] Bound interface: {}", interface_name);
                        
                        // Now allocate object ID
                        let object_id = self.allocate_object_id(&interface_name);
                        println!("[Bridge] Allocated object ID {} for {}", object_id, interface_name);
                    }
                }
                Ok(None)
            }
            _ => {
                println!("[Bridge] Unknown wl_registry opcode: {}", opcode);
                Ok(None)
            }
        }
    }

    /// Handle wl_compositor messages
    fn handle_compositor_message(&mut self, opcode: u16, payload: &[u8]) -> Result<Option<WaylandMessage>, &'static str> {
        match opcode {
            protocol::compositor_request::CREATE_SURFACE => {
                println!("[Bridge] wl_compositor.create_surface");
                if payload.len() >= 4 {
                    let surface_id = u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    println!("[Bridge] Created surface ID: {}", surface_id);
                    self.objects.insert(surface_id, String::from("wl_surface"));
                    self.surface_manager.create_surface(surface_id);
                }
                Ok(None)
            }
            _ => {
                println!("[Bridge] Unknown wl_compositor opcode: {}", opcode);
                Ok(None)
            }
        }
    }

    /// Handle wl_surface messages
    fn handle_surface_message(&mut self, surface_id: u32, opcode: u16, _payload: &[u8]) -> Result<Option<WaylandMessage>, &'static str> {
        match opcode {
            protocol::surface_request::ATTACH => {
                println!("[Bridge] wl_surface.attach on surface {}", surface_id);
                Ok(None)
            }
            protocol::surface_request::DAMAGE => {
                println!("[Bridge] wl_surface.damage on surface {}", surface_id);
                Ok(None)
            }
            protocol::surface_request::COMMIT => {
                println!("[Bridge] wl_surface.commit on surface {}", surface_id);
                if let Some(surface) = self.surface_manager.get_surface_mut(surface_id) {
                    surface.commit();
                }
                Ok(None)
            }
            _ => {
                println!("[Bridge] Unknown wl_surface opcode: {}", opcode);
                Ok(None)
            }
        }
    }

    /// Handle wl_shm messages
    fn handle_shm_message(&mut self, opcode: u16, payload: &[u8]) -> Result<Option<WaylandMessage>, &'static str> {
        match opcode {
            shm::shm_request::CREATE_POOL => {
                println!("[Bridge] wl_shm.create_pool");
                if payload.len() >= 12 {
                    let pool_id = u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    // Note: FD is passed via handle transfer (Socket::recv_handle)
                    // The Linux compatibility layer converts SCM_RIGHTS to handle transfer
                    let size = i32::from_ne_bytes([payload[8], payload[9], payload[10], payload[11]]);
                    println!("[Bridge] Created pool ID: {} size: {}", pool_id, size);
                    self.objects.insert(pool_id, String::from("wl_shm_pool"));
                    // TODO: Receive handle using Socket::recv_handle() and store it
                    self.shm_manager.create_pool(pool_id, -1, size); // FD will be received separately
                }
                Ok(None)
            }
            _ => {
                println!("[Bridge] Unknown wl_shm opcode: {}", opcode);
                Ok(None)
            }
        }
    }

    /// Handle wl_shm_pool messages
    fn handle_shm_pool_message(&mut self, pool_id: u32, opcode: u16, payload: &[u8]) -> Result<Option<WaylandMessage>, &'static str> {
        match opcode {
            shm::shm_pool_request::CREATE_BUFFER => {
                println!("[Bridge] wl_shm_pool.create_buffer");
                if payload.len() >= 24 {
                    let buffer_id = u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let offset = i32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
                    let width = i32::from_ne_bytes([payload[8], payload[9], payload[10], payload[11]]);
                    let height = i32::from_ne_bytes([payload[12], payload[13], payload[14], payload[15]]);
                    let stride = i32::from_ne_bytes([payload[16], payload[17], payload[18], payload[19]]);
                    let format = u32::from_ne_bytes([payload[20], payload[21], payload[22], payload[23]]);
                    
                    println!("[Bridge] Buffer: {}x{} stride:{} format:{}", width, height, stride, format);
                    self.objects.insert(buffer_id, String::from("wl_buffer"));
                    self.shm_manager.create_buffer(buffer_id, pool_id, offset, width, height, stride, format)?;
                }
                Ok(None)
            }
            shm::shm_pool_request::DESTROY => {
                println!("[Bridge] wl_shm_pool.destroy");
                self.shm_manager.destroy_pool(pool_id);
                Ok(None)
            }
            shm::shm_pool_request::RESIZE => {
                println!("[Bridge] wl_shm_pool.resize");
                if payload.len() >= 4 {
                    let new_size = i32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    self.shm_manager.resize_pool(pool_id, new_size)?;
                }
                Ok(None)
            }
            _ => {
                println!("[Bridge] Unknown wl_shm_pool opcode: {}", opcode);
                Ok(None)
            }
        }
    }

    /// Handle wl_buffer messages
    fn handle_buffer_message(&mut self, buffer_id: u32, opcode: u16, _payload: &[u8]) -> Result<Option<WaylandMessage>, &'static str> {
        match opcode {
            shm::buffer_request::DESTROY => {
                println!("[Bridge] wl_buffer.destroy");
                self.shm_manager.destroy_buffer(buffer_id);
                Ok(None)
            }
            _ => {
                println!("[Bridge] Unknown wl_buffer opcode: {}", opcode);
                Ok(None)
            }
        }
    }

    /// Handle xdg_wm_base messages
    fn handle_xdg_wm_base_message(&mut self, opcode: u16, payload: &[u8]) -> Result<Option<WaylandMessage>, &'static str> {
        match opcode {
            xdg_shell::wm_base_request::GET_XDG_SURFACE => {
                println!("[Bridge] xdg_wm_base.get_xdg_surface");
                if payload.len() >= 8 {
                    let xdg_surface_id = u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    let wl_surface_id = u32::from_ne_bytes([payload[4], payload[5], payload[6], payload[7]]);
                    println!("[Bridge] XDG surface ID: {} for wl_surface: {}", xdg_surface_id, wl_surface_id);
                    self.objects.insert(xdg_surface_id, String::from("xdg_surface"));
                    self.xdg_shell_manager.create_xdg_surface(xdg_surface_id, wl_surface_id);
                }
                Ok(None)
            }
            xdg_shell::wm_base_request::PONG => {
                println!("[Bridge] xdg_wm_base.pong");
                Ok(None)
            }
            _ => {
                println!("[Bridge] Unknown xdg_wm_base opcode: {}", opcode);
                Ok(None)
            }
        }
    }

    /// Handle xdg_surface messages
    fn handle_xdg_surface_message(&mut self, xdg_surface_id: u32, opcode: u16, payload: &[u8]) -> Result<Option<WaylandMessage>, &'static str> {
        match opcode {
            xdg_shell::xdg_surface_request::GET_TOPLEVEL => {
                println!("[Bridge] xdg_surface.get_toplevel");
                if payload.len() >= 4 {
                    let xdg_toplevel_id = u32::from_ne_bytes([payload[0], payload[1], payload[2], payload[3]]);
                    println!("[Bridge] XDG toplevel ID: {}", xdg_toplevel_id);
                    self.objects.insert(xdg_toplevel_id, String::from("xdg_toplevel"));
                    self.xdg_shell_manager.create_toplevel(xdg_surface_id, xdg_toplevel_id)?;
                    
                    // Set surface role to toplevel
                    if let Some(xdg_surface) = self.xdg_shell_manager.get_xdg_surface(xdg_surface_id) {
                        let wl_surface_id = xdg_surface.wl_surface_id;
                        if let Some(surface) = self.surface_manager.get_surface_mut(wl_surface_id) {
                            surface.set_role(surface::SurfaceRole::XdgToplevel);
                        }
                    }
                }
                Ok(None)
            }
            xdg_shell::xdg_surface_request::ACK_CONFIGURE => {
                println!("[Bridge] xdg_surface.ack_configure");
                Ok(None)
            }
            _ => {
                println!("[Bridge] Unknown xdg_surface opcode: {}", opcode);
                Ok(None)
            }
        }
    }

    /// Handle xdg_toplevel messages
    fn handle_xdg_toplevel_message(&mut self, _xdg_toplevel_id: u32, opcode: u16, _payload: &[u8]) -> Result<Option<WaylandMessage>, &'static str> {
        match opcode {
            xdg_shell::xdg_toplevel_request::SET_TITLE => {
                println!("[Bridge] xdg_toplevel.set_title");
                // Parse title from payload (string format)
                Ok(None)
            }
            xdg_shell::xdg_toplevel_request::SET_APP_ID => {
                println!("[Bridge] xdg_toplevel.set_app_id");
                Ok(None)
            }
            xdg_shell::xdg_toplevel_request::MOVE => {
                println!("[Bridge] xdg_toplevel.move");
                Ok(None)
            }
            xdg_shell::xdg_toplevel_request::RESIZE => {
                println!("[Bridge] xdg_toplevel.resize");
                Ok(None)
            }
            _ => {
                println!("[Bridge] Unknown xdg_toplevel opcode: {}", opcode);
                Ok(None)
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
    println!("[Bridge] Clients can connect with WAYLAND_DISPLAY=wayland-0");

    if let Err(e) = bridge.run() {
        println!("[Bridge] Error: {}", e);
        return 1;
    }

    0
}
