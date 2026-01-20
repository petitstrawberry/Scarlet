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
//! ```no_run
//! use sws_client::{Connection, Surface};
//!
//! // Connect to SWS
//! let mut connection = Connection::connect("/tmp/sws.sock")?;
//!
//! // Create a surface (window)
//! let surface = connection.create_surface(800, 600)?;
//!
//! // Draw to the surface buffer
//! surface.with_buffer(|buffer| {
//!     // Draw pixels...
//! });
//!
//! // Commit changes
//! surface.commit()?;
//!
//! // Poll for events
//! loop {
//!     connection.dispatch()?;
//! }
//! ```

#![no_std]

extern crate scarlet_std as std;

mod connection;
mod error;
pub mod event;
mod surface;

pub use connection::Connection;
pub use error::Error;
pub use event::{Event, InputEvent};
pub use event::event_type;
pub use event::abs_code;
pub use event::rel_code;
pub use event::key_code;
pub use surface::Surface;

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
