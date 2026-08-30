//! Scarlet device and mapped-target integration for the SGFX frontend.

use alloc::rc::Rc;
use gpu_raw::Gpu;
use sgfx_core::backend::CommandExecutor;

use crate::{BackendKind, BackendPreference, Error, Instance, Result, ir};

#[cfg(all(
    not(feature = "backend-scarlet-virgl"),
    feature = "backend-scarlet-adreno"
))]
pub use sgfx_backend_scarlet_adreno::Handle;
#[cfg(feature = "backend-scarlet-virgl")]
pub use sgfx_backend_scarlet_virgl::Handle;

/// Backend-neutral Scarlet rendering capabilities.
#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    rendering: bool,
    presentation: bool,
    image_upload: bool,
    image_readback: bool,
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

    /// Return whether rendered BGRA images can be read back synchronously.
    ///
    /// # Returns
    ///
    /// `true` when image-to-CPU transfer is available.
    pub const fn supports_image_readback(&self) -> bool {
        self.image_readback
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
pub enum Device {
    /// VirGL execution through Scarlet's VirtIO GPU ABI.
    #[cfg(feature = "backend-scarlet-virgl")]
    Virgl(sgfx_backend_scarlet_virgl::Device),
    /// Native Qualcomm Adreno execution through Scarlet's GPU ABI.
    #[cfg(feature = "backend-scarlet-adreno")]
    Adreno(sgfx_backend_scarlet_adreno::Device),
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
        match self {
            #[cfg(feature = "backend-scarlet-virgl")]
            Self::Virgl(_) => BackendKind::ScarletVirgl,
            #[cfg(feature = "backend-scarlet-adreno")]
            Self::Adreno(_) => BackendKind::ScarletAdreno,
        }
    }

    /// Return portable capabilities for the selected Scarlet backend.
    ///
    /// # Returns
    ///
    /// Backend-neutral rendering capabilities.
    pub fn capabilities(&self) -> Capabilities {
        match self {
            #[cfg(feature = "backend-scarlet-virgl")]
            Self::Virgl(device) => {
                let capabilities = device.capabilities();
                Capabilities {
                    rendering: capabilities.supports_rendering(),
                    presentation: capabilities.supports_presentation(),
                    image_upload: capabilities.supports_image_upload(),
                    image_readback: capabilities.supports_image_readback(),
                    depth: capabilities.supports_depth(),
                }
            }
            #[cfg(feature = "backend-scarlet-adreno")]
            Self::Adreno(device) => {
                let capabilities = device.capabilities();
                Capabilities {
                    rendering: capabilities.supports_rendering(),
                    presentation: capabilities.supports_presentation(),
                    image_upload: capabilities.supports_image_upload(),
                    image_readback: capabilities.supports_image_readback(),
                    depth: capabilities.supports_depth(),
                }
            }
        }
    }

    /// Create a context through the selected backend.
    ///
    /// # Returns
    ///
    /// A frontend context or device error.
    pub fn create_context(&self) -> Result<Context> {
        match self {
            #[cfg(feature = "backend-scarlet-virgl")]
            Self::Virgl(device) => device
                .create_context()
                .map(Context::Virgl)
                .map_err(Error::ScarletVirglHandle),
            #[cfg(feature = "backend-scarlet-adreno")]
            Self::Adreno(device) => device
                .create_context()
                .map(Context::Adreno)
                .map_err(Error::ScarletAdrenoHandle),
        }
    }
}

impl Instance {
    /// Open a Scarlet graphics device through the selected backend.
    ///
    /// Automatic selection opens the GPU once, queries its backend identifier,
    /// and moves that same connection into the matching compiled backend.
    ///
    /// # Arguments
    ///
    /// * `path` - Scarlet GPU device path.
    ///
    /// # Returns
    ///
    /// A frontend device or backend error.
    pub fn open_device(&self, path: &str) -> Result<Device> {
        let gpu = Gpu::open(path).map_err(|_| Error::ScarletGpu)?;
        let info = gpu.query_info().map_err(|_| Error::ScarletGpu)?;
        match self.preference() {
            BackendPreference::Auto => open_auto(gpu, info),
            BackendPreference::ScarletVirgl => open_virgl(gpu, info),
            BackendPreference::ScarletAdreno => open_adreno(gpu, info),
            BackendPreference::Wgpu => Err(Error::BackendUnavailable(BackendKind::Wgpu)),
            BackendPreference::Metal => Err(Error::BackendUnavailable(BackendKind::Metal)),
        }
    }
}

fn open_auto(gpu: Gpu, info: gpu_raw::GpuQueryInfo) -> Result<Device> {
    match select_auto_backend(&info)? {
        BackendKind::ScarletVirgl => open_virgl(gpu, info),
        BackendKind::ScarletAdreno => open_adreno(gpu, info),
        BackendKind::Wgpu | BackendKind::Metal => Err(Error::ScarletBackendUnsupported),
    }
}

fn select_auto_backend(info: &gpu_raw::GpuQueryInfo) -> Result<BackendKind> {
    #[cfg(feature = "backend-scarlet-virgl")]
    if sgfx_backend_scarlet_virgl::Device::supports(&info) {
        return Ok(BackendKind::ScarletVirgl);
    }
    #[cfg(feature = "backend-scarlet-adreno")]
    if sgfx_backend_scarlet_adreno::Device::supports(&info) {
        return Ok(BackendKind::ScarletAdreno);
    }
    Err(Error::ScarletBackendUnsupported)
}

fn open_virgl(gpu: Gpu, info: gpu_raw::GpuQueryInfo) -> Result<Device> {
    #[cfg(feature = "backend-scarlet-virgl")]
    {
        if !sgfx_backend_scarlet_virgl::Device::supports(&info) {
            return Err(Error::BackendDeviceMismatch(BackendKind::ScarletVirgl));
        }
        return sgfx_backend_scarlet_virgl::Device::from_gpu(gpu, info)
            .map(Device::Virgl)
            .map_err(Error::ScarletVirglHandle);
    }
    #[cfg(not(feature = "backend-scarlet-virgl"))]
    {
        let _ = (gpu, info);
        Err(Error::BackendUnavailable(BackendKind::ScarletVirgl))
    }
}

fn open_adreno(gpu: Gpu, info: gpu_raw::GpuQueryInfo) -> Result<Device> {
    #[cfg(feature = "backend-scarlet-adreno")]
    {
        if !sgfx_backend_scarlet_adreno::Device::supports(&info) {
            return Err(Error::BackendDeviceMismatch(BackendKind::ScarletAdreno));
        }
        return sgfx_backend_scarlet_adreno::Device::from_gpu(gpu, info)
            .map(Device::Adreno)
            .map_err(Error::ScarletAdrenoHandle);
    }
    #[cfg(not(feature = "backend-scarlet-adreno"))]
    {
        let _ = (gpu, info);
        Err(Error::BackendUnavailable(BackendKind::ScarletAdreno))
    }
}

/// Scarlet context selected by the SGFX frontend.
pub enum Context {
    /// A VirGL rendering context.
    #[cfg(feature = "backend-scarlet-virgl")]
    Virgl(sgfx_backend_scarlet_virgl::Context),
    /// A native Adreno rendering context.
    #[cfg(feature = "backend-scarlet-adreno")]
    Adreno(sgfx_backend_scarlet_adreno::Context),
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
        match self {
            #[cfg(feature = "backend-scarlet-virgl")]
            Self::Virgl(context) => context
                .create_mapped_target_session(resources, targets)
                .map(MappedTargetSession::Virgl)
                .map_err(Error::ScarletVirglIr),
            #[cfg(feature = "backend-scarlet-adreno")]
            Self::Adreno(context) => context
                .create_mapped_target_session(resources, targets)
                .map(MappedTargetSession::Adreno)
                .map_err(Error::ScarletAdrenoIr),
        }
    }
}

/// Scarlet mapped-target session selected by the SGFX frontend.
pub enum MappedTargetSession {
    /// A VirGL mapped-target session.
    #[cfg(feature = "backend-scarlet-virgl")]
    Virgl(sgfx_backend_scarlet_virgl::MappedTargetSession),
    /// A native Adreno mapped-target session.
    #[cfg(feature = "backend-scarlet-adreno")]
    Adreno(sgfx_backend_scarlet_adreno::MappedTargetSession),
}

impl MappedTargetSession {
    /// Import a transferred shared BGRA image into a logical sampled texture.
    pub fn import_shared_bgra_texture(
        &mut self,
        texture: ir::TextureId,
        handle: Handle,
    ) -> Result<()> {
        match self {
            #[cfg(feature = "backend-scarlet-virgl")]
            Self::Virgl(session) => session
                .import_shared_bgra_texture(texture, handle)
                .map_err(Error::ScarletVirglIr),
            #[cfg(feature = "backend-scarlet-adreno")]
            Self::Adreno(session) => session
                .import_shared_bgra_texture(texture, handle)
                .map_err(Error::ScarletAdrenoIr),
        }
    }

    /// Detach and release a previously imported sampled texture.
    pub fn release_imported_texture(&mut self, texture: ir::TextureId) -> Result<()> {
        match self {
            #[cfg(feature = "backend-scarlet-virgl")]
            Self::Virgl(session) => session
                .release_imported_texture(texture)
                .map_err(Error::ScarletVirglIr),
            #[cfg(feature = "backend-scarlet-adreno")]
            Self::Adreno(session) => session
                .release_imported_texture(texture)
                .map_err(Error::ScarletAdrenoIr),
        }
    }

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
        match self {
            #[cfg(feature = "backend-scarlet-virgl")]
            Self::Virgl(session) => session
                .image(target)
                .map(|image| ImageRef {
                    backend: Image::Virgl(image),
                })
                .map_err(Error::ScarletVirglIr),
            #[cfg(feature = "backend-scarlet-adreno")]
            Self::Adreno(session) => session
                .image(target)
                .map(|image| ImageRef {
                    backend: Image::Adreno(image),
                })
                .map_err(Error::ScarletAdrenoIr),
        }
    }

    /// Read one mapped presentation-target rectangle into a BGRA buffer.
    ///
    /// # Arguments
    ///
    /// * `target` - Logical presentation texture identity.
    /// * `destination` - Complete writable BGRA destination buffer.
    /// * `destination_stride` - Bytes between destination rows.
    /// * `rect` - Source target rectangle written at identical destination coordinates.
    ///
    /// # Returns
    ///
    /// Success after synchronous readback, or an error when the selected
    /// backend does not expose image-to-CPU transfer.
    pub fn readback_bgra(
        &self,
        target: ir::TextureId,
        destination: &mut [u8],
        destination_stride: u32,
        rect: ir::PixelRect,
    ) -> Result<()> {
        match self {
            #[cfg(feature = "backend-scarlet-virgl")]
            Self::Virgl(session) => session
                .readback_bgra(target, destination, destination_stride, rect)
                .map_err(Error::ScarletVirglIr),
            #[cfg(feature = "backend-scarlet-adreno")]
            Self::Adreno(session) => session
                .readback_bgra(target, destination, destination_stride, rect)
                .map_err(Error::ScarletAdrenoIr),
        }
    }

    /// Bind the selected backend queue and resources for command execution.
    ///
    /// # Returns
    ///
    /// A frontend executor delegating complete command buffers to its backend.
    pub fn executor(&mut self) -> Executor<'_> {
        match self {
            #[cfg(feature = "backend-scarlet-virgl")]
            Self::Virgl(session) => Executor::Virgl(session.executor()),
            #[cfg(feature = "backend-scarlet-adreno")]
            Self::Adreno(session) => Executor::Adreno(session.executor()),
        }
    }
}

enum Image<'a> {
    #[cfg(feature = "backend-scarlet-virgl")]
    Virgl(&'a sgfx_backend_scarlet_virgl::Image),
    #[cfg(feature = "backend-scarlet-adreno")]
    Adreno(&'a sgfx_backend_scarlet_adreno::Image),
}

/// Borrowed Scarlet presentation image exposed by the SGFX frontend.
pub struct ImageRef<'a> {
    backend: Image<'a>,
}

impl ImageRef<'_> {
    /// Return the image width in pixels.
    ///
    /// # Returns
    ///
    /// Physical image width.
    pub fn width(&self) -> u32 {
        match &self.backend {
            #[cfg(feature = "backend-scarlet-virgl")]
            Image::Virgl(image) => image.width(),
            #[cfg(feature = "backend-scarlet-adreno")]
            Image::Adreno(image) => image.width(),
        }
    }

    /// Return the image height in pixels.
    ///
    /// # Returns
    ///
    /// Physical image height.
    pub fn height(&self) -> u32 {
        match &self.backend {
            #[cfg(feature = "backend-scarlet-virgl")]
            Image::Virgl(image) => image.height(),
            #[cfg(feature = "backend-scarlet-adreno")]
            Image::Adreno(image) => image.height(),
        }
    }

    /// Borrow the Scarlet shared-image capability.
    ///
    /// # Returns
    ///
    /// Handle retained by the selected backend session.
    pub fn shared_handle(&self) -> &Handle {
        match &self.backend {
            #[cfg(feature = "backend-scarlet-virgl")]
            Image::Virgl(image) => image.shared_handle(),
            #[cfg(feature = "backend-scarlet-adreno")]
            Image::Adreno(image) => image.shared_handle(),
        }
    }
}

/// Scarlet command executor selected by the SGFX frontend.
pub enum Executor<'a> {
    /// A VirGL command executor.
    #[cfg(feature = "backend-scarlet-virgl")]
    Virgl(sgfx_backend_scarlet_virgl::Executor<'a>),
    /// A native Adreno command executor.
    #[cfg(feature = "backend-scarlet-adreno")]
    Adreno(sgfx_backend_scarlet_adreno::Executor<'a>),
}

impl CommandExecutor for Executor<'_> {
    type Error = Error;

    fn execute<'r, 'data>(&mut self, commands: &ir::CommandBuffer<'r, 'data>) -> Result<()> {
        match self {
            #[cfg(feature = "backend-scarlet-virgl")]
            Self::Virgl(executor) => executor.execute(commands).map_err(Error::ScarletVirglIr),
            #[cfg(feature = "backend-scarlet-adreno")]
            Self::Adreno(executor) => executor.execute(commands).map_err(Error::ScarletAdrenoIr),
        }
    }
}

#[cfg(test)]
mod tests {
    use gpu_raw::{
        GPU_DEVICE_STATE_READY, GPU_EXECUTION_SUPPORT_MEMORY, GPU_EXECUTION_SUPPORT_QUEUE,
        GPU_RESULT_SUCCESS, GpuQueryInfo,
    };

    use super::select_auto_backend;
    use crate::BackendKind;

    fn ready_info(backend_id: &[u8]) -> GpuQueryInfo {
        let mut info = GpuQueryInfo::new();
        info.result = GPU_RESULT_SUCCESS;
        info.device_state = GPU_DEVICE_STATE_READY;
        info.execution_support = GPU_EXECUTION_SUPPORT_QUEUE | GPU_EXECUTION_SUPPORT_MEMORY;
        info.max_opaque_command_size = 64 * 1024;
        info.backend_id_len = backend_id.len() as u32;
        info.backend_id[..backend_id.len()].copy_from_slice(backend_id);
        info
    }

    #[cfg(feature = "backend-scarlet-virgl")]
    #[test]
    fn auto_selects_virgl_for_the_virtio_gpu_id() {
        let info = ready_info(sgfx_backend_scarlet_virgl::BACKEND_ID);
        assert_eq!(
            select_auto_backend(&info).unwrap(),
            BackendKind::ScarletVirgl
        );
    }

    #[cfg(feature = "backend-scarlet-adreno")]
    #[test]
    fn auto_selects_adreno_for_the_qcom_adreno_id() {
        let info = ready_info(sgfx_backend_scarlet_adreno::BACKEND_ID);
        assert_eq!(
            select_auto_backend(&info).unwrap(),
            BackendKind::ScarletAdreno
        );
    }
}
