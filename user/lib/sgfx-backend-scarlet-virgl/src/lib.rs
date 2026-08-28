//! Complete Scarlet GPU ABI and VirGL execution backend for SGFX.
//!
//! This crate owns Scarlet device selection, contexts, queues, physical
//! resources, logical-resource materialization, IR validation and lowering,
//! transport budgeting, synchronization, and command submission. Applications
//! can use mapped-target sessions so none of that execution policy leaks into
//! platform or renderer code.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
#[cfg(not(feature = "std"))]
extern crate scarlet_std as std;

use alloc::{rc::Rc, vec::Vec};
use gpu_raw::{Gpu, GpuQueryInfo};
#[cfg(feature = "std")]
pub use scarlet_os::handle::{Handle, HandleError, HandleResult};
#[cfg(feature = "std")]
use scarlet_os::ipc::SharedMemory;
#[cfg(not(feature = "std"))]
pub use std::handle::{Handle, HandleError, HandleResult};
#[cfg(not(feature = "std"))]
use std::ipc::SharedMemory;

/// Stable backend identifier advertised by Scarlet's VirtIO GPU transport.
pub const BACKEND_ID: &[u8] = b"virtio-gpu";

/// Return whether a Scarlet GPU backend identifier selects VirGL execution.
///
/// # Arguments
///
/// * `backend_id` - Exact backend identifier reported by the GPU service.
///
/// # Returns
///
/// `true` only for Scarlet's stable VirtIO GPU backend identifier.
pub fn matches_backend_id(backend_id: &[u8]) -> bool {
    backend_id == BACKEND_ID
}

/// Backend-neutral logical graphics intermediate representation.
///
/// This module defines validated resource descriptors and command buffers.
/// Supported command subsets can be lowered through [`Queue::submit_ir`].
pub use sgfx_core::ir;

mod driver;
mod ir_execute;
mod virgl;

pub use ir_execute::{IrResources, IrSubmitError, UnsupportedIrFeature};

/// Device capabilities expressed in application rendering terms.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    rendering: bool,
    presentation: bool,
    image_upload: bool,
    depth: bool,
}

impl Capabilities {
    /// Return whether the device can execute the built-in rendering pipeline.
    ///
    /// # Returns
    ///
    /// `true` when rendering commands are available.
    pub const fn supports_rendering(&self) -> bool {
        self.rendering
    }

    /// Return whether render-target images can be presented to a display.
    ///
    /// # Returns
    ///
    /// `true` when presentation is available.
    pub const fn supports_presentation(&self) -> bool {
        self.presentation
    }

    /// Return whether sampled BGRA textures can be uploaded and composed.
    ///
    /// # Returns
    ///
    /// `true` when sampled texture upload and composition are available.
    pub const fn supports_image_upload(&self) -> bool {
        self.image_upload
    }

    /// Return whether depth attachments and depth testing are available.
    ///
    /// # Returns
    ///
    /// `true` when `Depth32Float` render attachments can be used.
    pub const fn supports_depth(&self) -> bool {
        self.depth
    }
}

/// Backend-neutral graphics device connection.
pub struct Device {
    backend: Rc<driver::Device>,
}

impl Device {
    /// Return whether already-queried GPU information selects this backend.
    ///
    /// # Arguments
    ///
    /// * `info` - Information returned by a Scarlet GPU control connection.
    ///
    /// # Returns
    ///
    /// `true` only when this compiled backend can adopt the connection.
    pub fn supports(info: &GpuQueryInfo) -> bool {
        driver::Device::supports(info)
    }

    /// Adopt an already-opened Scarlet GPU connection.
    ///
    /// # Arguments
    ///
    /// * `gpu` - Owning connection used to obtain `info`.
    /// * `info` - Query result returned from that same connection.
    ///
    /// # Returns
    ///
    /// A compatible VirGL device or a handle error.
    pub fn from_gpu(gpu: Gpu, info: GpuQueryInfo) -> HandleResult<Self> {
        Ok(Self {
            backend: Rc::new(driver::Device::from_gpu(gpu, info)?),
        })
    }

    /// Open a graphics device and select a compatible private backend.
    ///
    /// # Arguments
    ///
    /// * `path` - Device path such as `/dev/gpu0`.
    ///
    /// # Returns
    ///
    /// An opened graphics device or a handle error.
    pub fn open(path: &str) -> HandleResult<Self> {
        Ok(Self {
            backend: Rc::new(driver::Device::open(path)?),
        })
    }

    /// Return the rendering capabilities selected for this device.
    ///
    /// # Returns
    ///
    /// Application-level device capabilities.
    pub fn capabilities(&self) -> Capabilities {
        self.backend.capabilities()
    }

    /// Create an application rendering context.
    ///
    /// # Returns
    ///
    /// A context that owns render targets, pipelines, and queues.
    pub fn create_context(&self) -> HandleResult<Context> {
        Ok(Context {
            backend: Rc::new(self.backend.create_context()?),
        })
    }
}

/// Rendering context that owns application graphics objects.
pub struct Context {
    backend: Rc<driver::Context>,
}

impl Context {
    /// Create an empty persistent cache for one logical IR resource table.
    ///
    /// # Arguments
    ///
    /// * `resources` - Shared table retained for the full lifetime of the cache.
    ///   Keep another [`Rc`] clone when command buffers and the mutable cache
    ///   are stored in the same renderer.
    ///
    /// # Returns
    ///
    /// An empty IR resource cache, or [`IrSubmitError::OutOfMemory`] when its
    /// bounded mapping metadata or persistent private backend storage cannot be
    /// allocated. Textures, samplers, and pipelines materialize lazily for this
    /// context as they are first referenced by IR submission.
    pub fn create_ir_resources(
        &self,
        resources: Rc<ir::ResourceTable>,
    ) -> Result<IrResources, IrSubmitError> {
        IrResources::new(resources, &self.backend)
    }

    /// Create and map all physical images for logical presentation targets.
    ///
    /// # Arguments
    ///
    /// * `resources` - Logical resource table retained by the session.
    /// * `targets` - `PRESENT` texture identities to materialize and map.
    ///
    /// # Returns
    ///
    /// A session owning its context, queue, resource cache, and images, or a
    /// logical-resource, allocation, mapping, or device error.
    pub fn create_mapped_target_session(
        &self,
        resources: Rc<ir::ResourceTable>,
        targets: &[ir::TextureId],
    ) -> Result<MappedTargetSession, IrSubmitError> {
        let mut cache = self.create_ir_resources(Rc::clone(&resources))?;
        let queue = self.create_queue()?;
        let mut images = Vec::new();
        images
            .try_reserve_exact(targets.len())
            .map_err(|_| IrSubmitError::OutOfMemory)?;
        for &target in targets {
            if images.iter().any(|(candidate, _)| *candidate == target) {
                return Err(IrSubmitError::TextureAlreadyMapped);
            }
            let reference = resources.texture_ref(target)?;
            let descriptor = resources.texture(reference)?;
            let required = ir::TextureUsage::RENDER_ATTACHMENT | ir::TextureUsage::PRESENT;
            if !descriptor.usage().contains(required) {
                return Err(IrSubmitError::Unsupported(
                    UnsupportedIrFeature::TargetUsage,
                ));
            }
            let image =
                Rc::new(self.create_shared_image(
                    descriptor.extent().width(),
                    descriptor.extent().height(),
                )?);
            cache.map_image(target, Rc::clone(&image))?;
            images.push((target, image));
        }
        Ok(MappedTargetSession {
            imported_textures: Vec::new(),
            context: Context {
                backend: Rc::clone(&self.backend),
            },
            queue,
            resources: cache,
            images,
        })
    }

    /// Create a presentation-capable render-target image.
    ///
    /// # Arguments
    ///
    /// * `width` - Non-zero image width in pixels.
    /// * `height` - Non-zero image height in pixels.
    ///
    /// # Returns
    ///
    /// An image usable by render passes and display presentation.
    pub fn create_image(&self, width: u32, height: u32) -> HandleResult<Image> {
        Ok(Image {
            backend: self.backend.create_image(width, height)?,
            width,
            height,
        })
    }

    /// Create a render target whose GPU image capability can be shared.
    ///
    /// The image remains owned by this context and may be rendered through the
    /// normal SGFX APIs. [`Image::shared_handle`] exposes a borrowed capability
    /// suitable for transferring to a compositor without copying pixels.
    ///
    /// # Arguments
    ///
    /// * `width` - Non-zero image width in pixels.
    /// * `height` - Non-zero image height in pixels.
    ///
    /// # Returns
    ///
    /// A render-target, presentable, sampled image or a handle error.
    pub fn create_shared_image(&self, width: u32, height: u32) -> HandleResult<Image> {
        Ok(Image {
            backend: self.backend.create_shared_image(width, height)?,
            width,
            height,
        })
    }

    /// Create a sampled BGRA texture for subsequent composition passes.
    ///
    /// # Arguments
    ///
    /// * `width` - Non-zero texture width in pixels.
    /// * `height` - Non-zero texture height in pixels.
    ///
    /// # Returns
    ///
    /// A texture that may be uploaded through this context and sampled by a
    /// [`CompositionPass`], or a handle error.
    pub fn create_sampled_bgra_texture(&self, width: u32, height: u32) -> HandleResult<Texture> {
        Ok(Texture {
            backend: self.backend.create_sampled_bgra_texture(width, height)?,
            width,
            height,
        })
    }

    /// Create a sampled BGRA texture imported from a SharedMemory backing store.
    ///
    /// # Arguments
    ///
    /// * `shared_memory` - SharedMemory object containing the source pixels.
    /// * `width` - Non-zero texture width in pixels.
    /// * `height` - Non-zero texture height in pixels.
    /// * `shm_offset` - Byte offset of pixel `(0, 0)` in SharedMemory.
    /// * `source_stride` - Number of bytes between SharedMemory source rows.
    ///
    /// # Returns
    ///
    /// A sampled texture retaining a kernel-side import pin, or a handle error.
    pub fn create_imported_bgra_texture(
        &self,
        shared_memory: &SharedMemory,
        width: u32,
        height: u32,
        shm_offset: usize,
        source_stride: u32,
    ) -> HandleResult<Texture> {
        Ok(Texture {
            backend: self.backend.create_imported_bgra_texture(
                shared_memory,
                width,
                height,
                shm_offset,
                source_stride,
            )?,
            width,
            height,
        })
    }

    /// Import a transferred shared GPU image as a sampled BGRA texture.
    ///
    /// The transferred handle is consumed by the returned texture. The image
    /// must have BGRA8 format and sampled usage, and it must originate from a
    /// GPU backend compatible with this context.
    ///
    /// # Arguments
    ///
    /// * `handle` - Owning GPU image capability transferred from another process.
    ///
    /// # Returns
    ///
    /// A sampled texture attached to this context or a handle error.
    pub fn import_shared_bgra_texture(&self, handle: Handle) -> HandleResult<Texture> {
        let (backend, width, height) = self.backend.import_shared_bgra_texture(handle)?;
        Ok(Texture {
            backend,
            width,
            height,
        })
    }

    /// Upload one strided BGRA damage rectangle into a sampled texture.
    ///
    /// Source rows begin at the first byte of `pixels`; `damage` specifies the
    /// destination rectangle in the texture using top-left pixel coordinates.
    /// The source slice must contain one `damage.width()`-pixel BGRA row for
    /// every row in `damage` using `source_stride` bytes between row starts.
    ///
    /// # Arguments
    ///
    /// * `texture` - Texture created by this context.
    /// * `pixels` - BGRA source rectangle bytes.
    /// * `source_stride` - Source row stride in bytes.
    /// * `damage` - Destination texture rectangle in top-left pixel coordinates.
    ///
    /// # Returns
    ///
    /// Success after the synchronous GPU upload, or a handle error.
    pub fn upload_texture_bgra(
        &self,
        texture: &Texture,
        pixels: &[u8],
        source_stride: u32,
        damage: PixelRect,
    ) -> HandleResult<()> {
        if !damage.is_within(texture.width, texture.height) {
            return Err(HandleError::InvalidParameter);
        }
        self.backend
            .upload_texture_bgra(&texture.backend, pixels, source_stride, damage)
    }

    /// Transfer one damage rectangle from an imported texture's SharedMemory backing.
    ///
    /// # Arguments
    ///
    /// * `texture` - Texture created by [`Context::create_imported_bgra_texture`].
    /// * `damage` - Destination texture rectangle in top-left pixel coordinates.
    ///
    /// # Returns
    ///
    /// Success after synchronous transfer completion, or a handle error. This
    /// path does not pass a userspace pixel pointer to the kernel.
    pub fn transfer_imported_bgra_rect(
        &self,
        texture: &Texture,
        damage: PixelRect,
    ) -> HandleResult<()> {
        if !damage.is_within(texture.width, texture.height) {
            return Err(HandleError::InvalidParameter);
        }
        self.backend
            .transfer_imported_bgra_rect(&texture.backend, damage)
    }

    /// Detach a texture from this context and consume it deterministically.
    ///
    /// # Arguments
    ///
    /// * `texture` - Texture to detach and release.
    ///
    /// # Returns
    ///
    /// Success after backend detach completes. If detach fails, callers should
    /// discard the context before dropping or replacing imported SharedMemory.
    pub fn release_texture(&self, texture: Texture) -> HandleResult<()> {
        self.backend.release_texture(texture.backend)
    }

    /// Detach a render-target image from this context and consume it deterministically.
    ///
    /// # Arguments
    ///
    /// * `image` - Render-target image to detach and release.
    ///
    /// # Returns
    ///
    /// Success after backend detach completes, or a handle error.
    pub fn release_image(&self, image: Image) -> HandleResult<()> {
        self.backend.release_image(image.backend)
    }

    /// Create the built-in vertex-color triangle pipeline for one render target.
    ///
    /// # Arguments
    ///
    /// * `image` - Render target that the pipeline will draw into.
    /// * `description` - Pipeline kind and vertex capacity.
    ///
    /// # Returns
    ///
    /// A pipeline compatible with `image` or a handle error.
    pub fn create_pipeline(
        &self,
        image: &Image,
        description: PipelineDesc,
    ) -> HandleResult<Pipeline> {
        Ok(Pipeline {
            backend: Rc::new(self.backend.create_pipeline(&image.backend, description)?),
        })
    }

    /// Create a graphics queue for submitting render passes.
    ///
    /// # Returns
    ///
    /// A queue that executes render passes synchronously.
    pub fn create_queue(&self) -> HandleResult<Queue> {
        Ok(Queue {
            backend: self.backend.create_queue()?,
        })
    }
}

/// Scarlet execution session owning mapped presentation targets and backend state.
pub struct MappedTargetSession {
    // Drop mapped image owners and the cache's retained owners before the
    // queue and context handles they depend on.
    imported_textures: Vec<(ir::TextureId, Texture)>,
    images: Vec<(ir::TextureId, Rc<Image>)>,
    resources: IrResources,
    queue: Queue,
    context: Context,
}

impl MappedTargetSession {
    /// Import a transferred shared BGRA image into a logical sampled texture.
    pub fn import_shared_bgra_texture(
        &mut self,
        texture: ir::TextureId,
        handle: Handle,
    ) -> Result<(), IrSubmitError> {
        if self
            .imported_textures
            .iter()
            .any(|(candidate, _)| *candidate == texture)
        {
            return Err(IrSubmitError::TextureAlreadyMapped);
        }
        let imported = self.context.import_shared_bgra_texture(handle)?;
        if let Err(error) = self
            .resources
            .map_texture(&self.context, texture, &imported)
        {
            let _ = self.context.release_texture(imported);
            return Err(error);
        }
        self.imported_textures.push((texture, imported));
        Ok(())
    }

    /// Detach and release a previously imported sampled texture.
    pub fn release_imported_texture(
        &mut self,
        texture: ir::TextureId,
    ) -> Result<(), IrSubmitError> {
        let index = self
            .imported_textures
            .iter()
            .position(|(candidate, _)| *candidate == texture)
            .ok_or(IrSubmitError::ImageNotMapped)?;
        let imported = &self.imported_textures[index].1;
        self.resources
            .unmap_texture(&self.context, texture, imported)?;
        let (_, imported) = self.imported_textures.remove(index);
        self.context.release_texture(imported)?;
        Ok(())
    }

    /// Borrow the physical image mapped to a logical target.
    ///
    /// # Arguments
    ///
    /// * `target` - Logical presentation texture identity.
    ///
    /// # Returns
    ///
    /// The mapped image or [`IrSubmitError::ImageNotMapped`].
    pub fn image(&self, target: ir::TextureId) -> Result<&Image, IrSubmitError> {
        self.images
            .iter()
            .find(|(candidate, _)| *candidate == target)
            .map(|(_, image)| image.as_ref())
            .ok_or(IrSubmitError::ImageNotMapped)
    }

    /// Bind all session state for portable command execution.
    ///
    /// # Returns
    ///
    /// A backend-owned SGFX command executor.
    pub fn executor(&mut self) -> Executor<'_> {
        self.queue.executor(&self.context, &mut self.resources)
    }
}

/// Queue that submits complete application render passes.
pub struct Queue {
    backend: driver::Queue,
}

impl Queue {
    /// Bind this queue to its context and persistent IR resource cache.
    ///
    /// # Arguments
    ///
    /// * `context` - Context that created this queue and resource cache.
    /// * `resources` - Persistent IR resources used by submitted commands.
    ///
    /// # Returns
    ///
    /// A Scarlet/VirGL executor implementing
    /// [`sgfx_core::backend::CommandExecutor`].
    pub fn executor<'a>(
        &'a self,
        context: &'a Context,
        resources: &'a mut IrResources,
    ) -> Executor<'a> {
        Executor {
            queue: self,
            context,
            resources,
        }
    }

    /// Submit a render pass and wait for its rendering work to complete.
    ///
    /// # Arguments
    ///
    /// * `render_pass` - Render target, clear color, pipeline, and vertices to execute.
    ///
    /// # Returns
    ///
    /// Success or a handle error.
    pub fn submit(&self, render_pass: &RenderPass<'_>) -> HandleResult<()> {
        let draw = render_pass
            .draw
            .as_ref()
            .ok_or(HandleError::InvalidParameter)?;
        self.backend.submit(
            &render_pass.image.backend,
            render_pass.viewport,
            render_pass.clear_color,
            draw.pipeline.as_ref(),
            &draw.vertices,
        )
    }

    /// Submit an ordered 2D composition pass synchronously.
    ///
    /// # Arguments
    ///
    /// * `composition` - Render target, load behavior, and ordered composition operations.
    ///
    /// # Returns
    ///
    /// Success after all composition draws complete, or a handle error.
    pub fn submit_composition(&self, composition: &CompositionPass<'_>) -> HandleResult<()> {
        self.backend.submit_composition(
            &composition.image.backend,
            composition.clear_color,
            &composition.operations,
        )
    }
}

/// Scarlet/VirGL command executor bound to all backend submission state.
pub struct Executor<'a> {
    queue: &'a Queue,
    context: &'a Context,
    resources: &'a mut IrResources,
}

impl sgfx_core::backend::CommandExecutor for Executor<'_> {
    type Error = IrSubmitError;

    fn execute<'r, 'data>(
        &mut self,
        commands: &ir::CommandBuffer<'r, 'data>,
    ) -> Result<(), Self::Error> {
        self.queue.submit_ir(self.context, self.resources, commands)
    }
}

/// Renderable image that can be presented through a display surface.
pub struct Image {
    backend: driver::Image,
    width: u32,
    height: u32,
}

/// Sampled BGRA texture owned by one rendering context.
pub struct Texture {
    backend: driver::Texture,
    width: u32,
    height: u32,
}

impl Texture {
    /// Return the texture width in pixels.
    ///
    /// # Returns
    ///
    /// The texture width.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Return the texture height in pixels.
    ///
    /// # Returns
    ///
    /// The texture height.
    pub const fn height(&self) -> u32 {
        self.height
    }
}

impl Image {
    /// Return the image width in pixels.
    ///
    /// # Returns
    ///
    /// The image width.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Return the image height in pixels.
    ///
    /// # Returns
    ///
    /// The image height.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Return the shareable GPU image capability owned by this image.
    ///
    /// The returned handle is borrowed. Transferring it through a Scarlet
    /// socket creates a capability in the receiver without consuming this
    /// image's ownership.
    ///
    /// # Returns
    ///
    /// The GPU image capability backing this render target.
    pub fn shared_handle(&self) -> &Handle {
        self.backend.shared_handle()
    }
}

/// Built-in application pipeline kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineKind {
    /// Interleaved four-component clip-space position and RGB color vertices.
    ClipSpaceVertexColor,
}

/// Triangle face selection used by rasterization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CullMode {
    /// Rasterize both front-facing and back-facing triangles.
    None,
    /// Discard front-facing triangles.
    Front,
    /// Discard back-facing triangles.
    Back,
}

/// Winding direction that identifies front-facing triangles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontFace {
    /// Clockwise triangle winding is front-facing.
    Clockwise,
    /// Counter-clockwise triangle winding is front-facing.
    CounterClockwise,
}

/// Configuration for a built-in application pipeline.
#[derive(Debug, Clone, Copy)]
pub struct PipelineDesc {
    kind: PipelineKind,
    max_vertices: usize,
    cull_mode: CullMode,
    front_face: FrontFace,
}

impl PipelineDesc {
    /// Construct a clip-space vertex-color triangle pipeline description.
    ///
    /// # Arguments
    ///
    /// * `max_vertices` - Maximum vertices accepted in one render pass.
    ///
    /// # Returns
    ///
    /// A description for the built-in clip-space vertex-color pipeline.
    pub const fn clip_space_vertex_color(max_vertices: usize) -> Self {
        Self {
            kind: PipelineKind::ClipSpaceVertexColor,
            max_vertices,
            cull_mode: CullMode::None,
            front_face: FrontFace::CounterClockwise,
        }
    }

    /// Return the built-in pipeline kind.
    ///
    /// # Returns
    ///
    /// The pipeline kind selected by this description.
    pub const fn kind(&self) -> PipelineKind {
        self.kind
    }

    /// Return the maximum vertices accepted in one render pass.
    ///
    /// # Returns
    ///
    /// The pipeline vertex capacity.
    pub const fn max_vertices(&self) -> usize {
        self.max_vertices
    }

    /// Select the triangle faces discarded by rasterization.
    ///
    /// # Arguments
    ///
    /// * `cull_mode` - Face selection to discard.
    ///
    /// # Returns
    ///
    /// An updated pipeline description.
    pub const fn with_cull_mode(mut self, cull_mode: CullMode) -> Self {
        self.cull_mode = cull_mode;
        self
    }

    /// Select the winding direction treated as front-facing.
    ///
    /// # Arguments
    ///
    /// * `front_face` - Winding direction for visible front faces.
    ///
    /// # Returns
    ///
    /// An updated pipeline description.
    pub const fn with_front_face(mut self, front_face: FrontFace) -> Self {
        self.front_face = front_face;
        self
    }

    pub(crate) const fn cull_mode(&self) -> CullMode {
        self.cull_mode
    }

    pub(crate) const fn front_face(&self) -> FrontFace {
        self.front_face
    }
}

/// Built-in graphics pipeline state.
pub struct Pipeline {
    backend: Rc<driver::Pipeline>,
}

/// RGBA floating-point clear color.
#[derive(Debug, Clone, Copy)]
pub struct Color {
    red: f32,
    green: f32,
    blue: f32,
    alpha: f32,
}

impl Color {
    /// Construct an RGBA color.
    ///
    /// # Arguments
    ///
    /// * `red` - Red component.
    /// * `green` - Green component.
    /// * `blue` - Blue component.
    /// * `alpha` - Alpha component.
    ///
    /// # Returns
    ///
    /// The requested color.
    pub const fn rgba(red: f32, green: f32, blue: f32, alpha: f32) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    pub(crate) fn is_finite_unit(self) -> bool {
        self.red.is_finite()
            && self.green.is_finite()
            && self.blue.is_finite()
            && self.alpha.is_finite()
            && (0.0..=1.0).contains(&self.red)
            && (0.0..=1.0).contains(&self.green)
            && (0.0..=1.0).contains(&self.blue)
            && (0.0..=1.0).contains(&self.alpha)
    }
}

/// Top-left pixel rectangle used for texture, destination, and clip regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl PixelRect {
    /// Construct a top-left pixel rectangle.
    ///
    /// # Arguments
    ///
    /// * `x` - Left pixel coordinate.
    /// * `y` - Top pixel coordinate.
    /// * `width` - Rectangle width in pixels.
    /// * `height` - Rectangle height in pixels.
    ///
    /// # Returns
    ///
    /// The requested rectangle. Operations validate non-zero dimensions and
    /// destination bounds before submission.
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Return the left pixel coordinate.
    ///
    /// # Returns
    ///
    /// The left pixel coordinate.
    pub const fn x(&self) -> u32 {
        self.x
    }

    /// Return the top pixel coordinate.
    ///
    /// # Returns
    ///
    /// The top pixel coordinate.
    pub const fn y(&self) -> u32 {
        self.y
    }

    /// Return the rectangle width in pixels.
    ///
    /// # Returns
    ///
    /// The rectangle width.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Return the rectangle height in pixels.
    ///
    /// # Returns
    ///
    /// The rectangle height.
    pub const fn height(&self) -> u32 {
        self.height
    }

    pub(crate) fn is_within(&self, width: u32, height: u32) -> bool {
        self.width != 0
            && self.height != 0
            && self
                .x
                .checked_add(self.width)
                .is_some_and(|right| right <= width)
            && self
                .y
                .checked_add(self.height)
                .is_some_and(|bottom| bottom <= height)
    }
}

/// Choice of how a textured composition operation derives effective alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAlpha {
    /// Multiply sampled source alpha by global opacity.
    Respect,
    /// Ignore sampled source alpha and use global opacity directly.
    Ignore,
}

/// Maximum ordered operations accepted by one composition pass.
pub const MAX_COMPOSITION_OPERATIONS: usize = 96;

/// Ordered 2D composition of sampled textures and solid overlays into one image.
pub struct CompositionPass<'a> {
    image: &'a Image,
    clear_color: Option<Color>,
    operations: Vec<driver::CompositionOperation<'a>>,
    operation_capacity: usize,
}

impl<'a> CompositionPass<'a> {
    /// Begin a composition pass with the maximum supported operation capacity.
    ///
    /// # Arguments
    ///
    /// * `image` - Render target for the composition.
    /// * `clear_color` - Straight-alpha background color written before operations.
    ///
    /// # Returns
    ///
    /// An empty composition pass, or a handle error for an invalid clear color.
    pub fn new(image: &'a Image, clear_color: Color) -> HandleResult<Self> {
        Self::with_operation_capacity(image, clear_color, MAX_COMPOSITION_OPERATIONS)
    }

    /// Begin a composition pass that preserves existing render-target pixels.
    ///
    /// The caller must overwrite every damaged pixel before presentation. This
    /// mode is intended for clipped incremental composition into an image that
    /// has already received a complete composition pass.
    ///
    /// # Arguments
    ///
    /// * `image` - Previously initialized render target to update.
    ///
    /// # Returns
    ///
    /// An empty composition pass that does not clear the target.
    pub fn preserving(image: &'a Image) -> Self {
        Self {
            image,
            clear_color: None,
            operations: Vec::new(),
            operation_capacity: MAX_COMPOSITION_OPERATIONS,
        }
    }

    /// Begin a composition pass with a bounded operation capacity.
    ///
    /// # Arguments
    ///
    /// * `image` - Render target for the composition.
    /// * `clear_color` - Straight-alpha background color written before operations.
    /// * `operation_capacity` - Maximum ordered operations for this pass.
    ///
    /// # Returns
    ///
    /// An empty composition pass, or a handle error when the capacity or clear
    /// color is invalid.
    pub fn with_operation_capacity(
        image: &'a Image,
        clear_color: Color,
        operation_capacity: usize,
    ) -> HandleResult<Self> {
        if !clear_color.is_finite_unit()
            || operation_capacity == 0
            || operation_capacity > MAX_COMPOSITION_OPERATIONS
        {
            return Err(HandleError::InvalidParameter);
        }
        Ok(Self {
            image,
            clear_color: Some(clear_color),
            operations: Vec::new(),
            operation_capacity,
        })
    }

    /// Append an ordered textured rectangle operation.
    ///
    /// Source and destination coordinates use top-left pixels directly. When
    /// source alpha is respected, effective alpha is sampled alpha multiplied by
    /// `opacity`; otherwise effective alpha is exactly `opacity`. Source RGB is
    /// always interpreted as straight alpha.
    ///
    /// # Arguments
    ///
    /// * `texture` - Sampled source texture.
    /// * `destination` - Destination render-target rectangle.
    /// * `source` - Source texture rectangle.
    /// * `opacity` - Finite global opacity in the inclusive range `0.0..=1.0`.
    /// * `source_alpha` - Whether to respect sampled alpha.
    /// * `clip` - Optional destination scissor rectangle.
    ///
    /// # Returns
    ///
    /// Success after appending the operation, or a handle error for invalid
    /// rectangles, opacity, or operation capacity.
    pub fn draw_textured_rect(
        &mut self,
        texture: &'a Texture,
        destination: PixelRect,
        source: PixelRect,
        opacity: f32,
        source_alpha: SourceAlpha,
        clip: Option<PixelRect>,
    ) -> HandleResult<()> {
        if !destination.is_within(self.image.width, self.image.height)
            || !source.is_within(texture.width, texture.height)
            || clip.is_some_and(|rect| !rect.is_within(self.image.width, self.image.height))
            || !opacity.is_finite()
            || !(0.0..=1.0).contains(&opacity)
        {
            return Err(HandleError::InvalidParameter);
        }
        self.reserve_operation()?;
        self.operations
            .push(driver::CompositionOperation::Textured {
                texture: &texture.backend,
                destination,
                source,
                opacity,
                source_alpha,
                clip,
            });
        Ok(())
    }

    /// Append an ordered straight-alpha solid rectangle operation.
    ///
    /// # Arguments
    ///
    /// * `destination` - Destination render-target rectangle.
    /// * `color` - Finite straight-alpha overlay color with unit components.
    /// * `clip` - Optional destination scissor rectangle.
    ///
    /// # Returns
    ///
    /// Success after appending the operation, or a handle error for invalid
    /// rectangles, color, or operation capacity.
    pub fn draw_solid_rect(
        &mut self,
        destination: PixelRect,
        color: Color,
        clip: Option<PixelRect>,
    ) -> HandleResult<()> {
        if !destination.is_within(self.image.width, self.image.height)
            || !color.is_finite_unit()
            || clip.is_some_and(|rect| !rect.is_within(self.image.width, self.image.height))
        {
            return Err(HandleError::InvalidParameter);
        }
        self.reserve_operation()?;
        self.operations.push(driver::CompositionOperation::Solid {
            destination,
            color,
            clip,
        });
        Ok(())
    }

    fn reserve_operation(&mut self) -> HandleResult<()> {
        if self.operations.len() >= self.operation_capacity {
            return Err(HandleError::OutOfResources);
        }
        self.operations
            .try_reserve(1)
            .map_err(|_| HandleError::OutOfResources)
    }
}

/// Pixel dimensions used to map normalized coordinates into a render target.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    width: u32,
    height: u32,
}

impl Viewport {
    /// Construct a viewport covering a render target.
    ///
    /// # Arguments
    ///
    /// * `width` - Non-zero viewport width in pixels.
    /// * `height` - Non-zero viewport height in pixels.
    ///
    /// # Returns
    ///
    /// The requested viewport.
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Return the viewport width in pixels.
    ///
    /// # Returns
    ///
    /// The viewport width.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Return the viewport height in pixels.
    ///
    /// # Returns
    ///
    /// The viewport height.
    pub const fn height(&self) -> u32 {
        self.height
    }
}

/// One interleaved homogeneous clip-space position and RGB color vertex.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VertexClip4Color3 {
    clip_position: [f32; 4],
    color: [f32; 3],
}

impl VertexClip4Color3 {
    /// Construct an interleaved clip-space position and color vertex.
    ///
    /// # Arguments
    ///
    /// * `clip_position` - Homogeneous clip-space position before perspective division.
    /// * `color` - RGB vertex color.
    ///
    /// # Returns
    ///
    /// The requested vertex.
    pub const fn new(clip_position: [f32; 4], color: [f32; 3]) -> Self {
        Self {
            clip_position,
            color,
        }
    }
}

/// Complete description of one color render pass.
pub struct RenderPass<'a> {
    image: &'a Image,
    viewport: Viewport,
    clear_color: Color,
    draw: Option<Draw>,
}

struct Draw {
    pipeline: Rc<driver::Pipeline>,
    vertices: Vec<VertexClip4Color3>,
}

impl<'a> RenderPass<'a> {
    /// Begin a pass that clears and renders into one image.
    ///
    /// # Arguments
    ///
    /// * `image` - Render target for the pass.
    /// * `viewport` - Pixel area used for rendering.
    /// * `clear_color` - Background color written before drawing.
    ///
    /// # Returns
    ///
    /// A render pass ready to receive one built-in draw call.
    pub const fn new(image: &'a Image, viewport: Viewport, clear_color: Color) -> Self {
        Self {
            image,
            viewport,
            clear_color,
            draw: None,
        }
    }

    /// Set the clip-space vertex-color triangle draw for this pass.
    ///
    /// # Arguments
    ///
    /// * `pipeline` - Built-in clip-space vertex-color pipeline created for this image.
    /// * `vertices` - Triangle-list vertices to upload and rasterize.
    ///
    /// # Returns
    ///
    /// The updated render pass.
    pub fn draw_clip_space_vertex_color(
        &mut self,
        pipeline: &Pipeline,
        vertices: &[VertexClip4Color3],
    ) {
        self.draw = Some(Draw {
            pipeline: Rc::clone(&pipeline.backend),
            vertices: Vec::from(vertices),
        });
    }
}
