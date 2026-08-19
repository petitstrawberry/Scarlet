//! WGPU execution backend for the portable SGFX IR.
//!
//! This module intentionally does not use Scarlet GPU handles, display
//! surfaces, or shared-memory capabilities. A caller supplies a WGPU device
//! and queue, maps a logical presentation texture to an owned WGPU image, and
//! submits the same validated IR command buffers used by the native backend.
//! A persistent resource cache can rebind that logical presentation texture to
//! each newly acquired WGPU surface frame.

use alloc::{borrow::Cow, format, rc::Rc, string::String, sync::Arc, vec::Vec};
use core::fmt;

use bytemuck::{Pod, Zeroable};
use wgpu as raw;

use crate::ir::{
    self, AddressMode, BlendComponent, BlendFactor, BlendOp, BufferId, BufferRef, BufferUsage,
    Command, CommandBuffer, CompareFunction, DepthLoadOp, DrawUniforms, FilterMode,
    FragmentProgram, IndexFormat, LoadOp, RenderPassDesc, RenderPipelineDesc, RenderPipelineId,
    ResourceTable, SamplerId, SamplerRef, StoreOp, TextureDesc, TextureFormat, TextureId,
    TextureRef, TextureSampleMode, TextureUsage, VertexAttribute, VertexFormat,
};

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
    /// Acquiring a WGPU surface frame failed.
    Surface(raw::SurfaceError),
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
            Self::Surface(error) => {
                write!(formatter, "failed to acquire WGPU surface frame: {error}")
            }
        }
    }
}

/// Logical SGFX features that are not yet represented by this backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedFeature {
    /// A write command appeared after a copy or render pass had been encoded.
    LateUpload,
    /// A render pipeline uses a format or vertex convention outside WGPU's
    /// current portable lowering.
    Pipeline,
    /// A sampled texture format is incompatible with the selected fragment
    /// program.
    TextureFormat,
    /// WGPU cannot express a depth clear restricted to a rectangular render
    /// area without an explicit clear draw.
    PartialDepthClear,
    /// The physical surface format cannot be represented by SGFX.
    SurfaceFormat,
}

/// WGPU device and queue pair used by the SGFX backend.
#[derive(Clone)]
pub struct Device {
    device: Arc<raw::Device>,
    queue: Arc<raw::Queue>,
    identity: Arc<()>,
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
    pub fn new(device: raw::Device, queue: raw::Queue) -> Self {
        Self {
            device: Arc::new(device),
            queue: Arc::new(queue),
            identity: Arc::new(()),
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
    /// A WGPU-backed image or an invalid-state error for unsupported formats.
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
                TextureFormat::Bgra8Unorm | TextureFormat::Rgba8Unorm | TextureFormat::R8Unorm
            )
        {
            return Err(Error::InvalidState);
        }
        let gpu = create_gpu_texture(
            self.device.raw_device(),
            width,
            height,
            format,
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

    /// Acquire the current WGPU surface frame as an SGFX image.
    ///
    /// The returned frame must be presented after all commands targeting its
    /// image have been submitted. WGPU surface acquisition and presentation
    /// remain platform responsibilities and are deliberately outside the
    /// portable SGFX IR.
    ///
    /// # Arguments
    ///
    /// * `surface` - Configured WGPU surface to acquire from.
    ///
    /// # Returns
    ///
    /// An acquired surface frame or the WGPU surface error.
    pub fn acquire_surface_frame<'surface>(
        &self,
        surface: &raw::Surface<'surface>,
    ) -> Result<SurfaceFrame> {
        let frame = surface.get_current_texture().map_err(Error::Surface)?;
        let format = logical_format(frame.texture.format())
            .ok_or(Error::Unsupported(UnsupportedFeature::SurfaceFormat))?;
        let gpu = Arc::new(GpuTexture {
            view: frame
                .texture
                .create_view(&raw::TextureViewDescriptor::default()),
            texture: frame.texture.clone(),
            format: frame.texture.format(),
            logical_format: format,
            width: frame.texture.width(),
            height: frame.texture.height(),
        });
        Ok(SurfaceFrame {
            frame,
            image: Arc::new(Image {
                device_identity: Arc::clone(&self.device.identity),
                gpu,
            }),
        })
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

/// Acquired WGPU surface frame and its SGFX image view.
pub struct SurfaceFrame {
    frame: raw::SurfaceTexture,
    image: Arc<Image>,
}

impl SurfaceFrame {
    /// Borrow the acquired image for resource mapping.
    ///
    /// # Returns
    ///
    /// The WGPU-backed image associated with this surface frame.
    pub fn image(&self) -> Arc<Image> {
        Arc::clone(&self.image)
    }

    /// Present the acquired surface frame.
    ///
    /// Consumes the frame so it cannot be presented twice.
    pub fn present(self) {
        self.frame.present();
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
        if !Arc::ptr_eq(&self.context.device.identity, &image.device_identity) {
            return Err(Error::InvalidState);
        }
        let reference = self.resources.texture_ref(texture)?;
        let descriptor = self.resources.texture(reference)?;
        let required = TextureUsage::RENDER_ATTACHMENT | TextureUsage::PRESENT;
        if !descriptor.usage().contains(required)
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
            *mapped = Arc::clone(&image.gpu);
        } else {
            self.mapped_images.push((texture, Arc::clone(&image.gpu)));
        }
        Ok(())
    }

    fn texture(&mut self, reference: TextureRef<'_>) -> Result<Arc<GpuTexture>> {
        let descriptor = self.resources.texture(reference)?;
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
        let mut usage = raw::BufferUsages::empty();
        if descriptor.usage().contains(BufferUsage::VERTEX) {
            usage |= raw::BufferUsages::VERTEX;
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
        let sampler = Arc::new(self.context.device.raw_device().create_sampler(
            &raw::SamplerDescriptor {
                label: Some("sgfx wgpu sampler"),
                address_mode_u: address_mode(descriptor.address_u()),
                address_mode_v: address_mode(descriptor.address_v()),
                address_mode_w: raw::AddressMode::ClampToEdge,
                mag_filter: filter_mode(descriptor.mag_filter()),
                min_filter: filter_mode(descriptor.min_filter()),
                mipmap_filter: raw::FilterMode::Nearest,
                ..raw::SamplerDescriptor::default()
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

impl Queue {
    /// Submit one validated SGFX command buffer.
    ///
    /// Upload commands are expected before the first texture copy or render
    /// pass. The command buffer is translated into one WGPU submission, and
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
    pub fn submit<'r, 'data>(
        &self,
        resources: &mut Resources,
        commands: &CommandBuffer<'r, 'data>,
    ) -> Result<()> {
        if !Arc::ptr_eq(
            &self.context.device.identity,
            &resources.context.device.identity,
        ) {
            return Err(Error::InvalidState);
        }
        if !core::ptr::eq(resources.resources.as_ref(), commands.resources()) {
            return Err(Error::ResourceTableMismatch);
        }
        let mut encoder = self.context.device.raw_device().create_command_encoder(
            &raw::CommandEncoderDescriptor {
                label: Some("sgfx wgpu command encoder"),
            },
        );
        let mut buffer_writes = Vec::new();
        let mut texture_writes = Vec::new();
        let mut upload_phase = true;
        let mut index = 0;
        while index < commands.commands().len() {
            match commands.commands().get(index).ok_or(Error::InvalidState)? {
                Command::WriteBuffer {
                    buffer,
                    offset,
                    data,
                } => {
                    if !upload_phase {
                        return Err(Error::Unsupported(UnsupportedFeature::LateUpload));
                    }
                    let buffer = resources.buffer(*buffer)?;
                    buffer_writes.push((buffer, *offset, *data));
                }
                Command::WriteTexture { texture, write } => {
                    if !upload_phase {
                        return Err(Error::Unsupported(UnsupportedFeature::LateUpload));
                    }
                    let texture_resource = resources.texture(*texture)?;
                    texture_writes.push((
                        texture_resource,
                        write.destination(),
                        write.bytes_per_row(),
                        write.data(),
                    ));
                }
                Command::CopyTextureToTexture {
                    source,
                    source_rect,
                    destination,
                    destination_rect,
                } => {
                    upload_phase = false;
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
                    upload_phase = false;
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
                Command::EndRenderPass => return Err(Error::InvalidState),
                _ => return Err(Error::InvalidState),
            }
            index += 1;
        }
        for (buffer, offset, data) in buffer_writes {
            self.context
                .device
                .raw_queue()
                .write_buffer(&buffer.buffer, offset, data);
        }
        for (texture, destination, bytes_per_row, data) in texture_writes {
            self.context.device.raw_queue().write_texture(
                raw::TexelCopyTextureInfo {
                    texture: &texture.texture,
                    mip_level: 0,
                    origin: raw::Origin3d {
                        x: destination.x(),
                        y: destination.y(),
                        z: 0,
                    },
                    aspect: raw::TextureAspect::All,
                },
                data,
                raw::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(destination.height()),
                },
                raw::Extent3d {
                    width: destination.width(),
                    height: destination.height(),
                    depth_or_array_layers: 1,
                },
            );
        }
        self.context
            .device
            .raw_queue()
            .submit(core::iter::once(encoder.finish()));
        Ok(())
    }

    fn encode_render_pass<'r, 'data>(
        &self,
        resources: &mut Resources,
        encoder: &mut raw::CommandEncoder,
        desc: RenderPassDesc<'r>,
        commands: &[Command<'r, 'data>],
    ) -> Result<()> {
        let target = resources.texture(desc.target())?;
        let depth = desc
            .depth_attachment()
            .map(|attachment| resources.texture(attachment.target()))
            .transpose()?;
        let full_area = render_area_is_full(desc.area(), target.width, target.height);
        let partial_clear_color = if full_area {
            None
        } else {
            match desc.load() {
                LoadOp::Clear(color) => Some(color),
                LoadOp::Load | LoadOp::DontCare => None,
            }
        };
        let color_load = if full_area {
            color_load_op(desc.load())
        } else {
            raw::LoadOp::Load
        };
        let color_attachment = raw::RenderPassColorAttachment {
            view: &target.view,
            resolve_target: None,
            ops: raw::Operations {
                load: color_load,
                store: store_op(desc.store()),
            },
        };
        let color_attachments = [Some(color_attachment)];
        let depth_load = desc.depth_attachment().map(|attachment| attachment.load());
        let depth_store = desc.depth_attachment().map(|attachment| attachment.store());
        let depth_ops = match (depth_load, depth_store) {
            (Some(load), Some(store)) => {
                let load = if full_area {
                    depth_load_op(load)
                } else {
                    match load {
                        DepthLoadOp::Clear(_) => {
                            return Err(Error::Unsupported(UnsupportedFeature::PartialDepthClear));
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
        let mut render_pass = encoder.begin_render_pass(&raw::RenderPassDescriptor {
            label: Some("sgfx wgpu render pass"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: depth_attachment,
            timestamp_writes: None,
            occlusion_query_set: None,
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
        let mut state = PassState {
            pipeline: None,
            pipeline_id: None,
            vertex_buffer: None,
            index_buffer: None,
            texture: None,
            sampler: None,
            uniforms: None,
            has_depth_attachment: depth.is_some(),
        };
        for command in commands {
            match command {
                Command::SetPipeline(pipeline) => {
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
                Command::SetVertexBuffer { buffer, offset } => {
                    state.vertex_buffer = Some((resources.buffer(*buffer)?, *offset));
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
                Command::Draw {
                    vertex_count,
                    first_vertex,
                } => {
                    self.encode_draw(&mut render_pass, &state, *vertex_count, *first_vertex)?;
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
                    )?;
                }
                Command::BeginRenderPass(_)
                | Command::EndRenderPass
                | Command::WriteBuffer { .. }
                | Command::WriteTexture { .. }
                | Command::CopyTextureToTexture { .. } => return Err(Error::InvalidState),
            }
        }
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
    ) -> Result<()> {
        let pipeline = state.pipeline.as_ref().ok_or(Error::InvalidState)?;
        let (vertex_buffer, offset) = state.vertex_buffer.as_ref().ok_or(Error::InvalidState)?;
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
    ) -> Result<()> {
        let pipeline = state.pipeline.as_ref().ok_or(Error::InvalidState)?;
        let (vertex_buffer, vertex_offset) =
            state.vertex_buffer.as_ref().ok_or(Error::InvalidState)?;
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
                resource: raw::BindingResource::TextureView(&texture.view),
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

struct PassState {
    pipeline: Option<Arc<GpuPipeline>>,
    pipeline_id: Option<RenderPipelineId>,
    vertex_buffer: Option<(Arc<GpuBuffer>, u64)>,
    index_buffer: Option<(Arc<GpuBuffer>, u64, IndexFormat)>,
    texture: Option<Arc<GpuTexture>>,
    sampler: Option<Arc<raw::Sampler>>,
    uniforms: Option<DrawUniforms>,
    has_depth_attachment: bool,
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

fn create_gpu_texture(
    device: &raw::Device,
    width: u32,
    height: u32,
    format: TextureFormat,
    usage: raw::TextureUsages,
) -> Result<Arc<GpuTexture>> {
    let raw_format =
        raw_format(format).ok_or(Error::Unsupported(UnsupportedFeature::SurfaceFormat))?;
    let texture = device.create_texture(&raw::TextureDescriptor {
        label: Some("sgfx wgpu texture"),
        size: raw::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: raw::TextureDimension::D2,
        format: raw_format,
        usage,
        view_formats: &[],
    });
    let view = texture.create_view(&raw::TextureViewDescriptor::default());
    Ok(Arc::new(GpuTexture {
        texture,
        view,
        format: raw_format,
        logical_format: format,
        width,
        height,
    }))
}

fn texture_usage(descriptor: TextureDesc) -> Result<raw::TextureUsages> {
    let mut usage = raw::TextureUsages::empty();
    if descriptor.usage().contains(TextureUsage::SAMPLED) {
        usage |= raw::TextureUsages::TEXTURE_BINDING;
    }
    if descriptor.usage().contains(TextureUsage::RENDER_ATTACHMENT) {
        usage |= raw::TextureUsages::RENDER_ATTACHMENT;
    }
    if descriptor.usage().contains(TextureUsage::COPY_SRC) {
        usage |= raw::TextureUsages::COPY_SRC;
    }
    if descriptor.usage().contains(TextureUsage::COPY_DST) {
        usage |= raw::TextureUsages::COPY_DST;
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
        TextureFormat::Rgba8Unorm => Some(raw::TextureFormat::Rgba8Unorm),
        TextureFormat::R8Unorm => Some(raw::TextureFormat::R8Unorm),
        TextureFormat::Depth32Float => Some(raw::TextureFormat::Depth32Float),
    }
}

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

fn create_pipeline(
    device: &raw::Device,
    descriptor: &RenderPipelineDesc,
    target_format: raw::TextureFormat,
    sample_format: Option<TextureFormat>,
) -> Result<GpuPipeline> {
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
            bias: raw::DepthBiasState::default(),
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
            VertexFormat::Float32x2 => raw::VertexFormat::Float32x2,
            VertexFormat::Float32x3 => raw::VertexFormat::Float32x3,
            VertexFormat::Float32x4 => raw::VertexFormat::Float32x4,
            VertexFormat::Unorm8x4 => raw::VertexFormat::Unorm8x4,
        },
        offset: u64::from(attribute.offset()),
        shader_location: attribute.location(),
    }
}

fn blend_component(component: BlendComponent) -> raw::BlendComponent {
    raw::BlendComponent {
        src_factor: blend_factor(component.source_factor()),
        dst_factor: blend_factor(component.destination_factor()),
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
    }
}

fn blend_operation(operation: BlendOp) -> raw::BlendOperation {
    match operation {
        BlendOp::Add => raw::BlendOperation::Add,
        BlendOp::Subtract => raw::BlendOperation::Subtract,
        BlendOp::ReverseSubtract => raw::BlendOperation::ReverseSubtract,
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
        VertexFormat::Float32x2 => "vec2<f32>",
        VertexFormat::Float32x3 => "vec3<f32>",
        VertexFormat::Float32x4 | VertexFormat::Unorm8x4 => "vec4<f32>",
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
    use alloc::rc::Rc;
    use alloc::vec;

    use super::*;
    use crate::ir::{
        AddressMode, BlendState, BufferDesc, Color, CommandEncoder, DrawUniforms, Extent2D,
        FilterMode, PixelRect, PrimitiveTopology, RasterState, RenderPassDesc, SamplerDesc,
        StoreOp, TextureWrite, Transform, VertexBufferLayout,
    };

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
    fn headless_scissor_limits_the_drawn_pixels() {
        let instance = raw::Instance::new(&raw::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&raw::RequestAdapterOptions {
            power_preference: raw::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("headless WGPU adapter");
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
            [-1.0, -1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
            [1.0, -1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
            [1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
            [-1.0, -1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
            [1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
            [-1.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0],
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
                    RasterState::new(ir::CullMode::None, ir::FrontFace::CounterClockwise),
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
        assert_eq!(pixel(8, 4), [0, 0, 255, 255]);
        assert_eq!(pixel(8, 12), [255, 255, 255, 255]);
    }

    #[test]
    fn headless_triangle_submission_uses_the_portable_ir() {
        let instance = raw::Instance::new(&raw::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&raw::RequestAdapterOptions {
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
        });
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
        let (raw_device, raw_queue) = match pollster::block_on(
            adapter.request_device(&raw::DeviceDescriptor::default(), None),
        ) {
            Ok(pair) => pair,
            Err(_) => return,
        };
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
