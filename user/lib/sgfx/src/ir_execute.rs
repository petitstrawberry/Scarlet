//! Ordered lowering of portable logical IR to the active backend.

use alloc::{rc::Rc, vec::Vec};

use crate::driver::{
    self, IrAddressMode, IrBlendComponent, IrBlendFactor, IrBlendOp, IrBlendState, IrCullMode,
    IrDraw, IrFilterMode, IrFragmentProgram, IrFrontFace, IrPipelineState, IrRect, IrSamplerState,
    IrSubmission, IrTextureFormat, IrTextureUpload, IrUniforms, IrVertex, MAX_IR_VERTICES,
};
use crate::ir::{
    self, AddressMode, BlendComponent, BlendFactor, BlendOp, BlendState, BufferRef, BufferUsage,
    Command, CommandBuffer, DrawUniforms, FilterMode, FragmentProgram, IndexFormat, LoadOp,
    MAX_BUFFERS, MAX_TEXTURES, PrimitiveTopology, RenderPipelineRef, ResourceTable, SamplerDesc,
    SamplerRef, TextureDesc, TextureFormat, TextureId, TextureRef, TextureSampleMode, TextureUsage,
    TextureWrite, VertexAttribute, VertexFormat,
};
use crate::{Context, HandleError, Image, Queue};

/// An IR feature that the active backend facade cannot lower faithfully yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedIrFeature {
    /// The command sequence is outside the one-upload-phase, one-pass subset.
    CommandSequence,
    /// More than one buffer upload was recorded.
    MultipleBufferUploads,
    /// More than one render pass was recorded.
    MultipleRenderPasses,
    /// More than one draw was recorded.
    MultipleDraws,
    /// Texture upload is unavailable through an older lowering path.
    TextureUpload,
    /// Texture-to-texture copies are not available through this lowering path.
    TextureCopy,
    /// Index buffers and indexed draws are not available through this lowering path.
    IndexedDrawing,
    /// Texture sampling is unavailable through an older lowering path.
    TextureSampling,
    /// Sampler objects are unavailable through an older lowering path.
    Sampler,
    /// Scissor state is unavailable through an older lowering path.
    Scissor,
    /// The presentation target format is unsupported.
    TargetFormat,
    /// The presentation target usage combination is unsupported.
    TargetUsage,
    /// The render area is unsupported.
    RenderArea,
    /// The attachment load operation is unsupported.
    LoadOperation,
    /// The attachment store operation is unsupported.
    StoreOperation,
    /// The clear color is unsupported.
    ClearColor,
    /// The pipeline target format is unsupported.
    PipelineTargetFormat,
    /// The pipeline primitive topology is unsupported.
    PrimitiveTopology,
    /// The pipeline vertex layout is unsupported.
    VertexLayout,
    /// The pipeline fragment program is unsupported.
    FragmentProgram,
    /// The pipeline blend state is unsupported.
    BlendState,
    /// The logical vertex buffer usage or upload layout is unsupported.
    VertexBuffer,
    /// Draw uniforms are unsupported.
    DrawUniforms,
    /// The backend command stream would exceed its fixed 64 KiB transport limit.
    CommandStream,
}

/// Failure while mapping or submitting a logical IR command buffer.
#[derive(Debug)]
pub enum IrSubmitError {
    /// A logical resource reference or descriptor failed validation.
    InvalidIr(ir::Error),
    /// The command buffer and resource cache use different resource tables.
    ResourceTableMismatch,
    /// A context or queue differs from the context that created the resource cache.
    ContextMismatch,
    /// The logical target extent differs from the physical presentation image.
    TargetExtentMismatch,
    /// The active render-pass texture has no mapped physical image.
    ImageNotMapped,
    /// A logical texture was already mapped to an image.
    TextureAlreadyMapped,
    /// A physical image was already mapped to a different logical texture.
    ImageAlreadyMapped,
    /// A valid IR feature is not implemented by the active lowering path.
    Unsupported(UnsupportedIrFeature),
    /// Uploaded vertex bytes are malformed, non-finite, or outside the supported color range.
    InvalidVertexData,
    /// Allocation for persistent shadow data or decoded backend data failed.
    OutOfMemory,
    /// The active graphics backend rejected resource creation, upload, or submission.
    Backend(HandleError),
}

impl From<ir::Error> for IrSubmitError {
    fn from(error: ir::Error) -> Self {
        Self::InvalidIr(error)
    }
}

impl From<HandleError> for IrSubmitError {
    fn from(error: HandleError) -> Self {
        Self::Backend(error)
    }
}

/// Owning persistent association between one resource table, mapped images,
/// CPU buffer shadows, and private backend materialization.
///
/// The cache retains shared ownership of its table and every mapped image, so
/// it can live in the same renderer object without self-references or leaked
/// allocations. Command buffers remain frame-local and borrow a caller-held
/// clone of the same resource table.
pub struct IrResources {
    resources: Rc<ResourceTable>,
    context_id: i32,
    backend: driver::IrResources,
    images: Vec<ImageMapping>,
    buffer_shadows: Vec<Option<Vec<u8>>>,
}

struct ImageMapping {
    texture: TextureId,
    image: Rc<Image>,
    backend_registered: bool,
}

impl IrResources {
    pub(crate) fn new(
        resources: Rc<ResourceTable>,
        context: &driver::Context,
    ) -> Result<Self, IrSubmitError> {
        let mut images = Vec::new();
        images
            .try_reserve_exact(MAX_TEXTURES)
            .map_err(|_| IrSubmitError::OutOfMemory)?;
        let mut buffer_shadows = Vec::new();
        buffer_shadows
            .try_reserve_exact(MAX_BUFFERS)
            .map_err(|_| IrSubmitError::OutOfMemory)?;
        for _ in 0..MAX_BUFFERS {
            buffer_shadows.push(None);
        }
        Ok(Self {
            resources,
            context_id: context.context_id(),
            backend: context.create_ir_resources()?,
            images,
            buffer_shadows,
        })
    }

    /// Return the resource table that owns this cache's logical references.
    ///
    /// # Returns
    ///
    /// The table retained by the cache when it was created.
    pub fn resources(&self) -> &ResourceTable {
        self.resources.as_ref()
    }

    /// Map a logical presentation texture to a physical render-target image.
    ///
    /// # Arguments
    ///
    /// * `texture` - Persistent identity owned by this cache's resource table.
    /// * `image` - Physical image created by this cache's creating context.
    ///
    /// # Returns
    ///
    /// Success after retaining shared ownership of the image, or an error for a
    /// table/context mismatch, an unsupported target descriptor, extent
    /// mismatch, or a conflicting mapping.
    pub fn map_image(
        &mut self,
        texture: TextureId,
        image: Rc<Image>,
    ) -> Result<(), IrSubmitError> {
        if image.as_ref().backend.context_id() != self.context_id {
            return Err(IrSubmitError::ContextMismatch);
        }
        let texture_ref = self.resources.texture_ref(texture)?;
        let descriptor = self.resources.texture(texture_ref)?;
        if descriptor.format() != TextureFormat::Bgra8Unorm {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::TargetFormat,
            ));
        }
        let required_usage = TextureUsage::RENDER_ATTACHMENT | TextureUsage::PRESENT;
        let allowed_usage = required_usage
            | TextureUsage::SAMPLED
            | TextureUsage::COPY_SRC
            | TextureUsage::COPY_DST;
        if !descriptor.usage().contains(required_usage)
            || !allowed_usage.contains(descriptor.usage())
        {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::TargetUsage,
            ));
        }
        let extent = descriptor.extent();
        if extent.width() != image.as_ref().width
            || extent.height() != image.as_ref().height
        {
            return Err(IrSubmitError::TargetExtentMismatch);
        }
        if self.images.iter().any(|mapping| mapping.texture == texture) {
            return Err(IrSubmitError::TextureAlreadyMapped);
        }
        if self
            .images
            .iter()
            .any(|mapping| Rc::ptr_eq(&mapping.image, &image))
        {
            return Err(IrSubmitError::ImageAlreadyMapped);
        }
        self.images.push(ImageMapping {
            texture,
            image,
            backend_registered: false,
        });
        Ok(())
    }

    fn mapped_image(&self, texture: TextureRef<'_>) -> Result<Rc<Image>, IrSubmitError> {
        if !core::ptr::eq(texture.owner, self.resources.as_ref()) {
            return Err(IrSubmitError::ResourceTableMismatch);
        }
        self.resources.texture(texture)?;
        self.images
            .iter()
            .find(|mapping| mapping.texture == texture.id())
            .map(|mapping| Rc::clone(&mapping.image))
            .ok_or(IrSubmitError::ImageNotMapped)
    }

    fn shadow(&self, buffer: BufferRef<'_>) -> Result<Option<&[u8]>, IrSubmitError> {
        self.resources.buffer(buffer)?;
        Ok(self
            .buffer_shadows
            .get(buffer.index)
            .and_then(Option::as_deref))
    }

    fn commit_buffer_updates(&mut self, updates: Vec<BufferShadowUpdate>) {
        for update in updates {
            if let Some(slot) = self.buffer_shadows.get_mut(update.slot) {
                *slot = Some(update.bytes);
            }
        }
    }
}

struct BufferShadowUpdate {
    slot: usize,
    bytes: Vec<u8>,
}

struct PendingBuffers {
    updates: Vec<BufferShadowUpdate>,
}

enum BufferBytes<'pending, 'resource> {
    Pending(&'pending [u8]),
    Persistent(&'resource [u8]),
}

impl BufferBytes<'_, '_> {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Pending(bytes) => bytes,
            Self::Persistent(bytes) => bytes,
        }
    }
}

impl PendingBuffers {
    fn new() -> Self {
        Self {
            updates: Vec::new(),
        }
    }

    fn write(
        &mut self,
        resources: &IrResources,
        buffer: BufferRef<'_>,
        offset: u64,
        data: &[u8],
    ) -> Result<(), IrSubmitError> {
        let descriptor = resources.resources().buffer(buffer)?;
        if !descriptor.usage().contains(BufferUsage::COPY_DST) {
            return Err(IrSubmitError::InvalidIr(ir::Error::InvalidUsage));
        }
        let length = u64::try_from(data.len()).map_err(|_| IrSubmitError::InvalidVertexData)?;
        let end = offset
            .checked_add(length)
            .ok_or(IrSubmitError::InvalidVertexData)?;
        if end > descriptor.size() {
            return Err(IrSubmitError::InvalidVertexData);
        }
        let start = usize::try_from(offset).map_err(|_| IrSubmitError::InvalidVertexData)?;
        let write_end = start
            .checked_add(data.len())
            .ok_or(IrSubmitError::InvalidVertexData)?;
        let slot = buffer.index;
        let bytes = if let Some(update) = self.updates.iter_mut().find(|update| update.slot == slot)
        {
            &mut update.bytes
        } else {
            let bytes = if let Some(previous) = resources.shadow(buffer)? {
                Vec::from(previous)
            } else {
                Vec::new()
            };
            self.updates
                .try_reserve(1)
                .map_err(|_| IrSubmitError::OutOfMemory)?;
            self.updates.push(BufferShadowUpdate { slot, bytes });
            &mut self
                .updates
                .last_mut()
                .ok_or(IrSubmitError::OutOfMemory)?
                .bytes
        };
        if bytes.len() < write_end {
            bytes
                .try_reserve_exact(write_end - bytes.len())
                .map_err(|_| IrSubmitError::OutOfMemory)?;
            bytes.resize(write_end, 0);
        }
        let destination = bytes
            .get_mut(start..write_end)
            .ok_or(IrSubmitError::InvalidVertexData)?;
        destination.copy_from_slice(data);
        Ok(())
    }

    fn bytes<'pending, 'resource, 'r>(
        &'pending self,
        resources: &'resource IrResources,
        buffer: BufferRef<'r>,
    ) -> Result<BufferBytes<'pending, 'resource>, IrSubmitError> {
        if let Some(update) = self
            .updates
            .iter()
            .find(|update| update.slot == buffer.index)
        {
            return Ok(BufferBytes::Pending(&update.bytes));
        }
        resources
            .shadow(buffer)?
            .map(BufferBytes::Persistent)
            .ok_or(IrSubmitError::InvalidVertexData)
    }
}

struct ExecutionPlan {
    events: Vec<ExecutionEvent>,
    buffer_updates: Vec<BufferShadowUpdate>,
}

enum ExecutionEvent {
    Upload(driver::IrTextureSpec, driver::IrTextureUpload),
    Copy(driver::IrTextureCopy),
    Pass(ExecutionPass),
}

struct ExecutionPass {
    target: ExecutionTarget,
    submission: IrSubmission,
}

enum ExecutionTarget {
    Mapped(Rc<Image>),
    Internal(driver::IrTextureSpec),
}

struct ActivePass<'r> {
    attachment: TextureRef<'r>,
    target: ExecutionTarget,
    submission: IrSubmission,
    pipeline: Option<RenderPipelineRef<'r>>,
    vertex_buffer: Option<(BufferRef<'r>, u64)>,
    index_buffer: Option<(BufferRef<'r>, u64, IndexFormat)>,
    texture: Option<TextureRef<'r>>,
    sampler: Option<SamplerRef<'r>>,
    uniforms: Option<DrawUniforms>,
    scissor: Option<ir::PixelRect>,
}

#[derive(Clone, Copy)]
struct PipelineInfo {
    state: IrPipelineState,
    stride: u32,
    position: VertexAttribute,
    secondary: Option<VertexAttribute>,
}

impl Queue {
    /// Validate, lower, and synchronously submit one logical IR command buffer.
    ///
    /// The supported subset contains any number of `WriteBuffer` and
    /// `WriteTexture` commands before one or more mapped presentation render
    /// passes. Each pass may contain ordered indexed and non-indexed triangle
    /// draws. Texture copies and internally allocated offscreen targets remain
    /// unsupported.
    ///
    /// # Arguments
    ///
    /// * `context` - Context that created `resources` and this queue.
    /// * `resources` - Persistent logical-resource cache created by `context`.
    /// * `commands` - Finished logical command buffer to execute.
    ///
    /// # Returns
    ///
    /// Success after synchronous texture upload and ordered backend queue
    /// submissions, or a validation, unsupported-feature, allocation, or
    /// backend error. The entire command stream, including every resolved index,
    /// is validated before the first backend submission.
    pub fn submit_ir<'r, 'data>(
        &self,
        context: &Context,
        resources: &mut IrResources,
        commands: &CommandBuffer<'r, 'data>,
    ) -> Result<(), IrSubmitError> {
        if !core::ptr::eq(resources.resources(), commands.resources()) {
            return Err(IrSubmitError::ResourceTableMismatch);
        }
        if resources.context_id != context.backend.context_id()
            || self.backend.context_id() != resources.context_id
        {
            return Err(IrSubmitError::ContextMismatch);
        }
        let plan = ExecutionPlan::from_commands(resources, commands)?;
        register_mapped_images(context, resources)?;
        for event in &plan.events {
            match event {
                ExecutionEvent::Upload(texture, upload) => self.backend.upload_ir_texture(
                    &context.backend,
                    &mut resources.backend,
                    *texture,
                    upload,
                ),
                ExecutionEvent::Copy(copy) => {
                    self.backend
                        .copy_ir_texture(&context.backend, &mut resources.backend, *copy)
                }
                ExecutionEvent::Pass(pass) => match &pass.target {
                    ExecutionTarget::Mapped(image) => self.backend.submit_ir(
                        &context.backend,
                        &mut resources.backend,
                        &image.as_ref().backend,
                        &pass.submission,
                    ),
                    ExecutionTarget::Internal(target) => self.backend.submit_ir_internal(
                        &context.backend,
                        &mut resources.backend,
                        *target,
                        &pass.submission,
                    ),
                },
            }
            .map_err(IrSubmitError::Backend)?;
        }
        resources.commit_buffer_updates(plan.buffer_updates);
        Ok(())
    }
}

fn register_mapped_images(
    context: &Context,
    resources: &mut IrResources,
) -> Result<(), IrSubmitError> {
    let resource_table = Rc::clone(&resources.resources);
    for index in 0..resources.images.len() {
        let (texture, image, registered) = {
            let mapping = resources
                .images
                .get(index)
                .ok_or(IrSubmitError::ImageNotMapped)?;
            (
                mapping.texture,
                Rc::clone(&mapping.image),
                mapping.backend_registered,
            )
        };
        if !registered {
            let texture = resource_table.texture_ref(texture)?;
            let descriptor = resource_table.texture(texture)?;
            context.backend.map_ir_image(
                &mut resources.backend,
                texture_spec(texture, descriptor),
                &image.as_ref().backend,
            )?;
            if let Some(mapping) = resources.images.get_mut(index) {
                mapping.backend_registered = true;
            }
        }
    }
    Ok(())
}

impl ExecutionPlan {
    fn from_commands<'r, 'data>(
        resources: &IrResources,
        commands: &CommandBuffer<'r, 'data>,
    ) -> Result<Self, IrSubmitError> {
        let mut pending_buffers = PendingBuffers::new();
        let mut events = Vec::new();
        let mut active = None;
        let mut seen_pass = false;

        for command in commands.commands() {
            match command {
                Command::CopyTextureToTexture {
                    source,
                    source_rect,
                    destination,
                    destination_rect,
                } if active.is_none() => {
                    let source_desc = resources.resources().texture(*source)?;
                    let destination_desc = resources.resources().texture(*destination)?;
                    if !source_desc.usage().contains(TextureUsage::COPY_SRC)
                        || !destination_desc.usage().contains(TextureUsage::COPY_DST)
                        || source_desc.format() != destination_desc.format()
                        || !source_rect.same_extent(*destination_rect)
                        || !source_rect.is_within(source_desc.extent())
                        || !destination_rect.is_within(destination_desc.extent())
                    {
                        return Err(IrSubmitError::InvalidIr(ir::Error::InvalidUsage));
                    }
                    let source_spec = texture_spec(*source, source_desc);
                    let destination_spec = texture_spec(*destination, destination_desc);
                    if !materializable_texture(source_spec)
                        || !materializable_texture(destination_spec)
                    {
                        return Err(IrSubmitError::Unsupported(
                            UnsupportedIrFeature::TargetUsage,
                        ));
                    }
                    events
                        .try_reserve(1)
                        .map_err(|_| IrSubmitError::OutOfMemory)?;
                    events.push(ExecutionEvent::Copy(driver::IrTextureCopy {
                        source: source_spec,
                        source_rect: ir_rect(*source_rect),
                        destination: destination_spec,
                        destination_rect: ir_rect(*destination_rect),
                    }));
                }
                Command::WriteBuffer {
                    buffer,
                    offset,
                    data,
                } if !seen_pass && active.is_none() => {
                    pending_buffers.write(resources, *buffer, *offset, data)?;
                }
                Command::WriteTexture { texture, write } if active.is_none() => {
                    let descriptor = resources.resources().texture(*texture)?;
                    if !descriptor.usage().contains(TextureUsage::COPY_DST) {
                        return Err(IrSubmitError::InvalidIr(ir::Error::InvalidUsage));
                    }
                    let spec = texture_spec(*texture, descriptor);
                    if !materializable_texture(spec) {
                        return Err(IrSubmitError::Unsupported(
                            UnsupportedIrFeature::TargetUsage,
                        ));
                    }
                    if resources
                        .images
                        .iter()
                        .any(|mapping| mapping.texture == texture.id())
                    {
                        return Err(IrSubmitError::Unsupported(
                            UnsupportedIrFeature::TextureUpload,
                        ));
                    }
                    events
                        .try_reserve(1)
                        .map_err(|_| IrSubmitError::OutOfMemory)?;
                    events.push(ExecutionEvent::Upload(
                        spec,
                        convert_texture_upload(spec, *write)?,
                    ));
                }
                Command::BeginRenderPass(desc) if active.is_none() => {
                    let descriptor = resources.resources().texture(desc.target())?;
                    if descriptor.format() != TextureFormat::Bgra8Unorm {
                        return Err(IrSubmitError::Unsupported(
                            UnsupportedIrFeature::TargetFormat,
                        ));
                    }
                    let target = match resources.mapped_image(desc.target()) {
                        Ok(image) => {
                            if !area_within_image(desc.area(), &image) {
                                return Err(IrSubmitError::InvalidIr(ir::Error::OutOfBounds));
                            }
                            ExecutionTarget::Mapped(image)
                        }
                        Err(IrSubmitError::ImageNotMapped) => {
                            let spec = texture_spec(desc.target(), descriptor);
                            if spec.present || !spec.render_attachment {
                                return Err(IrSubmitError::ImageNotMapped);
                            }
                            ExecutionTarget::Internal(spec)
                        }
                        Err(error) => return Err(error),
                    };
                    if !desc.area().is_within(descriptor.extent()) {
                        return Err(IrSubmitError::InvalidIr(ir::Error::OutOfBounds));
                    }
                    active = Some(ActivePass {
                        attachment: desc.target(),
                        target,
                        submission: IrSubmission {
                            clear_color: match desc.load() {
                                LoadOp::Clear(color) => Some(color.components()),
                                LoadOp::Load | LoadOp::DontCare => None,
                            },
                            render_area: IrRect {
                                x: desc.area().x(),
                                y: desc.area().y(),
                                width: desc.area().width(),
                                height: desc.area().height(),
                            },
                            vertices: Vec::new(),
                            draws: Vec::new(),
                            texture_uploads: Vec::new(),
                        },
                        pipeline: None,
                        vertex_buffer: None,
                        index_buffer: None,
                        texture: None,
                        sampler: None,
                        uniforms: None,
                        scissor: None,
                    });
                    seen_pass = true;
                }
                Command::EndRenderPass => {
                    let pass = active.take().ok_or(IrSubmitError::Unsupported(
                        UnsupportedIrFeature::CommandSequence,
                    ))?;
                    if pass.submission.draws.is_empty() {
                        return Err(IrSubmitError::Unsupported(
                            UnsupportedIrFeature::CommandSequence,
                        ));
                    }
                    events
                        .try_reserve(1)
                        .map_err(|_| IrSubmitError::OutOfMemory)?;
                    events.push(ExecutionEvent::Pass(ExecutionPass {
                        target: pass.target,
                        submission: pass.submission,
                    }));
                }
                Command::SetPipeline(reference) => {
                    active_pass_mut(&mut active)?.pipeline = Some(*reference)
                }
                Command::SetVertexBuffer { buffer, offset } => {
                    active_pass_mut(&mut active)?.vertex_buffer = Some((*buffer, *offset));
                }
                Command::SetIndexBuffer {
                    buffer,
                    offset,
                    format,
                } => {
                    active_pass_mut(&mut active)?.index_buffer = Some((*buffer, *offset, *format));
                }
                Command::SetTexture(reference) => {
                    active_pass_mut(&mut active)?.texture = Some(*reference)
                }
                Command::SetSampler(reference) => {
                    active_pass_mut(&mut active)?.sampler = Some(*reference)
                }
                Command::SetUniforms(value) => {
                    active_pass_mut(&mut active)?.uniforms = Some(*value)
                }
                Command::SetScissor(value) => {
                    let pass = active_pass_mut(&mut active)?;
                    if let Some(rectangle) = value {
                        if !rect_within(*rectangle, pass.submission.render_area) {
                            return Err(IrSubmitError::InvalidIr(ir::Error::OutOfBounds));
                        }
                    }
                    pass.scissor = *value;
                }
                Command::Draw {
                    vertex_count,
                    first_vertex,
                } => {
                    let pass = active_pass_mut(&mut active)?;
                    let decoded = decode_draw_vertices(
                        resources,
                        &pending_buffers,
                        pass,
                        *first_vertex,
                        *vertex_count,
                    )?;
                    append_draw(pass, decoded)?;
                }
                Command::DrawIndexed {
                    index_count,
                    first_index,
                    base_vertex,
                } => {
                    let pass = active_pass_mut(&mut active)?;
                    let decoded = decode_indexed_draw_vertices(
                        resources,
                        &pending_buffers,
                        pass,
                        *first_index,
                        *index_count,
                        *base_vertex,
                    )?;
                    append_draw(pass, decoded)?;
                }
                _ => {
                    return Err(IrSubmitError::Unsupported(
                        UnsupportedIrFeature::CommandSequence,
                    ));
                }
            }
        }
        if active.is_some() || events.is_empty() {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::CommandSequence,
            ));
        }
        Ok(Self {
            events,
            buffer_updates: pending_buffers.updates,
        })
    }
}

struct DecodedDraw {
    vertices: Vec<IrVertex>,
    pipeline: IrPipelineState,
    texture: Option<driver::IrTextureSpec>,
    sampler: Option<IrSamplerState>,
    uniforms: IrUniforms,
    scissor: IrRect,
}

fn active_pass_mut<'pass, 'r>(
    active: &'pass mut Option<ActivePass<'r>>,
) -> Result<&'pass mut ActivePass<'r>, IrSubmitError> {
    active.as_mut().ok_or(IrSubmitError::Unsupported(
        UnsupportedIrFeature::CommandSequence,
    ))
}

fn area_within_image(area: ir::PixelRect, image: &Image) -> bool {
    area.x()
        .checked_add(area.width())
        .is_some_and(|right| right <= image.width)
        && area
            .y()
            .checked_add(area.height())
            .is_some_and(|bottom| bottom <= image.height)
}

fn decode_draw_vertices(
    resources: &IrResources,
    pending_buffers: &PendingBuffers,
    pass: &ActivePass<'_>,
    first_vertex: u32,
    vertex_count: u32,
) -> Result<DecodedDraw, IrSubmitError> {
    let (info, buffer, offset, uniforms, texture, sampler) = draw_state(resources, pass)?;
    let bytes = pending_buffers.bytes(resources, buffer)?;
    let vertices = decode_vertices(
        resources.resources(),
        bytes.as_slice(),
        buffer,
        offset,
        first_vertex,
        vertex_count,
        info,
    )?;
    Ok(DecodedDraw {
        vertices,
        pipeline: info.state,
        texture,
        sampler,
        uniforms,
        scissor: pass_scissor(pass),
    })
}

fn decode_indexed_draw_vertices(
    resources: &IrResources,
    pending_buffers: &PendingBuffers,
    pass: &ActivePass<'_>,
    first_index: u32,
    index_count: u32,
    base_vertex: i32,
) -> Result<DecodedDraw, IrSubmitError> {
    if index_count == 0 || index_count % 3 != 0 {
        return Err(IrSubmitError::InvalidVertexData);
    }
    let (info, vertex_buffer, vertex_offset, uniforms, texture, sampler) =
        draw_state(resources, pass)?;
    let (index_buffer, index_offset, index_format) = pass
        .index_buffer
        .ok_or(IrSubmitError::InvalidIr(ir::Error::IndexBufferNotSet))?;
    let index_descriptor = resources.resources().buffer(index_buffer)?;
    if !index_descriptor.usage().contains(BufferUsage::INDEX) {
        return Err(IrSubmitError::InvalidIr(ir::Error::InvalidUsage));
    }
    let index_bytes = pending_buffers.bytes(resources, index_buffer)?;
    let vertex_bytes = pending_buffers.bytes(resources, vertex_buffer)?;
    let index_size =
        usize::try_from(index_format.byte_size()).map_err(|_| IrSubmitError::InvalidVertexData)?;
    let first = usize::try_from(first_index).map_err(|_| IrSubmitError::InvalidVertexData)?;
    let count = usize::try_from(index_count).map_err(|_| IrSubmitError::InvalidVertexData)?;
    let start = usize::try_from(index_offset)
        .ok()
        .and_then(|offset| {
            first
                .checked_mul(index_size)
                .and_then(|value| offset.checked_add(value))
        })
        .ok_or(IrSubmitError::InvalidVertexData)?;
    let end = start
        .checked_add(
            count
                .checked_mul(index_size)
                .ok_or(IrSubmitError::InvalidVertexData)?,
        )
        .ok_or(IrSubmitError::InvalidVertexData)?;
    let encoded = index_bytes
        .as_slice()
        .get(start..end)
        .ok_or(IrSubmitError::InvalidVertexData)?;
    let mut vertices = Vec::new();
    vertices
        .try_reserve_exact(count)
        .map_err(|_| IrSubmitError::OutOfMemory)?;
    for item in encoded.chunks_exact(index_size) {
        let index = match index_format {
            IndexFormat::Uint16 => u32::from(read_u16(item, 0)?),
            IndexFormat::Uint32 => read_u32(item, 0)?,
        };
        let resolved = i64::from(index) + i64::from(base_vertex);
        if resolved < 0 {
            return Err(IrSubmitError::InvalidVertexData);
        }
        let vertex_index =
            usize::try_from(resolved).map_err(|_| IrSubmitError::InvalidVertexData)?;
        vertices.push(decode_vertex(
            resources.resources(),
            vertex_bytes.as_slice(),
            vertex_buffer,
            vertex_offset,
            vertex_index,
            info,
        )?);
    }
    if vertices.len() != count {
        return Err(IrSubmitError::InvalidVertexData);
    }
    Ok(DecodedDraw {
        vertices,
        pipeline: info.state,
        texture,
        sampler,
        uniforms,
        scissor: pass_scissor(pass),
    })
}

fn draw_state<'r>(
    resources: &IrResources,
    pass: &ActivePass<'r>,
) -> Result<
    (
        PipelineInfo,
        BufferRef<'r>,
        u64,
        IrUniforms,
        Option<driver::IrTextureSpec>,
        Option<IrSamplerState>,
    ),
    IrSubmitError,
> {
    let pipeline = pass
        .pipeline
        .ok_or(IrSubmitError::InvalidIr(ir::Error::PipelineNotSet))?;
    let info = pipeline_info(resources.resources(), pipeline)?;
    let (buffer, offset) = pass
        .vertex_buffer
        .ok_or(IrSubmitError::InvalidIr(ir::Error::VertexBufferNotSet))?;
    let uniforms = pass
        .uniforms
        .ok_or(IrSubmitError::InvalidIr(ir::Error::UniformsNotSet))?;
    let (texture, sampler) =
        texture_binding(resources.resources(), info, pass.texture, pass.sampler)?;
    if pass
        .texture
        .is_some_and(|texture| texture == pass.attachment)
    {
        return Err(IrSubmitError::InvalidIr(ir::Error::AttachmentFeedback));
    }
    Ok((
        info,
        buffer,
        offset,
        uniform_state(uniforms),
        texture,
        sampler,
    ))
}

fn pass_scissor(pass: &ActivePass<'_>) -> IrRect {
    match pass.scissor {
        Some(value) => IrRect {
            x: value.x(),
            y: value.y(),
            width: value.width(),
            height: value.height(),
        },
        None => pass.submission.render_area,
    }
}

fn append_draw(pass: &mut ActivePass<'_>, draw: DecodedDraw) -> Result<(), IrSubmitError> {
    let start_vertex = pass.submission.vertices.len();
    let new_len =
        start_vertex
            .checked_add(draw.vertices.len())
            .ok_or(IrSubmitError::Unsupported(
                UnsupportedIrFeature::CommandStream,
            ))?;
    if new_len > MAX_IR_VERTICES {
        return Err(IrSubmitError::Unsupported(
            UnsupportedIrFeature::CommandStream,
        ));
    }
    pass.submission
        .vertices
        .try_reserve(draw.vertices.len())
        .map_err(|_| IrSubmitError::OutOfMemory)?;
    pass.submission.vertices.extend(draw.vertices);
    pass.submission
        .draws
        .try_reserve(1)
        .map_err(|_| IrSubmitError::OutOfMemory)?;
    pass.submission.draws.push(IrDraw {
        start_vertex,
        vertex_count: new_len - start_vertex,
        pipeline: draw.pipeline,
        texture: draw.texture,
        sampler: draw.sampler,
        uniforms: draw.uniforms,
        scissor: draw.scissor,
    });
    Ok(())
}

fn pipeline_info(
    resources: &ResourceTable,
    reference: RenderPipelineRef<'_>,
) -> Result<PipelineInfo, IrSubmitError> {
    let (target, topology, fragment, blend, raster, stride, position, secondary) = resources
        .with_pipeline(reference, |descriptor| {
            let layout = descriptor.vertex_buffer();
            let position = layout
                .attributes()
                .iter()
                .find(|attribute| attribute.location() == 0)
                .copied();
            let secondary = layout
                .attributes()
                .iter()
                .find(|attribute| attribute.location() == 1)
                .copied();
            (
                descriptor.target_format(),
                descriptor.topology(),
                descriptor.fragment(),
                descriptor.blend(),
                descriptor.raster(),
                layout.stride(),
                position,
                secondary,
            )
        })?;
    if target != TextureFormat::Bgra8Unorm {
        return Err(IrSubmitError::Unsupported(
            UnsupportedIrFeature::PipelineTargetFormat,
        ));
    }
    if topology != PrimitiveTopology::TriangleList {
        return Err(IrSubmitError::Unsupported(
            UnsupportedIrFeature::PrimitiveTopology,
        ));
    }
    let position = position.ok_or(IrSubmitError::Unsupported(
        UnsupportedIrFeature::VertexLayout,
    ))?;
    if !matches!(
        position.format(),
        VertexFormat::Float32x2 | VertexFormat::Float32x3 | VertexFormat::Float32x4
    ) {
        return Err(IrSubmitError::Unsupported(
            UnsupportedIrFeature::VertexLayout,
        ));
    }
    let secondary_valid = match fragment {
        FragmentProgram::Solid => true,
        FragmentProgram::VertexColor => matches!(
            secondary.map(VertexAttribute::format),
            Some(VertexFormat::Float32x3 | VertexFormat::Float32x4 | VertexFormat::Unorm8x4)
        ),
        FragmentProgram::Texture(_) => matches!(
            secondary.map(VertexAttribute::format),
            Some(VertexFormat::Float32x2)
        ),
    };
    if !secondary_valid {
        return Err(IrSubmitError::Unsupported(
            UnsupportedIrFeature::VertexLayout,
        ));
    }
    Ok(PipelineInfo {
        state: IrPipelineState {
            slot: reference.index,
            fragment: fragment_program(fragment),
            blend: blend_state(blend),
            cull_mode: match raster.cull_mode() {
                ir::CullMode::None => IrCullMode::None,
                ir::CullMode::Front => IrCullMode::Front,
                ir::CullMode::Back => IrCullMode::Back,
            },
            front_face: match raster.front_face() {
                ir::FrontFace::Clockwise => IrFrontFace::Clockwise,
                ir::FrontFace::CounterClockwise => IrFrontFace::CounterClockwise,
            },
        },
        stride,
        position,
        secondary,
    })
}

fn texture_binding(
    resources: &ResourceTable,
    pipeline: PipelineInfo,
    texture: Option<TextureRef<'_>>,
    sampler: Option<SamplerRef<'_>>,
) -> Result<(Option<driver::IrTextureSpec>, Option<IrSamplerState>), IrSubmitError> {
    if !matches!(
        pipeline.state.fragment,
        IrFragmentProgram::TextureRgba
            | IrFragmentProgram::TextureRgbIgnoreAlpha
            | IrFragmentProgram::TextureAlphaMask
    ) {
        return Ok((None, None));
    }
    let texture = texture.ok_or(IrSubmitError::InvalidIr(ir::Error::TextureBindingNotSet))?;
    let sampler = sampler.ok_or(IrSubmitError::InvalidIr(ir::Error::TextureBindingNotSet))?;
    let descriptor = resources.texture(texture)?;
    if !descriptor.usage().contains(TextureUsage::SAMPLED) {
        return Err(IrSubmitError::InvalidIr(ir::Error::InvalidUsage));
    }
    if descriptor.format() == TextureFormat::R8Unorm
        && !matches!(pipeline.state.fragment, IrFragmentProgram::TextureAlphaMask)
    {
        return Err(IrSubmitError::Unsupported(
            UnsupportedIrFeature::FragmentProgram,
        ));
    }
    let sampler_descriptor = resources.sampler(sampler)?;
    Ok((
        Some(texture_spec(texture, descriptor)),
        Some(sampler_state(sampler_descriptor, sampler.index)),
    ))
}

fn sampler_state(descriptor: SamplerDesc, slot: usize) -> IrSamplerState {
    IrSamplerState {
        slot,
        min_filter: match descriptor.min_filter() {
            FilterMode::Nearest => IrFilterMode::Nearest,
            FilterMode::Linear => IrFilterMode::Linear,
        },
        mag_filter: match descriptor.mag_filter() {
            FilterMode::Nearest => IrFilterMode::Nearest,
            FilterMode::Linear => IrFilterMode::Linear,
        },
        address_u: match descriptor.address_u() {
            AddressMode::ClampToEdge => IrAddressMode::ClampToEdge,
            AddressMode::Repeat => IrAddressMode::Repeat,
            AddressMode::MirrorRepeat => IrAddressMode::MirrorRepeat,
        },
        address_v: match descriptor.address_v() {
            AddressMode::ClampToEdge => IrAddressMode::ClampToEdge,
            AddressMode::Repeat => IrAddressMode::Repeat,
            AddressMode::MirrorRepeat => IrAddressMode::MirrorRepeat,
        },
    }
}

fn decode_vertices(
    resources: &ResourceTable,
    bytes: &[u8],
    buffer: BufferRef<'_>,
    vertex_offset: u64,
    first_vertex: u32,
    vertex_count: u32,
    pipeline: PipelineInfo,
) -> Result<Vec<IrVertex>, IrSubmitError> {
    let count = usize::try_from(vertex_count).map_err(|_| IrSubmitError::InvalidVertexData)?;
    let first = usize::try_from(first_vertex).map_err(|_| IrSubmitError::InvalidVertexData)?;
    if vertex_count == 0 || vertex_count % 3 != 0 {
        return Err(IrSubmitError::InvalidVertexData);
    }
    let mut vertices = Vec::new();
    vertices
        .try_reserve_exact(count)
        .map_err(|_| IrSubmitError::OutOfMemory)?;
    for relative in 0..count {
        let vertex_index = first
            .checked_add(relative)
            .ok_or(IrSubmitError::InvalidVertexData)?;
        vertices.push(decode_vertex(
            resources,
            bytes,
            buffer,
            vertex_offset,
            vertex_index,
            pipeline,
        )?);
    }
    if vertices.len() != count {
        return Err(IrSubmitError::InvalidVertexData);
    }
    Ok(vertices)
}

fn decode_vertex(
    resources: &ResourceTable,
    bytes: &[u8],
    buffer: BufferRef<'_>,
    vertex_offset: u64,
    vertex_index: usize,
    pipeline: PipelineInfo,
) -> Result<IrVertex, IrSubmitError> {
    let descriptor = resources.buffer(buffer)?;
    if !descriptor.usage().contains(BufferUsage::VERTEX) {
        return Err(IrSubmitError::InvalidIr(ir::Error::InvalidUsage));
    }
    let stride = usize::try_from(pipeline.stride).map_err(|_| IrSubmitError::InvalidVertexData)?;
    let start = usize::try_from(vertex_offset)
        .ok()
        .and_then(|offset| {
            vertex_index
                .checked_mul(stride)
                .and_then(|value| offset.checked_add(value))
        })
        .ok_or(IrSubmitError::InvalidVertexData)?;
    let end = start
        .checked_add(stride)
        .ok_or(IrSubmitError::InvalidVertexData)?;
    let record = bytes
        .get(start..end)
        .ok_or(IrSubmitError::InvalidVertexData)?;
    let position = decode_position(record, pipeline.position)?;
    let secondary = decode_secondary(record, pipeline.secondary, pipeline.state.fragment)?;
    Ok(IrVertex {
        position,
        secondary,
    })
}

fn decode_position(record: &[u8], attribute: VertexAttribute) -> Result<[f32; 4], IrSubmitError> {
    let offset =
        usize::try_from(attribute.offset()).map_err(|_| IrSubmitError::InvalidVertexData)?;
    let position = match attribute.format() {
        VertexFormat::Float32x2 => [
            read_f32(record, offset)?,
            read_f32(record, offset + 4)?,
            0.0,
            1.0,
        ],
        VertexFormat::Float32x3 => [
            read_f32(record, offset)?,
            read_f32(record, offset + 4)?,
            read_f32(record, offset + 8)?,
            1.0,
        ],
        VertexFormat::Float32x4 => [
            read_f32(record, offset)?,
            read_f32(record, offset + 4)?,
            read_f32(record, offset + 8)?,
            read_f32(record, offset + 12)?,
        ],
        VertexFormat::Unorm8x4 => return Err(IrSubmitError::InvalidVertexData),
    };
    if !position.iter().all(|value| value.is_finite()) {
        return Err(IrSubmitError::InvalidVertexData);
    }
    Ok(position)
}

fn decode_secondary(
    record: &[u8],
    attribute: Option<VertexAttribute>,
    fragment: IrFragmentProgram,
) -> Result<[f32; 4], IrSubmitError> {
    let Some(attribute) = attribute else {
        return Ok([1.0; 4]);
    };
    let offset =
        usize::try_from(attribute.offset()).map_err(|_| IrSubmitError::InvalidVertexData)?;
    match fragment {
        IrFragmentProgram::Solid => Ok([1.0; 4]),
        IrFragmentProgram::VertexColor => {
            let color = match attribute.format() {
                VertexFormat::Float32x3 => [
                    read_f32(record, offset)?,
                    read_f32(record, offset + 4)?,
                    read_f32(record, offset + 8)?,
                    1.0,
                ],
                VertexFormat::Float32x4 => [
                    read_f32(record, offset)?,
                    read_f32(record, offset + 4)?,
                    read_f32(record, offset + 8)?,
                    read_f32(record, offset + 12)?,
                ],
                VertexFormat::Unorm8x4 => [
                    read_u8(record, offset)? as f32 / 255.0,
                    read_u8(record, offset + 1)? as f32 / 255.0,
                    read_u8(record, offset + 2)? as f32 / 255.0,
                    read_u8(record, offset + 3)? as f32 / 255.0,
                ],
                VertexFormat::Float32x2 => return Err(IrSubmitError::InvalidVertexData),
            };
            if !color
                .iter()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
            {
                return Err(IrSubmitError::InvalidVertexData);
            }
            Ok(color)
        }
        IrFragmentProgram::TextureRgba
        | IrFragmentProgram::TextureRgbIgnoreAlpha
        | IrFragmentProgram::TextureAlphaMask => {
            if attribute.format() != VertexFormat::Float32x2 {
                return Err(IrSubmitError::InvalidVertexData);
            }
            let uv = [read_f32(record, offset)?, read_f32(record, offset + 4)?];
            if !uv.iter().all(|value| value.is_finite()) {
                return Err(IrSubmitError::InvalidVertexData);
            }
            Ok([uv[0], uv[1], 0.0, 1.0])
        }
    }
}

fn read_f32(bytes: &[u8], offset: usize) -> Result<f32, IrSubmitError> {
    let end = offset
        .checked_add(4)
        .ok_or(IrSubmitError::InvalidVertexData)?;
    let array: [u8; 4] = bytes
        .get(offset..end)
        .ok_or(IrSubmitError::InvalidVertexData)?
        .try_into()
        .map_err(|_| IrSubmitError::InvalidVertexData)?;
    Ok(f32::from_le_bytes(array))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, IrSubmitError> {
    let end = offset
        .checked_add(2)
        .ok_or(IrSubmitError::InvalidVertexData)?;
    let array: [u8; 2] = bytes
        .get(offset..end)
        .ok_or(IrSubmitError::InvalidVertexData)?
        .try_into()
        .map_err(|_| IrSubmitError::InvalidVertexData)?;
    Ok(u16::from_le_bytes(array))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, IrSubmitError> {
    let end = offset
        .checked_add(4)
        .ok_or(IrSubmitError::InvalidVertexData)?;
    let array: [u8; 4] = bytes
        .get(offset..end)
        .ok_or(IrSubmitError::InvalidVertexData)?
        .try_into()
        .map_err(|_| IrSubmitError::InvalidVertexData)?;
    Ok(u32::from_le_bytes(array))
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, IrSubmitError> {
    bytes
        .get(offset)
        .copied()
        .ok_or(IrSubmitError::InvalidVertexData)
}

fn convert_texture_upload(
    texture: driver::IrTextureSpec,
    write: TextureWrite<'_>,
) -> Result<IrTextureUpload, IrSubmitError> {
    let destination = write.destination();
    let tight = usize::try_from(destination.width())
        .ok()
        .and_then(|width| {
            width.checked_mul(match texture.format {
                IrTextureFormat::R8 => 1,
                IrTextureFormat::Bgra8 | IrTextureFormat::Rgba8 => 4,
            })
        })
        .ok_or(IrSubmitError::InvalidVertexData)?;
    let source_stride =
        usize::try_from(write.bytes_per_row()).map_err(|_| IrSubmitError::InvalidVertexData)?;
    let height =
        usize::try_from(destination.height()).map_err(|_| IrSubmitError::InvalidVertexData)?;
    let required = source_stride
        .checked_mul(
            height
                .checked_sub(1)
                .ok_or(IrSubmitError::InvalidVertexData)?,
        )
        .and_then(|value| value.checked_add(tight))
        .ok_or(IrSubmitError::InvalidVertexData)?;
    if write.data().len() < required || source_stride < tight {
        return Err(IrSubmitError::InvalidVertexData);
    }
    let output_len = usize::try_from(destination.width())
        .ok()
        .and_then(|width| width.checked_mul(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(IrSubmitError::OutOfMemory)?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(output_len)
        .map_err(|_| IrSubmitError::OutOfMemory)?;
    for row in 0..height {
        let start = row
            .checked_mul(source_stride)
            .ok_or(IrSubmitError::InvalidVertexData)?;
        let source = write
            .data()
            .get(start..start + tight)
            .ok_or(IrSubmitError::InvalidVertexData)?;
        match texture.format {
            IrTextureFormat::Bgra8 => pixels.extend_from_slice(source),
            IrTextureFormat::Rgba8 => {
                for pixel in source.chunks_exact(4) {
                    pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
            }
            IrTextureFormat::R8 => {
                for value in source {
                    pixels.extend_from_slice(&[0, 0, 0, *value]);
                }
            }
        }
    }
    Ok(IrTextureUpload {
        texture,
        destination: IrRect {
            x: destination.x(),
            y: destination.y(),
            width: destination.width(),
            height: destination.height(),
        },
        pixels,
    })
}

fn uniform_state(uniforms: DrawUniforms) -> IrUniforms {
    IrUniforms {
        transform: uniforms.transform().columns(),
        color: uniforms.color().components(),
    }
}

fn fragment_program(fragment: FragmentProgram) -> IrFragmentProgram {
    match fragment {
        FragmentProgram::Solid => IrFragmentProgram::Solid,
        FragmentProgram::VertexColor => IrFragmentProgram::VertexColor,
        FragmentProgram::Texture(TextureSampleMode::Rgba) => IrFragmentProgram::TextureRgba,
        FragmentProgram::Texture(TextureSampleMode::RgbIgnoreAlpha) => {
            IrFragmentProgram::TextureRgbIgnoreAlpha
        }
        FragmentProgram::Texture(TextureSampleMode::AlphaMask) => {
            IrFragmentProgram::TextureAlphaMask
        }
    }
}

fn blend_state(state: BlendState) -> IrBlendState {
    IrBlendState {
        color: blend_component(state.color()),
        alpha: blend_component(state.alpha()),
    }
}

fn blend_component(component: BlendComponent) -> IrBlendComponent {
    IrBlendComponent {
        source_factor: match component.source_factor() {
            BlendFactor::Zero => IrBlendFactor::Zero,
            BlendFactor::One => IrBlendFactor::One,
            BlendFactor::SourceAlpha => IrBlendFactor::SourceAlpha,
            BlendFactor::OneMinusSourceAlpha => IrBlendFactor::OneMinusSourceAlpha,
            BlendFactor::DestinationAlpha => IrBlendFactor::DestinationAlpha,
            BlendFactor::OneMinusDestinationAlpha => IrBlendFactor::OneMinusDestinationAlpha,
        },
        destination_factor: match component.destination_factor() {
            BlendFactor::Zero => IrBlendFactor::Zero,
            BlendFactor::One => IrBlendFactor::One,
            BlendFactor::SourceAlpha => IrBlendFactor::SourceAlpha,
            BlendFactor::OneMinusSourceAlpha => IrBlendFactor::OneMinusSourceAlpha,
            BlendFactor::DestinationAlpha => IrBlendFactor::DestinationAlpha,
            BlendFactor::OneMinusDestinationAlpha => IrBlendFactor::OneMinusDestinationAlpha,
        },
        operation: match component.operation() {
            BlendOp::Add => IrBlendOp::Add,
            BlendOp::Subtract => IrBlendOp::Subtract,
            BlendOp::ReverseSubtract => IrBlendOp::ReverseSubtract,
        },
    }
}

fn rect_within(rectangle: ir::PixelRect, area: IrRect) -> bool {
    rectangle.x() >= area.x
        && rectangle.y() >= area.y
        && rectangle
            .x()
            .checked_add(rectangle.width())
            .is_some_and(|right| right <= area.x + area.width)
        && rectangle
            .y()
            .checked_add(rectangle.height())
            .is_some_and(|bottom| bottom <= area.y + area.height)
}

fn ir_rect(rectangle: ir::PixelRect) -> IrRect {
    IrRect {
        x: rectangle.x(),
        y: rectangle.y(),
        width: rectangle.width(),
        height: rectangle.height(),
    }
}

fn texture_spec(texture: TextureRef<'_>, descriptor: TextureDesc) -> driver::IrTextureSpec {
    let extent = descriptor.extent();
    let usage = descriptor.usage();
    driver::IrTextureSpec {
        slot: texture.index,
        width: extent.width(),
        height: extent.height(),
        sampled: usage.contains(TextureUsage::SAMPLED),
        render_attachment: usage.contains(TextureUsage::RENDER_ATTACHMENT),
        copy_destination: usage.contains(TextureUsage::COPY_DST),
        present: usage.contains(TextureUsage::PRESENT),
        format: match descriptor.format() {
            TextureFormat::Bgra8Unorm => IrTextureFormat::Bgra8,
            TextureFormat::Rgba8Unorm => IrTextureFormat::Rgba8,
            TextureFormat::R8Unorm => IrTextureFormat::R8,
        },
    }
}

fn materializable_texture(spec: driver::IrTextureSpec) -> bool {
    spec.sampled || spec.render_attachment || spec.copy_destination || spec.present
}
