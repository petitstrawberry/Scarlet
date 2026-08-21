//! Legacy compatibility facade for existing Scarlet SGFX applications.
//!
//! New applications and platform composition roots should use the
//! cross-platform `sgfx` frontend. This crate temporarily preserves the former
//! Scarlet/VirGL immediate-mode surface while remaining explicitly legacy.

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

    /// Present one modified SGFX image region through a Scarlet display surface.
    ///
    /// # Arguments
    ///
    /// * `display` - Destination Scarlet display surface.
    /// * `region` - Non-empty image region modified since the preceding present.
    ///
    /// # Returns
    ///
    /// Success after the regional presentation, or a Scarlet handle error.
    fn present_region(
        &self,
        display: &framebuffer::DisplaySurface,
        region: framebuffer::DisplayPresentRegion,
    ) -> HandleResult<()>;
}

impl SgfxImagePresentExt for Image {
    fn present(&self, display: &framebuffer::DisplaySurface) -> HandleResult<()> {
        display.present_image(self.shared_handle(), None)
    }

    fn present_region(
        &self,
        display: &framebuffer::DisplaySurface,
        region: framebuffer::DisplayPresentRegion,
    ) -> HandleResult<()> {
        display.present_image(self.shared_handle(), Some(region))
    }
}
