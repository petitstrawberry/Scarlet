//! Scarlet composition facade for SGFX applications and platform integrations.
//!
//! This environment-level crate selects and reexports the complete Scarlet
//! execution backend, allowing consumers such as `platform-sws` to own SGFX
//! sessions without naming a VirGL implementation dependency. Cross-platform
//! renderers should depend only on `sgfx-core` and its backend contract.

#![cfg_attr(not(feature = "std"), no_std)]

pub use sgfx_backend_scarlet_virgl::*;

/// Compatibility presentation extension for direct Scarlet framebuffer apps.
///
/// New window-system integrations should own presentation in their platform
/// crate instead of importing this trait into a renderer.
pub trait SgfxImagePresentExt {
    /// Present this SGFX image through a Scarlet display surface.
    ///
    /// # Arguments
    ///
    /// * `display` - Destination Scarlet display surface.
    ///
    /// # Returns
    ///
    /// Success after presentation, or a Scarlet handle error.
    fn present(&self, display: &framebuffer::DisplaySurface) -> HandleResult<()>;
}

impl SgfxImagePresentExt for Image {
    fn present(&self, display: &framebuffer::DisplaySurface) -> HandleResult<()> {
        display.present_image(self.shared_handle(), None)
    }
}
