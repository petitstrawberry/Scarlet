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
mod surface;

use protocol::{WaylandArg, WaylandMessage, MessageHeader};
use registry::Registry;
use surface::SurfaceManager;
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
                    
                    // Send global events for all available interfaces
                    // We'll send them all at once for simplicity
                    // In a real implementation, we might want to batch these
                    for global_msg in self.registry.get_global_events(registry_id) {
                        let bytes = global_msg.encode();
                        // TODO: Queue these for sending
                        println!("[Bridge] Would send global event: {} bytes", bytes.len());
                    }
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
                    std::thread::sleep(core::time::Duration::from_millis(100));
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
