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
pub use surface::Surface;
