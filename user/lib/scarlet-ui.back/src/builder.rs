//! Window builder for declarative window creation
//!
//! Provides a fluent API for creating windows with event handlers.

use crate::delegate::WindowDelegate;
use crate::window::Window;
use crate::{Application, Color};
use scarlet_std::string::String;

/// Builder for creating windows with a fluent API
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{Application, WindowBuilder};
///
/// struct MyDelegate;
/// impl scarlet_ui::WindowDelegate for MyDelegate {}
///
/// let app = Application::new().unwrap();
/// let window = WindowBuilder::new()
///     .title("My App")
///     .size(800, 600)
///     .resizable(false)
///     .build(&app, MyDelegate);
/// ```
pub struct WindowBuilder {
    title: String,
    width: u32,
    height: u32,
    resizable: bool,
    background: Color,
}

impl WindowBuilder {
    /// Create a new window builder with default settings
    pub fn new() -> Self {
        Self {
            title: String::new(),
            width: 640,
            height: 480,
            resizable: true,
            background: Color::WINDOW_BG,
        }
    }

    /// Set the window title
    pub fn title(mut self, title: &str) -> Self {
        self.title = String::from(title);
        self
    }

    /// Set the window size
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Set whether the window is resizable
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Set the window background color
    pub fn background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    /// Build the window with the given delegate
    ///
    /// The delegate handles all window events.
    pub fn build<D: WindowDelegate + 'static>(
        self,
        app: &mut Application,
        delegate: D,
    ) -> Result<WindowHandle, &'static str> {
        let title = if self.title.is_empty() {
            "Untitled"
        } else {
            self.title.as_str()
        };

        let window = app.create_window_internal(title, self.width, self.height)?;
        let surface_id = window.surface_id();

        // Register delegate with application
        app.register_delegate(surface_id, window, delegate, self.background);

        Ok(WindowHandle { surface_id })
    }
}

impl Default for WindowBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle to a window for external control
///
/// This is a lightweight reference that can be used to:
/// - Request redraw
/// - Close the window
/// - Query window state
#[derive(Debug, Clone, Copy)]
pub struct WindowHandle {
    surface_id: u32,
}

impl WindowHandle {
    /// Get the underlying surface ID
    pub fn id(&self) -> u32 {
        self.surface_id
    }
}
