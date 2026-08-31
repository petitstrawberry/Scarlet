//! SWS Client Library - Low-level client for Scarlet Window Server
//!
//! This library provides the low-level connection management, event dispatch,
//! and shared memory handling for communicating with the Scarlet Window Server.
//!
//! # Architecture
//!
//! This library follows an event-driven design similar to Wayland:
//! - Single socket connection managed internally
//! - Non-blocking I/O with explicit polling
//! - Shared memory for pixel buffers (zero-copy)
//! - Event callbacks for asynchronous notification
//!
//! # Usage
//!
//! ## Basic Window Creation
//!
//! ```no_run
//! use sws_client::{Connection, SurfaceBuilder};
//!
//! // Connect to SWS
//! let connection = Connection::connect_default()?;
//!
//! // Create a surface (window) using builder pattern
//! let window_id = SurfaceBuilder::new()
//!     .app_id("com.example.app")
//!     .app_name("My App")
//!     .size(800, 600)
//!     .build(&connection)?;
//!
//! // Draw to the surface buffer
//! if connection.with_surface_mut(window_id, |surface| {
//!     surface.with_buffer(|buffer, _width, _height| {
//!         // Draw pixels...
//!     });
//! }).is_some() {
//!     connection.commit(window_id)?;
//! }
//!
//! // Poll for events
//! loop {
//!     connection.dispatch()?;
//!     while let Some(event) = connection.poll_event() {
//!         // Handle events...
//!     }
//! }
//! # Ok::<(), sws_client::Error>(())
//! ```
//!
//! ## Desktop Background Window
//!
//! ```no_run
//! use sws_client::{Connection, SurfaceBuilder};
//! use sws_protocol::window_types;
//!
//! # fn main() -> Result<(), sws_client::Error> {
//! let mut connection = Connection::connect_default()?;
//!
//! let desktop_id = SurfaceBuilder::new()
//!     .app_id("com.example.desktop")
//!     .app_name("Desktop")
//!     .size(1920, 1080)
//!     .window_type(window_types::DESKTOP)
//!     .resizable(false)
//!     .focus_on_create(false)
//!     .active_on_focus(false)
//!     .build(&mut connection)?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Window with Initial Position
//!
//! ```no_run
//! use sws_client::{Connection, SurfaceBuilder};
//!
//! # fn main() -> Result<(), sws_client::Error> {
//! let mut connection = Connection::connect_default()?;
//!
//! let window_id = SurfaceBuilder::new()
//!     .app_id("com.example.app")
//!     .app_name("My App")
//!     .size(400, 300)
//!     .position(100, 100)
//!     .build(&mut connection)?;
//! # Ok(())
//! # }
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate scarlet_std as std;

mod builder;
mod connection;
mod error;
pub mod event;
mod os;
mod surface;

pub use builder::SurfaceBuilder;
pub use connection::{
    Capabilities, Connection, EventReceiver, InputMethodInfo, RequestToken, Response,
    SgfxBufferIdentity, WindowListEntry,
};
pub use error::Error;
pub use event::abs_code;
pub use event::event_type;
pub use event::key_code;
pub use event::rel_code;
pub use event::{Event, ImeContextState, InputEnvironment, InputEvent};
pub use os::Handle;
pub use surface::Surface;
pub use sws_protocol::{WindowGeometry, window_state};
pub use sws_protocol::{CursorIcon, SgfxDamageRect};

/// Transient relationship policy flags for child windows.
///
/// This is a typed wrapper around the protocol bitset. Higher-level crates
/// should prefer this over passing raw `u32`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TransientFlags(u32);

impl TransientFlags {
    pub const NONE: Self = Self(0);
    pub const FOLLOW_PARENT_MOVE: Self = Self(sws_protocol::transient_flags::FOLLOW_PARENT_MOVE);
    pub const RAISE_WITH_PARENT: Self = Self(sws_protocol::transient_flags::RAISE_WITH_PARENT);

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl core::ops::BitOr for TransientFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for TransientFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl From<u32> for TransientFlags {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<TransientFlags> for u32 {
    fn from(value: TransientFlags) -> Self {
        value.0
    }
}

/// Backward-compatible raw constants (bits).
///
/// Prefer using [`TransientFlags`] for new code.
pub mod transient_flags {
    use super::TransientFlags;

    pub const FOLLOW_PARENT_MOVE: u32 = TransientFlags::FOLLOW_PARENT_MOVE.bits();
    pub const RAISE_WITH_PARENT: u32 = TransientFlags::RAISE_WITH_PARENT.bits();
}

/// Per-window size constraints.
///
/// All values are in pixels.
/// - `0` means "unset".
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowSizeLimits {
    pub min_width: u32,
    pub min_height: u32,
    pub max_width: u32,
    pub max_height: u32,
}

impl WindowSizeLimits {
    pub const NONE: Self = Self {
        min_width: 0,
        min_height: 0,
        max_width: 0,
        max_height: 0,
    };
}
