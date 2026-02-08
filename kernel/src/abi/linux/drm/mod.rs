//! Linux DRM (Direct Rendering Manager) compatibility module
//!
//! This module provides Linux DRM compatibility for Scarlet kernel,
//! allowing Linux graphics applications to use standard DRM ioctls.

pub mod device;
pub mod ioctl;

use device::register_drm_devices;

/// Initialize Linux DRM subsystem
pub fn init() {
    crate::early_println!("[DRM] Initializing Linux DRM compatibility layer");
    register_drm_devices();
}
