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
    /// Last buffer ID attached to SWS (avoid redundant attach)
    pub last_attached_buffer: Option<u32>,
    /// Last buffer ID we committed (used to delay wl_buffer.release for zero-copy)
    pub last_committed_buffer: Option<u32>,
    /// Buffers pending wl_buffer.release (sent after SWS consumes updates)
    pub pending_release: Vec<u32>,
    /// Surface role (e.g., "xdg_toplevel")
    pub role: Option<SurfaceRole>,
    /// Width and height (set when buffer is attached)
    pub width: u32,
    pub height: u32,
    /// Frame callback IDs requested for the next surface commit.
    pub pending_callbacks: Vec<u32>,
    /// Opaque region (region ID or None for entire surface)
    pub opaque_region: Option<u32>,
    /// Input region (region ID or None for entire surface)
    pub input_region: Option<u32>,
    /// Buffer scale factor (HiDPI support, default 1)
    pub buffer_scale: i32,
    /// Buffer transform (rotation/flipping, default 0 = normal)
    pub buffer_transform: i32,
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
            last_attached_buffer: None,
            last_committed_buffer: None,
            pending_release: Vec::new(),
            role: None,
            width: 0,
            height: 0,
            pending_callbacks: Vec::new(),
            opaque_region: None,
            input_region: None,
            buffer_scale: 1,
            buffer_transform: 0,
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

    /// Add a frame callback to the next surface commit.
    ///
    /// # Arguments
    ///
    /// * `callback_id` - Wayland callback object created by `wl_surface.frame`
    pub fn set_pending_callback(&mut self, callback_id: u32) {
        self.pending_callbacks.push(callback_id);
    }

    /// Take every frame callback associated with the next surface commit.
    ///
    /// # Returns
    ///
    /// Callback object IDs in request order.
    pub fn take_pending_callbacks(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.pending_callbacks)
    }

    /// Commit surface state (this is when changes take effect)
    pub fn commit(&mut self) {
        // Clear damage after commit
        self.damage.clear();
    }

    pub fn swap_committed_buffer(&mut self, new_buffer: Option<u32>) -> Option<u32> {
        let old = self.last_committed_buffer;
        self.last_committed_buffer = new_buffer;
        old
    }

    /// Set the surface role
    pub fn set_role(&mut self, role: SurfaceRole) {
        self.role = Some(role);
    }

    /// Set the buffer scale factor
    pub fn set_buffer_scale(&mut self, scale: i32) {
        self.buffer_scale = scale.max(1); // Minimum scale is 1
    }

    /// Set the buffer transform
    pub fn set_buffer_transform(&mut self, transform: i32) {
        self.buffer_transform = transform;
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

    /// Set the opaque region for a surface
    pub fn set_opaque_region(&mut self, wl_surface_id: u32, region_id: Option<u32>) {
        if let Some(surface) = self.surfaces.get_mut(&wl_surface_id) {
            surface.opaque_region = region_id;
        }
    }

    /// Set the input region for a surface
    pub fn set_input_region(&mut self, wl_surface_id: u32, region_id: Option<u32>) {
        if let Some(surface) = self.surfaces.get_mut(&wl_surface_id) {
            surface.input_region = region_id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Surface;

    #[test]
    fn commit_preserves_all_requested_frame_callbacks() {
        let mut surface = Surface::new(7);
        surface.set_pending_callback(41);
        surface.set_pending_callback(42);

        assert_eq!(surface.take_pending_callbacks(), std::vec![41, 42]);
        assert!(surface.take_pending_callbacks().is_empty());
    }
}
