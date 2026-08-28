//! Ordered lowering of portable logical IR to the Scarlet VirGL adapter.

use alloc::{rc::Rc, vec::Vec};

use crate::driver::{
    self, IrAddressMode, IrBlendComponent, IrBlendFactor, IrBlendOp, IrBlendState, IrBufferSpec,
    IrCompareFunction, IrCullMode, IrDepthState, IrDraw, IrFilterMode, IrFragmentProgram,
    IrFrontFace, IrPipelineState, IrRect, IrSamplerState, IrSubmission, IrTextureFormat,
    IrTextureUpload, IrUniforms, IrVertex, IrVertexBufferBinding, MAX_IR_VERTICES,
};
use crate::ir::{
    self, AddressMode, BlendComponent, BlendFactor, BlendOp, BlendState, BufferRef, BufferUsage,
    Command, CommandBuffer, CompareFunction, DepthLoadOp, DrawUniforms, FilterMode,
    FragmentProgram, IndexFormat, LoadOp, MAX_BUFFERS, MAX_TEXTURES, PrimitiveTopology,
    RenderPipelineRef, ResourceTable, SamplerDesc, SamplerRef, TextureDesc, TextureFormat,
    TextureId, TextureRef, TextureSampleMode, TextureUsage, TextureWrite, VertexAttribute,
    VertexFormat,
};
use crate::{Context, HandleError, Image, Queue, Texture};

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
    buffer_revisions: Vec<u64>,
    canonical_buffer_revisions: Vec<Option<u64>>,
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
        let mut buffer_revisions = Vec::new();
        buffer_revisions
            .try_reserve_exact(MAX_BUFFERS)
            .map_err(|_| IrSubmitError::OutOfMemory)?;
        let mut canonical_buffer_revisions = Vec::new();
        canonical_buffer_revisions
            .try_reserve_exact(MAX_BUFFERS)
            .map_err(|_| IrSubmitError::OutOfMemory)?;
        for _ in 0..MAX_BUFFERS {
            buffer_shadows.push(None);
            buffer_revisions.push(0);
            canonical_buffer_revisions.push(None);
        }
        Ok(Self {
            resources,
            context_id: context.context_id(),
            backend: context.create_ir_resources()?,
            images,
            buffer_shadows,
            buffer_revisions,
            canonical_buffer_revisions,
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
    pub fn map_image(&mut self, texture: TextureId, image: Rc<Image>) -> Result<(), IrSubmitError> {
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
        if extent.width() != image.as_ref().width || extent.height() != image.as_ref().height {
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

    pub(crate) fn map_texture(
        &mut self,
        context: &Context,
        texture: TextureId,
        image: &Texture,
    ) -> Result<(), IrSubmitError> {
        let texture_ref = self.resources.texture_ref(texture)?;
        let descriptor = self.resources.texture(texture_ref)?;
        if descriptor.format() != TextureFormat::Bgra8Unorm
            || !descriptor.usage().contains(TextureUsage::SAMPLED)
            || descriptor.usage().contains(TextureUsage::PRESENT)
        {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::TargetUsage,
            ));
        }
        let extent = descriptor.extent();
        if extent.width() != image.width() || extent.height() != image.height() {
            return Err(IrSubmitError::TargetExtentMismatch);
        }
        context.backend.map_ir_texture(
            &mut self.backend,
            texture_spec(texture_ref, descriptor),
            &image.backend,
        )?;
        Ok(())
    }

    pub(crate) fn unmap_texture(
        &mut self,
        context: &Context,
        texture: TextureId,
        image: &Texture,
    ) -> Result<(), IrSubmitError> {
        let texture_ref = self.resources.texture_ref(texture)?;
        let descriptor = self.resources.texture(texture_ref)?;
        context.backend.unmap_ir_texture(
            &mut self.backend,
            texture_spec(texture_ref, descriptor),
            &image.backend,
        )?;
        Ok(())
    }

    fn mapped_image(&self, texture: TextureRef<'_>) -> Result<Rc<Image>, IrSubmitError> {
        if !texture.belongs_to(self.resources.as_ref()) {
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
            .get(buffer.slot())
            .and_then(Option::as_deref))
    }

    fn buffer_revision(&self, slot: usize) -> Result<u64, IrSubmitError> {
        self.buffer_revisions
            .get(slot)
            .copied()
            .ok_or(IrSubmitError::InvalidVertexData)
    }

    fn canonical_buffer_revision(&self, slot: usize) -> Option<u64> {
        self.canonical_buffer_revisions.get(slot).copied().flatten()
    }

    fn commit_buffer_updates(&mut self, updates: Vec<BufferShadowUpdate>) {
        for update in updates {
            if let Some(slot) = self.buffer_shadows.get_mut(update.slot) {
                *slot = Some(update.bytes);
            }
            if let Some(revision) = self.buffer_revisions.get_mut(update.slot) {
                *revision = update.revision;
            }
        }
    }

    fn commit_canonical_validations(&mut self, validations: Vec<(usize, u64)>) {
        for (slot, revision) in validations {
            if let Some(validated) = self.canonical_buffer_revisions.get_mut(slot) {
                *validated = Some(revision);
            }
        }
    }
}

struct BufferShadowUpdate {
    slot: usize,
    revision: u64,
    bytes: Vec<u8>,
}

struct PendingBuffers {
    updates: Vec<BufferShadowUpdate>,
    canonical_validations: Vec<(usize, u64)>,
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
            canonical_validations: Vec::new(),
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
        let slot = buffer.slot();
        let bytes = if let Some(update) = self.updates.iter_mut().find(|update| update.slot == slot)
        {
            &mut update.bytes
        } else {
            let bytes = if let Some(previous) = resources.shadow(buffer)? {
                Vec::from(previous)
            } else {
                Vec::new()
            };
            let revision = resources
                .buffer_revision(slot)?
                .checked_add(1)
                .ok_or(IrSubmitError::OutOfMemory)?;
            self.updates
                .try_reserve(1)
                .map_err(|_| IrSubmitError::OutOfMemory)?;
            self.updates.push(BufferShadowUpdate {
                slot,
                revision,
                bytes,
            });
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
            .find(|update| update.slot == buffer.slot())
        {
            return Ok(BufferBytes::Pending(&update.bytes));
        }
        resources
            .shadow(buffer)?
            .map(BufferBytes::Persistent)
            .ok_or(IrSubmitError::InvalidVertexData)
    }

    fn revision(
        &self,
        resources: &IrResources,
        buffer: BufferRef<'_>,
    ) -> Result<u64, IrSubmitError> {
        self.updates
            .iter()
            .find(|update| update.slot == buffer.slot())
            .map(|update| update.revision)
            .map(Ok)
            .unwrap_or_else(|| resources.buffer_revision(buffer.slot()))
    }

    fn canonical_needs_validation(
        &self,
        resources: &IrResources,
        slot: usize,
        revision: u64,
    ) -> bool {
        resources.canonical_buffer_revision(slot) != Some(revision)
            && !self.canonical_validations.contains(&(slot, revision))
    }

    fn mark_canonical_validated(
        &mut self,
        slot: usize,
        revision: u64,
    ) -> Result<(), IrSubmitError> {
        if !self.canonical_validations.contains(&(slot, revision)) {
            self.canonical_validations
                .try_reserve(1)
                .map_err(|_| IrSubmitError::OutOfMemory)?;
            self.canonical_validations.push((slot, revision));
        }
        Ok(())
    }
}

struct ExecutionPlan {
    events: Vec<ExecutionEvent>,
    buffer_updates: Vec<BufferShadowUpdate>,
    canonical_validations: Vec<(usize, u64)>,
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

#[derive(Clone)]
enum ExecutionTarget {
    Mapped(Rc<Image>),
    Internal(driver::IrTextureSpec),
}

struct ActivePass<'r> {
    attachment: TextureRef<'r>,
    target: ExecutionTarget,
    depth_attachment: Option<TextureRef<'r>>,
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
    tertiary: Option<VertexAttribute>,
}

impl Queue {
    /// Validate, lower, and synchronously submit one logical IR command buffer.
    ///
    /// The supported subset contains buffer and texture uploads, compatible
    /// texture copies, and one or more render passes targeting either mapped
    /// presentation images or backend-materialized offscreen textures. Each
    /// pass may contain ordered indexed and non-indexed triangle draws.
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
        plan.validate_transport_chunks()?;
        register_mapped_images(context, resources)?;
        let mut prepared_buffers = Vec::new();
        for event in &plan.events {
            let ExecutionEvent::Pass(pass) = event else {
                continue;
            };
            for draw in &pass.submission.draws {
                let Some(binding) = draw.vertex_buffer else {
                    continue;
                };
                if prepared_buffers.contains(&binding.buffer.slot) {
                    continue;
                }
                let bytes = plan
                    .buffer_updates
                    .iter()
                    .find(|update| update.slot == binding.buffer.slot)
                    .map(|update| update.bytes.as_slice())
                    .or_else(|| {
                        resources
                            .buffer_shadows
                            .get(binding.buffer.slot)
                            .and_then(Option::as_deref)
                    })
                    .ok_or(IrSubmitError::InvalidVertexData)?;
                self.backend.prepare_ir_buffer(
                    &context.backend,
                    &mut resources.backend,
                    binding.buffer,
                    bytes,
                )?;
                prepared_buffers
                    .try_reserve(1)
                    .map_err(|_| IrSubmitError::OutOfMemory)?;
                prepared_buffers.push(binding.buffer.slot);
            }
        }
        // Buffer uploads are already visible to the backend at this point.
        // Commit their CPU revisions before executing later events so a pass
        // failure cannot leave the backend and shadow cache claiming the same
        // revision for different bytes on a retry. Texture uploads likewise
        // are not rolled back when a later event fails.
        let ExecutionPlan {
            events,
            buffer_updates,
            canonical_validations,
        } = plan;
        resources.commit_buffer_updates(buffer_updates);
        resources.commit_canonical_validations(canonical_validations);
        for event in &events {
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
    fn validate_transport_chunks(&self) -> Result<(), IrSubmitError> {
        for event in &self.events {
            let ExecutionEvent::Pass(pass) = event else {
                continue;
            };
            let submission = &pass.submission;
            if submission.draws.is_empty() && !submission_has_clear(submission)
                || submission.vertices.len() > MAX_IR_VERTICES
                || submission.draws.len() > MAX_IR_DRAWS_PER_SUBMISSION
                || !chunk_fits_transport(submission.vertices.len(), submission.draws.len())
            {
                return Err(IrSubmitError::InvalidVertexData);
            }
            for draw in &submission.draws {
                if draw.vertex_count == 0 || !draw.vertex_count.is_multiple_of(3) {
                    return Err(IrSubmitError::InvalidVertexData);
                }
                if draw.vertex_buffer.is_none()
                    && draw
                        .start_vertex
                        .checked_add(draw.vertex_count)
                        .is_none_or(|end| end > submission.vertices.len())
                {
                    return Err(IrSubmitError::InvalidVertexData);
                }
            }
        }
        Ok(())
    }

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
                    let depth_attachment = desc.depth_attachment();
                    let depth_spec = if let Some(depth) = depth_attachment {
                        let depth_descriptor = resources.resources().texture(depth.target())?;
                        Some(texture_spec(depth.target(), depth_descriptor))
                    } else {
                        None
                    };
                    active = Some(ActivePass {
                        attachment: desc.target(),
                        target,
                        depth_attachment: depth_attachment.map(|depth| depth.target()),
                        submission: IrSubmission {
                            clear_color: match desc.load() {
                                LoadOp::Clear(color) => Some(color.components()),
                                LoadOp::Load | LoadOp::DontCare => None,
                            },
                            depth_attachment: depth_spec,
                            clear_depth: depth_attachment.and_then(|depth| match depth.load() {
                                DepthLoadOp::Clear(value) => Some(value),
                                DepthLoadOp::Load | DepthLoadOp::DontCare => None,
                            }),
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
                    if pass.submission.draws.is_empty() && !submission_has_clear(&pass.submission) {
                        return Err(IrSubmitError::Unsupported(
                            UnsupportedIrFeature::CommandSequence,
                        ));
                    }
                    let chunks = split_pass(ExecutionPass {
                        target: pass.target,
                        submission: pass.submission,
                    })?;
                    events
                        .try_reserve(chunks.len())
                        .map_err(|_| IrSubmitError::OutOfMemory)?;
                    events.extend(chunks.into_iter().map(ExecutionEvent::Pass));
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
                    if let Some(rectangle) = value
                        && !rect_within(*rectangle, pass.submission.render_area)
                    {
                        return Err(IrSubmitError::InvalidIr(ir::Error::OutOfBounds));
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
                        &mut pending_buffers,
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
            canonical_validations: pending_buffers.canonical_validations,
        })
    }
}

/// Return whether a draw-free pass still changes an attachment.
fn submission_has_clear(submission: &IrSubmission) -> bool {
    submission.clear_color.is_some() || submission.clear_depth.is_some()
}

struct DecodedDraw {
    vertices: DecodedVertices,
    pipeline: IrPipelineState,
    texture: Option<driver::IrTextureSpec>,
    sampler: Option<IrSamplerState>,
    uniforms: IrUniforms,
    scissor: IrRect,
}

enum DecodedVertices {
    Inline(Vec<IrVertex>),
    Persistent {
        binding: IrVertexBufferBinding,
        first_vertex: usize,
        vertex_count: usize,
    },
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
    pending_buffers: &mut PendingBuffers,
    pass: &ActivePass<'_>,
    first_vertex: u32,
    vertex_count: u32,
) -> Result<DecodedDraw, IrSubmitError> {
    let (info, buffer, offset, uniforms, texture, sampler) = draw_state(resources, pass)?;
    let descriptor = resources.resources().buffer(buffer)?;
    let persistent_supported = is_canonical_persistent_layout(info)
        && descriptor.size() <= u64::from(u32::MAX)
        && offset <= u64::from(u32::MAX);
    let vertices = if persistent_supported {
        let revision = pending_buffers.revision(resources, buffer)?;
        let needs_validation =
            pending_buffers.canonical_needs_validation(resources, buffer.slot(), revision);
        {
            let bytes = pending_buffers.bytes(resources, buffer)?;
            validate_canonical_vertex_range(
                resources.resources(),
                bytes.as_slice(),
                buffer,
                offset,
                first_vertex,
                vertex_count,
            )?;
            if needs_validation {
                validate_canonical_buffer(bytes.as_slice())?;
            }
        }
        if needs_validation {
            pending_buffers.mark_canonical_validated(buffer.slot(), revision)?;
        }
        let offset = u32::try_from(offset).map_err(|_| IrSubmitError::InvalidVertexData)?;
        DecodedVertices::Persistent {
            binding: IrVertexBufferBinding {
                buffer: IrBufferSpec {
                    slot: buffer.slot(),
                    size: descriptor.size(),
                    revision,
                },
                offset,
            },
            first_vertex: usize::try_from(first_vertex)
                .map_err(|_| IrSubmitError::InvalidVertexData)?,
            vertex_count: usize::try_from(vertex_count)
                .map_err(|_| IrSubmitError::InvalidVertexData)?,
        }
    } else {
        let bytes = pending_buffers.bytes(resources, buffer)?;
        DecodedVertices::Inline(decode_vertices(
            resources.resources(),
            bytes.as_slice(),
            buffer,
            offset,
            first_vertex,
            vertex_count,
            info,
        )?)
    };
    Ok(DecodedDraw {
        vertices,
        pipeline: info.state,
        texture,
        sampler,
        uniforms,
        scissor: pass_scissor(pass),
    })
}

fn is_canonical_persistent_layout(info: PipelineInfo) -> bool {
    info.stride == 40
        && info.position == VertexAttribute::new(0, VertexFormat::Float32x4, 0)
        && info.secondary == Some(VertexAttribute::new(1, VertexFormat::Float32x4, 16))
        && info.tertiary == Some(VertexAttribute::new(2, VertexFormat::Float32x2, 32))
        && matches!(
            info.state.fragment,
            IrFragmentProgram::VertexColor
                | IrFragmentProgram::TextureVertexColorRgba
                | IrFragmentProgram::TextureVertexColorRgbIgnoreAlpha
                | IrFragmentProgram::TextureVertexColorAlphaMask
        )
}

fn validate_canonical_vertex_range(
    resources: &ResourceTable,
    bytes: &[u8],
    buffer: BufferRef<'_>,
    vertex_offset: u64,
    first_vertex: u32,
    vertex_count: u32,
) -> Result<(), IrSubmitError> {
    let descriptor = resources.buffer(buffer)?;
    if !descriptor.usage().contains(BufferUsage::VERTEX)
        || vertex_count == 0
        || !vertex_count.is_multiple_of(3)
    {
        return Err(IrSubmitError::InvalidVertexData);
    }
    let start = usize::try_from(vertex_offset)
        .ok()
        .and_then(|offset| {
            usize::try_from(first_vertex)
                .ok()
                .and_then(|first| first.checked_mul(40))
                .and_then(|first| offset.checked_add(first))
        })
        .ok_or(IrSubmitError::InvalidVertexData)?;
    let byte_count = usize::try_from(vertex_count)
        .ok()
        .and_then(|count| count.checked_mul(40))
        .ok_or(IrSubmitError::InvalidVertexData)?;
    let end = start
        .checked_add(byte_count)
        .ok_or(IrSubmitError::InvalidVertexData)?;
    bytes
        .get(start..end)
        .ok_or(IrSubmitError::InvalidVertexData)?;
    Ok(())
}

fn validate_canonical_buffer(bytes: &[u8]) -> Result<(), IrSubmitError> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(40) {
        return Err(IrSubmitError::InvalidVertexData);
    }
    for record in bytes.chunks_exact(40) {
        for offset in [0, 4, 8, 12, 32, 36] {
            if !read_f32(record, offset)?.is_finite() {
                return Err(IrSubmitError::InvalidVertexData);
            }
        }
        for offset in [16, 20, 24, 28] {
            let component = read_f32(record, offset)?;
            if !component.is_finite() || !(0.0..=1.0).contains(&component) {
                return Err(IrSubmitError::InvalidVertexData);
            }
        }
    }
    Ok(())
}

fn decode_indexed_draw_vertices(
    resources: &IrResources,
    pending_buffers: &PendingBuffers,
    pass: &ActivePass<'_>,
    first_index: u32,
    index_count: u32,
    base_vertex: i32,
) -> Result<DecodedDraw, IrSubmitError> {
    if index_count == 0 || !index_count.is_multiple_of(3) {
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
        vertices: DecodedVertices::Inline(vertices),
        pipeline: info.state,
        texture,
        sampler,
        uniforms,
        scissor: pass_scissor(pass),
    })
}

#[allow(clippy::type_complexity)]
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
    if info.state.depth.is_some() != pass.depth_attachment.is_some() {
        return Err(IrSubmitError::InvalidIr(ir::Error::InvalidDescriptor));
    }
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
    let (start_vertex, vertex_count, vertex_buffer) = match draw.vertices {
        DecodedVertices::Inline(vertices) => {
            let start_vertex = pass.submission.vertices.len();
            let new_len = start_vertex
                .checked_add(vertices.len())
                .ok_or(IrSubmitError::OutOfMemory)?;
            pass.submission
                .vertices
                .try_reserve(vertices.len())
                .map_err(|_| IrSubmitError::OutOfMemory)?;
            pass.submission.vertices.extend(vertices);
            (start_vertex, new_len - start_vertex, None)
        }
        DecodedVertices::Persistent {
            binding,
            first_vertex,
            vertex_count,
        } => (first_vertex, vertex_count, Some(binding)),
    };
    pass.submission
        .draws
        .try_reserve(1)
        .map_err(|_| IrSubmitError::OutOfMemory)?;
    pass.submission.draws.push(IrDraw {
        start_vertex,
        vertex_count,
        vertex_buffer,
        pipeline: draw.pipeline,
        texture: draw.texture,
        sampler: draw.sampler,
        uniforms: draw.uniforms,
        scissor: draw.scissor,
    });
    Ok(())
}

const IR_COMMAND_TRANSPORT_BYTES: usize = 64 * 1024;
const IR_COMMAND_FIXED_BUDGET: usize = 6 * 1024;
const IR_DRAW_COMMAND_BUDGET: usize = 256;
const IR_INLINE_VERTEX_BYTES: usize = 10 * core::mem::size_of::<f32>();

// This draw-count guard complements the byte budget and bounds planner work
// for persistent buffers, whose vertices do not occupy the command stream.
const MAX_IR_DRAWS_PER_SUBMISSION: usize = 64;

fn chunk_fits_transport(vertex_count: usize, draw_count: usize) -> bool {
    vertex_count
        .checked_mul(IR_INLINE_VERTEX_BYTES)
        .and_then(|bytes| bytes.checked_add(IR_COMMAND_FIXED_BUDGET))
        .and_then(|bytes| {
            draw_count
                .checked_mul(IR_DRAW_COMMAND_BUDGET)
                .and_then(|draw_bytes| bytes.checked_add(draw_bytes))
        })
        .is_some_and(|bytes| bytes <= IR_COMMAND_TRANSPORT_BYTES)
}

fn inline_vertex_capacity(vertex_count: usize, draw_count: usize) -> usize {
    let draw_bytes = match draw_count.checked_mul(IR_DRAW_COMMAND_BUDGET) {
        Some(bytes) => bytes,
        None => return 0,
    };
    let used = match vertex_count
        .checked_mul(IR_INLINE_VERTEX_BYTES)
        .and_then(|bytes| bytes.checked_add(IR_COMMAND_FIXED_BUDGET))
        .and_then(|bytes| bytes.checked_add(draw_bytes))
    {
        Some(bytes) => bytes,
        None => return 0,
    };
    let transport_capacity = IR_COMMAND_TRANSPORT_BYTES
        .saturating_sub(used)
        .checked_div(IR_INLINE_VERTEX_BYTES)
        .unwrap_or(0);
    transport_capacity.min(MAX_IR_VERTICES.saturating_sub(vertex_count))
}

fn split_pass(pass: ExecutionPass) -> Result<Vec<ExecutionPass>, IrSubmitError> {
    let IrSubmission {
        clear_color,
        depth_attachment,
        clear_depth,
        render_area,
        vertices,
        draws,
        texture_uploads,
    } = pass.submission;
    let mut chunks = Vec::new();
    let mut chunk_vertices = Vec::new();
    let mut chunk_draws = Vec::new();
    let mut first_chunk = true;

    for draw in draws {
        if draw.vertex_buffer.is_none() {
            let end = draw
                .start_vertex
                .checked_add(draw.vertex_count)
                .ok_or(IrSubmitError::InvalidVertexData)?;
            if end > vertices.len() || !draw.vertex_count.is_multiple_of(3) {
                return Err(IrSubmitError::InvalidVertexData);
            }
            let mut source_start = draw.start_vertex;
            while source_start < end {
                let mut capacity = inline_vertex_capacity(
                    chunk_vertices.len(),
                    chunk_draws.len().saturating_add(1),
                );
                capacity -= capacity % 3;
                if chunk_draws.len() >= MAX_IR_DRAWS_PER_SUBMISSION || capacity == 0 {
                    push_pass_chunk(
                        &mut chunks,
                        &pass.target,
                        clear_color,
                        depth_attachment,
                        clear_depth,
                        render_area,
                        core::mem::take(&mut chunk_vertices),
                        core::mem::take(&mut chunk_draws),
                        if first_chunk {
                            texture_uploads.clone()
                        } else {
                            Vec::new()
                        },
                        first_chunk,
                    )?;
                    first_chunk = false;
                    continue;
                }
                let count = (end - source_start).min(capacity);
                let mut segment = draw.clone();
                segment.start_vertex = chunk_vertices.len();
                segment.vertex_count = count;
                chunk_vertices
                    .try_reserve_exact(count)
                    .map_err(|_| IrSubmitError::OutOfMemory)?;
                chunk_vertices.extend_from_slice(&vertices[source_start..source_start + count]);
                chunk_draws
                    .try_reserve(1)
                    .map_err(|_| IrSubmitError::OutOfMemory)?;
                chunk_draws.push(segment);
                source_start += count;
                if source_start < end {
                    push_pass_chunk(
                        &mut chunks,
                        &pass.target,
                        clear_color,
                        depth_attachment,
                        clear_depth,
                        render_area,
                        core::mem::take(&mut chunk_vertices),
                        core::mem::take(&mut chunk_draws),
                        if first_chunk {
                            texture_uploads.clone()
                        } else {
                            Vec::new()
                        },
                        first_chunk,
                    )?;
                    first_chunk = false;
                }
            }
        } else {
            if chunk_draws.len() >= MAX_IR_DRAWS_PER_SUBMISSION
                || !chunk_fits_transport(chunk_vertices.len(), chunk_draws.len() + 1)
            {
                push_pass_chunk(
                    &mut chunks,
                    &pass.target,
                    clear_color,
                    depth_attachment,
                    clear_depth,
                    render_area,
                    core::mem::take(&mut chunk_vertices),
                    core::mem::take(&mut chunk_draws),
                    if first_chunk {
                        texture_uploads.clone()
                    } else {
                        Vec::new()
                    },
                    first_chunk,
                )?;
                first_chunk = false;
            }
            chunk_draws
                .try_reserve(1)
                .map_err(|_| IrSubmitError::OutOfMemory)?;
            chunk_draws.push(draw);
        }
    }
    push_pass_chunk(
        &mut chunks,
        &pass.target,
        clear_color,
        depth_attachment,
        clear_depth,
        render_area,
        chunk_vertices,
        chunk_draws,
        if first_chunk {
            texture_uploads
        } else {
            Vec::new()
        },
        first_chunk,
    )?;
    Ok(chunks)
}

#[allow(clippy::too_many_arguments)]
fn push_pass_chunk(
    chunks: &mut Vec<ExecutionPass>,
    target: &ExecutionTarget,
    clear_color: Option<[f32; 4]>,
    depth_attachment: Option<driver::IrTextureSpec>,
    clear_depth: Option<f32>,
    render_area: IrRect,
    vertices: Vec<IrVertex>,
    draws: Vec<IrDraw>,
    texture_uploads: Vec<IrTextureUpload>,
    first: bool,
) -> Result<(), IrSubmitError> {
    chunks
        .try_reserve(1)
        .map_err(|_| IrSubmitError::OutOfMemory)?;
    chunks.push(ExecutionPass {
        target: target.clone(),
        submission: IrSubmission {
            clear_color: first.then_some(clear_color).flatten(),
            depth_attachment,
            clear_depth: first.then_some(clear_depth).flatten(),
            render_area,
            vertices,
            draws,
            texture_uploads,
        },
    });
    Ok(())
}

fn pipeline_info(
    resources: &ResourceTable,
    reference: RenderPipelineRef<'_>,
) -> Result<PipelineInfo, IrSubmitError> {
    let (target, topology, fragment, blend, raster, depth, stride, position, secondary, tertiary) = {
        let descriptor = resources.render_pipeline(reference)?;
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
        let tertiary = layout
            .attributes()
            .iter()
            .find(|attribute| attribute.location() == 2)
            .copied();
        let result = (
            descriptor.target_format(),
            descriptor.topology(),
            descriptor.fragment(),
            descriptor.blend(),
            descriptor.raster(),
            descriptor.depth_state(),
            layout.stride(),
            position,
            secondary,
            tertiary,
        );
        result
    };
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
        FragmentProgram::TextureVertexColor(_) => {
            matches!(
                secondary.map(VertexAttribute::format),
                Some(VertexFormat::Float32x3 | VertexFormat::Float32x4 | VertexFormat::Unorm8x4)
            ) && matches!(
                tertiary.map(VertexAttribute::format),
                Some(VertexFormat::Float32x2)
            )
        }
    };
    if !secondary_valid {
        return Err(IrSubmitError::Unsupported(
            UnsupportedIrFeature::VertexLayout,
        ));
    }
    Ok(PipelineInfo {
        state: IrPipelineState {
            slot: reference.slot(),
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
            depth: depth.map(|depth| IrDepthState {
                compare: match depth.compare() {
                    CompareFunction::Never => IrCompareFunction::Never,
                    CompareFunction::Less => IrCompareFunction::Less,
                    CompareFunction::Equal => IrCompareFunction::Equal,
                    CompareFunction::LessEqual => IrCompareFunction::LessEqual,
                    CompareFunction::Greater => IrCompareFunction::Greater,
                    CompareFunction::NotEqual => IrCompareFunction::NotEqual,
                    CompareFunction::GreaterEqual => IrCompareFunction::GreaterEqual,
                    CompareFunction::Always => IrCompareFunction::Always,
                },
                write_enabled: depth.write_enabled(),
            }),
        },
        stride,
        position,
        secondary,
        tertiary,
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
            | IrFragmentProgram::TextureVertexColorRgba
            | IrFragmentProgram::TextureVertexColorRgbIgnoreAlpha
            | IrFragmentProgram::TextureVertexColorAlphaMask
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
        && !matches!(
            pipeline.state.fragment,
            IrFragmentProgram::TextureAlphaMask | IrFragmentProgram::TextureVertexColorAlphaMask
        )
    {
        return Err(IrSubmitError::Unsupported(
            UnsupportedIrFeature::FragmentProgram,
        ));
    }
    let sampler_descriptor = resources.sampler(sampler)?;
    Ok((
        Some(texture_spec(texture, descriptor)),
        Some(sampler_state(sampler_descriptor, sampler.slot())),
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
    if vertex_count == 0 || !vertex_count.is_multiple_of(3) {
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
    let tertiary = decode_tertiary(
        record,
        pipeline.tertiary,
        pipeline.secondary,
        pipeline.state.fragment,
    )?;
    Ok(IrVertex {
        position,
        secondary,
        tertiary,
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
        IrFragmentProgram::TextureVertexColorRgba
        | IrFragmentProgram::TextureVertexColorRgbIgnoreAlpha
        | IrFragmentProgram::TextureVertexColorAlphaMask => {
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
    }
}

fn decode_tertiary(
    record: &[u8],
    attribute: Option<VertexAttribute>,
    texture_attribute: Option<VertexAttribute>,
    fragment: IrFragmentProgram,
) -> Result<[f32; 2], IrSubmitError> {
    let attribute = match fragment {
        IrFragmentProgram::TextureRgba
        | IrFragmentProgram::TextureRgbIgnoreAlpha
        | IrFragmentProgram::TextureAlphaMask => texture_attribute,
        IrFragmentProgram::TextureVertexColorRgba
        | IrFragmentProgram::TextureVertexColorRgbIgnoreAlpha
        | IrFragmentProgram::TextureVertexColorAlphaMask => attribute,
        IrFragmentProgram::Solid | IrFragmentProgram::VertexColor => None,
    };
    let Some(attribute) = attribute else {
        return Ok([0.0; 2]);
    };
    if attribute.format() != VertexFormat::Float32x2 {
        return Err(IrSubmitError::InvalidVertexData);
    }
    let offset =
        usize::try_from(attribute.offset()).map_err(|_| IrSubmitError::InvalidVertexData)?;
    let uv = [read_f32(record, offset)?, read_f32(record, offset + 4)?];
    if !uv.iter().all(|value| value.is_finite()) {
        return Err(IrSubmitError::InvalidVertexData);
    }
    Ok(uv)
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
    if matches!(texture.format, IrTextureFormat::Depth32Float) {
        return Err(IrSubmitError::Unsupported(
            UnsupportedIrFeature::TextureUpload,
        ));
    }
    let destination = write.destination();
    let tight = usize::try_from(destination.width())
        .ok()
        .and_then(|width| {
            width.checked_mul(match texture.format {
                IrTextureFormat::R8 => 1,
                IrTextureFormat::Bgra8 | IrTextureFormat::Rgba8 => 4,
                IrTextureFormat::Depth32Float => unreachable!(),
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
            IrTextureFormat::Depth32Float => unreachable!(),
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
        FragmentProgram::TextureVertexColor(TextureSampleMode::Rgba) => {
            IrFragmentProgram::TextureVertexColorRgba
        }
        FragmentProgram::TextureVertexColor(TextureSampleMode::RgbIgnoreAlpha) => {
            IrFragmentProgram::TextureVertexColorRgbIgnoreAlpha
        }
        FragmentProgram::TextureVertexColor(TextureSampleMode::AlphaMask) => {
            IrFragmentProgram::TextureVertexColorAlphaMask
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
        slot: texture.slot(),
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
            TextureFormat::Depth32Float => IrTextureFormat::Depth32Float,
        },
    }
}

fn materializable_texture(spec: driver::IrTextureSpec) -> bool {
    spec.sampled || spec.render_attachment || spec.copy_destination || spec.present
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::{
        IrBlendComponent, IrBlendFactor, IrBlendOp, IrBlendState, IrCullMode, IrFragmentProgram,
        IrFrontFace,
    };

    fn target() -> driver::IrTextureSpec {
        driver::IrTextureSpec {
            slot: 0,
            width: 64,
            height: 64,
            sampled: false,
            render_attachment: true,
            copy_destination: false,
            present: false,
            format: IrTextureFormat::Bgra8,
        }
    }

    fn draw(start_vertex: usize, vertex_count: usize, order: usize) -> IrDraw {
        let blend_component = IrBlendComponent {
            source_factor: IrBlendFactor::One,
            destination_factor: IrBlendFactor::Zero,
            operation: IrBlendOp::Add,
        };
        IrDraw {
            start_vertex,
            vertex_count,
            vertex_buffer: None,
            pipeline: IrPipelineState {
                slot: 0,
                fragment: IrFragmentProgram::VertexColor,
                blend: IrBlendState {
                    color: blend_component,
                    alpha: blend_component,
                },
                cull_mode: IrCullMode::None,
                front_face: IrFrontFace::CounterClockwise,
                depth: None,
            },
            texture: None,
            sampler: None,
            uniforms: IrUniforms {
                transform: [0.0; 16],
                color: [order as f32, 0.0, 0.0, 0.0],
            },
            scissor: IrRect {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            },
        }
    }

    fn execution_pass(vertices: Vec<IrVertex>, draws: Vec<IrDraw>) -> ExecutionPass {
        ExecutionPass {
            target: ExecutionTarget::Internal(target()),
            submission: IrSubmission {
                clear_color: Some([0.1, 0.2, 0.3, 1.0]),
                depth_attachment: None,
                clear_depth: None,
                render_area: IrRect {
                    x: 0,
                    y: 0,
                    width: 64,
                    height: 64,
                },
                vertices,
                draws,
                texture_uploads: Vec::new(),
            },
        }
    }

    fn depth_target() -> driver::IrTextureSpec {
        driver::IrTextureSpec {
            slot: 1,
            width: 64,
            height: 64,
            sampled: false,
            render_attachment: true,
            copy_destination: false,
            present: false,
            format: IrTextureFormat::Depth32Float,
        }
    }

    fn plan_with_pass(pass: ExecutionPass) -> ExecutionPlan {
        ExecutionPlan {
            events: vec![ExecutionEvent::Pass(pass)],
            buffer_updates: Vec::new(),
            canonical_validations: Vec::new(),
        }
    }

    #[test]
    fn clear_only_color_pass_is_lowered_and_transport_valid() {
        let chunks = split_pass(execution_pass(Vec::new(), Vec::new())).expect("split pass");

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].submission.draws.is_empty());
        assert!(chunks[0].submission.clear_color.is_some());
        assert!(
            plan_with_pass(chunks.into_iter().next().expect("clear pass"))
                .validate_transport_chunks()
                .is_ok()
        );
    }

    #[test]
    fn clear_only_depth_pass_is_lowered_and_transport_valid() {
        let mut pass = execution_pass(Vec::new(), Vec::new());
        pass.submission.clear_color = None;
        pass.submission.depth_attachment = Some(depth_target());
        pass.submission.clear_depth = Some(1.0);
        let chunks = split_pass(pass).expect("split pass");

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].submission.draws.is_empty());
        assert!(chunks[0].submission.clear_depth.is_some());
        assert!(
            plan_with_pass(chunks.into_iter().next().expect("clear pass"))
                .validate_transport_chunks()
                .is_ok()
        );
    }

    #[test]
    fn empty_pass_without_a_clear_remains_rejected() {
        let mut pass = execution_pass(Vec::new(), Vec::new());
        pass.submission.clear_color = None;
        let plan = plan_with_pass(pass);

        assert!(matches!(
            plan.validate_transport_chunks(),
            Err(IrSubmitError::InvalidVertexData)
        ));
    }

    #[test]
    fn oversized_inline_pass_is_split_in_order_without_repeating_clear() {
        let vertex = IrVertex {
            position: [0.0; 4],
            secondary: [0.0; 4],
            tertiary: [0.0; 2],
        };
        let vertex_count = MAX_IR_VERTICES + 6;
        let vertices = vec![vertex; vertex_count];
        let draws = vec![draw(0, vertex_count, 7)];
        let chunks = split_pass(execution_pass(vertices, draws)).expect("split pass");

        assert!(chunks.len() > 1);
        assert!(chunks[0].submission.clear_color.is_some());
        assert!(
            chunks[1..]
                .iter()
                .all(|chunk| chunk.submission.clear_color.is_none())
        );
        assert!(chunks.iter().all(|chunk| {
            chunk.submission.vertices.len() <= MAX_IR_VERTICES
                && chunk.submission.draws.len() <= MAX_IR_DRAWS_PER_SUBMISSION
                && chunk_fits_transport(
                    chunk.submission.vertices.len(),
                    chunk.submission.draws.len(),
                )
        }));
        let planned_vertices: usize = chunks
            .iter()
            .flat_map(|chunk| chunk.submission.draws.iter())
            .map(|draw| {
                assert_eq!(draw.uniforms.color[0], 7.0);
                assert!(draw.vertex_count.is_multiple_of(3));
                draw.vertex_count
            })
            .sum();
        assert_eq!(planned_vertices, vertex_count);
    }

    #[test]
    fn persistent_draws_are_chunked_without_inline_vertices() {
        let draw_count = MAX_IR_DRAWS_PER_SUBMISSION + 5;
        let mut draws: Vec<_> = (0..draw_count).map(|index| draw(0, 3, index)).collect();
        for draw in &mut draws {
            draw.vertex_buffer = Some(IrVertexBufferBinding {
                buffer: IrBufferSpec {
                    slot: 1,
                    size: 120,
                    revision: 1,
                },
                offset: 0,
            });
        }
        let chunks = split_pass(execution_pass(Vec::new(), draws)).expect("split pass");

        assert_eq!(chunks.len(), 2);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.submission.vertices.is_empty()
                    && chunk.submission.draws.len() <= MAX_IR_DRAWS_PER_SUBMISSION
                    && chunk_fits_transport(0, chunk.submission.draws.len()))
        );
    }
}
