//! Compatibility facade for existing Scarlet SGFX applications.
//!
//! New cross-platform consumers should depend on `sgfx-core` plus an explicit
//! backend. This facade preserves the former `sgfx` package while Scarlet
//! applications migrate to explicit platform composition.

#![cfg_attr(not(feature = "std"), no_std)]

pub use scarlet_sgfx_virgl::*;

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
