//! XDG Shell Protocol Support
//!
//! The XDG Shell protocol is used by Wayland clients to create desktop
//! windows with standard window management features (minimize, maximize,
//! close, etc.).

use std::collections::BTreeMap;

/// XDG surface state
#[derive(Debug)]
pub struct XdgSurface {
    /// XDG surface object ID
    pub xdg_surface_id: u32,
    /// Underlying wl_surface ID
    pub wl_surface_id: u32,
    /// XDG toplevel (if this is a toplevel window)
    pub toplevel: Option<XdgToplevel>,
    /// Serial from the last configure event sent
    pub last_configure_serial: Option<u32>,
    /// Serial from the last ack_configure received
    pub last_ack_serial: Option<u32>,
}

/// XDG toplevel (application window) state
#[derive(Debug)]
pub struct XdgToplevel {
    /// XDG toplevel object ID
    pub xdg_toplevel_id: u32,
    /// Window title
    pub title: Option<std::string::String>,
    /// App ID (identifier for the application)
    pub app_id: Option<std::string::String>,
    /// Minimum size
    pub min_size: Option<(i32, i32)>,
    /// Maximum size
    pub max_size: Option<(i32, i32)>,
}

impl XdgToplevel {
    pub fn new(xdg_toplevel_id: u32) -> Self {
        Self {
            xdg_toplevel_id,
            title: None,
            app_id: None,
            min_size: None,
            max_size: None,
        }
    }
}

/// XDG Shell manager
pub struct XdgShellManager {
    /// Map of xdg_surface ID -> XdgSurface
    surfaces: BTreeMap<u32, XdgSurface>,
}

impl XdgShellManager {
    /// Create a new XDG shell manager
    pub fn new() -> Self {
        Self {
            surfaces: BTreeMap::new(),
        }
    }

    /// Create a new XDG surface
    pub fn create_xdg_surface(&mut self, xdg_surface_id: u32, wl_surface_id: u32) {
        self.surfaces.insert(
            xdg_surface_id,
            XdgSurface {
                xdg_surface_id,
                wl_surface_id,
                toplevel: None,
                last_configure_serial: None,
                last_ack_serial: None,
            },
        );
    }

    /// Get an XDG surface
    pub fn get_xdg_surface(&self, xdg_surface_id: u32) -> Option<&XdgSurface> {
        self.surfaces.get(&xdg_surface_id)
    }

    /// Get a mutable XDG surface
    pub fn get_xdg_surface_mut(&mut self, xdg_surface_id: u32) -> Option<&mut XdgSurface> {
        self.surfaces.get_mut(&xdg_surface_id)
    }

    /// Get an XDG surface by wl_surface ID
    pub fn get_xdg_surface_by_wl_surface_mut(
        &mut self,
        wl_surface_id: u32,
    ) -> Option<&mut XdgSurface> {
        self.surfaces
            .values_mut()
            .find(|surface| surface.wl_surface_id == wl_surface_id)
    }

    /// Get XDG surface IDs by wl_surface ID
    pub fn get_xdg_surface_ids_by_wl_surface(
        &self,
        wl_surface_id: u32,
    ) -> Option<(u32, Option<u32>)> {
        self.surfaces
            .values()
            .find(|surface| surface.wl_surface_id == wl_surface_id)
            .map(|surface| {
                (
                    surface.xdg_surface_id,
                    surface.toplevel.as_ref().map(|t| t.xdg_toplevel_id),
                )
            })
    }

    /// Create a toplevel for an XDG surface
    pub fn create_toplevel(
        &mut self,
        xdg_surface_id: u32,
        xdg_toplevel_id: u32,
    ) -> Result<(), &'static str> {
        let surface = self
            .surfaces
            .get_mut(&xdg_surface_id)
            .ok_or("XDG surface not found")?;

        surface.toplevel = Some(XdgToplevel::new(xdg_toplevel_id));
        Ok(())
    }

    /// Get a mutable toplevel by ID (returns underlying wl_surface_id too)
    pub fn get_toplevel_mut(&mut self, xdg_toplevel_id: u32) -> Option<(&mut XdgToplevel, u32)> {
        for surface in self.surfaces.values_mut() {
            if let Some(toplevel) = surface.toplevel.as_mut()
                && toplevel.xdg_toplevel_id == xdg_toplevel_id
            {
                return Some((toplevel, surface.wl_surface_id));
            }
        }
        None
    }

    /// Clear a toplevel by ID, returning the owning wl_surface_id if found
    pub fn clear_toplevel(&mut self, xdg_toplevel_id: u32) -> Option<u32> {
        for surface in self.surfaces.values_mut() {
            if let Some(toplevel) = &surface.toplevel
                && toplevel.xdg_toplevel_id == xdg_toplevel_id
            {
                surface.toplevel = None;
                return Some(surface.wl_surface_id);
            }
        }
        None
    }

    /// Destroy an XDG surface
    pub fn destroy_xdg_surface(&mut self, xdg_surface_id: u32) {
        self.surfaces.remove(&xdg_surface_id);
    }
}

/// xdg_wm_base opcodes (requests from client)
pub mod wm_base_request {
    pub const DESTROY: u16 = 0;
    pub const CREATE_POSITIONER: u16 = 1;
    pub const GET_XDG_SURFACE: u16 = 2;
    pub const PONG: u16 = 3;
}

/// xdg_wm_base opcodes (events from server)
pub mod wm_base_event {
    pub const PING: u16 = 0;
}

/// xdg_surface opcodes (requests from client)
pub mod xdg_surface_request {
    pub const DESTROY: u16 = 0;
    pub const GET_TOPLEVEL: u16 = 1;
    pub const GET_POPUP: u16 = 2;
    pub const SET_WINDOW_GEOMETRY: u16 = 3;
    pub const ACK_CONFIGURE: u16 = 4;
}

/// xdg_surface opcodes (events from server)
pub mod xdg_surface_event {
    pub const CONFIGURE: u16 = 0;
}

/// xdg_toplevel opcodes (requests from client)
pub mod xdg_toplevel_request {
    pub const DESTROY: u16 = 0;
    pub const SET_PARENT: u16 = 1;
    pub const SET_TITLE: u16 = 2;
    pub const SET_APP_ID: u16 = 3;
    pub const SHOW_WINDOW_MENU: u16 = 4;
    pub const MOVE: u16 = 5;
    pub const RESIZE: u16 = 6;
    pub const SET_MAX_SIZE: u16 = 7;
    pub const SET_MIN_SIZE: u16 = 8;
    pub const SET_MAXIMIZED: u16 = 9;
    pub const UNSET_MAXIMIZED: u16 = 10;
    pub const SET_FULLSCREEN: u16 = 11;
    pub const UNSET_FULLSCREEN: u16 = 12;
    pub const SET_MINIMIZED: u16 = 13;
}

/// xdg_toplevel opcodes (events from server)
pub mod xdg_toplevel_event {
    pub const CONFIGURE: u16 = 0;
    pub const CLOSE: u16 = 1;
}
