//! Platform abstraction for window systems
//!
//! This module provides the PlatformWindow trait for abstracting
//! different window system backends (SWS, SDL2, Winit, etc.).

mod sws;

pub use sws::SWSPlatformWindow;

use crate::geometry::Size;
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
}
