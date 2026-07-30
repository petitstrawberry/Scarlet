//! Lowering of the currently supported logical IR subset to the active backend.

use alloc::vec::Vec;
use core::mem::size_of;

use crate::ir::{
    self, BlendState, BufferRef, BufferUsage, Command, CommandBuffer, DrawUniforms,
    FragmentProgram, LoadOp, PrimitiveTopology, RenderPassDesc, RenderPipelineRef, ResourceTable,
    StoreOp, TextureFormat, TextureRef, TextureUsage, VertexFormat,
};
use crate::{
    Color, Context, CullMode, FrontFace, HandleError, Image, PipelineDesc, Queue, RenderPass,
    VertexClip4Color3, Viewport,
};

/// An IR feature that the active backend facade cannot lower faithfully yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedIrFeature {
    /// The command sequence is not the single-upload, single-pass shape currently supported.
    CommandSequence,
    /// More than one buffer upload was recorded.
    MultipleBufferUploads,
    /// More than one render pass was recorded.
    MultipleRenderPasses,
    /// More than one draw was recorded.
    MultipleDraws,
    /// Texture upload is not available through the initial IR lowering path.
    TextureUpload,
    /// Texture-to-texture copies are not available through the initial IR lowering path.
    TextureCopy,
    /// Index buffers and indexed draws are not available through the initial IR lowering path.
    IndexedDrawing,
    /// Sampled textures are not available through the initial IR lowering path.
    TextureSampling,
    /// Sampler objects are not available through the initial IR lowering path.
    Sampler,
    /// Scissor state is not available through the initial IR lowering path.
    Scissor,
    /// The presentation target format is unsupported.
    TargetFormat,
    /// The presentation target usage combination is unsupported.
    TargetUsage,
    /// Only a render pass covering the complete presentation target is supported.
    RenderArea,
    /// Only a clear attachment load operation is supported.
    LoadOperation,
    /// Only a store attachment operation is supported.
    StoreOperation,
    /// The clear color cannot be represented by the active backend facade.
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
    /// Draw uniforms require backend functionality that is not implemented yet.
    DrawUniforms,
}

/// Failure while mapping or submitting a logical IR command buffer.
#[derive(Debug)]
pub enum IrSubmitError {
    /// A logical resource reference or descriptor failed validation.
    InvalidIr(ir::Error),
    /// The command buffer and presentation target use different resource tables.
    ResourceTableMismatch,
    /// The logical target extent differs from the physical presentation image.
    TargetExtentMismatch,
    /// A valid IR feature is not implemented by the active lowering path.
    Unsupported(UnsupportedIrFeature),
    /// Uploaded vertex bytes are malformed, non-finite, or outside the supported color range.
    InvalidVertexData,
    /// Allocation for decoded backend vertices failed.
    OutOfMemory,
    /// The active graphics backend rejected resource creation or submission.
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

/// Typed association between one logical presentation texture and one real image.
///
/// The mapping retains only safe references. Backend resource identifiers and
/// transport details remain private to `sgfx`.
#[derive(Clone, Copy)]
pub struct IrPresentTarget<'r, 'image> {
    resources: &'r ResourceTable,
    texture: TextureRef<'r>,
    image: &'image Image,
}

impl Image {
    /// Associate this image with one logical presentation texture.
    ///
    /// # Arguments
    ///
    /// * `resources` - Resource table that owns `texture`.
    /// * `texture` - BGRA render attachment with presentation usage.
    ///
    /// # Returns
    ///
    /// A typed target accepted by [`Queue::submit_ir`], or an error when the
    /// logical texture cannot be represented by this image.
    pub fn map_ir_present_target<'r>(
        &self,
        resources: &'r ResourceTable,
        texture: TextureRef<'r>,
    ) -> Result<IrPresentTarget<'r, '_>, IrSubmitError> {
        let descriptor = resources.texture(texture)?;
        if descriptor.format() != TextureFormat::Bgra8Unorm {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::TargetFormat,
            ));
        }
        let supported_usage = TextureUsage::RENDER_ATTACHMENT | TextureUsage::PRESENT;
        if descriptor.usage() != supported_usage {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::TargetUsage,
            ));
        }
        let extent = descriptor.extent();
        if extent.width() != self.width || extent.height() != self.height {
            return Err(IrSubmitError::TargetExtentMismatch);
        }
        Ok(IrPresentTarget {
            resources,
            texture,
            image: self,
        })
    }
}

struct ExecutionPlan {
    clear_color: Color,
    pipeline: PipelineDesc,
    vertices: Vec<VertexClip4Color3>,
}

struct VertexDecodeRequest<'r, 'data> {
    resources: &'r ResourceTable,
    upload_buffer: BufferRef<'r>,
    upload_offset: u64,
    upload_data: &'data [u8],
    vertex_buffer: BufferRef<'r>,
    vertex_offset: u64,
    first_vertex: u32,
    vertex_count: u32,
}

impl Queue {
    /// Validate, lower, and synchronously submit one logical IR command buffer.
    ///
    /// The initial lowering path intentionally supports one explicit subset:
    /// one complete-target clear/store render pass containing one non-indexed
    /// vertex-color triangle-list draw. Unsupported valid IR features return an
    /// explicit [`IrSubmitError::Unsupported`] error before backend submission.
    ///
    /// # Arguments
    ///
    /// * `context` - Context used to materialize backend pipeline state.
    /// * `target` - Explicit mapping from the logical presentation texture to an image.
    /// * `commands` - Finished logical command buffer to execute.
    ///
    /// # Returns
    ///
    /// Success after synchronous backend submission, or a validation,
    /// unsupported-feature, allocation, or backend error.
    pub fn submit_ir<'r, 'image, 'data>(
        &self,
        context: &Context,
        target: IrPresentTarget<'r, 'image>,
        commands: &CommandBuffer<'r, 'data>,
    ) -> Result<(), IrSubmitError> {
        if !core::ptr::eq(target.resources, commands.resources()) {
            return Err(IrSubmitError::ResourceTableMismatch);
        }

        let plan = ExecutionPlan::from_commands(target, commands)?;
        let pipeline = context.create_pipeline(target.image, plan.pipeline)?;
        let viewport = Viewport::new(target.image.width, target.image.height);
        let mut render_pass = RenderPass::new(target.image, viewport, plan.clear_color);
        render_pass.draw_clip_space_vertex_color(&pipeline, &plan.vertices);
        self.submit(&render_pass).map_err(IrSubmitError::Backend)
    }
}

impl ExecutionPlan {
    fn from_commands<'r, 'image, 'data>(
        target: IrPresentTarget<'r, 'image>,
        commands: &CommandBuffer<'r, 'data>,
    ) -> Result<Self, IrSubmitError> {
        let stream = commands.commands();
        Self::reject_unimplemented_commands(stream)?;
        Self::validate_command_counts(stream)?;
        if stream.len() != 7 {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::CommandSequence,
            ));
        }

        let (upload_buffer, upload_offset, upload_data) = match &stream[0] {
            Command::WriteBuffer {
                buffer,
                offset,
                data,
            } => (*buffer, *offset, *data),
            _ => return Err(Self::unsupported_sequence()),
        };
        let pass = match &stream[1] {
            Command::BeginRenderPass(pass) => *pass,
            _ => return Err(Self::unsupported_sequence()),
        };
        let pipeline = match &stream[2] {
            Command::SetPipeline(pipeline) => *pipeline,
            _ => return Err(Self::unsupported_sequence()),
        };
        let (vertex_buffer, vertex_offset) = match &stream[3] {
            Command::SetVertexBuffer { buffer, offset } => (*buffer, *offset),
            _ => return Err(Self::unsupported_sequence()),
        };
        let uniforms = match &stream[4] {
            Command::SetUniforms(uniforms) => *uniforms,
            _ => return Err(Self::unsupported_sequence()),
        };
        let (vertex_count, first_vertex) = match &stream[5] {
            Command::Draw {
                vertex_count,
                first_vertex,
            } => (*vertex_count, *first_vertex),
            _ => return Err(Self::unsupported_sequence()),
        };
        if !matches!(stream[6], Command::EndRenderPass) {
            return Err(Self::unsupported_sequence());
        }

        let clear_color = Self::validate_pass(target, pass)?;
        let pipeline_description =
            Self::validate_pipeline(target.resources, pipeline, vertex_count)?;
        Self::validate_uniforms(uniforms)?;
        let vertices = Self::decode_vertices(VertexDecodeRequest {
            resources: target.resources,
            upload_buffer,
            upload_offset,
            upload_data,
            vertex_buffer,
            vertex_offset,
            first_vertex,
            vertex_count,
        })?;
        Ok(Self {
            clear_color,
            pipeline: pipeline_description,
            vertices,
        })
    }

    fn reject_unimplemented_commands(stream: &[Command<'_, '_>]) -> Result<(), IrSubmitError> {
        for command in stream {
            let unsupported = match command {
                Command::WriteTexture { .. } => Some(UnsupportedIrFeature::TextureUpload),
                Command::CopyTextureToTexture { .. } => Some(UnsupportedIrFeature::TextureCopy),
                Command::SetIndexBuffer { .. } | Command::DrawIndexed { .. } => {
                    Some(UnsupportedIrFeature::IndexedDrawing)
                }
                Command::SetTexture(_) => Some(UnsupportedIrFeature::TextureSampling),
                Command::SetSampler(_) => Some(UnsupportedIrFeature::Sampler),
                Command::SetScissor(_) => Some(UnsupportedIrFeature::Scissor),
                _ => None,
            };
            if let Some(feature) = unsupported {
                return Err(IrSubmitError::Unsupported(feature));
            }
        }
        Ok(())
    }

    fn validate_command_counts(stream: &[Command<'_, '_>]) -> Result<(), IrSubmitError> {
        let uploads = stream
            .iter()
            .filter(|command| matches!(command, Command::WriteBuffer { .. }))
            .count();
        let passes = stream
            .iter()
            .filter(|command| matches!(command, Command::BeginRenderPass(_)))
            .count();
        let draws = stream
            .iter()
            .filter(|command| matches!(command, Command::Draw { .. }))
            .count();
        if uploads != 1 {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::MultipleBufferUploads,
            ));
        }
        if passes != 1 {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::MultipleRenderPasses,
            ));
        }
        if draws != 1 {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::MultipleDraws,
            ));
        }
        Ok(())
    }

    fn validate_pass(
        target: IrPresentTarget<'_, '_>,
        pass: RenderPassDesc<'_>,
    ) -> Result<Color, IrSubmitError> {
        if pass.target() != target.texture {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::MultipleRenderPasses,
            ));
        }
        let area = pass.area();
        if area.x() != 0
            || area.y() != 0
            || area.width() != target.image.width
            || area.height() != target.image.height
        {
            return Err(IrSubmitError::Unsupported(UnsupportedIrFeature::RenderArea));
        }
        let clear = match pass.load() {
            LoadOp::Clear(color) => color,
            LoadOp::Load | LoadOp::DontCare => {
                return Err(IrSubmitError::Unsupported(
                    UnsupportedIrFeature::LoadOperation,
                ));
            }
        };
        if pass.store() != StoreOp::Store {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::StoreOperation,
            ));
        }
        let [red, green, blue, alpha] = clear.components();
        if ![red, green, blue, alpha]
            .iter()
            .all(|component| (0.0..=1.0).contains(component))
        {
            return Err(IrSubmitError::Unsupported(UnsupportedIrFeature::ClearColor));
        }
        Ok(Color::rgba(red, green, blue, alpha))
    }

    fn validate_pipeline(
        resources: &ResourceTable,
        pipeline: RenderPipelineRef<'_>,
        vertex_count: u32,
    ) -> Result<PipelineDesc, IrSubmitError> {
        let (target_format, topology, layout_valid, fragment, blend, raster) = resources
            .with_pipeline(pipeline, |descriptor| {
                let attributes = descriptor.vertex_buffer().attributes();
                let layout_valid = usize::try_from(descriptor.vertex_buffer().stride()).ok()
                    == Some(size_of::<VertexClip4Color3>())
                    && attributes.len() == 2
                    && attributes[0].location() == 0
                    && attributes[0].format() == VertexFormat::Float32x4
                    && attributes[0].offset() == 0
                    && attributes[1].location() == 1
                    && attributes[1].format() == VertexFormat::Float32x3
                    && attributes[1].offset() == 16;
                (
                    descriptor.target_format(),
                    descriptor.topology(),
                    layout_valid,
                    descriptor.fragment(),
                    descriptor.blend(),
                    descriptor.raster(),
                )
            })?;
        if target_format != TextureFormat::Bgra8Unorm {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::PipelineTargetFormat,
            ));
        }
        if topology != PrimitiveTopology::TriangleList {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::PrimitiveTopology,
            ));
        }
        if !layout_valid {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::VertexLayout,
            ));
        }
        if fragment != FragmentProgram::VertexColor {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::FragmentProgram,
            ));
        }
        if blend != BlendState::REPLACE {
            return Err(IrSubmitError::Unsupported(UnsupportedIrFeature::BlendState));
        }
        let max_vertices =
            usize::try_from(vertex_count).map_err(|_| IrSubmitError::InvalidVertexData)?;
        let cull_mode = match raster.cull_mode() {
            ir::CullMode::None => CullMode::None,
            ir::CullMode::Front => CullMode::Front,
            ir::CullMode::Back => CullMode::Back,
        };
        let front_face = match raster.front_face() {
            ir::FrontFace::Clockwise => FrontFace::Clockwise,
            ir::FrontFace::CounterClockwise => FrontFace::CounterClockwise,
        };
        Ok(PipelineDesc::clip_space_vertex_color(max_vertices)
            .with_cull_mode(cull_mode)
            .with_front_face(front_face))
    }

    fn validate_uniforms(uniforms: DrawUniforms) -> Result<(), IrSubmitError> {
        if uniforms.transform() != ir::Transform::identity()
            || uniforms.color().components() != [1.0, 1.0, 1.0, 1.0]
        {
            Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::DrawUniforms,
            ))
        } else {
            Ok(())
        }
    }

    fn decode_vertices(
        request: VertexDecodeRequest<'_, '_>,
    ) -> Result<Vec<VertexClip4Color3>, IrSubmitError> {
        let VertexDecodeRequest {
            resources,
            upload_buffer,
            upload_offset,
            upload_data,
            vertex_buffer,
            vertex_offset,
            first_vertex,
            vertex_count,
        } = request;
        if upload_buffer != vertex_buffer || upload_offset != 0 {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::VertexBuffer,
            ));
        }
        let descriptor = resources.buffer(vertex_buffer)?;
        let supported_usage = BufferUsage::VERTEX | BufferUsage::COPY_DST;
        let upload_len =
            u64::try_from(upload_data.len()).map_err(|_| IrSubmitError::InvalidVertexData)?;
        if descriptor.usage() != supported_usage || descriptor.size() != upload_len {
            return Err(IrSubmitError::Unsupported(
                UnsupportedIrFeature::VertexBuffer,
            ));
        }

        let stride = size_of::<VertexClip4Color3>();
        let first = usize::try_from(first_vertex).map_err(|_| IrSubmitError::InvalidVertexData)?;
        let count = usize::try_from(vertex_count).map_err(|_| IrSubmitError::InvalidVertexData)?;
        let base = usize::try_from(vertex_offset)
            .ok()
            .and_then(|offset| {
                first
                    .checked_mul(stride)
                    .and_then(|first_bytes| offset.checked_add(first_bytes))
            })
            .ok_or(IrSubmitError::InvalidVertexData)?;
        let byte_len = count
            .checked_mul(stride)
            .ok_or(IrSubmitError::InvalidVertexData)?;
        let end = base
            .checked_add(byte_len)
            .ok_or(IrSubmitError::InvalidVertexData)?;
        let selected = upload_data
            .get(base..end)
            .ok_or(IrSubmitError::InvalidVertexData)?;

        let mut vertices = Vec::new();
        vertices
            .try_reserve_exact(count)
            .map_err(|_| IrSubmitError::OutOfMemory)?;
        for encoded in selected.chunks_exact(stride) {
            let mut cursor = 0usize;
            let clip_position = [
                Self::read_f32(encoded, &mut cursor)?,
                Self::read_f32(encoded, &mut cursor)?,
                Self::read_f32(encoded, &mut cursor)?,
                Self::read_f32(encoded, &mut cursor)?,
            ];
            let color = [
                Self::read_f32(encoded, &mut cursor)?,
                Self::read_f32(encoded, &mut cursor)?,
                Self::read_f32(encoded, &mut cursor)?,
            ];
            if !clip_position.iter().all(|component| component.is_finite())
                || !color
                    .iter()
                    .all(|component| component.is_finite() && (0.0..=1.0).contains(component))
            {
                return Err(IrSubmitError::InvalidVertexData);
            }
            vertices.push(VertexClip4Color3::new(clip_position, color));
        }
        if vertices.len() != count {
            return Err(IrSubmitError::InvalidVertexData);
        }
        Ok(vertices)
    }

    fn read_f32(bytes: &[u8], cursor: &mut usize) -> Result<f32, IrSubmitError> {
        let end = cursor
            .checked_add(size_of::<f32>())
            .ok_or(IrSubmitError::InvalidVertexData)?;
        let encoded = bytes
            .get(*cursor..end)
            .ok_or(IrSubmitError::InvalidVertexData)?;
        let encoded: [u8; 4] = encoded
            .try_into()
            .map_err(|_| IrSubmitError::InvalidVertexData)?;
        *cursor = end;
        Ok(f32::from_le_bytes(encoded))
    }

    const fn unsupported_sequence() -> IrSubmitError {
        IrSubmitError::Unsupported(UnsupportedIrFeature::CommandSequence)
    }
}
