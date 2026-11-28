//! WGPU execution backend for the portable SGFX IR.
//!
//! This module intentionally does not use Scarlet GPU handles, display
//! surfaces, or shared-memory capabilities. A caller supplies a WGPU device
//! and queue, maps a logical presentation texture to an owned WGPU image, and
//! submits the same validated IR command buffers used by the native backend.
//! A persistent resource cache can rebind that logical presentation texture to
//! each newly acquired WGPU surface frame.

extern crate alloc;

use alloc::{borrow::Cow, format, rc::Rc, string::String, sync::Arc, vec::Vec};
use core::fmt;

use bytemuck::{Pod, Zeroable};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use wgpu as raw;

use sgfx_core::backend::{CommandSubmitter, SubmitError};

mod blit;
mod completion;
mod programmable;
mod queries;
mod readback;
mod vertex_input;
pub use completion::Submission;

use sgfx_core::ir::{
    self, AddressMode, BlendComponent, BlendFactor, BlendOp, BufferId, BufferRef, BufferUsage,
    Command, CommandBuffer, CompareFunction, DepthLoadOp, DrawUniforms, FilterMode,
    FragmentProgram, IndexFormat, LoadOp, RenderPassDesc, RenderPipelineDesc, RenderPipelineId,
    ResourceTable, SamplerId, SamplerRef, StoreOp, TextureDesc, TextureFormat, TextureId,
    TextureRef, TextureSampleMode, TextureUsage, VertexAttribute, VertexFormat,
};

/// WGPU inserts ordinary resource transitions while preserving queue order.
/// Storage/global dependencies and completion still use their explicit paths.
pub const IMPLICIT_RESOURCE_BARRIERS: bool = true;

/// Result returned by the WGPU backend.
pub type Result<T> = core::result::Result<T, Error>;

/// Failure returned while translating or submitting SGFX commands through WGPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A logical IR descriptor or command failed validation.
    InvalidIr(ir::Error),
    /// A command buffer belongs to a different resource table than the cache.
    ResourceTableMismatch,
    /// A presentation texture was submitted without a physical image mapping.
    ImageNotMapped,
    /// A physical image was mapped to more than one logical texture.
    ImageAlreadyMapped,
    /// The backend cannot represent a valid logical IR feature yet.
    Unsupported(UnsupportedFeature),
    /// The command stream or persistent backend state is inconsistent.
    InvalidState,
    /// Native window handles could not be bound to a WGPU surface.
    SurfaceCreation,
    /// No WGPU adapter can present to the requested surface.
    AdapterUnavailable,
    /// The selected WGPU adapter could not create a device and queue.
    DeviceRequest,
    /// A configured surface frame could not be acquired for presentation.
    SurfaceAcquire,
    /// The device was lost or explicitly destroyed; completion is not certified.
    DeviceLost,
    /// Submission retirement could not be established by the private marker.
    CompletionObservation,
    /// WGPU rejected a programmable shader, pipeline, or binding.
    Validation(String),
}

impl From<ir::Error> for Error {
    fn from(error: ir::Error) -> Self {
        Self::InvalidIr(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIr(error) => write!(formatter, "invalid SGFX IR: {error:?}"),
            Self::ResourceTableMismatch => formatter.write_str("resource table mismatch"),
            Self::ImageNotMapped => formatter.write_str("presentation image is not mapped"),
            Self::ImageAlreadyMapped => formatter.write_str("image is already mapped"),
            Self::Unsupported(feature) => {
                write!(formatter, "unsupported SGFX feature: {feature:?}")
            }
            Self::InvalidState => formatter.write_str("invalid WGPU backend state"),
            Self::SurfaceCreation => formatter.write_str("WGPU surface creation failed"),
            Self::AdapterUnavailable => formatter.write_str("no compatible WGPU adapter"),
            Self::DeviceRequest => formatter.write_str("WGPU device creation failed"),
            Self::SurfaceAcquire => formatter.write_str("WGPU surface acquisition failed"),
            Self::DeviceLost => formatter.write_str("WGPU device was lost or destroyed"),
            Self::Validation(message) => write!(formatter, "WGPU validation failed: {message}"),
            Self::CompletionObservation => {
                formatter.write_str("WGPU completion observation failed")
            }
        }
    }
}

/// Logical SGFX features that are not yet represented by this backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedFeature {
    /// Instance divisors larger than one need vertex pulling.
    VertexDivisor,
    /// Shareable presentation images are unavailable on this WGPU platform.
    PresentationPlatform,
    /// Reserved for compatibility; ordered uploads are now supported.
    LateUpload,
    /// A buffer upload offset or byte length is not a multiple of WGPU's
    /// four-byte copy alignment. The portable IR permits byte-granular writes.
    BufferWriteAlignment,
    /// A buffer's byte size or a texture's dimensions exceed the selected
    /// WGPU device's resource-size limits.
    ResourceSize,
    /// A render pipeline uses a format or vertex convention outside WGPU's
    /// current portable lowering.
    Pipeline,
    /// Indexed strips need explicit restart semantics or a faithful index
    /// conversion. WGPU's implicit restart must not reinterpret ordinary indices.
    IndexedTriangleStrip,
    /// Indexed line strips also require a faithful conversion of restart indices.
    IndexedLineStrip,
    /// WGPU has no native triangle-fan topology; conversion is not implemented.
    TriangleFan,
    /// WGPU constant depth bias must be an integer within i32 range.
    DepthBias,
    /// A sampled texture format is incompatible with the selected fragment
    /// program.
    TextureFormat,
    /// WGPU cannot express a depth clear restricted to a rectangular render
    /// area without an explicit clear draw.
    PartialDepthClear,
    /// Partial color clears with multiple render targets need a separate clear pass.
    PartialMultiTargetClear,
    /// The physical surface format cannot be represented by SGFX.
    SurfaceFormat,
    /// Blocking waits are unavailable in a browser; use nonblocking observation.
    BlockingWait,
    /// Programmable execution needs native synchronous validation scopes.
    ProgrammableOnWeb,
    /// A buffer binding violates this WGPU device's offset alignment or size limits.
    BindingLimits,
    /// WGPU cannot retain negative LOD clamps when they change min/mag selection.
    SamplerLod,
    /// This device did not enable native transparent-black clamp-to-border.
    SamplerBorderColor,
    /// Partial source or destination mip blits need a regional sampling pass.
    PartialBlit,
    /// Signed or zero-height viewports require a different WGPU lowering.
    SignedViewport,
    /// Dispatch dimensions exceed the device workgroup-count limit.
    DispatchLimits,
}

/// WGPU device and queue pair used by the SGFX backend.
#[derive(Clone)]
pub struct Device {
    device: Arc<raw::Device>,
    queue: Arc<raw::Queue>,
    identity: Arc<()>,
    tracker: Arc<completion::Tracker>,
    validation: Arc<std::sync::Mutex<()>>,
}

impl Device {
    /// Wrap an existing WGPU device and queue.
    ///
    /// # Arguments
    ///
    /// * `device` - WGPU device used to create resources and command encoders.
    /// * `queue` - WGPU queue used to upload data and submit command buffers.
    ///
    /// # Returns
    ///
    /// A reusable SGFX WGPU device.
    ///
    /// This installs SGFX's device-lost callback for completion observation.
    /// Do not replace it or wrap another alias of this raw device while using
    /// SGFX completion receipts. Other raw WGPU error handlers are unchanged.
    /// Do not concurrently interleave raw WGPU error scopes with SGFX calls:
    /// WGPU validation scopes belong to the device, not the calling thread.
    pub fn new(device: raw::Device, queue: raw::Queue) -> Self {
        let tracker = Arc::new(completion::Tracker::default());
        let loss_tracker = Arc::clone(&tracker);
        device.set_device_lost_callback(move |_, _| {
            loss_tracker
                .lost
                .store(true, core::sync::atomic::Ordering::Release);
        });
        Self {
            device: Arc::new(device),
            queue: Arc::new(queue),
            identity: Arc::new(()),
            tracker,
            validation: Arc::new(std::sync::Mutex::new(())),
        }
    }

    /// Create a rendering context sharing this device and queue.
    ///
    /// # Returns
    ///
    /// A context for images, IR resource caches, and queues.
    pub fn create_context(&self) -> Context {
        Context {
            device: self.clone(),
            queries: Default::default(),
        }
    }

    /// Borrow the underlying WGPU device.
    ///
    /// # Returns
    ///
    /// The wrapped WGPU device.
    pub fn raw_device(&self) -> &raw::Device {
        self.device.as_ref()
    }

    /// Borrow the underlying WGPU queue.
    ///
    /// # Returns
    ///
    /// The wrapped WGPU queue.
    pub fn raw_queue(&self) -> &raw::Queue {
        self.queue.as_ref()
    }
}

/// WGPU rendering context used to create SGFX resources.
#[derive(Clone)]
pub struct Context {
    device: Device,
    queries: Arc<std::sync::Mutex<queries::State>>,
}

impl Context {
    /// Borrow the wrapped WGPU device for platform presentation work.
    ///
    /// # Returns
    ///
    /// The WGPU device paired with this context.
    pub fn raw_device(&self) -> &raw::Device {
        self.device.raw_device()
    }

    /// Borrow the wrapped WGPU queue for platform presentation work.
    ///
    /// # Returns
    ///
    /// The WGPU queue paired with this context.
    pub fn raw_queue(&self) -> &raw::Queue {
        self.device.raw_queue()
    }

    /// Create an offscreen render-target image.
    ///
    /// # Arguments
    ///
    /// * `width` - Non-zero image width in pixels.
    /// * `height` - Non-zero image height in pixels.
    /// * `format` - Portable SGFX color format.
    ///
    /// # Returns
    ///
    /// A WGPU-backed image, an invalid-state error for zero dimensions or
    /// unsupported formats, or [`UnsupportedFeature::ResourceSize`] when the
    /// dimensions exceed this device's limits.
    pub fn create_image(
        &self,
        width: u32,
        height: u32,
        format: TextureFormat,
    ) -> Result<Arc<Image>> {
        if width == 0
            || height == 0
            || !matches!(
                format,
                TextureFormat::Bgra8Unorm
                    | TextureFormat::Rgba8Unorm
                    | TextureFormat::R8Unorm
                    | TextureFormat::Rg8Unorm
            )
        {
            return Err(Error::InvalidState);
        }
        let gpu = create_gpu_texture(
            self.device.raw_device(),
            width,
            height,
            format,
            1,
            1,
            raw::TextureUsages::TEXTURE_BINDING
                | raw::TextureUsages::RENDER_ATTACHMENT
                | raw::TextureUsages::COPY_SRC
                | raw::TextureUsages::COPY_DST,
        )?;
        Ok(Arc::new(Image {
            device_identity: Arc::clone(&self.device.identity),
            gpu,
        }))
    }

    /// Create a persistent logical-resource cache for this context.
    ///
    /// # Arguments
    ///
    /// * `resources` - Resource table whose references will be submitted.
    ///
    /// # Returns
    ///
    /// An empty WGPU materialization cache.
    pub fn create_resources(&self, resources: Rc<ResourceTable>) -> Resources {
        Resources {
            context: self.clone(),
            resources,
            textures: Vec::new(),
            buffers: Vec::new(),
            samplers: Vec::new(),
            pipelines: Vec::new(),
            clear_pipelines: Vec::new(),
            mapped_images: Vec::new(),
            programmable: programmable::Cache::default(),
            blit: blit::Cache::default(),
        }
    }

    /// Create a command queue for this context.
    ///
    /// # Returns
    ///
    /// A queue that translates and submits SGFX IR command buffers.
    pub fn create_queue(&self) -> Queue {
        Queue {
            context: self.clone(),
        }
    }

    /// Create and map all physical images for a set of logical presentation targets.
    ///
    /// # Arguments
    ///
    /// * `resources` - Logical resource table retained by the session.
    /// * `targets` - `PRESENT` texture identities to materialize and map.
    ///
    /// # Returns
    ///
    /// A session owning its context, queue, resource cache, and images, or a
    /// descriptor, allocation, or mapping error.
    pub fn create_mapped_target_session(
        &self,
        resources: Rc<ResourceTable>,
        targets: &[TextureId],
    ) -> Result<MappedTargetSession> {
        let mut cache = self.create_resources(Rc::clone(&resources));
        let mut images = Vec::new();
        images
            .try_reserve_exact(targets.len())
            .map_err(|_| Error::InvalidState)?;
        for &target in targets {
            if images.iter().any(|(candidate, _)| *candidate == target) {
                return Err(Error::InvalidState);
            }
            let reference = resources.texture_ref(target)?;
            let descriptor = resources.texture(reference)?;
            let required = TextureUsage::RENDER_ATTACHMENT | TextureUsage::PRESENT;
            if !descriptor.usage().contains(required) {
                return Err(Error::InvalidState);
            }
            let image = self.create_image(
                descriptor.extent().width(),
                descriptor.extent().height(),
                descriptor.format(),
            )?;
            cache.map_image(target, Arc::clone(&image))?;
            images.push((target, image));
        }
        Ok(MappedTargetSession {
            context: self.clone(),
            queue: self.create_queue(),
            resources: cache,
            images,
        })
    }
}

/// WGPU execution session owning mapped presentation targets and backend state.
pub struct MappedTargetSession {
    context: Context,
    queue: Queue,
    resources: Resources,
    images: Vec<(TextureId, Arc<Image>)>,
}

impl MappedTargetSession {
    /// Borrow the physical image mapped to a logical target.
    ///
    /// # Arguments
    ///
    /// * `target` - Logical presentation texture identity.
    ///
    /// # Returns
    ///
    /// The mapped image or [`Error::ImageNotMapped`].
    pub fn image(&self, target: TextureId) -> Result<&Image> {
        self.images
            .iter()
            .find(|(candidate, _)| *candidate == target)
            .map(|(_, image)| image.as_ref())
            .ok_or(Error::ImageNotMapped)
    }

    /// Bind the session's queue and resources for command execution.
    ///
    /// # Returns
    ///
    /// A backend-owned SGFX command executor.
    pub fn executor(&mut self) -> Executor<'_> {
        self.queue.executor(&mut self.resources)
    }

    /// Borrow the underlying WGPU device for platform presentation work.
    ///
    /// # Returns
    ///
    /// The WGPU device owned by this session's context.
    pub fn raw_device(&self) -> &raw::Device {
        self.context.raw_device()
    }

    /// Borrow the underlying WGPU queue for platform presentation work.
    ///
    /// # Returns
    ///
    /// The WGPU queue owned by this session's context.
    pub fn raw_queue(&self) -> &raw::Queue {
        self.context.raw_queue()
    }
}

/// WGPU window surface, device, and presentation state owned by the backend.
pub struct WindowContext {
    surface: raw::Surface<'static>,
    config: raw::SurfaceConfiguration,
    context: Context,
    sampler: raw::Sampler,
    blit_pipeline: raw::RenderPipeline,
    // Fields drop in declaration order. Retain the instance until the surface
    // and every resource created from its adapter have been released.
    _instance: raw::Instance,
}

impl WindowContext {
    /// Create a complete WGPU execution context for native window handles.
    ///
    /// # Arguments
    ///
    /// * `raw_display_handle` - Display handle retained by the platform.
    /// * `raw_window_handle` - Window handle retained by the platform.
    /// * `width` - Initial physical surface width.
    /// * `height` - Initial physical surface height.
    ///
    /// # Returns
    ///
    /// A surface-compatible WGPU context, or an initialization error.
    ///
    /// # Safety
    ///
    /// Both raw handles must remain valid until the returned context is
    /// dropped. Callers must drop SGFX rendering state before the native
    /// window and display objects.
    pub unsafe fn new(
        raw_display_handle: RawDisplayHandle,
        raw_window_handle: RawWindowHandle,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        // SAFETY: forwarded unchanged from this constructor's contract.
        unsafe {
            Self::new_with_transparency(raw_display_handle, raw_window_handle, width, height, false)
        }
    }

    /// Create a complete WGPU execution context for native window handles.
    ///
    /// `transparent` requests a compositor mode that preserves the presented
    /// image's alpha channel. The backend keeps straight-alpha presentation
    /// when supported and premultiplies only when the surface requires it.
    ///
    /// # Safety
    ///
    /// Both raw handles must remain valid until the returned context is
    /// dropped. Callers must drop SGFX rendering state before the native
    /// window and display objects.
    pub unsafe fn new_with_transparency(
        raw_display_handle: RawDisplayHandle,
        raw_window_handle: RawWindowHandle,
        width: u32,
        height: u32,
        transparent: bool,
    ) -> Result<Self> {
        let instance = raw::Instance::new(&raw::InstanceDescriptor::default());
        // SAFETY: the caller guarantees that both native handles outlive the
        // returned context and its surface.
        let surface = unsafe {
            instance.create_surface_unsafe(raw::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle,
                raw_window_handle,
            })
        }
        .map_err(|_| Error::SurfaceCreation)?;
        let adapter = pollster::block_on(instance.request_adapter(&raw::RequestAdapterOptions {
            power_preference: raw::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .ok_or(Error::AdapterUnavailable)?;
        let (device, queue) = pollster::block_on(async {
            adapter
                .request_device(&raw::DeviceDescriptor::default(), None)
                .await
        })
        .map_err(|_| Error::DeviceRequest)?;
        device.on_uncaptured_error(Box::new(|error| {
            eprintln!("[SGFX/WGPU] uncaptured error: {error}");
        }));
        let device = Device::new(device, queue);
        let context = device.create_context();
        Self::from_parts(
            instance,
            &adapter,
            context,
            surface,
            width,
            height,
            transparent,
        )
    }

    /// Create a browser WebGPU context without blocking the JavaScript thread.
    /// The surface owns its canvas handle. WebGL fallback is intentionally disabled.
    #[cfg(target_arch = "wasm32")]
    pub async fn from_canvas(canvas: web_sys::HtmlCanvasElement) -> Result<Self> {
        let width = canvas.width().max(1);
        let height = canvas.height().max(1);
        let instance = raw::Instance::new(&raw::InstanceDescriptor {
            backends: raw::Backends::BROWSER_WEBGPU,
            ..Default::default()
        });
        let surface = instance
            .create_surface(raw::SurfaceTarget::Canvas(canvas))
            .map_err(|_| Error::SurfaceCreation)?;
        let adapter = instance
            .request_adapter(&raw::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .ok_or(Error::AdapterUnavailable)?;
        let (device, queue) = adapter
            .request_device(&raw::DeviceDescriptor::default(), None)
            .await
            .map_err(|_| Error::DeviceRequest)?;
        let context = Device::new(device, queue).create_context();
        Self::from_parts(instance, &adapter, context, surface, width, height, false)
    }

    /// Create presentation state for an existing WGPU device and CAMetalLayer.
    ///
    /// This is used by API frontends whose device was selected before the
    /// native surface was created, including Vulkan's instance/device split.
    /// The supplied instance and adapter must be the ones used to create the
    /// context's device.
    ///
    /// # Safety
    ///
    /// `layer` must point to a live CAMetalLayer until this context is dropped.
    #[cfg(target_os = "macos")]
    pub unsafe fn from_core_animation_layer(
        instance: raw::Instance,
        adapter: &raw::Adapter,
        context: Context,
        layer: *mut core::ffi::c_void,
        width: u32,
        height: u32,
        transparent: bool,
    ) -> Result<Self> {
        if layer.is_null() {
            return Err(Error::SurfaceCreation);
        }
        // SAFETY: the caller keeps the CAMetalLayer alive for the returned
        // context's lifetime.
        let surface = unsafe {
            instance.create_surface_unsafe(raw::SurfaceTargetUnsafe::CoreAnimationLayer(layer))
        }
        .map_err(|_| Error::SurfaceCreation)?;
        Self::from_parts(
            instance,
            adapter,
            context,
            surface,
            width,
            height,
            transparent,
        )
    }

    fn from_parts(
        instance: raw::Instance,
        adapter: &raw::Adapter,
        context: Context,
        surface: raw::Surface<'static>,
        width: u32,
        height: u32,
        transparent: bool,
    ) -> Result<Self> {
        let capabilities = surface.get_capabilities(adapter);
        let format = select_surface_format(&capabilities.formats).ok_or(Error::SurfaceCreation)?;
        let alpha_mode = select_surface_alpha_mode(&capabilities.alpha_modes, transparent)
            .ok_or(Error::SurfaceCreation)?;
        let config = raw::SurfaceConfiguration {
            usage: raw::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: raw::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: Vec::new(),
        };
        surface.configure(context.raw_device(), &config);
        let sampler = context
            .raw_device()
            .create_sampler(&raw::SamplerDescriptor {
                label: Some("sgfx WGPU presentation sampler"),
                address_mode_u: raw::AddressMode::ClampToEdge,
                address_mode_v: raw::AddressMode::ClampToEdge,
                address_mode_w: raw::AddressMode::ClampToEdge,
                mag_filter: raw::FilterMode::Nearest,
                min_filter: raw::FilterMode::Nearest,
                mipmap_filter: raw::FilterMode::Nearest,
                ..Default::default()
            });
        let blit_pipeline = create_blit_pipeline(
            context.raw_device(),
            config.format,
            alpha_mode == raw::CompositeAlphaMode::PreMultiplied,
        );
        Ok(Self {
            surface,
            config,
            context,
            sampler,
            blit_pipeline,
            _instance: instance,
        })
    }

    /// Create mapped physical targets through this surface-compatible device.
    ///
    /// # Arguments
    ///
    /// * `resources` - Logical SGFX resource table.
    /// * `targets` - Presentation texture identities to materialize.
    ///
    /// # Returns
    ///
    /// A mapped session sharing the window context's WGPU device.
    pub fn create_mapped_target_session(
        &self,
        resources: Rc<ResourceTable>,
        targets: &[TextureId],
    ) -> Result<MappedTargetSession> {
        self.context
            .create_mapped_target_session(resources, targets)
    }

    /// Reconfigure the native surface after a physical resize.
    ///
    /// # Arguments
    ///
    /// * `width` - New physical width.
    /// * `height` - New physical height.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface
            .configure(self.context.raw_device(), &self.config);
    }

    /// Present one mapped SGFX target to the native window surface.
    ///
    /// # Arguments
    ///
    /// * `session` - Session owning the target image.
    /// * `target` - Logical target texture to present.
    ///
    /// # Returns
    ///
    /// Success after the surface frame is submitted and presented.
    pub fn present(&mut self, session: &MappedTargetSession, target: TextureId) -> Result<()> {
        let image = session.image(target)?;
        self.present_image(image)
    }

    /// Present a physical SGFX image created by this context.
    pub fn present_image(&mut self, image: &Image) -> Result<()> {
        if !Arc::ptr_eq(&self.context.device.identity, &image.device_identity)
            || image.width() != self.config.width
            || image.height() != self.config.height
        {
            return Err(Error::InvalidState);
        }
        let frame = self
            .surface
            .get_current_texture()
            .map_err(|_| Error::SurfaceAcquire)?;
        let view = frame
            .texture
            .create_view(&raw::TextureViewDescriptor::default());
        let bind_group = self
            .context
            .raw_device()
            .create_bind_group(&raw::BindGroupDescriptor {
                label: Some("sgfx WGPU presentation bind group"),
                layout: &self.blit_pipeline.get_bind_group_layout(0),
                entries: &[
                    raw::BindGroupEntry {
                        binding: 0,
                        resource: raw::BindingResource::TextureView(image.raw_view()),
                    },
                    raw::BindGroupEntry {
                        binding: 1,
                        resource: raw::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
        let mut encoder =
            self.context
                .raw_device()
                .create_command_encoder(&raw::CommandEncoderDescriptor {
                    label: Some("sgfx WGPU presentation encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&raw::RenderPassDescriptor {
                label: Some("sgfx WGPU presentation pass"),
                color_attachments: &[Some(raw::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: raw::Operations {
                        load: raw::LoadOp::Clear(raw::Color::BLACK),
                        store: raw::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.context
            .raw_queue()
            .submit(core::iter::once(encoder.finish()));
        frame.present();
        let _ = self.context.raw_device().poll(raw::Maintain::Poll);
        Ok(())
    }
}

/// WGPU-backed SGFX image.
#[derive(Clone)]
pub struct Image {
    device_identity: Arc<()>,
    gpu: Arc<GpuTexture>,
}

impl Image {
    /// Return the image width in pixels.
    ///
    /// # Returns
    ///
    /// The image width.
    pub fn width(&self) -> u32 {
        self.gpu.width
    }

    /// Return the image height in pixels.
    ///
    /// # Returns
    ///
    /// The image height.
    pub fn height(&self) -> u32 {
        self.gpu.height
    }

    /// Return the portable logical image format.
    ///
    /// # Returns
    ///
    /// The image format used by SGFX IR descriptors.
    pub fn format(&self) -> TextureFormat {
        self.gpu.logical_format
    }

    /// Borrow the underlying WGPU texture for platform presentation or copy.
    ///
    /// # Returns
    ///
    /// The WGPU texture owned by this SGFX image.
    pub fn raw_texture(&self) -> &raw::Texture {
        &self.gpu.texture
    }

    /// Borrow the default view of the underlying WGPU texture.
    ///
    /// # Returns
    ///
    /// The WGPU texture view used by SGFX render passes.
    pub fn raw_view(&self) -> &raw::TextureView {
        &self.gpu.view
    }
}

/// Persistent logical-resource materialization cache for one WGPU context.
pub struct Resources {
    context: Context,
    resources: Rc<ResourceTable>,
    textures: Vec<(TextureId, Arc<GpuTexture>)>,
    buffers: Vec<(BufferId, Arc<GpuBuffer>)>,
    samplers: Vec<(SamplerId, Arc<raw::Sampler>)>,
    pipelines: Vec<(
        RenderPipelineId,
        raw::TextureFormat,
        Option<TextureFormat>,
        Arc<GpuPipeline>,
    )>,
    clear_pipelines: Vec<(raw::TextureFormat, bool, Arc<GpuClearPipeline>)>,
    mapped_images: Vec<(TextureId, Arc<GpuTexture>)>,
    programmable: programmable::Cache,
    blit: blit::Cache,
}

impl Resources {
    /// Return the logical resource table retained by this cache.
    ///
    /// # Returns
    ///
    /// The resource table used to brand submitted references.
    pub fn resource_table(&self) -> &ResourceTable {
        self.resources.as_ref()
    }

    /// Remove a retired texture's allocation and dependent cached views.
    /// Encoded commands retain their own strong resource references.
    pub fn release_texture(&mut self, id: TextureId) -> Result<()> {
        self.resources.texture_ref(id)?;
        self.textures.retain(|(candidate, _)| *candidate != id);
        self.mapped_images.retain(|(candidate, _)| *candidate != id);
        self.programmable.groups.clear();
        Ok(())
    }

    /// Remove the cached allocation for a retired logical buffer.
    pub fn release_buffer(&mut self, id: BufferId) -> Result<()> {
        self.resources.buffer_ref(id)?;
        self.buffers.retain(|(candidate, _)| *candidate != id);
        self.programmable.groups.clear();
        Ok(())
    }

    /// Drop a bind group's cached materialization once its recorded commands
    /// have been encoded. Encoded/submitted WGPU commands retain their own
    /// resource references; retiring this cache does not require a GPU wait.
    /// The caller separately retires the identity in the resource table.
    pub fn release_bind_group(&mut self, id: ir::BindGroupId) -> Result<()> {
        self.resources.bind_group_ref(id)?;
        self.programmable
            .groups
            .retain(|(candidate, _)| *candidate != id);
        Ok(())
    }

    /// Map a logical presentation texture to a physical WGPU image.
    ///
    /// # Arguments
    ///
    /// * `texture` - Logical texture with `RENDER_ATTACHMENT | PRESENT` usage.
    /// * `image` - Physical image with matching dimensions, format, and device.
    ///
    /// # Returns
    ///
    /// Success or a resource/table/format mapping error. Mapping the same
    /// logical texture again replaces its previous physical image, which is
    /// required when presenting a newly acquired WGPU surface frame.
    pub fn map_image(&mut self, texture: TextureId, image: Arc<Image>) -> Result<()> {
        self.bind_image(
            texture,
            image,
            TextureUsage::RENDER_ATTACHMENT | TextureUsage::PRESENT,
        )
    }

    /// Bind a shared color target for a producer without requiring presentation usage.
    pub fn map_color_target(&mut self, texture: TextureId, image: Arc<Image>) -> Result<()> {
        self.bind_image(texture, image, TextureUsage::RENDER_ATTACHMENT)
    }

    /// Bind a shared image for shader sampling by a consumer.
    pub fn import_sampled_image(&mut self, texture: TextureId, image: Arc<Image>) -> Result<()> {
        let desc = self
            .resources
            .texture(self.resources.texture_ref(texture)?)?;
        let read_only = TextureUsage::SAMPLED | TextureUsage::COPY_SRC;
        if !read_only.contains(desc.usage()) {
            return Err(Error::InvalidState);
        }
        self.bind_image(texture, image, TextureUsage::SAMPLED)
    }

    /// Bind a sampled image with the caller's authorized transfer usage.
    pub fn map_sampled_transfer_image(&mut self, texture: TextureId, image: Arc<Image>) -> Result<()> {
        let desc = self.resources.texture(self.resources.texture_ref(texture)?)?;
        let supported = TextureUsage::SAMPLED | TextureUsage::COPY_SRC | TextureUsage::COPY_DST;
        if !supported.contains(desc.usage()) {
            return Err(Error::InvalidState);
        }
        self.bind_image(texture, image, TextureUsage::SAMPLED)
    }

    fn bind_image(
        &mut self,
        texture: TextureId,
        image: Arc<Image>,
        required: TextureUsage,
    ) -> Result<()> {
        if !Arc::ptr_eq(&self.context.device.identity, &image.device_identity) {
            return Err(Error::InvalidState);
        }
        let reference = self.resources.texture_ref(texture)?;
        let descriptor = self.resources.texture(reference)?;
        if !descriptor.usage().contains(required)
            || descriptor.usage().contains(TextureUsage::STORAGE)
            || descriptor.dimension_1d()
            || descriptor.mip_level_count() != 1
            || descriptor.array_layer_count() != 1
            || descriptor.extent().width() != image.width()
            || descriptor.extent().height() != image.height()
            || descriptor.format() != image.format()
        {
            return Err(Error::InvalidState);
        }
        if self
            .mapped_images
            .iter()
            .any(|(candidate, mapped)| *candidate != texture && Arc::ptr_eq(mapped, &image.gpu))
        {
            return Err(Error::ImageAlreadyMapped);
        }
        if let Some((_, mapped)) = self
            .mapped_images
            .iter_mut()
            .find(|(candidate, _)| *candidate == texture)
        {
            if Arc::ptr_eq(mapped, &image.gpu) {
                return Ok(());
            }
            *mapped = Arc::clone(&image.gpu);
        } else {
            self.mapped_images.push((texture, Arc::clone(&image.gpu)));
        }
        self.programmable.groups.clear();
        Ok(())
    }

    /// Remove a physical presentation mapping for a logical texture.
    pub fn unmap_image(&mut self, texture: TextureId) {
        self.mapped_images
            .retain(|(candidate, _)| *candidate != texture);
        self.programmable.groups.clear();
    }

    fn texture(&mut self, reference: TextureRef<'_>) -> Result<Arc<GpuTexture>> {
        let descriptor = self.resources.texture(reference)?;
        if descriptor.dimension_1d() {
            return Err(Error::Unsupported(UnsupportedFeature::SurfaceFormat));
        }
        let id = reference.id();
        if let Some((_, image)) = self
            .mapped_images
            .iter()
            .find(|(candidate, _)| *candidate == id)
        {
            return Ok(Arc::clone(image));
        }
        if descriptor.usage().contains(TextureUsage::PRESENT) {
            return Err(Error::ImageNotMapped);
        }
        if let Some((_, texture)) = self.textures.iter().find(|(candidate, _)| *candidate == id) {
            return Ok(Arc::clone(texture));
        }
        let usage = texture_usage(descriptor)?;
        let texture = create_gpu_texture(
            self.context.device.raw_device(),
            descriptor.extent().width(),
            descriptor.extent().height(),
            descriptor.format(),
            descriptor.mip_level_count(),
            descriptor.array_layer_count(),
            usage,
        )?;
        self.textures.push((id, Arc::clone(&texture)));
        Ok(texture)
    }

    fn buffer(&mut self, reference: BufferRef<'_>) -> Result<Arc<GpuBuffer>> {
        let descriptor = self.resources.buffer(reference)?;
        let id = reference.id();
        if let Some((_, buffer)) = self.buffers.iter().find(|(candidate, _)| *candidate == id) {
            return Ok(Arc::clone(buffer));
        }
        if descriptor.size() > self.context.device.raw_device().limits().max_buffer_size {
            return Err(Error::Unsupported(UnsupportedFeature::ResourceSize));
        }
        let mut usage = raw::BufferUsages::empty();
        if descriptor.usage().contains(BufferUsage::VERTEX) {
            usage |= raw::BufferUsages::VERTEX;
        }
        if descriptor.usage().contains(BufferUsage::UNIFORM) {
            usage |= raw::BufferUsages::UNIFORM;
        }
        if descriptor.usage().contains(BufferUsage::STORAGE) {
            usage |= raw::BufferUsages::STORAGE;
        }
        if descriptor.usage().contains(BufferUsage::INDEX) {
            usage |= raw::BufferUsages::INDEX;
        }
        if descriptor.usage().contains(BufferUsage::COPY_SRC) {
            usage |= raw::BufferUsages::COPY_SRC;
        }
        if descriptor.usage().contains(BufferUsage::COPY_DST) {
            usage |= raw::BufferUsages::COPY_DST;
        }
        let buffer = Arc::new(GpuBuffer {
            buffer: self
                .context
                .device
                .raw_device()
                .create_buffer(&raw::BufferDescriptor {
                    label: Some("sgfx wgpu buffer"),
                    size: descriptor.size(),
                    usage,
                    mapped_at_creation: false,
                }),
        });
        self.buffers.push((id, Arc::clone(&buffer)));
        Ok(buffer)
    }

    fn sampler(&mut self, reference: SamplerRef<'_>) -> Result<Arc<raw::Sampler>> {
        let descriptor = self.resources.sampler(reference)?;
        let id = reference.id();
        if let Some((_, sampler)) = self.samplers.iter().find(|(candidate, _)| *candidate == id) {
            return Ok(Arc::clone(sampler));
        }
        // Clamping negative LODs to zero preserves filtering only when min
        // and mag filters agree; otherwise it can change which filter is used.
        if descriptor.min_lod() < 0.0 && descriptor.min_filter() != descriptor.mag_filter() {
            return Err(Error::Unsupported(UnsupportedFeature::SamplerLod));
        }
        if descriptor.anisotropy() > 1
            && (descriptor.min_filter() != FilterMode::Linear
                || descriptor.mag_filter() != FilterMode::Linear
                || descriptor.mip_filter() != FilterMode::Linear)
        {
            return Err(Error::Unsupported(UnsupportedFeature::SamplerLod));
        }
        let border = descriptor.address_u() == AddressMode::ClampToBorder
            || descriptor.address_v() == AddressMode::ClampToBorder;
        if border
            && !self
                .context
                .device
                .raw_device()
                .features()
                .contains(raw::Features::ADDRESS_MODE_CLAMP_TO_BORDER)
        {
            return Err(Error::Unsupported(UnsupportedFeature::SamplerBorderColor));
        }
        let sampler = Arc::new(self.context.device.raw_device().create_sampler(
            &raw::SamplerDescriptor {
                label: Some("sgfx wgpu sampler"),
                address_mode_u: address_mode(descriptor.address_u()),
                address_mode_v: address_mode(descriptor.address_v()),
                address_mode_w: raw::AddressMode::ClampToEdge,
                mag_filter: filter_mode(descriptor.mag_filter()),
                min_filter: filter_mode(descriptor.min_filter()),
                mipmap_filter: filter_mode(descriptor.mip_filter()),
                // Logical textures have at most 32 levels. Larger Vulkan LOD
                // maxima are equivalent to this physical clamp.
                lod_min_clamp: descriptor.min_lod().clamp(0.0, 32.0),
                lod_max_clamp: descriptor.max_lod().clamp(0.0, 32.0),
                compare: descriptor.compare().map(compare_function),
                anisotropy_clamp: u16::from(descriptor.anisotropy()),
                border_color: border.then_some(raw::SamplerBorderColor::TransparentBlack),
            },
        ));
        self.samplers.push((id, Arc::clone(&sampler)));
        Ok(sampler)
    }

    fn pipeline(
        &mut self,
        id: RenderPipelineId,
        target_format: raw::TextureFormat,
        sample_format: Option<TextureFormat>,
    ) -> Result<Arc<GpuPipeline>> {
        let reference = self.resources.render_pipeline_ref(id)?;
        let descriptor = self.resources.render_pipeline(reference)?;
        let sample_format = match descriptor.fragment() {
            FragmentProgram::Texture(_) | FragmentProgram::TextureVertexColor(_) => sample_format,
            FragmentProgram::Solid | FragmentProgram::VertexColor => None,
        };
        if let Some((_, _, _, pipeline)) =
            self.pipelines
                .iter()
                .find(|(candidate, format, sampled, _)| {
                    *candidate == id && *format == target_format && *sampled == sample_format
                })
        {
            return Ok(Arc::clone(pipeline));
        }
        let pipeline = Arc::new(create_pipeline(
            self.context.device.raw_device(),
            &descriptor,
            target_format,
            sample_format,
        )?);
        self.pipelines
            .push((id, target_format, sample_format, Arc::clone(&pipeline)));
        Ok(pipeline)
    }

    fn pipeline_for_id(
        &mut self,
        id: RenderPipelineId,
        target_format: raw::TextureFormat,
        sample_format: Option<TextureFormat>,
    ) -> Result<Arc<GpuPipeline>> {
        self.pipeline(id, target_format, sample_format)
    }

    fn clear_pipeline(
        &mut self,
        target_format: raw::TextureFormat,
        has_depth: bool,
    ) -> Arc<GpuClearPipeline> {
        if let Some((_, _, pipeline)) = self
            .clear_pipelines
            .iter()
            .find(|(format, depth, _)| *format == target_format && *depth == has_depth)
        {
            return Arc::clone(pipeline);
        }
        let pipeline = Arc::new(create_clear_pipeline(
            self.context.device.raw_device(),
            target_format,
            has_depth,
        ));
        self.clear_pipelines
            .push((target_format, has_depth, Arc::clone(&pipeline)));
        pipeline
    }
}

/// Queue that lowers and submits validated SGFX IR through WGPU.
#[derive(Clone)]
pub struct Queue {
    context: Context,
}

fn validate_indexed_topologies(commands: &CommandBuffer<'_, '_>) -> Result<()> {
    let mut pipeline = None;
    for command in commands.commands() {
        match command {
            Command::SetViewport(viewport) if viewport.components()[3] <= 0.0 => {
                return Err(Error::Unsupported(UnsupportedFeature::SignedViewport));
            }
            Command::BeginRenderPass(_) | Command::EndRenderPass | Command::SetPipeline(_) => {
                pipeline = None
            }
            Command::SetProgrammablePipeline(reference) => {
                pipeline = Some(
                    commands
                        .resources()
                        .programmable_render_pipeline(*reference)?
                        .topology(),
                );
                if pipeline == Some(ir::PrimitiveTopology::TriangleFan) {
                    return Err(Error::Unsupported(UnsupportedFeature::TriangleFan));
                }
            }
            Command::DrawIndexed { .. } | Command::DrawIndexedInstanced { .. }
                if pipeline == Some(ir::PrimitiveTopology::TriangleStrip) =>
            {
                return Err(Error::Unsupported(UnsupportedFeature::IndexedTriangleStrip));
            }
            Command::DrawIndexed { .. } | Command::DrawIndexedInstanced { .. }
                if pipeline == Some(ir::PrimitiveTopology::LineStrip) =>
            {
                return Err(Error::Unsupported(UnsupportedFeature::IndexedLineStrip));
            }
            _ => {}
        }
    }
    Ok(())
}

impl Queue {
    /// Bind this queue to a persistent resource cache for command execution.
    ///
    /// # Arguments
    ///
    /// * `resources` - Persistent cache created by this queue's context.
    ///
    /// # Returns
    ///
    /// A backend-owned executor implementing
    /// [`sgfx_core::backend::CommandExecutor`].
    pub fn executor<'a>(&'a self, resources: &'a mut Resources) -> Executor<'a> {
        Executor {
            queue: self,
            resources,
        }
    }

    /// Submit one validated SGFX command buffer.
    ///
    /// Upload bytes are copied to backend-owned staging buffers and encoded
    /// in command order. The stream becomes one WGPU submission, and
    /// this method returns after it has been queued; WGPU GPU completion
    /// remains asynchronous.
    ///
    /// # Arguments
    ///
    /// * `resources` - Persistent resource cache created by this queue's context.
    /// * `commands` - Finished command buffer using the same resource table.
    ///
    /// # Returns
    ///
    /// Success after queue submission, or a translation/backend error.
    /// Buffer upload offsets and lengths must be multiples of four bytes;
    /// otherwise [`UnsupportedFeature::BufferWriteAlignment`] is returned
    /// before the upload reaches WGPU.
    pub fn submit<'r, 'data>(
        &self,
        resources: &mut Resources,
        commands: &CommandBuffer<'r, 'data>,
    ) -> Result<()> {
        validate_indexed_topologies(commands)?;
        self.submit_inner(resources, commands, None).map(|_| ())
    }

    /// Submit with bounded, owned completion tracking and no GPU-completion wait.
    ///
    /// # Arguments
    ///
    /// * `resources` - Persistent cache from this queue's device.
    /// * `commands` - Finished logical commands and borrowed uploads.
    ///
    /// # Returns
    ///
    /// A receipt, `Busy` when tracking is full, a side-effect-free rejection
    /// for invalid ownership/device loss, or a failed-prefix receipt. Tracking
    /// capacity is shared by this device's SGFX queue wrappers and is reclaimed
    /// by completion callbacks, not by dropping a public receipt.
    pub fn submit_tracked<'r, 'data>(
        &self,
        resources: &mut Resources,
        commands: &CommandBuffer<'r, 'data>,
    ) -> core::result::Result<Submission, SubmitError<Error, Submission>> {
        let device = &self.context.device;
        if !Arc::ptr_eq(&device.identity, &resources.context.device.identity) {
            return Err(SubmitError::Rejected(Error::InvalidState));
        }
        if !core::ptr::eq(resources.resources.as_ref(), commands.resources()) {
            return Err(SubmitError::Rejected(Error::ResourceTableMismatch));
        }
        validate_indexed_topologies(commands).map_err(SubmitError::Rejected)?;
        let _ = device.raw_device().poll(raw::Maintain::Poll);
        if device
            .tracker
            .lost
            .load(core::sync::atomic::Ordering::Acquire)
        {
            return Err(SubmitError::Rejected(Error::DeviceLost));
        }
        let slot = device.tracker.reserve().ok_or(SubmitError::Busy)?;
        let marker = {
            let _guard = device
                .validation
                .lock()
                .map_err(|_| SubmitError::Rejected(Error::InvalidState))?;
            programmable::validated(device.raw_device(), || {
                Ok(device.raw_device().create_buffer(&raw::BufferDescriptor {
                    label: Some("sgfx submission completion marker"),
                    size: 4,
                    usage: raw::BufferUsages::COPY_DST | raw::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }))
            })
            .map_err(SubmitError::Rejected)?
        };
        match self.submit_inner(resources, commands, Some(&marker)) {
            Ok(index) => Ok(Submission::new(device.clone(), index, marker, slot)),
            Err(error) => {
                // Lowering can stage WGPU uniform writes before encountering
                // a later error. Flush and track that possible prefix rather
                // than claiming a transactional rejection or dropping its receipt.
                let mut checkpoint =
                    device
                        .raw_device()
                        .create_command_encoder(&raw::CommandEncoderDescriptor {
                            label: Some("sgfx failed submission checkpoint"),
                        });
                checkpoint.clear_buffer(&marker, 0, None);
                let index = device.raw_queue().submit([checkpoint.finish()]);
                Err(SubmitError::Failed {
                    error,
                    completion: Submission::new(device.clone(), index, marker, slot),
                })
            }
        }
    }

    fn submit_inner<'r, 'data>(
        &self,
        resources: &mut Resources,
        commands: &CommandBuffer<'r, 'data>,
        marker: Option<&raw::Buffer>,
    ) -> Result<raw::SubmissionIndex> {
        if !Arc::ptr_eq(
            &self.context.device.identity,
            &resources.context.device.identity,
        ) {
            return Err(Error::InvalidState);
        }
        if !core::ptr::eq(resources.resources.as_ref(), commands.resources()) {
            return Err(Error::ResourceTableMismatch);
        }
        let _ = self.context.raw_device().poll(raw::Maintain::Poll);
        if self
            .context
            .device
            .tracker
            .lost
            .load(core::sync::atomic::Ordering::Acquire)
        {
            return Err(Error::DeviceLost);
        }
        let _scope_guard = self
            .context
            .device
            .validation
            .lock()
            .map_err(|_| Error::InvalidState)?;
        let encoded = programmable::validated(self.context.raw_device(), || {
            self.encode_commands(resources, commands, marker)
        })?;
        programmable::validated(self.context.raw_device(), || {
            Ok(self.context.raw_queue().submit([encoded]))
        })
    }

    fn encode_commands<'r, 'data>(
        &self,
        resources: &mut Resources,
        commands: &CommandBuffer<'r, 'data>,
        marker: Option<&raw::Buffer>,
    ) -> Result<raw::CommandBuffer> {
        let mut encoder = self.context.device.raw_device().create_command_encoder(
            &raw::CommandEncoderDescriptor {
                label: Some("sgfx wgpu command encoder"),
            },
        );
        let mut index = 0;
        while index < commands.commands().len() {
            match commands.commands().get(index).ok_or(Error::InvalidState)? {
                Command::WriteBuffer {
                    buffer,
                    offset,
                    data,
                } => {
                    if !offset.is_multiple_of(raw::COPY_BUFFER_ALIGNMENT)
                        || !data
                            .len()
                            .is_multiple_of(raw::COPY_BUFFER_ALIGNMENT as usize)
                    {
                        return Err(Error::Unsupported(UnsupportedFeature::BufferWriteAlignment));
                    }
                    let buffer = resources.buffer(*buffer)?;
                    if !data.is_empty() {
                        let staging = create_staging_buffer(self.context.raw_device(), data)?;
                        encoder.copy_buffer_to_buffer(
                            &staging,
                            0,
                            &buffer.buffer,
                            *offset,
                            data.len() as u64,
                        );
                    }
                }
                Command::BlitTexture {
                    source,
                    source_mip,
                    destination,
                    destination_mip,
                    source_rect,
                    destination_rect,
                    filter,
                    flips,
                } => {
                    let src = resources
                        .resources
                        .texture(*source)?
                        .mip_extent(*source_mip)?;
                    let dst = resources
                        .resources
                        .texture(*destination)?
                        .mip_extent(*destination_mip)?;
                    if *flips != [false; 2]
                        || *source_rect != ir::PixelRect::new(0, 0, src.width(), src.height())?
                        || *destination_rect != ir::PixelRect::new(0, 0, dst.width(), dst.height())?
                    {
                        return Err(Error::Unsupported(UnsupportedFeature::PartialBlit));
                    }
                    let source = resources.texture(*source)?;
                    let destination = resources.texture(*destination)?;
                    resources.encode_mip_blit(
                        &mut encoder,
                        &source,
                        *source_mip,
                        &destination,
                        *destination_mip,
                        *filter,
                    )?;
                }
                Command::WriteTexture { texture, write } => {
                    let texture_resource = resources.texture(*texture)?;
                    encode_texture_upload(
                        self.context.raw_device(),
                        &mut encoder,
                        &texture_resource,
                        *write,
                    )?;
                }
                Command::CopyBufferToBuffer {
                    source,
                    source_offset,
                    destination,
                    destination_offset,
                    size,
                } => {
                    if !source_offset.is_multiple_of(raw::COPY_BUFFER_ALIGNMENT)
                        || !destination_offset.is_multiple_of(raw::COPY_BUFFER_ALIGNMENT)
                        || !size.is_multiple_of(raw::COPY_BUFFER_ALIGNMENT)
                    {
                        return Err(Error::Unsupported(UnsupportedFeature::BufferWriteAlignment));
                    }
                    let source = resources.buffer(*source)?;
                    let destination = resources.buffer(*destination)?;
                    encoder.copy_buffer_to_buffer(
                        &source.buffer,
                        *source_offset,
                        &destination.buffer,
                        *destination_offset,
                        *size,
                    );
                }
                Command::CopyTextureToTexture {
                    source,
                    source_rect,
                    destination,
                    destination_rect,
                } => {
                    let source = resources.texture(*source)?;
                    let destination = resources.texture(*destination)?;
                    encoder.copy_texture_to_texture(
                        raw::TexelCopyTextureInfo {
                            texture: &source.texture,
                            mip_level: 0,
                            origin: raw::Origin3d {
                                x: source_rect.x(),
                                y: source_rect.y(),
                                z: 0,
                            },
                            aspect: raw::TextureAspect::All,
                        },
                        raw::TexelCopyTextureInfo {
                            texture: &destination.texture,
                            mip_level: 0,
                            origin: raw::Origin3d {
                                x: destination_rect.x(),
                                y: destination_rect.y(),
                                z: 0,
                            },
                            aspect: raw::TextureAspect::All,
                        },
                        raw::Extent3d {
                            width: source_rect.width(),
                            height: source_rect.height(),
                            depth_or_array_layers: 1,
                        },
                    );
                }
                Command::BeginRenderPass(desc) => {
                    let end = commands.commands()[index + 1..]
                        .iter()
                        .position(|command| matches!(command, Command::EndRenderPass))
                        .map(|relative| index + 1 + relative)
                        .ok_or(Error::InvalidState)?;
                    self.encode_render_pass(
                        resources,
                        &mut encoder,
                        *desc,
                        &commands.commands()[index + 1..end],
                    )?;
                    index = end;
                }
                Command::BeginComputePass => {
                    let end = commands.commands()[index + 1..]
                        .iter()
                        .position(|command| matches!(command, Command::EndComputePass))
                        .map(|relative| index + 1 + relative)
                        .ok_or(Error::InvalidState)?;
                    self.encode_compute_pass(
                        resources,
                        &mut encoder,
                        &commands.commands()[index + 1..end],
                    )?;
                    index = end;
                }
                // IR validates this single-queue barrier's usage and pass scope.
                // WGPU inserts resource transitions between ordered encoder operations.
                Command::ResourceBarrier(_) => {}
                Command::EndRenderPass => return Err(Error::InvalidState),
                _ => return Err(Error::InvalidState),
            }
            index += 1;
        }
        if let Some(marker) = marker {
            encoder.clear_buffer(marker, 0, None);
        }
        Ok(encoder.finish())
    }

    fn encode_render_pass<'r, 'data>(
        &self,
        resources: &mut Resources,
        encoder: &mut raw::CommandEncoder,
        desc: RenderPassDesc<'r>,
        commands: &[Command<'r, 'data>],
    ) -> Result<()> {
        let target = resources.texture(desc.target())?;
        let colors = desc
            .color_attachments()
            .map(|attachment| {
                resources
                    .texture(attachment.target())
                    .map(|texture| (attachment, texture))
            })
            .collect::<Result<Vec<_>>>()?;
        let depth = desc
            .depth_attachment()
            .map(|attachment| resources.texture(attachment.target()))
            .transpose()?;
        let full_area = render_area_is_full(desc.area(), target.width, target.height);
        if !full_area
            && colors.len() > 1
            && colors
                .iter()
                .any(|(attachment, _)| matches!(attachment.load(), LoadOp::Clear(_)))
        {
            return Err(Error::Unsupported(
                UnsupportedFeature::PartialMultiTargetClear,
            ));
        }
        let partial_clear_color = if full_area {
            None
        } else {
            match desc.load() {
                LoadOp::Clear(color) => Some(color),
                LoadOp::Load | LoadOp::DontCare => None,
            }
        };
        let color_attachments = colors
            .iter()
            .map(|(attachment, texture)| {
                Some(raw::RenderPassColorAttachment {
                    view: &texture.view,
                    resolve_target: None,
                    ops: raw::Operations {
                        load: if full_area {
                            color_load_op(attachment.load())
                        } else {
                            raw::LoadOp::Load
                        },
                        store: store_op(attachment.store()),
                    },
                })
            })
            .collect::<Vec<_>>();
        let depth_load = desc.depth_attachment().map(|attachment| attachment.load());
        let depth_store = desc.depth_attachment().map(|attachment| attachment.store());
        let depth_ops = if desc
            .depth_attachment()
            .is_some_and(|attachment| attachment.read_only())
        {
            None
        } else {
            match (depth_load, depth_store) {
                (Some(load), Some(store)) => {
                    let load = if full_area {
                        depth_load_op(load)
                    } else {
                        match load {
                            DepthLoadOp::Clear(_) => {
                                return Err(Error::Unsupported(
                                    UnsupportedFeature::PartialDepthClear,
                                ));
                            }
                            DepthLoadOp::Load | DepthLoadOp::DontCare => raw::LoadOp::Load,
                        }
                    };
                    Some(raw::Operations {
                        load,
                        store: store_op(store),
                    })
                }
                (None, None) => None,
                _ => return Err(Error::InvalidState),
            }
        };
        let clear_pipeline =
            partial_clear_color.map(|_| resources.clear_pipeline(target.format, depth.is_some()));
        let depth_attachment = depth
            .as_ref()
            .map(|depth| raw::RenderPassDepthStencilAttachment {
                view: &depth.view,
                depth_ops,
                stencil_ops: None,
            });
        let query_sets = self.query_sets(false)?;
        let mut render_pass = encoder.begin_render_pass(&raw::RenderPassDescriptor {
            label: Some("sgfx wgpu render pass"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: depth_attachment,
            timestamp_writes: None,
            occlusion_query_set: query_sets.occlusion.as_ref(),
        });
        render_pass.set_scissor_rect(
            desc.area().x(),
            desc.area().y(),
            desc.area().width(),
            desc.area().height(),
        );
        if let (Some(color), Some(pipeline)) = (partial_clear_color, clear_pipeline.as_ref()) {
            self.encode_color_clear(&mut render_pass, pipeline, color);
        }
        if query_sets.occlusion.is_some() {
            render_pass.begin_occlusion_query(0);
        }
        if let Some(set) = &query_sets.statistics {
            render_pass.begin_pipeline_statistics_query(set, 0);
        }
        let mut state = PassState {
            pipeline: None,
            pipeline_id: None,
            vertex_buffers: std::array::from_fn(|_| None),
            index_buffer: None,
            texture: None,
            sampler: None,
            uniforms: None,
            has_depth_attachment: depth.is_some(),
            programmable: None,
            bind_groups: Vec::new(),
        };
        for command in commands {
            match command {
                Command::SetPipeline(pipeline) => {
                    state.programmable = None;
                    let sample_format =
                        state.texture.as_ref().map(|texture| texture.logical_format);
                    state.pipeline_id = Some(pipeline.id());
                    state.pipeline =
                        Some(resources.pipeline(pipeline.id(), target.format, sample_format)?);
                    let pipeline = state.pipeline.as_ref().ok_or(Error::InvalidState)?;
                    if pipeline.has_depth != state.has_depth_attachment {
                        return Err(Error::InvalidState);
                    }
                    render_pass.set_pipeline(&pipeline.pipeline);
                }
                Command::SetProgrammablePipeline(pipeline) => {
                    let pipeline = resources.programmable_pipeline(
                        pipeline.id(),
                        target.format,
                        state.has_depth_attachment,
                    )?;
                    if pipeline.has_depth != state.has_depth_attachment {
                        return Err(Error::InvalidState);
                    }
                    render_pass.set_pipeline(&pipeline.pipeline);
                    state.programmable = Some(pipeline);
                    state.pipeline = None;
                    state.pipeline_id = None;
                }
                Command::SetBindGroup { index, bind_group } => {
                    let group = resources.programmable_bind_group(*bind_group)?;
                    if let Some((_, current)) =
                        state.bind_groups.iter_mut().find(|(slot, _)| slot == index)
                    {
                        *current = group;
                    } else {
                        state.bind_groups.push((*index, group));
                    }
                }
                Command::SetVertexBuffer { buffer, offset } => {
                    state.vertex_buffers[0] = Some((resources.buffer(*buffer)?, *offset));
                }
                Command::SetVertexBufferSlot {
                    slot,
                    buffer,
                    offset,
                } => {
                    state.vertex_buffers[*slot as usize] =
                        Some((resources.buffer(*buffer)?, *offset));
                }
                Command::SetIndexBuffer {
                    buffer,
                    offset,
                    format,
                } => {
                    state.index_buffer = Some((resources.buffer(*buffer)?, *offset, *format));
                }
                Command::SetTexture(texture) => {
                    let texture = resources.texture(*texture)?;
                    state.texture = Some(Arc::clone(&texture));
                    if state
                        .pipeline
                        .as_ref()
                        .is_some_and(|pipeline| pipeline.requires_texture)
                    {
                        let pipeline_id = state.pipeline_id.ok_or(Error::InvalidState)?;
                        let pipeline = resources.pipeline_for_id(
                            pipeline_id,
                            target.format,
                            Some(texture.logical_format),
                        )?;
                        if pipeline.has_depth != state.has_depth_attachment {
                            return Err(Error::InvalidState);
                        }
                        state.pipeline = Some(pipeline);
                        let pipeline = state.pipeline.as_ref().ok_or(Error::InvalidState)?;
                        render_pass.set_pipeline(&pipeline.pipeline);
                    }
                }
                Command::SetSampler(sampler) => {
                    state.sampler = Some(resources.sampler(*sampler)?);
                }
                Command::SetDrawIndex(_) => {}
                Command::SetUniforms(uniforms) => state.uniforms = Some(*uniforms),
                Command::SetScissor(scissor) => {
                    let rectangle = scissor.unwrap_or(desc.area());
                    render_pass.set_scissor_rect(
                        rectangle.x(),
                        rectangle.y(),
                        rectangle.width(),
                        rectangle.height(),
                    );
                }
                Command::SetViewport(viewport) => {
                    let [x, y, width, height, min_depth, max_depth] = viewport.components();
                    render_pass.set_viewport(x, y, width, height, min_depth, max_depth);
                }
                Command::SetPushConstants {
                    stages,
                    offset,
                    data,
                } => {
                    let stages = programmable::shader_stages(*stages)
                        & (raw::ShaderStages::VERTEX | raw::ShaderStages::FRAGMENT);
                    if !stages.is_empty() {
                        render_pass.set_push_constants(stages, *offset, data);
                    }
                }
                Command::Draw {
                    vertex_count,
                    first_vertex,
                } => {
                    self.encode_draw(&mut render_pass, &state, *vertex_count, *first_vertex, 0..1)?;
                    self.assembly_query(
                        *vertex_count,
                        1,
                        state
                            .programmable
                            .as_ref()
                            .map_or(ir::PrimitiveTopology::TriangleList, |p| p.topology),
                    )?;
                }
                Command::DrawIndexed {
                    index_count,
                    first_index,
                    base_vertex,
                } => {
                    self.encode_indexed_draw(
                        &mut render_pass,
                        &state,
                        *index_count,
                        *first_index,
                        *base_vertex,
                        0..1,
                    )?;
                    self.assembly_query(
                        *index_count,
                        1,
                        state
                            .programmable
                            .as_ref()
                            .map_or(ir::PrimitiveTopology::TriangleList, |p| p.topology),
                    )?;
                }
                Command::DrawInstanced {
                    vertex_count,
                    first_vertex,
                    instance_count,
                    first_instance,
                } => {
                    self.encode_draw(
                        &mut render_pass,
                        &state,
                        *vertex_count,
                        *first_vertex,
                        *first_instance..*first_instance + *instance_count,
                    )?;
                    self.assembly_query(
                        *vertex_count,
                        *instance_count,
                        state
                            .programmable
                            .as_ref()
                            .map_or(ir::PrimitiveTopology::TriangleList, |p| p.topology),
                    )?;
                }
                Command::DrawIndexedInstanced {
                    index_count,
                    first_index,
                    base_vertex,
                    instance_count,
                    first_instance,
                } => {
                    self.encode_indexed_draw(
                        &mut render_pass,
                        &state,
                        *index_count,
                        *first_index,
                        *base_vertex,
                        *first_instance..*first_instance + *instance_count,
                    )?;
                    self.assembly_query(
                        *index_count,
                        *instance_count,
                        state
                            .programmable
                            .as_ref()
                            .map_or(ir::PrimitiveTopology::TriangleList, |p| p.topology),
                    )?;
                }
                _ => return Err(Error::InvalidState),
            }
        }
        if query_sets.occlusion.is_some() {
            render_pass.end_occlusion_query();
        }
        if query_sets.statistics.is_some() {
            render_pass.end_pipeline_statistics_query();
        }
        drop(render_pass);
        self.resolve_queries(encoder, query_sets)?;
        Ok(())
    }

    fn encode_color_clear(
        &self,
        render_pass: &mut raw::RenderPass<'_>,
        pipeline: &GpuClearPipeline,
        color: ir::Color,
    ) {
        let uniform_buffer =
            self.context
                .device
                .raw_device()
                .create_buffer(&raw::BufferDescriptor {
                    label: Some("sgfx wgpu partial clear uniforms"),
                    size: core::mem::size_of::<ClearUniforms>() as u64,
                    usage: raw::BufferUsages::UNIFORM | raw::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
        let uniforms = ClearUniforms {
            color: color.components(),
        };
        self.context.device.raw_queue().write_buffer(
            &uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );
        let bind_group =
            self.context
                .device
                .raw_device()
                .create_bind_group(&raw::BindGroupDescriptor {
                    label: Some("sgfx wgpu partial clear bind group"),
                    layout: &pipeline.bind_group_layout,
                    entries: &[raw::BindGroupEntry {
                        binding: 0,
                        resource: raw::BindingResource::Buffer(raw::BufferBinding {
                            buffer: &uniform_buffer,
                            offset: 0,
                            size: None,
                        }),
                    }],
                });
        render_pass.set_pipeline(&pipeline.pipeline);
        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }

    fn encode_draw(
        &self,
        render_pass: &mut raw::RenderPass<'_>,
        state: &PassState,
        vertex_count: u32,
        first_vertex: u32,
        instances: core::ops::Range<u32>,
    ) -> Result<()> {
        if let Some(pipeline) = &state.programmable {
            self.prepare_programmable_draw(render_pass, state, pipeline)?;
            render_pass.draw(first_vertex..first_vertex + vertex_count, instances);
            return Ok(());
        }
        let pipeline = state.pipeline.as_ref().ok_or(Error::InvalidState)?;
        let (vertex_buffer, offset) = state.vertex_buffers[0]
            .as_ref()
            .ok_or(Error::InvalidState)?;
        let uniforms = state.uniforms.ok_or(Error::InvalidState)?;
        let bind_group = self.create_bind_group(pipeline, uniforms, state)?;
        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.buffer.slice(*offset..));
        render_pass.draw(first_vertex..first_vertex + vertex_count, 0..1);
        Ok(())
    }

    fn encode_indexed_draw(
        &self,
        render_pass: &mut raw::RenderPass<'_>,
        state: &PassState,
        index_count: u32,
        first_index: u32,
        base_vertex: i32,
        instances: core::ops::Range<u32>,
    ) -> Result<()> {
        if let Some(pipeline) = &state.programmable {
            if pipeline.topology == ir::PrimitiveTopology::TriangleStrip {
                return Err(Error::Unsupported(UnsupportedFeature::IndexedTriangleStrip));
            }
            if pipeline.topology == ir::PrimitiveTopology::LineStrip {
                return Err(Error::Unsupported(UnsupportedFeature::IndexedLineStrip));
            }
            self.prepare_programmable_draw(render_pass, state, pipeline)?;
            let (buffer, offset, format) =
                state.index_buffer.as_ref().ok_or(Error::InvalidState)?;
            render_pass.set_index_buffer(buffer.buffer.slice(*offset..), index_format(*format));
            render_pass.draw_indexed(
                first_index..first_index + index_count,
                base_vertex,
                instances,
            );
            return Ok(());
        }
        let pipeline = state.pipeline.as_ref().ok_or(Error::InvalidState)?;
        let (vertex_buffer, vertex_offset) = state.vertex_buffers[0]
            .as_ref()
            .ok_or(Error::InvalidState)?;
        let (index_buffer, index_offset, index_kind) =
            state.index_buffer.as_ref().ok_or(Error::InvalidState)?;
        let uniforms = state.uniforms.ok_or(Error::InvalidState)?;
        let bind_group = self.create_bind_group(pipeline, uniforms, state)?;
        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.buffer.slice(*vertex_offset..));
        render_pass.set_index_buffer(
            index_buffer.buffer.slice(*index_offset..),
            index_format(*index_kind),
        );
        render_pass.draw_indexed(first_index..first_index + index_count, base_vertex, 0..1);
        Ok(())
    }

    fn prepare_programmable_draw(
        &self,
        pass: &mut raw::RenderPass<'_>,
        state: &PassState,
        pipeline: &programmable::RenderPipeline,
    ) -> Result<()> {
        for slot in 0..pipeline.vertex_buffer_count {
            let (buffer, offset) = state.vertex_buffers[slot]
                .as_ref()
                .ok_or(Error::InvalidState)?;
            pass.set_vertex_buffer(slot as u32, buffer.buffer.slice(*offset..));
        }
        for (index, empty) in pipeline.empty_groups.iter().enumerate() {
            let group = state
                .bind_groups
                .iter()
                .find(|(slot, _)| *slot == index as u32)
                .map(|(_, group)| group)
                .or(empty.as_ref())
                .ok_or(Error::InvalidState)?;
            pass.set_bind_group(index as u32, group.as_ref(), &[]);
        }
        Ok(())
    }

    fn create_bind_group(
        &self,
        pipeline: &GpuPipeline,
        uniforms: DrawUniforms,
        state: &PassState,
    ) -> Result<raw::BindGroup> {
        let sampled = if pipeline.requires_texture {
            let texture = state.texture.as_ref().ok_or(Error::InvalidState)?;
            let sampler = state.sampler.as_ref().ok_or(Error::InvalidState)?;
            if texture.logical_format == TextureFormat::R8Unorm
                && pipeline.sample_mode != Some(TextureSampleMode::AlphaMask)
            {
                return Err(Error::Unsupported(UnsupportedFeature::TextureFormat));
            }
            Some((texture, sampler))
        } else {
            None
        };
        let uniform_buffer =
            self.context
                .device
                .raw_device()
                .create_buffer(&raw::BufferDescriptor {
                    label: Some("sgfx wgpu draw uniforms"),
                    size: core::mem::size_of::<Uniforms>() as u64,
                    usage: raw::BufferUsages::UNIFORM | raw::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
        let bytes = Uniforms::from(uniforms);
        self.context.device.raw_queue().write_buffer(
            &uniform_buffer,
            0,
            bytemuck::bytes_of(&bytes),
        );
        let mut entries = Vec::with_capacity(if pipeline.requires_texture { 3 } else { 1 });
        entries.push(raw::BindGroupEntry {
            binding: 0,
            resource: raw::BindingResource::Buffer(raw::BufferBinding {
                buffer: &uniform_buffer,
                offset: 0,
                size: None,
            }),
        });
        if let Some((texture, sampler)) = sampled {
            entries.push(raw::BindGroupEntry {
                binding: 1,
                resource: raw::BindingResource::TextureView(&texture.sampled_view),
            });
            entries.push(raw::BindGroupEntry {
                binding: 2,
                resource: raw::BindingResource::Sampler(sampler.as_ref()),
            });
        }
        Ok(self
            .context
            .device
            .raw_device()
            .create_bind_group(&raw::BindGroupDescriptor {
                label: Some("sgfx wgpu draw bind group"),
                layout: &pipeline.bind_group_layout,
                entries: &entries,
            }))
    }
}

/// WGPU command executor bound to a queue and persistent resource cache.
pub struct Executor<'a> {
    queue: &'a Queue,
    resources: &'a mut Resources,
}

impl sgfx_core::backend::CommandExecutor for Executor<'_> {
    type Error = Error;

    fn execute<'r, 'data>(&mut self, commands: &CommandBuffer<'r, 'data>) -> Result<()> {
        self.queue.submit(self.resources, commands)
    }
}

impl CommandSubmitter for Executor<'_> {
    type Submission = Submission;

    fn submit<'r, 'data>(
        &mut self,
        commands: &CommandBuffer<'r, 'data>,
    ) -> core::result::Result<Submission, SubmitError<Error, Submission>> {
        self.queue.submit_tracked(self.resources, commands)
    }
}

struct PassState {
    pipeline: Option<Arc<GpuPipeline>>,
    pipeline_id: Option<RenderPipelineId>,
    vertex_buffers: [Option<(Arc<GpuBuffer>, u64)>; ir::MAX_VERTEX_BUFFERS],
    index_buffer: Option<(Arc<GpuBuffer>, u64, IndexFormat)>,
    texture: Option<Arc<GpuTexture>>,
    sampler: Option<Arc<raw::Sampler>>,
    uniforms: Option<DrawUniforms>,
    has_depth_attachment: bool,
    programmable: Option<Arc<programmable::RenderPipeline>>,
    bind_groups: Vec<(u32, Arc<raw::BindGroup>)>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    transform: [[f32; 4]; 4],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ClearUniforms {
    color: [f32; 4],
}

impl From<DrawUniforms> for Uniforms {
    fn from(uniforms: DrawUniforms) -> Self {
        let columns = uniforms.transform().columns();
        Self {
            transform: [
                [columns[0], columns[1], columns[2], columns[3]],
                [columns[4], columns[5], columns[6], columns[7]],
                [columns[8], columns[9], columns[10], columns[11]],
                [columns[12], columns[13], columns[14], columns[15]],
            ],
            color: uniforms.color().components(),
        }
    }
}

struct GpuTexture {
    texture: raw::Texture,
    view: raw::TextureView,
    sampled_view: raw::TextureView,
    format: raw::TextureFormat,
    logical_format: TextureFormat,
    width: u32,
    height: u32,
}

struct GpuBuffer {
    buffer: raw::Buffer,
}

struct GpuPipeline {
    pipeline: raw::RenderPipeline,
    bind_group_layout: raw::BindGroupLayout,
    requires_texture: bool,
    has_depth: bool,
    sample_mode: Option<TextureSampleMode>,
}

struct GpuClearPipeline {
    pipeline: raw::RenderPipeline,
    bind_group_layout: raw::BindGroupLayout,
}

// Mapped-at-creation staging owns borrowed upload data immediately. WGPU's
// encoder retains the staging resource until the submitted copy retires.
fn create_staging_buffer(device: &raw::Device, data: &[u8]) -> Result<raw::Buffer> {
    let buffer = programmable::validated(device, || {
        Ok(device.create_buffer(&raw::BufferDescriptor {
            label: Some("sgfx ordered upload staging"),
            size: (data.len() as u64).next_multiple_of(raw::COPY_BUFFER_ALIGNMENT),
            usage: raw::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        }))
    })?;
    buffer.slice(..).get_mapped_range_mut()[..data.len()].copy_from_slice(data);
    buffer.unmap();
    Ok(buffer)
}

fn encode_texture_upload(
    device: &raw::Device,
    encoder: &mut raw::CommandEncoder,
    texture: &GpuTexture,
    write: ir::TextureWrite<'_>,
) -> Result<()> {
    let area = write.destination();
    let row_size = area
        .width()
        .checked_mul(
            texture
                .logical_format
                .bytes_per_pixel()
                .ok_or(Error::Unsupported(UnsupportedFeature::TextureFormat))?,
        )
        .ok_or(Error::Unsupported(UnsupportedFeature::ResourceSize))?;
    let stride = row_size
        .checked_next_multiple_of(raw::COPY_BYTES_PER_ROW_ALIGNMENT)
        .ok_or(Error::Unsupported(UnsupportedFeature::ResourceSize))?;
    let size = u64::from(stride) * u64::from(area.height());
    if size > device.limits().max_buffer_size {
        return Err(Error::Unsupported(UnsupportedFeature::ResourceSize));
    }
    let staging = programmable::validated(device, || {
        Ok(device.create_buffer(&raw::BufferDescriptor {
            label: Some("sgfx ordered texture upload staging"),
            size,
            usage: raw::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        }))
    })?;
    {
        let mut destination = staging.slice(..).get_mapped_range_mut();
        for row in 0..area.height() as usize {
            let source_start = row * write.bytes_per_row() as usize;
            let target_start = row * stride as usize;
            destination[target_start..target_start + row_size as usize]
                .copy_from_slice(&write.data()[source_start..source_start + row_size as usize]);
        }
    }
    staging.unmap();
    encoder.copy_buffer_to_texture(
        raw::TexelCopyBufferInfo {
            buffer: &staging,
            layout: raw::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(area.height()),
            },
        },
        raw::TexelCopyTextureInfo {
            texture: &texture.texture,
            mip_level: write.mip_level(),
            origin: raw::Origin3d {
                x: area.x(),
                y: area.y(),
                z: write.array_layer(),
            },
            aspect: raw::TextureAspect::All,
        },
        raw::Extent3d {
            width: area.width(),
            height: area.height(),
            depth_or_array_layers: 1,
        },
    );
    Ok(())
}

fn create_gpu_texture(
    device: &raw::Device,
    width: u32,
    height: u32,
    format: TextureFormat,
    mip_level_count: u32,
    array_layer_count: u32,
    usage: raw::TextureUsages,
) -> Result<Arc<GpuTexture>> {
    let max_dimension = device.limits().max_texture_dimension_2d;
    if width > max_dimension
        || height > max_dimension
        || array_layer_count > device.limits().max_texture_array_layers
    {
        return Err(Error::Unsupported(UnsupportedFeature::ResourceSize));
    }
    let raw_format =
        raw_format(format).ok_or(Error::Unsupported(UnsupportedFeature::SurfaceFormat))?;
    let texture = device.create_texture(&raw::TextureDescriptor {
        label: Some("sgfx wgpu texture"),
        size: raw::Extent3d {
            width,
            height,
            depth_or_array_layers: array_layer_count,
        },
        mip_level_count,
        sample_count: 1,
        dimension: raw::TextureDimension::D2,
        format: raw_format,
        usage,
        view_formats: match format {
            TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb => &[
                raw::TextureFormat::Rgba8Unorm,
                raw::TextureFormat::Rgba8UnormSrgb,
            ],
            TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb => &[
                raw::TextureFormat::Bgra8Unorm,
                raw::TextureFormat::Bgra8UnormSrgb,
            ],
            _ => &[],
        },
    });
    let view = texture.create_view(&raw::TextureViewDescriptor {
        mip_level_count: Some(1),
        dimension: Some(raw::TextureViewDimension::D2),
        array_layer_count: Some(1),
        ..Default::default()
    });
    let sampled_view = texture.create_view(&raw::TextureViewDescriptor::default());
    Ok(Arc::new(GpuTexture {
        texture,
        view,
        sampled_view,
        format: raw_format,
        logical_format: format,
        width,
        height,
    }))
}

fn texture_usage(descriptor: TextureDesc) -> Result<raw::TextureUsages> {
    let mut usage = raw::TextureUsages::empty();
    if descriptor.usage().contains(TextureUsage::STORAGE) {
        usage |= raw::TextureUsages::STORAGE_BINDING;
    }
    if descriptor.usage().contains(TextureUsage::SAMPLED) {
        usage |= raw::TextureUsages::TEXTURE_BINDING;
    }
    if descriptor.usage().contains(TextureUsage::RENDER_ATTACHMENT) {
        usage |= raw::TextureUsages::RENDER_ATTACHMENT;
    }
    if descriptor.usage().contains(TextureUsage::COPY_SRC) {
        usage |= raw::TextureUsages::COPY_SRC;
        if matches!(
            descriptor.format(),
            TextureFormat::Rgba8Unorm
                | TextureFormat::Bgra8Unorm
                | TextureFormat::R8Unorm
                | TextureFormat::Rg8Unorm
        ) {
            usage |= raw::TextureUsages::TEXTURE_BINDING;
        }
    }
    if descriptor.usage().contains(TextureUsage::COPY_DST) {
        usage |= raw::TextureUsages::COPY_DST;
        if matches!(
            descriptor.format(),
            TextureFormat::Rgba8Unorm
                | TextureFormat::Bgra8Unorm
                | TextureFormat::R8Unorm
                | TextureFormat::Rg8Unorm
        ) {
            usage |= raw::TextureUsages::RENDER_ATTACHMENT;
        }
    }
    if usage.is_empty() {
        Err(Error::InvalidState)
    } else {
        Ok(usage)
    }
}

fn raw_format(format: TextureFormat) -> Option<raw::TextureFormat> {
    match format {
        TextureFormat::Bgra8Unorm => Some(raw::TextureFormat::Bgra8Unorm),
        TextureFormat::Bgra8UnormSrgb => Some(raw::TextureFormat::Bgra8UnormSrgb),
        TextureFormat::Rgba8Unorm => Some(raw::TextureFormat::Rgba8Unorm),
        TextureFormat::Rgba8UnormSrgb => Some(raw::TextureFormat::Rgba8UnormSrgb),
        TextureFormat::R8Unorm => Some(raw::TextureFormat::R8Unorm),
        TextureFormat::Rg8Unorm => Some(raw::TextureFormat::Rg8Unorm),
        TextureFormat::Nv12 => None,
        TextureFormat::Depth32Float => Some(raw::TextureFormat::Depth32Float),
        TextureFormat::Rgba16Float => Some(raw::TextureFormat::Rgba16Float),
        TextureFormat::Rgba32Float => Some(raw::TextureFormat::Rgba32Float),
    }
}

fn select_surface_format(formats: &[raw::TextureFormat]) -> Option<raw::TextureFormat> {
    formats
        .iter()
        .copied()
        .find(|format| {
            matches!(
                format,
                raw::TextureFormat::Bgra8Unorm | raw::TextureFormat::Rgba8Unorm
            )
        })
        .or_else(|| formats.iter().copied().find(|format| !format.is_srgb()))
        .or_else(|| formats.first().copied())
}

fn select_surface_alpha_mode(
    modes: &[raw::CompositeAlphaMode],
    transparent: bool,
) -> Option<raw::CompositeAlphaMode> {
    if transparent {
        modes
            .iter()
            .copied()
            .find(|mode| *mode == raw::CompositeAlphaMode::PostMultiplied)
            .or_else(|| {
                modes
                    .iter()
                    .copied()
                    .find(|mode| *mode == raw::CompositeAlphaMode::PreMultiplied)
            })
    } else {
        modes
            .iter()
            .copied()
            .find(|mode| *mode == raw::CompositeAlphaMode::Opaque)
            .or_else(|| modes.first().copied())
    }
}

fn create_blit_pipeline(
    device: &raw::Device,
    format: raw::TextureFormat,
    premultiply_alpha: bool,
) -> raw::RenderPipeline {
    let shader_source = if premultiply_alpha {
        BLIT_PREMULTIPLIED_SHADER
    } else {
        BLIT_SHADER
    };
    let shader = device.create_shader_module(raw::ShaderModuleDescriptor {
        label: Some("sgfx WGPU presentation shader"),
        source: raw::ShaderSource::Wgsl(shader_source.into()),
    });
    device.create_render_pipeline(&raw::RenderPipelineDescriptor {
        label: Some("sgfx WGPU presentation pipeline"),
        layout: None,
        vertex: raw::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(raw::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(raw::ColorTargetState {
                format,
                blend: Some(raw::BlendState::REPLACE),
                write_mask: raw::ColorWrites::ALL,
            })],
        }),
        primitive: raw::PrimitiveState::default(),
        depth_stencil: None,
        multisample: raw::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

const BLIT_SHADER: &str = r#"
struct VertexOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VertexOut {
    var positions = array<vec2f, 3>(
        vec2f(-1.0, -1.0),
        vec2f( 3.0, -1.0),
        vec2f(-1.0,  3.0),
    );
    var uvs = array<vec2f, 3>(
        vec2f(0.0, 1.0),
        vec2f(2.0, 1.0),
        vec2f(0.0, -1.0),
    );
    var out: VertexOut;
    out.position = vec4f(positions[vid], 0.0, 1.0);
    out.uv = uvs[vid];
    return out;
}

@group(0) @binding(0) var t_frame: texture_2d<f32>;
@group(0) @binding(1) var s_frame: sampler;

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4f {
    return textureSample(t_frame, s_frame, in.uv);
}
"#;

const BLIT_PREMULTIPLIED_SHADER: &str = r#"
struct VertexOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VertexOut {
    var positions = array<vec2f, 3>(
        vec2f(-1.0, -1.0),
        vec2f( 3.0, -1.0),
        vec2f(-1.0,  3.0),
    );
    var uvs = array<vec2f, 3>(
        vec2f(0.0, 1.0),
        vec2f(2.0, 1.0),
        vec2f(0.0, -1.0),
    );
    var out: VertexOut;
    out.position = vec4f(positions[vid], 0.0, 1.0);
    out.uv = uvs[vid];
    return out;
}

@group(0) @binding(0) var t_frame: texture_2d<f32>;
@group(0) @binding(1) var s_frame: sampler;

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4f {
    let color = textureSample(t_frame, s_frame, in.uv);
    return vec4f(color.rgb * color.a, color.a);
}
"#;

#[cfg(test)]
fn logical_format(format: raw::TextureFormat) -> Option<TextureFormat> {
    match format {
        raw::TextureFormat::Bgra8Unorm | raw::TextureFormat::Bgra8UnormSrgb => {
            Some(TextureFormat::Bgra8Unorm)
        }
        raw::TextureFormat::Rgba8Unorm | raw::TextureFormat::Rgba8UnormSrgb => {
            Some(TextureFormat::Rgba8Unorm)
        }
        _ => None,
    }
}

fn filter_mode(filter: FilterMode) -> raw::FilterMode {
    match filter {
        FilterMode::Nearest => raw::FilterMode::Nearest,
        FilterMode::Linear => raw::FilterMode::Linear,
    }
}

fn address_mode(address: AddressMode) -> raw::AddressMode {
    match address {
        AddressMode::ClampToEdge => raw::AddressMode::ClampToEdge,
        AddressMode::Repeat => raw::AddressMode::Repeat,
        AddressMode::MirrorRepeat => raw::AddressMode::MirrorRepeat,
        AddressMode::ClampToBorder => raw::AddressMode::ClampToBorder,
    }
}

fn index_format(format: IndexFormat) -> raw::IndexFormat {
    match format {
        IndexFormat::Uint16 => raw::IndexFormat::Uint16,
        IndexFormat::Uint32 => raw::IndexFormat::Uint32,
    }
}

fn color_load_op(load: LoadOp) -> raw::LoadOp<raw::Color> {
    match load {
        LoadOp::Load => raw::LoadOp::Load,
        LoadOp::Clear(color) => {
            let [r, g, b, a] = color.components();
            raw::LoadOp::Clear(raw::Color {
                r: f64::from(r),
                g: f64::from(g),
                b: f64::from(b),
                a: f64::from(a),
            })
        }
        LoadOp::DontCare => raw::LoadOp::Clear(raw::Color::TRANSPARENT),
    }
}

fn depth_load_op(load: DepthLoadOp) -> raw::LoadOp<f32> {
    match load {
        DepthLoadOp::Load => raw::LoadOp::Load,
        DepthLoadOp::Clear(value) => raw::LoadOp::Clear(value),
        DepthLoadOp::DontCare => raw::LoadOp::Clear(1.0),
    }
}

fn store_op(store: StoreOp) -> raw::StoreOp {
    match store {
        StoreOp::Store => raw::StoreOp::Store,
        StoreOp::DontCare => raw::StoreOp::Discard,
    }
}

fn render_area_is_full(area: ir::PixelRect, width: u32, height: u32) -> bool {
    area.x() == 0 && area.y() == 0 && area.width() == width && area.height() == height
}

fn create_clear_pipeline(
    device: &raw::Device,
    target_format: raw::TextureFormat,
    has_depth: bool,
) -> GpuClearPipeline {
    let shader = device.create_shader_module(raw::ShaderModuleDescriptor {
        label: Some("sgfx wgpu partial clear shader"),
        source: raw::ShaderSource::Wgsl(Cow::Borrowed(PARTIAL_CLEAR_SHADER)),
    });
    let bind_group_layout = device.create_bind_group_layout(&raw::BindGroupLayoutDescriptor {
        label: Some("sgfx wgpu partial clear bind group layout"),
        entries: &[raw::BindGroupLayoutEntry {
            binding: 0,
            visibility: raw::ShaderStages::FRAGMENT,
            ty: raw::BindingType::Buffer {
                ty: raw::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let pipeline_layout = device.create_pipeline_layout(&raw::PipelineLayoutDescriptor {
        label: Some("sgfx wgpu partial clear pipeline layout"),
        bind_group_layouts: &[&bind_group_layout],
        push_constant_ranges: &[],
    });
    let depth_stencil = has_depth.then_some(raw::DepthStencilState {
        format: raw::TextureFormat::Depth32Float,
        depth_write_enabled: false,
        depth_compare: raw::CompareFunction::Always,
        stencil: raw::StencilState::default(),
        bias: raw::DepthBiasState::default(),
    });
    let pipeline = device.create_render_pipeline(&raw::RenderPipelineDescriptor {
        label: Some("sgfx wgpu partial clear pipeline"),
        layout: Some(&pipeline_layout),
        vertex: raw::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: raw::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: raw::PrimitiveState::default(),
        depth_stencil,
        multisample: raw::MultisampleState::default(),
        fragment: Some(raw::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: raw::PipelineCompilationOptions::default(),
            targets: &[Some(raw::ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: raw::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });
    GpuClearPipeline {
        pipeline,
        bind_group_layout,
    }
}

const PARTIAL_CLEAR_SHADER: &str = r#"
struct ClearUniforms {
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> clear_uniforms: ClearUniforms;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return clear_uniforms.color;
}
"#;

fn raw_depth_bias(raster: ir::RasterState) -> raw::DepthBiasState {
    let [constant, slope_scale] = raster.depth_bias();
    raw::DepthBiasState {
        constant: constant as i32,
        slope_scale,
        clamp: 0.0,
    }
}
fn validate_depth_bias(raster: ir::RasterState) -> Result<()> {
    let [constant, _] = raster.depth_bias();
    if constant.fract() != 0.0
        || f64::from(constant) < f64::from(i32::MIN)
        || f64::from(constant) > f64::from(i32::MAX)
    {
        return Err(Error::Unsupported(UnsupportedFeature::DepthBias));
    }
    Ok(())
}

fn create_pipeline(
    device: &raw::Device,
    descriptor: &RenderPipelineDesc,
    target_format: raw::TextureFormat,
    sample_format: Option<TextureFormat>,
) -> Result<GpuPipeline> {
    validate_depth_bias(descriptor.raster())?;
    if descriptor.target_format() == TextureFormat::Depth32Float
        || descriptor.topology() != ir::PrimitiveTopology::TriangleList
    {
        return Err(Error::Unsupported(UnsupportedFeature::Pipeline));
    }
    let requires_texture = matches!(
        descriptor.fragment(),
        FragmentProgram::Texture(_) | FragmentProgram::TextureVertexColor(_)
    );
    let sample_mode = match descriptor.fragment() {
        FragmentProgram::Texture(mode) | FragmentProgram::TextureVertexColor(mode) => Some(mode),
        FragmentProgram::Solid | FragmentProgram::VertexColor => None,
    };
    let sample_format = requires_texture.then_some(sample_format).flatten();
    let shader_source = shader_source(descriptor, sample_format)?;
    let shader = device.create_shader_module(raw::ShaderModuleDescriptor {
        label: Some("sgfx wgpu generated shader"),
        source: raw::ShaderSource::Wgsl(Cow::Owned(shader_source)),
    });
    let bind_group_layout = device.create_bind_group_layout(&raw::BindGroupLayoutDescriptor {
        label: Some("sgfx wgpu pipeline bind group layout"),
        entries: &bind_group_entries(requires_texture),
    });
    let pipeline_layout = device.create_pipeline_layout(&raw::PipelineLayoutDescriptor {
        label: Some("sgfx wgpu pipeline layout"),
        bind_group_layouts: &[&bind_group_layout],
        push_constant_ranges: &[],
    });
    let attributes = descriptor
        .vertex_buffer()
        .attributes()
        .iter()
        .map(raw_vertex_attribute)
        .collect::<Vec<_>>();
    let vertex_layout = raw::VertexBufferLayout {
        array_stride: u64::from(descriptor.vertex_buffer().stride()),
        step_mode: raw::VertexStepMode::Vertex,
        attributes: &attributes,
    };
    let color_target = raw::ColorTargetState {
        format: target_format,
        blend: Some(raw::BlendState {
            color: blend_component(descriptor.blend().color()),
            alpha: blend_component(descriptor.blend().alpha()),
        }),
        write_mask: raw::ColorWrites::ALL,
    };
    let depth_stencil = descriptor
        .depth_state()
        .map(|depth| raw::DepthStencilState {
            format: raw::TextureFormat::Depth32Float,
            depth_write_enabled: depth.write_enabled(),
            depth_compare: compare_function(depth.compare()),
            stencil: raw::StencilState::default(),
            bias: raw_depth_bias(descriptor.raster()),
        });
    let pipeline = device.create_render_pipeline(&raw::RenderPipelineDescriptor {
        label: Some("sgfx wgpu render pipeline"),
        layout: Some(&pipeline_layout),
        vertex: raw::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: raw::PipelineCompilationOptions::default(),
            buffers: &[vertex_layout],
        },
        primitive: raw::PrimitiveState {
            topology: raw::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: match descriptor.raster().front_face() {
                ir::FrontFace::Clockwise => raw::FrontFace::Cw,
                ir::FrontFace::CounterClockwise => raw::FrontFace::Ccw,
            },
            cull_mode: match descriptor.raster().cull_mode() {
                ir::CullMode::None => None,
                ir::CullMode::Front => Some(raw::Face::Front),
                ir::CullMode::Back => Some(raw::Face::Back),
            },
            unclipped_depth: false,
            polygon_mode: raw::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil,
        multisample: raw::MultisampleState::default(),
        fragment: Some(raw::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: raw::PipelineCompilationOptions::default(),
            targets: &[Some(color_target)],
        }),
        multiview: None,
        cache: None,
    });
    Ok(GpuPipeline {
        pipeline,
        bind_group_layout,
        requires_texture,
        has_depth: descriptor.depth_state().is_some(),
        sample_mode,
    })
}

fn bind_group_entries(texture: bool) -> Vec<raw::BindGroupLayoutEntry> {
    let mut entries = vec![raw::BindGroupLayoutEntry {
        binding: 0,
        visibility: raw::ShaderStages::VERTEX | raw::ShaderStages::FRAGMENT,
        ty: raw::BindingType::Buffer {
            ty: raw::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }];
    if texture {
        entries.push(raw::BindGroupLayoutEntry {
            binding: 1,
            visibility: raw::ShaderStages::FRAGMENT,
            ty: raw::BindingType::Texture {
                sample_type: raw::TextureSampleType::Float { filterable: true },
                view_dimension: raw::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
        entries.push(raw::BindGroupLayoutEntry {
            binding: 2,
            visibility: raw::ShaderStages::FRAGMENT,
            ty: raw::BindingType::Sampler(raw::SamplerBindingType::Filtering),
            count: None,
        });
    }
    entries
}

fn raw_vertex_attribute(attribute: &VertexAttribute) -> raw::VertexAttribute {
    raw::VertexAttribute {
        format: match attribute.format() {
            VertexFormat::Float32 => raw::VertexFormat::Float32,
            VertexFormat::Float32x2 => raw::VertexFormat::Float32x2,
            VertexFormat::Float32x3 => raw::VertexFormat::Float32x3,
            VertexFormat::Float32x4 => raw::VertexFormat::Float32x4,
            VertexFormat::Unorm8x4 => raw::VertexFormat::Unorm8x4,
            VertexFormat::Sint32 => raw::VertexFormat::Sint32,
            VertexFormat::Uint32 | VertexFormat::Snorm10_10_10_2 => raw::VertexFormat::Uint32,
            VertexFormat::Float16x2 => raw::VertexFormat::Float16x2,
            VertexFormat::Float16x4 => raw::VertexFormat::Float16x4,
            VertexFormat::Sint16x4 => raw::VertexFormat::Sint16x4,
            VertexFormat::Sint16x2 => raw::VertexFormat::Sint16x2,
            VertexFormat::Snorm8x4 => raw::VertexFormat::Snorm8x4,
            VertexFormat::Uint32x2 => raw::VertexFormat::Uint32x2,
            VertexFormat::Uint32x3 => raw::VertexFormat::Uint32x3,
            VertexFormat::Uint32x4 => raw::VertexFormat::Uint32x4,
            VertexFormat::Sint32x2 => raw::VertexFormat::Sint32x2,
            VertexFormat::Sint32x3 => raw::VertexFormat::Sint32x3,
            VertexFormat::Sint32x4 => raw::VertexFormat::Sint32x4,

            VertexFormat::Uint16x2 => raw::VertexFormat::Uint16x2,
        },
        offset: u64::from(attribute.offset()),
        shader_location: attribute.location(),
    }
}

fn blend_component(component: BlendComponent) -> raw::BlendComponent {
    raw::BlendComponent {
        src_factor: if matches!(component.operation(), BlendOp::Min | BlendOp::Max) {
            raw::BlendFactor::One
        } else {
            blend_factor(component.source_factor())
        },
        dst_factor: if matches!(component.operation(), BlendOp::Min | BlendOp::Max) {
            raw::BlendFactor::One
        } else {
            blend_factor(component.destination_factor())
        },
        operation: blend_operation(component.operation()),
    }
}

fn blend_factor(factor: BlendFactor) -> raw::BlendFactor {
    match factor {
        BlendFactor::Zero => raw::BlendFactor::Zero,
        BlendFactor::One => raw::BlendFactor::One,
        BlendFactor::SourceAlpha => raw::BlendFactor::SrcAlpha,
        BlendFactor::OneMinusSourceAlpha => raw::BlendFactor::OneMinusSrcAlpha,
        BlendFactor::DestinationAlpha => raw::BlendFactor::DstAlpha,
        BlendFactor::OneMinusDestinationAlpha => raw::BlendFactor::OneMinusDstAlpha,
        BlendFactor::SourceColor => raw::BlendFactor::Src,
        BlendFactor::OneMinusSourceColor => raw::BlendFactor::OneMinusSrc,
        BlendFactor::DestinationColor => raw::BlendFactor::Dst,
        BlendFactor::OneMinusDestinationColor => raw::BlendFactor::OneMinusDst,
    }
}

fn blend_operation(operation: BlendOp) -> raw::BlendOperation {
    match operation {
        BlendOp::Add => raw::BlendOperation::Add,
        BlendOp::Subtract => raw::BlendOperation::Subtract,
        BlendOp::ReverseSubtract => raw::BlendOperation::ReverseSubtract,
        BlendOp::Min => raw::BlendOperation::Min,
        BlendOp::Max => raw::BlendOperation::Max,
    }
}

fn compare_function(compare: CompareFunction) -> raw::CompareFunction {
    match compare {
        CompareFunction::Never => raw::CompareFunction::Never,
        CompareFunction::Less => raw::CompareFunction::Less,
        CompareFunction::Equal => raw::CompareFunction::Equal,
        CompareFunction::LessEqual => raw::CompareFunction::LessEqual,
        CompareFunction::Greater => raw::CompareFunction::Greater,
        CompareFunction::NotEqual => raw::CompareFunction::NotEqual,
        CompareFunction::GreaterEqual => raw::CompareFunction::GreaterEqual,
        CompareFunction::Always => raw::CompareFunction::Always,
    }
}

fn shader_source(
    descriptor: &RenderPipelineDesc,
    sample_format: Option<TextureFormat>,
) -> Result<String> {
    let attributes = descriptor.vertex_buffer().attributes();
    let position =
        find_attribute(attributes, 0).ok_or(Error::Unsupported(UnsupportedFeature::Pipeline))?;
    let needs_color = matches!(
        descriptor.fragment(),
        FragmentProgram::VertexColor | FragmentProgram::TextureVertexColor(_)
    );
    let needs_texture = matches!(
        descriptor.fragment(),
        FragmentProgram::Texture(_) | FragmentProgram::TextureVertexColor(_)
    );
    let color = needs_color.then(|| find_attribute(attributes, 1));
    let uv_location = if matches!(
        descriptor.fragment(),
        FragmentProgram::TextureVertexColor(_)
    ) {
        2
    } else {
        1
    };
    let uv = needs_texture.then(|| find_attribute(attributes, uv_location));
    if (needs_color && color.flatten().is_none()) || (needs_texture && uv.flatten().is_none()) {
        return Err(Error::Unsupported(UnsupportedFeature::Pipeline));
    }
    let mut source = String::new();
    source.push_str(
        "struct Uniforms { transform: mat4x4<f32>, color: vec4<f32>, };\n\
         @group(0) @binding(0) var<uniform> uniforms: Uniforms;\n",
    );
    if needs_texture {
        source.push_str(
            "@group(0) @binding(1) var sampled_texture: texture_2d<f32>;\n\
             @group(0) @binding(2) var texture_sampler: sampler;\n",
        );
    }
    source.push_str("struct VertexInput {\n");
    for attribute in attributes {
        source.push_str(&format!(
            "  @location({}) attr{}: {},\n",
            attribute.location(),
            attribute.location(),
            wgsl_vertex_type(attribute.format()),
        ));
    }
    source.push_str("};\nstruct VertexOutput {\n  @builtin(position) position: vec4<f32>,\n  @location(0) color: vec4<f32>,\n  @location(1) uv: vec2<f32>,\n};\n");
    source.push_str(
        "@vertex fn vs_main(input: VertexInput) -> VertexOutput {\n  var output: VertexOutput;\n",
    );
    source.push_str(&format!(
        "  output.position = uniforms.transform * {};\n",
        position_expression(position),
    ));
    source.push_str(&format!(
        "  output.color = {};\n",
        color
            .flatten()
            .map(color_expression)
            .unwrap_or_else(|| String::from("vec4<f32>(1.0)")),
    ));
    source.push_str(&format!(
        "  output.uv = {};\n  return output;\n}}\n",
        uv.flatten()
            .map(|attribute| format!("input.attr{}", attribute.location()))
            .unwrap_or_else(|| String::from("vec2<f32>(0.0)")),
    ));
    source.push_str("@fragment fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {\n");
    match descriptor.fragment() {
        FragmentProgram::Solid => source.push_str("  return uniforms.color;\n"),
        FragmentProgram::VertexColor => source.push_str("  return input.color * uniforms.color;\n"),
        FragmentProgram::Texture(mode) => {
            source.push_str(
                "  let sampled = textureSample(sampled_texture, texture_sampler, input.uv);\n",
            );
            source.push_str(&format!(
                "  return {};\n",
                texture_expression(mode, "sampled", "uniforms.color", sample_format)
            ));
        }
        FragmentProgram::TextureVertexColor(mode) => {
            source.push_str(
                "  let sampled = textureSample(sampled_texture, texture_sampler, input.uv);\n",
            );
            source.push_str(&format!(
                "  return {};\n",
                texture_vertex_color_expression(
                    mode,
                    "sampled",
                    "input.color",
                    "uniforms.color",
                    sample_format,
                ),
            ));
        }
    }
    source.push_str("}\n");
    Ok(source)
}

fn find_attribute(attributes: &[VertexAttribute], location: u32) -> Option<VertexAttribute> {
    attributes
        .iter()
        .copied()
        .find(|attribute| attribute.location() == location)
}

fn wgsl_vertex_type(format: VertexFormat) -> &'static str {
    match format {
        VertexFormat::Float32 => "f32",
        VertexFormat::Sint32 => "i32",
        VertexFormat::Uint32 | VertexFormat::Snorm10_10_10_2 => "u32",
        VertexFormat::Sint16x4 => "vec4<i32>",
        VertexFormat::Sint16x2 => "vec2<i32>",
        VertexFormat::Uint32x2 => "vec2<u32>",
        VertexFormat::Uint32x3 => "vec3<u32>",
        VertexFormat::Uint32x4 => "vec4<u32>",
        VertexFormat::Sint32x2 => "vec2<i32>",
        VertexFormat::Sint32x3 => "vec3<i32>",
        VertexFormat::Sint32x4 => "vec4<i32>",

        VertexFormat::Uint16x2 => "vec2<u32>",
        VertexFormat::Float16x2 | VertexFormat::Float32x2 => "vec2<f32>",
        VertexFormat::Float32x3 => "vec3<f32>",
        VertexFormat::Float16x4
        | VertexFormat::Float32x4
        | VertexFormat::Unorm8x4
        | VertexFormat::Snorm8x4 => "vec4<f32>",
    }
}

fn position_expression(attribute: VertexAttribute) -> String {
    match attribute.format() {
        VertexFormat::Float32x2 => {
            format!("vec4<f32>(input.attr{}, 0.0, 1.0)", attribute.location())
        }
        VertexFormat::Float32x3 => format!("vec4<f32>(input.attr{}, 1.0)", attribute.location()),
        VertexFormat::Float32x4 | VertexFormat::Unorm8x4 => {
            format!("input.attr{}", attribute.location())
        }
        _ => unreachable!("fixed pipeline validates float vertex inputs"),
    }
}

fn color_expression(attribute: VertexAttribute) -> String {
    match attribute.format() {
        VertexFormat::Float32x3 => format!("vec4<f32>(input.attr{}, 1.0)", attribute.location()),
        VertexFormat::Float32x2 => {
            format!("vec4<f32>(input.attr{}, 0.0, 1.0)", attribute.location())
        }
        VertexFormat::Float32x4 | VertexFormat::Unorm8x4 => {
            format!("input.attr{}", attribute.location())
        }
        _ => unreachable!("fixed pipeline validates float vertex inputs"),
    }
}

fn texture_expression(
    mode: TextureSampleMode,
    sample: &str,
    color: &str,
    sample_format: Option<TextureFormat>,
) -> String {
    match mode {
        TextureSampleMode::Rgba => format!("{} * {}", sample, color),
        TextureSampleMode::RgbIgnoreAlpha => format!("vec4<f32>({}.rgb, 1.0) * {}", sample, color),
        TextureSampleMode::AlphaMask => {
            format!(
                "vec4<f32>({}.rgb, {} * {}.a)",
                color,
                sampled_alpha(sample, sample_format),
                color
            )
        }
    }
}

fn texture_vertex_color_expression(
    mode: TextureSampleMode,
    sample: &str,
    vertex_color: &str,
    color: &str,
    sample_format: Option<TextureFormat>,
) -> String {
    match mode {
        TextureSampleMode::Rgba => format!("{} * {} * {}", sample, vertex_color, color),
        TextureSampleMode::RgbIgnoreAlpha => {
            format!(
                "vec4<f32>({}.rgb, 1.0) * {} * {}",
                sample, vertex_color, color
            )
        }
        TextureSampleMode::AlphaMask => format!(
            "vec4<f32>({}.rgb * {}.rgb, {} * {}.a * {}.a)",
            color,
            vertex_color,
            sampled_alpha(sample, sample_format),
            vertex_color,
            color
        ),
    }
}

fn sampled_alpha(sample: &str, sample_format: Option<TextureFormat>) -> String {
    match sample_format {
        Some(TextureFormat::R8Unorm) => format!("{}.r", sample),
        _ => format!("{}.a", sample),
    }
}

#[cfg(test)]
mod tests {
    mod completion;
    mod execution;
    mod programmable;
    mod sampler;

    use alloc::rc::Rc;
    use alloc::vec;

    use super::*;
    use sgfx_core::ir::{
        AddressMode, BlendState, BufferDesc, Color, CommandEncoder, DrawUniforms, Extent2D,
        FilterMode, PixelRect, PrimitiveTopology, RasterState, RenderPassDesc, SamplerDesc,
        StoreOp, TextureWrite, Transform, VertexBufferLayout,
    };

    pub(crate) static HEADLESS_WGPU_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn request_headless_adapter(instance: &raw::Instance) -> Option<raw::Adapter> {
        pollster::block_on(instance.request_adapter(&raw::RequestAdapterOptions {
            power_preference: raw::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .or_else(|| {
            pollster::block_on(instance.request_adapter(&raw::RequestAdapterOptions {
                power_preference: raw::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
        })
    }

    #[test]
    fn surface_prefers_display_ready_unorm() {
        assert_eq!(
            select_surface_format(&[
                raw::TextureFormat::Bgra8UnormSrgb,
                raw::TextureFormat::Bgra8Unorm,
            ]),
            Some(raw::TextureFormat::Bgra8Unorm)
        );
    }

    #[test]
    fn surface_uses_srgb_when_it_is_the_only_choice() {
        assert_eq!(
            select_surface_format(&[raw::TextureFormat::Bgra8UnormSrgb]),
            Some(raw::TextureFormat::Bgra8UnormSrgb)
        );
    }

    #[test]
    fn transparent_surface_prefers_straight_alpha_compositing() {
        assert_eq!(
            select_surface_alpha_mode(
                &[
                    raw::CompositeAlphaMode::Opaque,
                    raw::CompositeAlphaMode::PreMultiplied,
                    raw::CompositeAlphaMode::PostMultiplied,
                ],
                true,
            ),
            Some(raw::CompositeAlphaMode::PostMultiplied)
        );
    }

    #[test]
    fn transparent_surface_uses_premultiplied_when_required() {
        assert_eq!(
            select_surface_alpha_mode(
                &[
                    raw::CompositeAlphaMode::Opaque,
                    raw::CompositeAlphaMode::PreMultiplied,
                ],
                true,
            ),
            Some(raw::CompositeAlphaMode::PreMultiplied)
        );
    }

    #[test]
    fn transparent_surface_rejects_opaque_only_compositing() {
        assert_eq!(
            select_surface_alpha_mode(&[raw::CompositeAlphaMode::Opaque], true),
            None
        );
    }

    #[test]
    fn opaque_surface_keeps_opaque_compositing() {
        assert_eq!(
            select_surface_alpha_mode(
                &[
                    raw::CompositeAlphaMode::PostMultiplied,
                    raw::CompositeAlphaMode::Opaque,
                ],
                false,
            ),
            Some(raw::CompositeAlphaMode::Opaque)
        );
    }

    fn pipeline(fragment: FragmentProgram) -> RenderPipelineDesc {
        RenderPipelineDesc::new(
            TextureFormat::Bgra8Unorm,
            PrimitiveTopology::TriangleList,
            VertexBufferLayout::new(
                40,
                vec![
                    VertexAttribute::new(0, VertexFormat::Float32x4, 0),
                    VertexAttribute::new(1, VertexFormat::Float32x4, 16),
                    VertexAttribute::new(2, VertexFormat::Float32x2, 32),
                ],
            )
            .expect("valid vertex layout"),
            fragment,
            BlendState::SOURCE_OVER_STRAIGHT_ALPHA,
            RasterState::new(ir::CullMode::None, ir::FrontFace::CounterClockwise),
        )
        .expect("valid pipeline")
    }

    #[test]
    fn generated_texture_vertex_color_shader_contains_both_bindings() {
        let source = shader_source(
            &pipeline(FragmentProgram::TextureVertexColor(TextureSampleMode::Rgba)),
            None,
        )
        .expect("shader source");
        assert!(source.contains("@binding(1) var sampled_texture"));
        assert!(source.contains("@binding(2) var texture_sampler"));
        assert!(source.contains("input.attr2"));
    }

    #[test]
    fn alpha_mask_shader_reads_the_r8_channel_for_glyph_atlases() {
        let descriptor = pipeline(FragmentProgram::TextureVertexColor(
            TextureSampleMode::AlphaMask,
        ));
        let r8_source = shader_source(&descriptor, Some(TextureFormat::R8Unorm))
            .expect("R8 alpha-mask shader source");
        assert!(r8_source.contains("sampled.r * input.color.a"));

        let rgba_source = shader_source(&descriptor, None).expect("RGBA alpha-mask shader source");
        assert!(rgba_source.contains("sampled.a * input.color.a"));
    }

    #[test]
    fn logical_surface_formats_accept_srgb_color_targets() {
        assert_eq!(
            logical_format(raw::TextureFormat::Bgra8UnormSrgb),
            Some(TextureFormat::Bgra8Unorm)
        );
        assert_eq!(
            logical_format(raw::TextureFormat::Rgba8UnormSrgb),
            Some(TextureFormat::Rgba8Unorm)
        );
        assert_eq!(logical_format(raw::TextureFormat::Depth32Float), None);
    }

    #[test]
    fn only_an_exact_target_sized_area_uses_attachment_clear() {
        let full = PixelRect::new(0, 0, 8, 6).expect("full area");
        let partial = PixelRect::new(1, 1, 6, 4).expect("partial area");
        assert!(render_area_is_full(full, 8, 6));
        assert!(!render_area_is_full(partial, 8, 6));
    }

    #[test]
    fn headless_ccw_front_face_survives_back_culling_and_scissor() {
        let _guard = HEADLESS_WGPU_TEST_LOCK
            .lock()
            .expect("lock headless WGPU tests");
        let instance = raw::Instance::new(&raw::InstanceDescriptor::default());
        let Some(adapter) = request_headless_adapter(&instance) else {
            #[cfg(target_os = "macos")]
            panic!("Metal adapter must be available for the SGFX WGPU backend test");
            #[cfg(not(target_os = "macos"))]
            {
                eprintln!("skipping SGFX WGPU backend test: no adapter available");
                return;
            }
        };
        let (raw_device, raw_queue) =
            pollster::block_on(adapter.request_device(&raw::DeviceDescriptor::default(), None))
                .expect("headless WGPU device");
        let device = Device::new(raw_device, raw_queue);
        let context = device.create_context();
        let image = context
            .create_image(16, 16, TextureFormat::Bgra8Unorm)
            .expect("headless target image");
        let resources = Rc::new(ResourceTable::new());
        let target = resources
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Bgra8Unorm,
                    Extent2D::new(16, 16).expect("target extent"),
                    TextureUsage::RENDER_ATTACHMENT
                        | TextureUsage::COPY_SRC
                        | TextureUsage::PRESENT,
                )
                .expect("target descriptor"),
            )
            .expect("target resource");
        let vertices: [[f32; 8]; 6] = [
            [-0.9, -0.8, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
            [-0.1, -0.8, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
            [-0.5, 0.8, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
            [0.1, -0.8, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
            [0.5, 0.8, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
            [0.9, -0.8, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
        ];
        let vertex_buffer = resources
            .define_buffer(
                BufferDesc::new(
                    core::mem::size_of_val(&vertices) as u64,
                    BufferUsage::VERTEX | BufferUsage::COPY_DST,
                )
                .expect("vertex descriptor"),
            )
            .expect("vertex resource");
        let pipeline = resources
            .define_render_pipeline(
                RenderPipelineDesc::new(
                    TextureFormat::Bgra8Unorm,
                    PrimitiveTopology::TriangleList,
                    VertexBufferLayout::new(
                        32,
                        vec![
                            VertexAttribute::new(0, VertexFormat::Float32x4, 0),
                            VertexAttribute::new(1, VertexFormat::Float32x4, 16),
                        ],
                    )
                    .expect("vertex layout"),
                    FragmentProgram::VertexColor,
                    BlendState::SOURCE_OVER_STRAIGHT_ALPHA,
                    RasterState::new(ir::CullMode::Back, ir::FrontFace::CounterClockwise),
                )
                .expect("pipeline descriptor"),
            )
            .expect("pipeline resource");
        let white = Color::rgba(1.0, 1.0, 1.0, 1.0).expect("clear color");
        let mut encoder = CommandEncoder::new(resources.as_ref());
        encoder
            .write_buffer(vertex_buffer, 0, bytemuck::cast_slice(&vertices))
            .expect("vertex upload");
        let pass = RenderPassDesc::new(
            resources.as_ref(),
            target,
            PixelRect::new(0, 0, 16, 16).expect("render area"),
            LoadOp::Clear(white),
            StoreOp::Store,
        )
        .expect("render pass");
        let mut pass = encoder.begin_render_pass(pass).expect("begin pass");
        pass.set_pipeline(pipeline).expect("set pipeline");
        pass.set_vertex_buffer(vertex_buffer, 0)
            .expect("set vertex buffer");
        pass.set_uniforms(DrawUniforms::new(Transform::identity(), white))
            .expect("set uniforms");
        pass.set_scissor(Some(PixelRect::new(0, 0, 16, 8).expect("top-half scissor")))
            .expect("set scissor");
        pass.draw(6, 0).expect("draw clipped rectangle");
        pass.end().expect("end pass");
        let commands = encoder.finish().expect("finish commands");
        let mut cache = context.create_resources(Rc::clone(&resources));
        cache
            .map_image(target.id(), Arc::clone(&image))
            .expect("map target image");
        context
            .create_queue()
            .submit(&mut cache, &commands)
            .expect("submit clipped frame");

        let bytes_per_row = 256u32;
        let readback = device.raw_device().create_buffer(&raw::BufferDescriptor {
            label: Some("sgfx scissor readback"),
            size: u64::from(bytes_per_row) * 16,
            usage: raw::BufferUsages::COPY_DST | raw::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut raw_encoder =
            device
                .raw_device()
                .create_command_encoder(&raw::CommandEncoderDescriptor {
                    label: Some("sgfx scissor readback encoder"),
                });
        raw_encoder.copy_texture_to_buffer(
            raw::TexelCopyTextureInfo {
                texture: image.raw_texture(),
                mip_level: 0,
                origin: raw::Origin3d::ZERO,
                aspect: raw::TextureAspect::All,
            },
            raw::TexelCopyBufferInfo {
                buffer: &readback,
                layout: raw::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(16),
                },
            },
            raw::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
        );
        device
            .raw_queue()
            .submit(core::iter::once(raw_encoder.finish()));
        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(raw::MapMode::Read, move |result| {
            sender.send(result).expect("send map result");
        });
        let _ = device.raw_device().poll(raw::Maintain::Wait);
        receiver
            .recv()
            .expect("receive map result")
            .expect("map readback");
        let bytes = slice.get_mapped_range();
        let pixel = |x: usize, y: usize| {
            let start = y * bytes_per_row as usize + x * 4;
            [
                bytes[start],
                bytes[start + 1],
                bytes[start + 2],
                bytes[start + 3],
            ]
        };
        assert_eq!(pixel(4, 4), [0, 0, 255, 255]);
        assert_eq!(pixel(12, 4), [255, 255, 255, 255]);
        assert_eq!(pixel(4, 12), [255, 255, 255, 255]);
    }

    #[test]
    fn headless_triangle_submission_uses_the_portable_ir() {
        let _guard = HEADLESS_WGPU_TEST_LOCK
            .lock()
            .expect("lock headless WGPU tests");
        let instance = raw::Instance::new(&raw::InstanceDescriptor::default());
        let adapter = request_headless_adapter(&instance);
        let Some(adapter) = adapter else {
            #[cfg(target_os = "macos")]
            panic!("Metal adapter must be available for the SGFX WGPU backend test");
            #[cfg(not(target_os = "macos"))]
            {
                eprintln!("skipping SGFX WGPU backend test: no adapter available");
                return;
            }
        };
        eprintln!("SGFX WGPU test adapter: {:?}", adapter.get_info());
        let (raw_device, raw_queue) =
            pollster::block_on(adapter.request_device(&raw::DeviceDescriptor::default(), None))
                .expect("headless WGPU device");
        let device = Device::new(raw_device, raw_queue);
        let context = device.create_context();
        let image = context
            .create_image(4, 4, TextureFormat::Bgra8Unorm)
            .expect("headless target image");
        let replacement = context
            .create_image(4, 4, TextureFormat::Bgra8Unorm)
            .expect("replacement target image");
        let resources = Rc::new(ResourceTable::new());
        let target = resources
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Bgra8Unorm,
                    Extent2D::new(4, 4).expect("target extent"),
                    TextureUsage::RENDER_ATTACHMENT | TextureUsage::PRESENT,
                )
                .expect("target descriptor"),
            )
            .expect("target resource");
        let vertices: [[f32; 8]; 3] = [
            [-0.5, -0.5, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
            [0.5, -0.5, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
            [0.0, 0.5, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0],
        ];
        let vertex_buffer = resources
            .define_buffer(
                BufferDesc::new(
                    core::mem::size_of_val(&vertices) as u64,
                    BufferUsage::VERTEX | BufferUsage::COPY_DST,
                )
                .expect("vertex descriptor"),
            )
            .expect("vertex resource");
        let layout = VertexBufferLayout::new(
            32,
            vec![
                VertexAttribute::new(0, VertexFormat::Float32x4, 0),
                VertexAttribute::new(1, VertexFormat::Float32x4, 16),
            ],
        )
        .expect("vertex layout");
        let pipeline = resources
            .define_render_pipeline(
                RenderPipelineDesc::new(
                    TextureFormat::Bgra8Unorm,
                    PrimitiveTopology::TriangleList,
                    layout,
                    FragmentProgram::VertexColor,
                    BlendState::SOURCE_OVER_STRAIGHT_ALPHA,
                    RasterState::new(ir::CullMode::None, ir::FrontFace::CounterClockwise),
                )
                .expect("pipeline descriptor"),
            )
            .expect("pipeline resource");
        let glyph_texture = resources
            .define_texture(
                TextureDesc::new(
                    TextureFormat::R8Unorm,
                    Extent2D::new(2, 2).expect("glyph extent"),
                    TextureUsage::SAMPLED | TextureUsage::COPY_DST,
                )
                .expect("glyph texture descriptor"),
            )
            .expect("glyph texture resource");
        let glyph_sampler = resources
            .define_sampler(SamplerDesc::new(
                FilterMode::Nearest,
                FilterMode::Nearest,
                AddressMode::ClampToEdge,
                AddressMode::ClampToEdge,
            ))
            .expect("glyph sampler resource");
        let glyph_vertices: [[f32; 10]; 3] = [
            [-0.5, -0.5, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 1.0],
            [0.5, -0.5, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
            [0.0, 0.5, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.5, 0.0],
        ];
        let glyph_buffer = resources
            .define_buffer(
                BufferDesc::new(
                    core::mem::size_of_val(&glyph_vertices) as u64,
                    BufferUsage::VERTEX | BufferUsage::COPY_DST,
                )
                .expect("glyph vertex descriptor"),
            )
            .expect("glyph vertex resource");
        let glyph_pipeline = resources
            .define_render_pipeline(
                RenderPipelineDesc::new(
                    TextureFormat::Bgra8Unorm,
                    PrimitiveTopology::TriangleList,
                    VertexBufferLayout::new(
                        40,
                        vec![
                            VertexAttribute::new(0, VertexFormat::Float32x4, 0),
                            VertexAttribute::new(1, VertexFormat::Float32x4, 16),
                            VertexAttribute::new(2, VertexFormat::Float32x2, 32),
                        ],
                    )
                    .expect("glyph vertex layout"),
                    FragmentProgram::TextureVertexColor(TextureSampleMode::AlphaMask),
                    BlendState::SOURCE_OVER_STRAIGHT_ALPHA,
                    RasterState::new(ir::CullMode::None, ir::FrontFace::CounterClockwise),
                )
                .expect("glyph pipeline descriptor"),
            )
            .expect("glyph pipeline resource");
        let area = PixelRect::new(0, 0, 4, 4).expect("render area");
        let clear = Color::rgba(0.0, 0.0, 0.0, 1.0).expect("clear color");
        let pass = RenderPassDesc::new(
            resources.as_ref(),
            target,
            area,
            LoadOp::Clear(clear),
            StoreOp::Store,
        )
        .expect("render pass");
        let glyph_pixels = [0_u8, 255, 255, 0];
        let mut encoder = CommandEncoder::new(resources.as_ref());
        encoder
            .write_buffer(vertex_buffer, 0, bytemuck::cast_slice(&vertices))
            .expect("vertex upload");
        encoder
            .write_texture(
                glyph_texture,
                TextureWrite::new(
                    PixelRect::new(0, 0, 2, 2).expect("glyph upload area"),
                    2,
                    &glyph_pixels,
                )
                .expect("glyph upload"),
            )
            .expect("record glyph upload");
        let mut render_pass = encoder.begin_render_pass(pass).expect("begin pass");
        render_pass.set_pipeline(pipeline).expect("set pipeline");
        render_pass
            .set_vertex_buffer(vertex_buffer, 0)
            .expect("set vertex buffer");
        render_pass
            .set_uniforms(DrawUniforms::new(Transform::identity(), clear))
            .expect("set uniforms");
        render_pass.draw(3, 0).expect("draw triangle");
        render_pass.end().expect("end pass");
        let glyph_pass = RenderPassDesc::new(
            resources.as_ref(),
            target,
            area,
            LoadOp::Load,
            StoreOp::Store,
        )
        .expect("glyph render pass");
        let mut glyph_render_pass = encoder
            .begin_render_pass(glyph_pass)
            .expect("begin glyph pass");
        glyph_render_pass
            .set_pipeline(glyph_pipeline)
            .expect("set glyph pipeline");
        glyph_render_pass
            .set_vertex_buffer(glyph_buffer, 0)
            .expect("set glyph vertex buffer");
        glyph_render_pass
            .set_texture(glyph_texture)
            .expect("set glyph texture");
        glyph_render_pass
            .set_sampler(glyph_sampler)
            .expect("set glyph sampler");
        glyph_render_pass
            .set_uniforms(DrawUniforms::new(Transform::identity(), clear))
            .expect("set glyph uniforms");
        glyph_render_pass.draw(3, 0).expect("draw glyph triangle");
        glyph_render_pass.end().expect("end glyph pass");
        let partial_clear = Color::rgba(0.1, 0.2, 0.3, 1.0).expect("partial clear color");
        let partial_pass = RenderPassDesc::new(
            resources.as_ref(),
            target,
            PixelRect::new(1, 1, 2, 2).expect("partial render area"),
            LoadOp::Clear(partial_clear),
            StoreOp::Store,
        )
        .expect("partial render pass");
        let mut partial_render_pass = encoder
            .begin_render_pass(partial_pass)
            .expect("begin partial pass");
        partial_render_pass
            .set_pipeline(pipeline)
            .expect("set partial pipeline");
        partial_render_pass
            .set_vertex_buffer(vertex_buffer, 0)
            .expect("set partial vertex buffer");
        partial_render_pass
            .set_uniforms(DrawUniforms::new(Transform::identity(), clear))
            .expect("set partial uniforms");
        partial_render_pass
            .draw(3, 0)
            .expect("draw partial triangle");
        partial_render_pass.end().expect("end partial pass");
        let commands = encoder.finish().expect("finish commands");
        let mut cache = context.create_resources(Rc::clone(&resources));
        cache
            .map_image(target.id(), image)
            .expect("map target image");
        cache
            .map_image(target.id(), replacement)
            .expect("replace target image");
        device
            .raw_device()
            .push_error_scope(raw::ErrorFilter::Validation);
        context
            .create_queue()
            .submit(&mut cache, &commands)
            .expect("submit IR frame");
        let _ = device.raw_device().poll(raw::Maintain::Wait);
        let error = pollster::block_on(device.raw_device().pop_error_scope());
        assert!(error.is_none(), "WGPU validation error: {error:?}");
    }
}
