//! Slint Backend for Scarlet Window Server
//!
//! This crate provides a complete Slint platform backend that integrates with the
//! Scarlet Window Server (SWS).

#![no_std]

extern crate scarlet_std as std;

mod platform;
mod window_adapter;
mod event_loop;

pub use platform::ScarletPlatform;

use std::boxed::Box;

/// Initialize the Slint-Scarlet backend.
///
/// This must be called before creating any Slint components.
pub fn init() -> Result<(), slint::platform::PlatformError> {
    let platform = ScarletPlatform::new()?;
    slint::platform::set_platform(Box::new(platform))
        .map_err(|_| slint::platform::PlatformError::Other("Failed to set platform".into()))
}
