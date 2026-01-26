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

use core::sync::atomic::{AtomicBool, Ordering};

/// Height of the client-side title bar when enabled.
pub const TITLEBAR_HEIGHT_PX: u32 = 32;

static USE_CSD_TITLEBAR: AtomicBool = AtomicBool::new(true);

/// Enable or disable the built-in client-side title bar.
///
/// When enabled, Slint content is rendered into the content area below the title bar,
/// and input coordinates are translated accordingly.
pub fn set_use_csd_titlebar(enabled: bool) {
    USE_CSD_TITLEBAR.store(enabled, Ordering::Relaxed);
}

pub(crate) fn use_csd_titlebar() -> bool {
    USE_CSD_TITLEBAR.load(Ordering::Relaxed)
}

/// Initialize the Slint-Scarlet backend.
///
/// This must be called before creating any Slint components.
pub fn init() -> Result<(), slint::platform::PlatformError> {
    let platform = ScarletPlatform::new()?;
    slint::platform::set_platform(Box::new(platform))
        .map_err(|_| slint::platform::PlatformError::Other("Failed to set platform".into()))
}
