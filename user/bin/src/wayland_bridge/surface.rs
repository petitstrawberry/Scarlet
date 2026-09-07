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
    /// Buffer selected by the last committed surface state.
    pub buffer_id: Option<u32>,
    /// Buffer selection staged by `wl_surface.attach` for the next commit.
    ///
    /// The outer option distinguishes "no attach request" from an explicit
    /// `attach(NULL)` represented by `Some(None)`.
    pending_buffer_id: Option<Option<u32>>,
    /// Pending damage regions (x, y, width, height)
    pub damage: Vec<(i32, i32, i32, i32)>,
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

/// Result of applying the buffer state staged for a surface commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferCommit {
    /// Whether this commit contained an explicit `wl_surface.attach` request.
    pub attached: bool,
    /// Whether the effective buffer changed from the previous commit.
    pub changed: bool,
    /// Effective buffer selected after applying the pending state.
    pub buffer_id: Option<u32>,
}

impl Surface {
    /// Create a new surface
    pub fn new(wl_surface_id: u32) -> Self {
        Self {
            wl_surface_id,
            sws_window_id: None,
            buffer_id: None,
            pending_buffer_id: None,
            damage: Vec::new(),
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

    /// Stage a buffer selection for the next surface commit.
    ///
    /// # Arguments
    ///
    /// * `buffer_id` - Buffer object to attach, or `None` to detach content.
    pub fn attach(&mut self, buffer_id: Option<u32>) {
        self.pending_buffer_id = Some(buffer_id);
    }

    /// Apply the pending buffer selection atomically.
    ///
    /// # Returns
    ///
    /// The applied buffer state, including whether this commit contained an
    /// explicit attach request.
    pub fn commit_buffer(&mut self) -> BufferCommit {
        let Some(pending_buffer_id) = self.pending_buffer_id.take() else {
            return BufferCommit {
                attached: false,
                changed: false,
                buffer_id: self.buffer_id,
            };
        };
        let changed = self.buffer_id != pending_buffer_id;
        self.buffer_id = pending_buffer_id;
        BufferCommit {
            attached: true,
            changed,
            buffer_id: self.buffer_id,
        }
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

#[cfg(test)]
mod buffer_state_tests {
    use super::{BufferCommit, Surface};

    #[test]
    fn attach_is_double_buffered_until_commit() {
        let mut surface = Surface::new(7);

        surface.attach(Some(11));
        assert_eq!(surface.buffer_id, None);
        assert_eq!(
            surface.commit_buffer(),
            BufferCommit {
                attached: true,
                changed: true,
                buffer_id: Some(11),
            }
        );
        assert_eq!(surface.buffer_id, Some(11));
    }

    #[test]
    fn commit_without_attach_keeps_the_selected_buffer() {
        let mut surface = Surface::new(7);
        surface.attach(Some(11));
        assert_eq!(
            surface.commit_buffer(),
            BufferCommit {
                attached: true,
                changed: true,
                buffer_id: Some(11),
            }
        );

        assert_eq!(
            surface.commit_buffer(),
            BufferCommit {
                attached: false,
                changed: false,
                buffer_id: Some(11),
            }
        );
        surface.attach(Some(11));
        assert_eq!(
            surface.commit_buffer(),
            BufferCommit {
                attached: true,
                changed: false,
                buffer_id: Some(11),
            }
        );
        surface.attach(None);
        assert_eq!(
            surface.commit_buffer(),
            BufferCommit {
                attached: true,
                changed: true,
                buffer_id: None,
            }
        );
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
