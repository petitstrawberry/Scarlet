//! Wayland Surface Management
//!
//! Surfaces represent drawable areas that can be displayed on screen.
//! This module manages the mapping between Wayland surfaces and SWS windows.

use std::collections::BTreeMap;
use std::vec::Vec;

/// State of a Wayland surface
#[derive(Debug)]
pub struct Surface {
    /// Wayland surface ID
    pub wl_surface_id: u32,
    /// Corresponding SWS window ID (if created)
    pub sws_window_id: Option<u32>,
    /// Attached buffer ID
    pub buffer_id: Option<u32>,
    /// Pending damage regions (x, y, width, height)
    pub damage: Vec<(i32, i32, i32, i32)>,
    /// Surface role (e.g., "xdg_toplevel")
    pub role: Option<SurfaceRole>,
    /// Width and height (set when buffer is attached)
    pub width: u32,
    pub height: u32,
}

/// Surface role determines how the surface is displayed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    /// XDG toplevel window (normal application window)
    XdgToplevel,
    /// XDG popup window
    XdgPopup,
    /// Cursor surface
    Cursor,
}

impl Surface {
    /// Create a new surface
    pub fn new(wl_surface_id: u32) -> Self {
        Self {
            wl_surface_id,
            sws_window_id: None,
            buffer_id: None,
            damage: Vec::new(),
            role: None,
            width: 0,
            height: 0,
        }
    }

    /// Attach a buffer to this surface
    pub fn attach(&mut self, buffer_id: u32) {
        self.buffer_id = Some(buffer_id);
    }

    /// Add damage to this surface
    pub fn add_damage(&mut self, x: i32, y: i32, width: i32, height: i32) {
        self.damage.push((x, y, width, height));
    }

    /// Commit surface state (this is when changes take effect)
    pub fn commit(&mut self) {
        // Clear damage after commit
        self.damage.clear();
    }

    /// Set the surface role
    pub fn set_role(&mut self, role: SurfaceRole) {
        self.role = Some(role);
    }
}

/// Surface manager
pub struct SurfaceManager {
    /// Map of Wayland surface ID -> Surface
    surfaces: BTreeMap<u32, Surface>,
}

impl SurfaceManager {
    /// Create a new surface manager
    pub fn new() -> Self {
        Self {
            surfaces: BTreeMap::new(),
        }
    }

    /// Create a new surface
    pub fn create_surface(&mut self, wl_surface_id: u32) {
        self.surfaces
            .insert(wl_surface_id, Surface::new(wl_surface_id));
    }

    /// Get a surface by ID
    pub fn get_surface(&self, wl_surface_id: u32) -> Option<&Surface> {
        self.surfaces.get(&wl_surface_id)
    }

    /// Get a mutable surface by ID
    pub fn get_surface_mut(&mut self, wl_surface_id: u32) -> Option<&mut Surface> {
        self.surfaces.get_mut(&wl_surface_id)
    }

    /// Destroy a surface
    pub fn destroy_surface(&mut self, wl_surface_id: u32) {
        self.surfaces.remove(&wl_surface_id);
    }
}
