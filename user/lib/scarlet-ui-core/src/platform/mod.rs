//! Platform abstraction for window systems.

use crate::buffer::Buffer;
use crate::compositor::DamageRect;
use crate::element::TextInputElementState;
use crate::error::Result;
use crate::event::Event;
use crate::geometry::{Point, Size};
use alloc::boxed::Box;
use alloc::string::String;
use core::any::Any;
use std::time::Duration;

/// Parameters used by a backend to create a window.
pub struct WindowCreateRequest {
    /// Stable application identifier.
    pub app_id: String,
    /// Initial window title.
    pub title: String,
    /// Initial logical window size.
    pub size: Size,
    /// Backend-specific window type.
    pub window_type: u32,
    /// Serialized top-level menu titles.
    pub menu_titles: String,
    /// Whether the window should receive focus when created.
    pub focus_on_create: bool,
    /// Whether focusing the window should activate the app.
    pub active_on_focus: bool,
    /// Whether the window contents are fully opaque.
    pub opaque: bool,
}

/// Creates platform windows for the application runner.
pub trait PlatformBackend {
    /// Return the current output scale in milli-units.
    fn output_scale_milli(&mut self) -> u32;

    /// Create a new platform window for the supplied request.
    fn create_window(&mut self, request: WindowCreateRequest) -> Result<Box<dyn PlatformWindow>>;
}

/// Platform-independent window interface
///
/// PlatformWindow abstracts platform-specific window functionality,
/// allowing ScarletUI to work with different window systems.
pub trait PlatformWindow: Any {
    /// Create a new platform window
    fn new(app_id: &str, title: &str, size: Size) -> Result<Self>
    where
        Self: Sized;

    /// Poll for events (returns None if no events available)
    fn poll_event(&mut self) -> Option<Event>;

    /// Wait until more events arrive or the timeout expires.
    fn wait_for_event(&mut self, timeout: Duration) {
        std::thread::sleep(timeout);
    }

    /// Return the window output scale in milli-units.
    fn output_scale_milli(&self) -> u32 {
        1000
    }

    /// Present a buffer to the screen
    fn present(&mut self, buffer: &Buffer);

    /// Present a buffer to the screen with optional physical damage rectangles.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Pixel buffer to present
    /// * `damage` - Physical pixel regions to update, or `None` for the whole buffer
    fn present_with_damage(&mut self, buffer: &Buffer, damage: Option<&[DamageRect]>) {
        let _ = damage;
        self.present(buffer);
    }

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

    /// Get the backend-native window ID as a backend-neutral integer.
    fn platform_window_id(&self) -> u64 {
        self.surface_id() as u64
    }

    /// Return mutable Any for backend-specific escape hatches.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Set whether the window is resizable
    fn set_resizable(&mut self, resizable: bool) -> Result<()>;

    /// Set whether the window contents are fully opaque.
    fn set_opaque(&mut self, opaque: bool) -> Result<()>;

    /// Update menu titles for the window (format: "menu1|menu2|menu3")
    fn set_menu_titles(&mut self, menu_titles: &str) -> Result<()>;

    /// Synchronize focused text-input state with the backend.
    fn sync_text_input(&mut self, _state: Option<&TextInputElementState>) {}
}
