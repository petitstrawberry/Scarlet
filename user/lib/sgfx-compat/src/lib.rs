//! Scarlet composition facade for SGFX applications and platform integrations.
//!
//! This environment-level crate owns concrete backend composition for Scarlet.
//! Complete backend crates validate, lower, budget, and submit SGFX IR for one
//! execution contract; consumers use this facade without naming that backend.
//!
//! The current Scarlet binding is VirGL. A future AGX backend belongs here as
//! another facade-selected implementation, not inside the VirGL backend.

#![cfg_attr(not(feature = "std"), no_std)]

pub use sgfx_backend_scarlet_virgl::{
    Capabilities, Color, CompositionPass, Context, CullMode, Executor, FrontFace, Handle,
    HandleError, HandleResult, Image, IrResources, IrSubmitError, MAX_COMPOSITION_OPERATIONS,
    MappedTargetSession, Pipeline, PipelineDesc, PipelineKind, PixelRect, Queue, RenderPass,
    SourceAlpha, Texture, UnsupportedIrFeature, VertexClip4Color3, Viewport, ir,
};

/// Scarlet graphics device selected by the SGFX environment facade.
pub struct Device {
    backend: sgfx_backend_scarlet_virgl::Device,
}

impl Device {
    /// Open a Scarlet graphics device through a compatible complete SGFX backend.
    ///
    /// The facade owns which complete backend implementation is compiled and
    /// exposed. The selected backend remains responsible for validating that
    /// the device matches its own execution contract.
    ///
    /// # Arguments
    ///
    /// * `path` - Device path such as `/dev/gpu0`.
    ///
    /// # Returns
    ///
    /// An opened facade device or a handle error when the selected backend does
    /// not accept the device.
    pub fn open(path: &str) -> HandleResult<Self> {
        Ok(Self {
            backend: sgfx_backend_scarlet_virgl::Device::open(path)?,
        })
    }

    /// Return the application-level capabilities of the selected backend.
    pub fn capabilities(&self) -> Capabilities {
        self.backend.capabilities()
    }

    /// Create a graphics context through the selected backend.
    pub fn create_context(&self) -> HandleResult<Context> {
        self.backend.create_context()
    }
}

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
