//! Builder pattern for window creation

use crate::Connection;
use crate::error::Error;

/// Builder for creating windows with many optional parameters
///
/// # Example
///
/// ```no_run
/// use sws_client::{Connection, SurfaceBuilder};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let mut conn = Connection::connect_default()?;
///
/// // Simple window with defaults
/// let window_id = SurfaceBuilder::new()
///     .app_id("com.example.app")
///     .app_name("My App")
///     .size(800, 600)
///     .build(&mut conn)?;
///
/// // Desktop background window
/// let desktop_id = SurfaceBuilder::new()
///     .app_id("com.example.desktop")
///     .app_name("Desktop")
///     .size(1920, 1080)
///     .window_type(sws_protocol::window_types::DESKTOP)
///     .resizable(false)
///     .focus_on_create(false)
///     .active_on_focus(false)
///     .build(&mut conn)?;
/// # Ok(())
/// # }
/// ```
pub struct SurfaceBuilder<'a> {
    // Required fields
    app_id: Option<&'a str>,
    app_name: Option<&'a str>,
    menu_titles: Option<&'a str>,
    width: u32,
    height: u32,

    // Optional fields with defaults
    window_type: u32,
    resizable: bool,
    focus_on_create: bool,
    active_on_focus: bool,
    position: Option<(i32, i32)>,
}

impl<'a> SurfaceBuilder<'a> {
    /// Create a new builder with default values
    ///
    /// Required fields that must be set before calling `build()`:
    /// - `app_id`
    /// - `app_name`
    /// - `size` (or `width` + `height` separately)
    pub fn new() -> Self {
        Self {
            app_id: None,
            app_name: None,
            menu_titles: Some(""),
            width: 800,
            height: 600,
            window_type: 0, // NORMAL
            resizable: true,
            focus_on_create: true,
            active_on_focus: true,
            position: None,
        }
    }

    /// Set the application identifier (e.g., "com.example.app")
    pub fn app_id(mut self, app_id: &'a str) -> Self {
        self.app_id = Some(app_id);
        self
    }

    /// Set the application name (e.g., "My Application")
    pub fn app_name(mut self, app_name: &'a str) -> Self {
        self.app_name = Some(app_name);
        self
    }

    /// Set the menu titles (format: "menu1|menu2|menu3")
    pub fn menu_titles(mut self, menu_titles: &'a str) -> Self {
        self.menu_titles = Some(menu_titles);
        self
    }

    /// Set the window size (width and height)
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Set the window width
    pub fn width(mut self, width: u32) -> Self {
        self.width = width;
        self
    }

    /// Set the window height
    pub fn height(mut self, height: u32) -> Self {
        self.height = height;
        self
    }

    /// Set the window type for Z-order management
    ///
    /// Values from `sws_protocol::window_types`:
    /// - `NORMAL = 0`: Standard application window (default)
    /// - `ALWAYS_ON_TOP = 1`: Stays above normal windows
    /// - `TASKBAR = 2`: Taskbar/dock window
    /// - `DESKTOP = 3`: Desktop background window
    pub fn window_type(mut self, window_type: u32) -> Self {
        self.window_type = window_type;
        self
    }

    /// Set whether the window can be resized by the user
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Set whether the window should be focused on creation
    pub fn focus_on_create(mut self, focus_on_create: bool) -> Self {
        self.focus_on_create = focus_on_create;
        self
    }

    /// Set whether focusing this window should make it the active application
    pub fn active_on_focus(mut self, active_on_focus: bool) -> Self {
        self.active_on_focus = active_on_focus;
        self
    }

    /// Set the initial window position (x, y)
    pub fn position(mut self, x: i32, y: i32) -> Self {
        self.position = Some((x, y));
        self
    }

    /// Build and create the window
    ///
    /// Returns the window ID on success, or an error if:
    /// - Required fields are missing
    /// - The connection fails
    /// - The server returns an error
    pub fn build(self, conn: &mut Connection) -> Result<u32, Error> {
        // Validate required fields
        let app_id = self.app_id.ok_or(Error::InvalidRequest)?;
        let app_name = self.app_name.ok_or(Error::InvalidRequest)?;
        let menu_titles = self.menu_titles.unwrap_or("");

        // Use position-aware method if position is set
        if let Some((x, y)) = self.position {
            conn.create_surface_with_type_and_policies_at(
                app_id,
                app_name,
                menu_titles,
                self.width,
                self.height,
                self.window_type,
                self.resizable,
                self.focus_on_create,
                self.active_on_focus,
                x,
                y,
            )
        } else {
            conn.create_surface_with_type_and_policies(
                app_id,
                app_name,
                menu_titles,
                self.width,
                self.height,
                self.window_type,
                self.resizable,
                self.focus_on_create,
                self.active_on_focus,
            )
        }
    }
}

impl<'a> Default for SurfaceBuilder<'a> {
    fn default() -> Self {
        Self::new()
    }
}
