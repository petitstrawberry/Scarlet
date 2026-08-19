//! Scarlet device and mapped-target integration for the SGFX frontend.

use alloc::rc::Rc;
use sgfx_core::backend::CommandExecutor;

use crate::{BackendKind, Error, Instance, Result, ir};

pub use sgfx_backend_scarlet_virgl::Handle;

/// Backend-neutral Scarlet rendering capabilities.
#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    rendering: bool,
    presentation: bool,
    image_upload: bool,
    depth: bool,
}

impl Capabilities {
    /// Return whether SGFX command execution is available.
    ///
    /// # Returns
    ///
    /// `true` when rendering is supported.
    pub const fn supports_rendering(&self) -> bool {
        self.rendering
    }

    /// Return whether mapped images may be presented.
    ///
    /// # Returns
    ///
    /// `true` when presentation is supported.
    pub const fn supports_presentation(&self) -> bool {
        self.presentation
    }

    /// Return whether sampled image upload is available.
    ///
    /// # Returns
    ///
    /// `true` when image upload is supported.
    pub const fn supports_image_upload(&self) -> bool {
        self.image_upload
    }

    /// Return whether depth attachments are available.
    ///
    /// # Returns
    ///
    /// `true` when depth-enabled passes are supported.
    pub const fn supports_depth(&self) -> bool {
        self.depth
    }
}

/// Scarlet graphics device selected by the SGFX frontend.
pub struct Device {
    backend: sgfx_backend_scarlet_virgl::Device,
}

impl Device {
    /// Open a Scarlet device using process backend selection policy.
    ///
    /// # Arguments
    ///
    /// * `path` - Scarlet GPU device path.
    ///
    /// # Returns
    ///
    /// A selected device or frontend/backend error.
    pub fn open(path: &str) -> Result<Self> {
        Instance::new()?.open_device(path)
    }

    /// Return the complete backend selected for this device.
    ///
    /// # Returns
    ///
    /// The stable Scarlet backend identity.
    pub const fn backend(&self) -> BackendKind {
        BackendKind::ScarletVirgl
    }

    /// Return portable capabilities for the selected Scarlet backend.
    ///
    /// # Returns
    ///
    /// Backend-neutral rendering capabilities.
    pub fn capabilities(&self) -> Capabilities {
        let capabilities = self.backend.capabilities();
        Capabilities {
            rendering: capabilities.supports_rendering(),
            presentation: capabilities.supports_presentation(),
            image_upload: capabilities.supports_image_upload(),
            depth: capabilities.supports_depth(),
        }
    }

    /// Create a context through the selected backend.
    ///
    /// # Returns
    ///
    /// A frontend context or device error.
    pub fn create_context(&self) -> Result<Context> {
        self.backend
            .create_context()
            .map(|backend| Context { backend })
            .map_err(Error::ScarletVirglHandle)
    }
}

impl Instance {
    /// Open a Scarlet graphics device through the selected backend.
    ///
    /// # Arguments
    ///
    /// * `path` - Scarlet GPU device path.
    ///
    /// # Returns
    ///
    /// A frontend device or backend error.
    pub fn open_device(&self, path: &str) -> Result<Device> {
        match self.backend() {
            BackendKind::ScarletVirgl => sgfx_backend_scarlet_virgl::Device::open(path)
                .map(|backend| Device { backend })
                .map_err(Error::ScarletVirglHandle),
            backend => Err(Error::BackendUnavailable(backend)),
        }
    }
}

/// Scarlet context selected by the SGFX frontend.
pub struct Context {
    backend: sgfx_backend_scarlet_virgl::Context,
}

impl Context {
    /// Create and map physical images for logical presentation targets.
    ///
    /// # Arguments
    ///
    /// * `resources` - Logical SGFX resource table.
    /// * `targets` - Presentation texture identities to materialize.
    ///
    /// # Returns
    ///
    /// A backend-owned mapped session or execution error.
    pub fn create_mapped_target_session(
        &self,
        resources: Rc<ir::ResourceTable>,
        targets: &[ir::TextureId],
    ) -> Result<MappedTargetSession> {
        self.backend
            .create_mapped_target_session(resources, targets)
            .map(|backend| MappedTargetSession { backend })
            .map_err(Error::ScarletVirglIr)
    }
}

/// Scarlet mapped-target session selected by the SGFX frontend.
pub struct MappedTargetSession {
    backend: sgfx_backend_scarlet_virgl::MappedTargetSession,
}

impl MappedTargetSession {
    /// Borrow a mapped presentation image without exposing its backend type.
    ///
    /// # Arguments
    ///
    /// * `target` - Logical presentation texture identity.
    ///
    /// # Returns
    ///
    /// A borrowed image view or mapping error.
    pub fn image(&self, target: ir::TextureId) -> Result<ImageRef<'_>> {
        self.backend
            .image(target)
            .map(|backend| ImageRef { backend })
            .map_err(Error::ScarletVirglIr)
    }

    /// Bind the selected backend queue and resources for command execution.
    ///
    /// # Returns
    ///
    /// A frontend executor delegating complete command buffers to VirGL.
    pub fn executor(&mut self) -> Executor<'_> {
        Executor {
            backend: self.backend.executor(),
        }
    }
}

/// Borrowed Scarlet presentation image exposed by the SGFX frontend.
#[derive(Clone, Copy)]
pub struct ImageRef<'a> {
    backend: &'a sgfx_backend_scarlet_virgl::Image,
}

impl ImageRef<'_> {
    /// Return the image width in pixels.
    ///
    /// # Returns
    ///
    /// Physical image width.
    pub fn width(&self) -> u32 {
        self.backend.width()
    }

    /// Return the image height in pixels.
    ///
    /// # Returns
    ///
    /// Physical image height.
    pub fn height(&self) -> u32 {
        self.backend.height()
    }

    /// Borrow the Scarlet shared-image capability.
    ///
    /// # Returns
    ///
    /// Handle retained by the selected backend session.
    pub fn shared_handle(&self) -> &Handle {
        self.backend.shared_handle()
    }
}

/// Scarlet command executor selected by the SGFX frontend.
pub struct Executor<'a> {
    backend: sgfx_backend_scarlet_virgl::Executor<'a>,
}

impl CommandExecutor for Executor<'_> {
    type Error = Error;

    fn execute<'r, 'data>(&mut self, commands: &ir::CommandBuffer<'r, 'data>) -> Result<()> {
        self.backend
            .execute(commands)
            .map_err(Error::ScarletVirglIr)
    }
}
