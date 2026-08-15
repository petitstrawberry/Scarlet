//! Validated logical upload, copy, and render commands.

use alloc::vec::Vec;

use super::{
    BufferRef, BufferUsage, Color, DrawUniforms, Error, FragmentProgram, IndexFormat, PixelRect,
    RenderPipelineRef, ResourceTable, Result, SamplerRef, TextureFormat, TextureRef, TextureUsage,
    TextureWrite,
};

/// Maximum commands retained by one logical command buffer.
pub const MAX_COMMANDS: usize = 4_096;

/// Color attachment initialization operation for a render pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadOp {
    /// Preserve the attachment contents in the render area.
    Load,
    /// Clear the render area to a finite color before drawing.
    Clear(Color),
    /// Permit a backend to discard prior attachment contents.
    DontCare,
}

/// Color attachment finalization operation for a render pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOp {
    /// Preserve attachment contents after the pass.
    Store,
    /// Permit a backend to discard attachment contents after the pass.
    DontCare,
}

/// Depth attachment initialization operation for a render pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DepthLoadOp {
    /// Preserve the attachment contents in the render area.
    Load,
    /// Clear the render area to the finite depth value before drawing.
    Clear(f32),
    /// Permit a backend to discard prior attachment contents.
    DontCare,
}

/// Validated depth attachment configuration for one render pass.
#[derive(Clone, Copy)]
pub struct DepthAttachment<'r> {
    pub(crate) target: TextureRef<'r>,
    pub(crate) load: DepthLoadOp,
    pub(crate) store: StoreOp,
}

impl<'r> DepthAttachment<'r> {
    /// Return the depth attachment reference.
    ///
    /// # Returns
    /// The lifetime-branded depth texture reference.
    pub const fn target(self) -> TextureRef<'r> {
        self.target
    }

    /// Return the depth attachment initialization operation.
    ///
    /// # Returns
    /// The configured depth load operation.
    pub const fn load(self) -> DepthLoadOp {
        self.load
    }

    /// Return the depth attachment finalization operation.
    ///
    /// # Returns
    /// The configured store operation.
    pub const fn store(self) -> StoreOp {
        self.store
    }
}

/// Validated configuration for one logical color render pass.
#[derive(Clone, Copy)]
pub struct RenderPassDesc<'r> {
    pub(crate) target: TextureRef<'r>,
    pub(crate) area: PixelRect,
    pub(crate) load: LoadOp,
    pub(crate) store: StoreOp,
    pub(crate) depth: Option<DepthAttachment<'r>>,
}

impl<'r> RenderPassDesc<'r> {
    /// Construct a validated color render-pass descriptor.
    ///
    /// # Arguments
    ///
    /// * `resources` - Table that owns `target`.
    /// * `target` - `RENDER_ATTACHMENT` texture.
    /// * `area` - Non-empty render area inside `target`.
    /// * `load` - Attachment initialization operation.
    /// * `store` - Attachment finalization operation.
    ///
    /// # Returns
    /// A validated pass descriptor, or an error for table mismatch, usage, or bounds.
    pub fn new(
        resources: &'r ResourceTable,
        target: TextureRef<'r>,
        area: PixelRect,
        load: LoadOp,
        store: StoreOp,
    ) -> Result<Self> {
        let texture = resources.texture(target)?;
        if texture.format() == TextureFormat::Depth32Float {
            return Err(Error::InvalidDescriptor);
        }
        if !texture.usage().contains(TextureUsage::RENDER_ATTACHMENT) {
            return Err(Error::InvalidUsage);
        }
        if !area.is_within(texture.extent()) {
            return Err(Error::OutOfBounds);
        }
        Ok(Self {
            target,
            area,
            load,
            store,
            depth: None,
        })
    }

    /// Add a validated depth attachment to this color render pass.
    ///
    /// # Arguments
    ///
    /// * `resources` - Table that owns `target` and the color attachment.
    /// * `target` - `Depth32Float` `RENDER_ATTACHMENT` texture.
    /// * `load` - Depth attachment initialization operation.
    /// * `store` - Depth attachment finalization operation.
    ///
    /// # Returns
    /// The updated descriptor, or an error for table mismatch, format, usage,
    /// bounds, or a non-finite/out-of-range clear value.
    pub fn with_depth_attachment(
        mut self,
        resources: &'r ResourceTable,
        target: TextureRef<'r>,
        load: DepthLoadOp,
        store: StoreOp,
    ) -> Result<Self> {
        let texture = resources.texture(target)?;
        let color_texture = resources.texture(self.target)?;
        if texture.format() != TextureFormat::Depth32Float {
            return Err(Error::InvalidDescriptor);
        }
        if !texture.usage().contains(TextureUsage::RENDER_ATTACHMENT) {
            return Err(Error::InvalidUsage);
        }
        if texture.extent() != color_texture.extent() || !self.area.is_within(texture.extent()) {
            return Err(Error::OutOfBounds);
        }
        if matches!(load, DepthLoadOp::Clear(value) if !value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Err(Error::InvalidDescriptor);
        }
        self.depth = Some(DepthAttachment {
            target,
            load,
            store,
        });
        Ok(self)
    }

    /// Return the color attachment reference.
    ///
    /// # Returns
    /// The lifetime-branded target texture reference.
    pub const fn target(self) -> TextureRef<'r> {
        self.target
    }

    /// Return the render area.
    ///
    /// # Returns
    /// The validated non-empty target rectangle.
    pub const fn area(self) -> PixelRect {
        self.area
    }

    /// Return the attachment initialization operation.
    ///
    /// # Returns
    /// The configured load operation.
    pub const fn load(self) -> LoadOp {
        self.load
    }

    /// Return the attachment finalization operation.
    ///
    /// # Returns
    /// The configured store operation.
    pub const fn store(self) -> StoreOp {
        self.store
    }

    /// Return the optional depth attachment configuration.
    ///
    /// # Returns
    /// The depth attachment when depth testing is available for this pass.
    pub const fn depth_attachment(self) -> Option<DepthAttachment<'r>> {
        self.depth
    }
}

/// One validated logical graphics command.
///
/// Commands are exposed only through [`CommandBuffer::commands`]. Resource
/// references remain lifetime-branded and cannot be constructed from raw IDs.
pub enum Command<'r, 'data> {
    /// Upload borrowed bytes into a logical buffer.
    WriteBuffer {
        /// Destination buffer reference.
        buffer: BufferRef<'r>,
        /// Destination byte offset.
        offset: u64,
        /// Borrowed source bytes.
        data: &'data [u8],
    },
    /// Upload borrowed pixel data into a logical texture.
    WriteTexture {
        /// Destination texture reference.
        texture: TextureRef<'r>,
        /// Validated pixel source layout and destination rectangle.
        write: TextureWrite<'data>,
    },
    /// Copy an equal-sized rectangle between compatible logical textures.
    CopyTextureToTexture {
        /// Source texture reference.
        source: TextureRef<'r>,
        /// Source rectangle.
        source_rect: PixelRect,
        /// Destination texture reference.
        destination: TextureRef<'r>,
        /// Destination rectangle.
        destination_rect: PixelRect,
    },
    /// Begin a render pass with explicit attachment operations.
    BeginRenderPass(RenderPassDesc<'r>),
    /// End the current render pass.
    EndRenderPass,
    /// Bind a render pipeline for subsequent draws.
    SetPipeline(RenderPipelineRef<'r>),
    /// Bind a vertex buffer for subsequent draws.
    SetVertexBuffer {
        /// Vertex buffer reference.
        buffer: BufferRef<'r>,
        /// Byte offset of vertex zero.
        offset: u64,
    },
    /// Bind an index buffer for subsequent indexed draws.
    SetIndexBuffer {
        /// Index buffer reference.
        buffer: BufferRef<'r>,
        /// Byte offset of index zero.
        offset: u64,
        /// Width of each encoded index.
        format: IndexFormat,
    },
    /// Bind a sampled texture for subsequent draws.
    SetTexture(TextureRef<'r>),
    /// Bind a sampler for subsequent draws.
    SetSampler(SamplerRef<'r>),
    /// Bind transform and color uniforms for subsequent draws.
    SetUniforms(DrawUniforms),
    /// Set or reset the scissor for subsequent draws.
    SetScissor(Option<PixelRect>),
    /// Issue a non-indexed draw.
    Draw {
        /// Number of vertices to draw.
        vertex_count: u32,
        /// First vertex relative to the bound vertex buffer.
        first_vertex: u32,
    },
    /// Issue an indexed draw.
    DrawIndexed {
        /// Number of indices to draw.
        index_count: u32,
        /// First index relative to the bound index buffer.
        first_index: u32,
        /// Signed base vertex applied by a backend.
        base_vertex: i32,
    },
}

/// Encoder that records validated logical graphics commands.
pub struct CommandEncoder<'r, 'data> {
    resources: &'r ResourceTable,
    commands: Vec<Command<'r, 'data>>,
    pass_open: bool,
}

impl<'r, 'data> CommandEncoder<'r, 'data> {
    /// Start an empty command encoder for one resource table.
    ///
    /// # Arguments
    ///
    /// * `resources` - Table whose branded references may be recorded.
    ///
    /// # Returns
    /// An empty encoder with the fixed [`MAX_COMMANDS`] bound.
    pub const fn new(resources: &'r ResourceTable) -> Self {
        Self {
            resources,
            commands: Vec::new(),
            pass_open: false,
        }
    }

    /// Record a buffer upload outside a render pass.
    ///
    /// # Arguments
    ///
    /// * `buffer` - `COPY_DST` buffer in this encoder's table.
    /// * `offset` - Destination byte offset.
    /// * `data` - Non-empty borrowed source bytes.
    ///
    /// # Returns
    /// Success, or an error for table mismatch, usage, range, pass state, or capacity.
    pub fn write_buffer(
        &mut self,
        buffer: BufferRef<'r>,
        offset: u64,
        data: &'data [u8],
    ) -> Result<()> {
        self.ensure_outside_pass()?;
        let desc = self.resources.buffer(buffer)?;
        Self::require_buffer_usage(desc.usage(), BufferUsage::COPY_DST)?;
        if data.is_empty() {
            return Err(Error::InvalidValue);
        }
        Self::validate_byte_range(
            offset,
            u64::try_from(data.len()).map_err(|_| Error::Overflow)?,
            desc.size(),
        )?;
        self.push(Command::WriteBuffer {
            buffer,
            offset,
            data,
        })
    }

    /// Record a texture upload outside a render pass.
    ///
    /// # Arguments
    ///
    /// * `texture` - `COPY_DST` texture in this encoder's table.
    /// * `write` - Borrowed pixel data and destination layout.
    ///
    /// # Returns
    /// Success, or an error for table mismatch, usage, bounds, source layout, pass state, or capacity.
    pub fn write_texture(
        &mut self,
        texture: TextureRef<'r>,
        write: TextureWrite<'data>,
    ) -> Result<()> {
        self.ensure_outside_pass()?;
        let desc = self.resources.texture(texture)?;
        Self::require_texture_usage(desc.usage(), TextureUsage::COPY_DST)?;
        if !write.destination().is_within(desc.extent()) {
            return Err(Error::OutOfBounds);
        }
        let tight = write
            .destination()
            .width()
            .checked_mul(desc.format().bytes_per_pixel())
            .ok_or(Error::Overflow)?;
        if write.bytes_per_row() < tight {
            return Err(Error::InvalidValue);
        }
        let rows_before_last = u64::from(write.destination().height() - 1);
        let required = u64::from(write.bytes_per_row())
            .checked_mul(rows_before_last)
            .and_then(|value| value.checked_add(u64::from(tight)))
            .ok_or(Error::Overflow)?;
        if u64::try_from(write.data().len()).map_err(|_| Error::Overflow)? < required {
            return Err(Error::OutOfBounds);
        }
        self.push(Command::WriteTexture { texture, write })
    }

    /// Record a format-preserving texture-to-texture copy outside a render pass.
    ///
    /// # Arguments
    ///
    /// * `source` - `COPY_SRC` source texture.
    /// * `source_rect` - Source rectangle.
    /// * `destination` - `COPY_DST` destination texture.
    /// * `destination_rect` - Equal-sized destination rectangle.
    ///
    /// # Returns
    /// Success, or an error for mismatched tables, usage, format, bounds, overlapping self-copy, pass state, or capacity.
    pub fn copy_texture_to_texture(
        &mut self,
        source: TextureRef<'r>,
        source_rect: PixelRect,
        destination: TextureRef<'r>,
        destination_rect: PixelRect,
    ) -> Result<()> {
        self.ensure_outside_pass()?;
        let source_desc = self.resources.texture(source)?;
        let destination_desc = self.resources.texture(destination)?;
        Self::require_texture_usage(source_desc.usage(), TextureUsage::COPY_SRC)?;
        Self::require_texture_usage(destination_desc.usage(), TextureUsage::COPY_DST)?;
        if source_desc.format() != destination_desc.format()
            || !source_rect.same_extent(destination_rect)
        {
            return Err(Error::InvalidDescriptor);
        }
        if !source_rect.is_within(source_desc.extent())
            || !destination_rect.is_within(destination_desc.extent())
        {
            return Err(Error::OutOfBounds);
        }
        if self.resources.same_texture(source, destination)?
            && Self::rectangles_overlap(source_rect, destination_rect)
        {
            return Err(Error::InvalidValue);
        }
        self.push(Command::CopyTextureToTexture {
            source,
            source_rect,
            destination,
            destination_rect,
        })
    }

    /// Begin a render pass described by explicit load and store operations.
    ///
    /// # Arguments
    ///
    /// * `desc` - Validated pass descriptor from this encoder's resource table.
    ///
    /// # Returns
    /// A pass encoder, or an error for usage, bounds, nested-pass state, or capacity.
    pub fn begin_render_pass<'encoder>(
        &'encoder mut self,
        desc: RenderPassDesc<'r>,
    ) -> Result<RenderPassEncoder<'encoder, 'r, 'data>> {
        self.ensure_outside_pass()?;
        let target_desc = self.resources.texture(desc.target)?;
        Self::require_texture_usage(target_desc.usage(), TextureUsage::RENDER_ATTACHMENT)?;
        if !desc.area.is_within(target_desc.extent()) {
            return Err(Error::OutOfBounds);
        }
        self.reserve_pass_begin()?;
        self.push(Command::BeginRenderPass(desc))?;
        self.pass_open = true;
        Ok(RenderPassEncoder {
            encoder: self,
            target: desc.target,
            target_format: target_desc.format(),
            area: desc.area,
            pipeline: None,
            vertex_buffer: None,
            index_buffer: None,
            texture: None,
            sampler: None,
            uniforms: None,
        })
    }

    /// Finish this encoder into an immutable logical command buffer.
    ///
    /// # Returns
    /// The finished command buffer, or [`Error::RenderPassStillActive`] when a pass was not ended.
    pub fn finish(self) -> Result<CommandBuffer<'r, 'data>> {
        if self.pass_open {
            Err(Error::RenderPassStillActive)
        } else {
            Ok(CommandBuffer {
                resources: self.resources,
                commands: self.commands,
            })
        }
    }

    fn ensure_outside_pass(&self) -> Result<()> {
        if self.pass_open {
            Err(Error::RenderPassActive)
        } else {
            Ok(())
        }
    }
    fn push(&mut self, command: Command<'r, 'data>) -> Result<()> {
        let reserved_end_slot = usize::from(self.pass_open);
        if self.commands.len() >= MAX_COMMANDS - reserved_end_slot {
            return Err(Error::CommandLimitExceeded);
        }
        self.commands
            .try_reserve(1)
            .map_err(|_| Error::OutOfMemory)?;
        self.commands.push(command);
        Ok(())
    }
    fn reserve_pass_begin(&self) -> Result<()> {
        if self.commands.len() > MAX_COMMANDS - 2 {
            Err(Error::CommandLimitExceeded)
        } else {
            Ok(())
        }
    }
    fn push_end_render_pass(&mut self) -> Result<()> {
        if self.commands.len() >= MAX_COMMANDS {
            return Err(Error::CommandLimitExceeded);
        }
        self.commands
            .try_reserve(1)
            .map_err(|_| Error::OutOfMemory)?;
        self.commands.push(Command::EndRenderPass);
        Ok(())
    }
    fn require_texture_usage(usage: TextureUsage, required: TextureUsage) -> Result<()> {
        if usage.contains(required) {
            Ok(())
        } else {
            Err(Error::InvalidUsage)
        }
    }
    fn require_buffer_usage(usage: BufferUsage, required: BufferUsage) -> Result<()> {
        if usage.contains(required) {
            Ok(())
        } else {
            Err(Error::InvalidUsage)
        }
    }
    fn validate_byte_range(offset: u64, length: u64, limit: u64) -> Result<()> {
        match offset.checked_add(length) {
            Some(end) if end <= limit => Ok(()),
            Some(_) => Err(Error::OutOfBounds),
            None => Err(Error::Overflow),
        }
    }
    fn rectangles_overlap(left: PixelRect, right: PixelRect) -> bool {
        left.x() < right.x() + right.width()
            && right.x() < left.x() + left.width()
            && left.y() < right.y() + right.height()
            && right.y() < left.y() + left.height()
    }
}

/// Encoder for commands within one active render pass.
pub struct RenderPassEncoder<'encoder, 'r, 'data> {
    encoder: &'encoder mut CommandEncoder<'r, 'data>,
    target: TextureRef<'r>,
    target_format: super::TextureFormat,
    area: PixelRect,
    pipeline: Option<RenderPipelineRef<'r>>,
    vertex_buffer: Option<(BufferRef<'r>, u64)>,
    index_buffer: Option<(BufferRef<'r>, u64, IndexFormat)>,
    texture: Option<TextureRef<'r>>,
    sampler: Option<SamplerRef<'r>>,
    uniforms: Option<DrawUniforms>,
}

impl<'encoder, 'r, 'data> RenderPassEncoder<'encoder, 'r, 'data> {
    /// Set the render pipeline for subsequent draws.
    ///
    /// # Arguments
    ///
    /// * `pipeline` - Pipeline from this encoder's resource table.
    ///
    /// # Returns
    /// Success, or an error when the table or target format differs.
    pub fn set_pipeline(&mut self, pipeline: RenderPipelineRef<'r>) -> Result<()> {
        let target_format = self
            .encoder
            .resources
            .with_pipeline(pipeline, |descriptor| descriptor.target_format())?;
        if target_format != self.target_format {
            return Err(Error::PipelineTargetMismatch);
        }
        self.encoder.push(Command::SetPipeline(pipeline))?;
        self.pipeline = Some(pipeline);
        Ok(())
    }

    /// Set the vertex buffer and first vertex byte offset.
    ///
    /// # Arguments
    ///
    /// * `buffer` - `VERTEX` buffer from this encoder's resource table.
    /// * `offset` - Byte offset of vertex zero.
    ///
    /// # Returns
    /// Success, or an error for table mismatch, usage, range, or capacity.
    pub fn set_vertex_buffer(&mut self, buffer: BufferRef<'r>, offset: u64) -> Result<()> {
        let desc = self.encoder.resources.buffer(buffer)?;
        CommandEncoder::require_buffer_usage(desc.usage(), BufferUsage::VERTEX)?;
        if offset > desc.size() {
            return Err(Error::OutOfBounds);
        }
        self.encoder
            .push(Command::SetVertexBuffer { buffer, offset })?;
        self.vertex_buffer = Some((buffer, offset));
        Ok(())
    }

    /// Set the index buffer, byte offset, and encoded index format.
    ///
    /// # Arguments
    ///
    /// * `buffer` - `INDEX` buffer from this encoder's resource table.
    /// * `offset` - Aligned byte offset of index zero.
    /// * `format` - Width of each encoded index.
    ///
    /// # Returns
    /// Success, or an error for table mismatch, usage, alignment, range, or capacity.
    pub fn set_index_buffer(
        &mut self,
        buffer: BufferRef<'r>,
        offset: u64,
        format: IndexFormat,
    ) -> Result<()> {
        let desc = self.encoder.resources.buffer(buffer)?;
        CommandEncoder::require_buffer_usage(desc.usage(), BufferUsage::INDEX)?;
        if offset > desc.size() {
            return Err(Error::OutOfBounds);
        }
        if !offset.is_multiple_of(format.byte_size()) {
            return Err(Error::InvalidValue);
        }
        self.encoder.push(Command::SetIndexBuffer {
            buffer,
            offset,
            format,
        })?;
        self.index_buffer = Some((buffer, offset, format));
        Ok(())
    }

    /// Set the sampled texture for a textured pipeline.
    ///
    /// # Arguments
    ///
    /// * `texture` - `SAMPLED` texture from this encoder's resource table.
    ///
    /// # Returns
    /// Success, or an error for usage, table mismatch, attachment feedback, or capacity.
    pub fn set_texture(&mut self, texture: TextureRef<'r>) -> Result<()> {
        let desc = self.encoder.resources.texture(texture)?;
        CommandEncoder::require_texture_usage(desc.usage(), TextureUsage::SAMPLED)?;
        if self.encoder.resources.same_texture(texture, self.target)? {
            return Err(Error::AttachmentFeedback);
        }
        self.encoder.push(Command::SetTexture(texture))?;
        self.texture = Some(texture);
        Ok(())
    }

    /// Set the sampler for a textured pipeline.
    ///
    /// # Arguments
    ///
    /// * `sampler` - Sampler from this encoder's resource table.
    ///
    /// # Returns
    /// Success, or an error for table mismatch or capacity.
    pub fn set_sampler(&mut self, sampler: SamplerRef<'r>) -> Result<()> {
        self.encoder.resources.sampler(sampler)?;
        self.encoder.push(Command::SetSampler(sampler))?;
        self.sampler = Some(sampler);
        Ok(())
    }

    /// Set finite transform and color uniforms for subsequent draws.
    ///
    /// # Arguments
    ///
    /// * `uniforms` - General transform and finite color values.
    ///
    /// # Returns
    /// Success, or an allocation/capacity error.
    pub fn set_uniforms(&mut self, uniforms: DrawUniforms) -> Result<()> {
        self.encoder.push(Command::SetUniforms(uniforms))?;
        self.uniforms = Some(uniforms);
        Ok(())
    }

    /// Set or disable the scissor for subsequent draws.
    ///
    /// # Arguments
    ///
    /// * `scissor` - A rectangle wholly inside the pass area, or `None` to reset clipping to the pass render area.
    ///
    /// # Returns
    /// Success, or [`Error::OutOfBounds`] for a scissor outside the render area. `None` never permits drawing outside that area.
    pub fn set_scissor(&mut self, scissor: Option<PixelRect>) -> Result<()> {
        if let Some(scissor) = scissor
            && (scissor.x() < self.area.x()
                || scissor.y() < self.area.y()
                || scissor.x() + scissor.width() > self.area.x() + self.area.width()
                || scissor.y() + scissor.height() > self.area.y() + self.area.height())
        {
            return Err(Error::OutOfBounds);
        }
        self.encoder.push(Command::SetScissor(scissor))
    }

    /// Record a non-indexed triangle-list draw.
    ///
    /// # Arguments
    ///
    /// * `vertex_count` - Positive multiple of three vertices.
    /// * `first_vertex` - First vertex relative to the bound vertex buffer.
    ///
    /// # Returns
    /// Success, or an error for missing state, vertex range, texture bindings, or capacity.
    pub fn draw(&mut self, vertex_count: u32, first_vertex: u32) -> Result<()> {
        self.validate_draw(vertex_count)?;
        let pipeline = self.pipeline.ok_or(Error::PipelineNotSet)?;
        let stride = self
            .encoder
            .resources
            .with_pipeline(pipeline, |descriptor| descriptor.vertex_buffer().stride())?;
        let (buffer, offset) = self.vertex_buffer.ok_or(Error::VertexBufferNotSet)?;
        let desc = self.encoder.resources.buffer(buffer)?;
        if offset % u64::from(stride) != 0 {
            return Err(Error::InvalidValue);
        }
        let vertices = u64::from(first_vertex)
            .checked_add(u64::from(vertex_count))
            .ok_or(Error::Overflow)?;
        let bytes = vertices
            .checked_mul(u64::from(stride))
            .ok_or(Error::Overflow)?;
        CommandEncoder::validate_byte_range(offset, bytes, desc.size())?;
        self.encoder.push(Command::Draw {
            vertex_count,
            first_vertex,
        })
    }

    /// Record an indexed triangle-list draw.
    ///
    /// # Arguments
    ///
    /// * `index_count` - Positive multiple of three indices.
    /// * `first_index` - First index relative to the bound index buffer.
    /// * `base_vertex` - Signed base vertex added by a future backend.
    ///
    /// # Returns
    /// Success, or an error for missing pipeline, vertex/index buffers, index range, bindings, or capacity. A future backend validates the maximum vertex addressed by opaque index data.
    pub fn draw_indexed(
        &mut self,
        index_count: u32,
        first_index: u32,
        base_vertex: i32,
    ) -> Result<()> {
        self.validate_draw(index_count)?;
        let pipeline = self.pipeline.ok_or(Error::PipelineNotSet)?;
        let stride = self
            .encoder
            .resources
            .with_pipeline(pipeline, |descriptor| descriptor.vertex_buffer().stride())?;
        let (vertex_buffer, vertex_offset) = self.vertex_buffer.ok_or(Error::VertexBufferNotSet)?;
        let vertex_desc = self.encoder.resources.buffer(vertex_buffer)?;
        if vertex_offset % u64::from(stride) != 0 {
            return Err(Error::InvalidValue);
        }
        CommandEncoder::validate_byte_range(vertex_offset, u64::from(stride), vertex_desc.size())?;
        let (buffer, offset, format) = self.index_buffer.ok_or(Error::IndexBufferNotSet)?;
        let desc = self.encoder.resources.buffer(buffer)?;
        let indices = u64::from(first_index)
            .checked_add(u64::from(index_count))
            .ok_or(Error::Overflow)?;
        let bytes = indices
            .checked_mul(format.byte_size())
            .ok_or(Error::Overflow)?;
        CommandEncoder::validate_byte_range(offset, bytes, desc.size())?;
        self.encoder.push(Command::DrawIndexed {
            index_count,
            first_index,
            base_vertex,
        })
    }

    /// End this render pass.
    ///
    /// # Returns
    /// Success, or an allocation/capacity error while recording the end command.
    pub fn end(self) -> Result<()> {
        self.encoder.push_end_render_pass()?;
        self.encoder.pass_open = false;
        Ok(())
    }

    fn validate_draw(&self, count: u32) -> Result<()> {
        if count == 0 || !count.is_multiple_of(3) {
            return Err(Error::InvalidValue);
        }
        let pipeline = self.pipeline.ok_or(Error::PipelineNotSet)?;
        let fragment = self
            .encoder
            .resources
            .with_pipeline(pipeline, |descriptor| descriptor.fragment())?;
        self.uniforms.ok_or(Error::UniformsNotSet)?;
        if matches!(
            fragment,
            FragmentProgram::Texture(_) | FragmentProgram::TextureVertexColor(_)
        ) && (self.texture.is_none() || self.sampler.is_none())
        {
            return Err(Error::TextureBindingNotSet);
        }
        Ok(())
    }
}

/// Finished, immutable logical command buffer with separate resource and data lifetimes.
pub struct CommandBuffer<'r, 'data> {
    pub(crate) resources: &'r ResourceTable,
    pub(crate) commands: Vec<Command<'r, 'data>>,
}

impl<'r, 'data> CommandBuffer<'r, 'data> {
    /// Return the resource table that owns this buffer's references.
    ///
    /// # Returns
    ///
    /// The lifetime-branded resource table retained at finish time.
    pub const fn resources(&self) -> &'r ResourceTable {
        self.resources
    }

    /// Return the immutable ordered command stream.
    ///
    /// # Returns
    ///
    /// The validated command slice. Its resource references cannot be forged
    /// because their owner and index fields are not public.
    pub fn commands(&self) -> &[Command<'r, 'data>] {
        &self.commands
    }

    /// Return the number of recorded commands.
    ///
    /// # Returns
    /// The immutable command count without exposing resource identities.
    pub const fn command_count(&self) -> usize {
        self.commands.len()
    }
    /// Return whether no commands were recorded.
    ///
    /// # Returns
    /// `true` when the command buffer is empty.
    pub const fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}
