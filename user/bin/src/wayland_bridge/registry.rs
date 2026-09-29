//! Wayland Registry Management
//!
//! The registry is used by clients to discover global objects (interfaces)
//! provided by the server.

use super::protocol::{WaylandArg, WaylandMessage};
use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;

/// A global interface available to clients
#[derive(Debug, Clone)]
pub struct Global {
    pub name: u32,
    pub interface: String,
    pub version: u32,
}

/// Registry manager
pub struct Registry {
    /// Map of global name -> interface info
    globals: BTreeMap<u32, Global>,
    /// Next available global name
    next_name: u32,
}

impl Registry {
    /// Create a new registry
    pub fn new() -> Self {
        let mut registry = Self {
            globals: BTreeMap::new(),
            next_name: 1,
        };

        registry.add_global("wl_seat", 5);
        registry.add_global("wl_compositor", 4);
        registry.add_global("wl_data_device_manager", 3);
        registry.add_global("wl_shm", 1);
        registry.add_global("wl_output", 3);
        registry.add_global("xdg_wm_base", 2);

        registry
    }

    /// Advertise only capabilities implemented by the connected SWS instance.
    pub fn set_surface_scenes(&mut self, enabled: bool) {
        self.globals.retain(|_, global| {
            global.interface != "wl_subcompositor" && global.interface != "wp_viewporter"
        });
        if enabled {
            self.add_global("wl_subcompositor", 1);
            self.add_global("wp_viewporter", 1);
        }
    }

    /// Add a global interface
    fn add_global(&mut self, interface: &str, version: u32) -> u32 {
        let name = self.next_name;
        self.next_name += 1;

        self.globals.insert(
            name,
            Global {
                name,
                interface: String::from(interface),
                version,
            },
        );

        name
    }

    /// Get all globals as Wayland messages for a specific registry object
    pub fn get_global_events(&self, registry_id: u32) -> Vec<WaylandMessage> {
        let mut messages = Vec::new();

        if crate::is_debug_enabled() {
            ::std::println!(
                "[Registry] Sending {} globals to registry {}",
                self.globals.len(),
                registry_id
            );
        }
        for global in self.globals.values() {
            if crate::is_debug_enabled() {
                ::std::println!(
                    "[Registry] Global {}: {} (version {})",
                    global.name,
                    global.interface,
                    global.version
                );
            }
            let mut msg = WaylandMessage::new(registry_id, super::protocol::registry_event::GLOBAL);
            msg.add_arg(WaylandArg::Uint(global.name));
            msg.add_arg(WaylandArg::String(global.interface.as_bytes().to_vec()));
            msg.add_arg(WaylandArg::Uint(global.version));
            messages.push(msg);
        }

        messages
    }

    /// Look up a global by name
    pub fn get_global(&self, name: u32) -> Option<&Global> {
        self.globals.get(&name)
    }
}
