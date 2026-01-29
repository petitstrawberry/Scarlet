//! Platform abstraction for window systems
//!
//! This module provides the PlatformWindow trait for abstracting
//! different window system backends (SWS, SDL2, Winit, etc.).

mod sws;

pub use sws::SWSPlatformWindow;

use crate::geometry::{Point, Size};
use crate::buffer::Buffer;
use crate::event::Event;
use crate::error::Result;

/// Platform-independent window interface
///
/// PlatformWindow abstracts platform-specific window functionality,
/// allowing ScarletUI to work with different window systems.
pub trait PlatformWindow {
    /// Create a new platform window
    fn new(app_id: &str, title: &str, size: Size) -> Result<Self>
    where
        Self: Sized;

    /// Poll for events (returns None if no events available)
    fn poll_event(&mut self) -> Option<Event>;

    /// Present a buffer to the screen
    fn present(&mut self, buffer: &Buffer);

    /// Set the window title
    fn set_title(&mut self, title: &str);

    /// Get the window size
    fn size(&self) -> Size;

    /// Resize the window
    fn resize(&mut self, width: u32, height: u32) -> Result<()>;

    /// Close the window
    fn close(&mut self) -> Result<()>;

    /// Minimize the window (hide it)
    fn minimize(&mut self) -> Result<()>;

    /// Maximize the window to screen dimensions
    fn maximize(&mut self) -> Result<()>;

    /// Restore the window from minimized or maximized state
    fn restore(&mut self) -> Result<()>;

    /// Request that the window manager begins an interactive move
    fn request_move(&mut self) -> Result<()>;

    /// Create a popup window (e.g., for dropdown menus)
    ///
    /// Returns the surface ID of the created popup window.
    fn create_popup(&mut self, position: Point, size: Size) -> Result<u32>;

    /// Destroy a popup window by surface ID
    fn destroy_popup(&mut self, surface_id: u32) -> Result<()>;

    /// Set the workarea (usable screen space excluding panels like taskbars)
    ///
    /// This informs the window manager about the area available for normal windows.
    fn set_workarea(&mut self, x: i32, y: i32, width: u32, height: u32) -> Result<()>;

    /// Create a window with a specific window type
    ///
    /// This is used to create special windows like TASKBAR, ALWAYS_ON_TOP, etc.
    fn create_window_with_type(
        &mut self,
        app_id: &str,
        title: &str,
        size: Size,
        window_type: u32,
    ) -> Result<Self>
    where
        Self: Sized;

    /// Move a window to a specific position
    fn move_window(&mut self, x: i32, y: i32) -> Result<()>;

    /// Set the window type (NORMAL, TASKBAR, ALWAYS_ON_TOP, etc.)
    fn set_window_type(&mut self, surface_id: u32, window_type: u32) -> Result<()>;

    /// Get the screen size
    fn get_screen_size(&mut self) -> Result<(u32, u32)>;

    /// Get the underlying surface ID (for SWS-specific operations)
    fn surface_id(&self) -> u32;

    /// Set whether the window is resizable
    fn set_resizable(&mut self, resizable: bool) -> Result<()>;

    /// Update menu titles for the window (format: "menu1|menu2|menu3")
    fn set_menu_titles(&mut self, menu_titles: &str) -> Result<()>;
}
