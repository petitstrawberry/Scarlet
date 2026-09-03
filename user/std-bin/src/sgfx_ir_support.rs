//! Private helpers for driving the modern SGFX frontend from Scarlet binaries.

use std::rc::Rc;
use std::vec::Vec;
use std::{error, fmt};

use framebuffer::{DisplayPresentRegion, DisplaySurface};
use scarlet_os::handle::Handle;
use sgfx::backend::CommandExecutor;
use sgfx::ir::{
    self, AddressMode, BlendState, BufferDesc, BufferId, BufferUsage, CommandEncoder, DrawUniforms,
    Extent2D, FilterMode, FragmentProgram, LoadOp, PixelRect, PrimitiveTopology, RasterState,
    RenderPassDesc, RenderPipelineDesc, RenderPipelineId, ResourceTable, SamplerDesc, SamplerId,
    StoreOp, TextureDesc, TextureFormat, TextureId, TextureSampleMode, TextureUsage, TextureWrite,
    Transform, VertexAttribute, VertexBufferLayout, VertexFormat,
};
use sgfx::{Context, Instance, MappedTargetSession};

const QUAD_VERTEX_STRIDE: u32 = 24;
const QUAD_VERTEX_COUNT: usize = 6;
const MAX_COMMANDS_PER_QUAD: usize = 7;
const QUAD_BATCH_COMMAND_OVERHEAD: usize = 3;
const MAX_QUADS_PER_COMMAND_BUFFER: usize =
    (ir::MAX_COMMANDS - QUAD_BATCH_COMMAND_OVERHEAD) / MAX_COMMANDS_PER_QUAD;

#[derive(Debug)]
pub(crate) enum Error {
    Frontend(sgfx::Error),
    Ir(ir::Error),
    UnsupportedCapabilities,
}

impl From<sgfx::Error> for Error {
    fn from(error: sgfx::Error) -> Self {
        Self::Frontend(error)
    }
}

impl From<ir::Error> for Error {
    fn from(error: ir::Error) -> Self {
        Self::Ir(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frontend(error) => write!(formatter, "SGFX frontend failed: {error}"),
            Self::Ir(error) => write!(formatter, "SGFX IR validation failed: {error:?}"),
            Self::UnsupportedCapabilities => {
                formatter.write_str("SGFX device lacks required mapped-target capabilities")
            }
        }
    }
}

impl error::Error for Error {}

/// One directly selected frontend context with a mapped presentation target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReusableImport {
    texture: TextureId,
    width: u32,
    height: u32,
}

fn take_reusable_import(
    imports: &mut Vec<ReusableImport>,
    width: u32,
    height: u32,
) -> Option<ReusableImport> {
    let index = imports
        .iter()
        .position(|entry| entry.width == width && entry.height == height)?;
    Some(imports.swap_remove(index))
}

pub(crate) struct MappedTarget {
    pub(crate) resources: Rc<ResourceTable>,
    pub(crate) texture: TextureId,
    pub(crate) width: u32,
    pub(crate) height: u32,
    presented_texture: Option<TextureId>,
    spare_texture: Option<TextureId>,
    current_initialized: bool,
    spare_initialized: bool,
    previous_damage: Option<PixelRect>,
    pending_damage: Option<PixelRect>,
    session: MappedTargetSession,
    reusable_imports: Vec<ReusableImport>,
}

impl MappedTarget {
    pub(crate) fn open(width: u32, height: u32) -> Result<Self, Error> {
        Self::open_with_target_count(width, height, 1, false)
    }

    /// Open a two-image presentation target for tear-free direct scanout.
    pub(crate) fn open_swapchain(width: u32, height: u32) -> Result<Self, Error> {
        // Readback is an optional remote-capture capability, not a prerequisite
        // for local GPU composition or presentation. Backends such as native
        // Adreno can therefore keep the desktop on the GPU while an attempted
        // capture reports its own unsupported readback operation.
        Self::open_with_target_count(width, height, 2, false)
    }

    fn open_with_target_count(
        width: u32,
        height: u32,
        target_count: usize,
        require_readback: bool,
    ) -> Result<Self, Error> {
        let instance = Instance::new()?;
        let device = instance.open_device("/dev/gpu0")?;
        let capabilities = device.capabilities();
        if !supports_mapped_target(
            capabilities.supports_rendering(),
            capabilities.supports_presentation(),
            capabilities.supports_image_upload(),
        ) || (require_readback && !capabilities.supports_image_readback())
        {
            return Err(Error::UnsupportedCapabilities);
        }
        let context = device.create_context()?;
        Self::from_context(context, width, height, target_count)
    }

    fn from_context(
        context: Context,
        width: u32,
        height: u32,
        target_count: usize,
    ) -> Result<Self, Error> {
        let resources = Rc::new(ResourceTable::new());
        let extent = Extent2D::new(width, height)?;
        let define_target = || -> Result<TextureId, Error> {
            Ok(resources
                .define_texture(TextureDesc::new(
                    TextureFormat::Bgra8Unorm,
                    extent,
                    TextureUsage::RENDER_ATTACHMENT
                        | TextureUsage::PRESENT
                        | TextureUsage::COPY_SRC
                        | TextureUsage::COPY_DST,
                )?)?
                .id())
        };
        let texture = define_target()?;
        let spare_texture = match target_count {
            1 => None,
            2 => Some(define_target()?),
            _ => return Err(ir::Error::InvalidValue.into()),
        };
        let mut targets = Vec::with_capacity(target_count);
        targets.push(texture);
        if let Some(spare) = spare_texture {
            targets.push(spare);
        }
        let session = context.create_mapped_target_session(Rc::clone(&resources), &targets)?;
        Ok(Self {
            resources,
            texture,
            width,
            height,
            presented_texture: None,
            spare_texture,
            current_initialized: false,
            spare_initialized: false,
            previous_damage: None,
            pending_damage: None,
            session,
            reusable_imports: Vec::new(),
        })
    }

    /// Expand logical damage by the age of the current swapchain image.
    pub(crate) fn prepare_render_area(&mut self, requested: PixelRect) -> Result<PixelRect, Error> {
        let full = PixelRect::new(0, 0, self.width, self.height)?;
        if requested.x().saturating_add(requested.width()) > self.width
            || requested.y().saturating_add(requested.height()) > self.height
        {
            return Err(ir::Error::InvalidValue.into());
        }
        self.pending_damage = Some(match self.pending_damage {
            Some(pending) => union_pixel_rect(pending, requested)?,
            None => requested,
        });
        if self.spare_texture.is_none() {
            return Ok(requested);
        }
        if !self.current_initialized {
            return Ok(full);
        }
        match self.previous_damage {
            Some(previous) => union_pixel_rect(previous, requested).map_err(Into::into),
            None => Ok(full),
        }
    }

    fn finish_present(&mut self) -> Result<(), Error> {
        let Some(mut spare) = self.spare_texture else {
            self.pending_damage = None;
            return Ok(());
        };
        let full = PixelRect::new(0, 0, self.width, self.height)?;
        let logical_damage = self.pending_damage.take().unwrap_or(full);
        self.current_initialized = true;
        self.previous_damage = Some(logical_damage);
        core::mem::swap(&mut self.texture, &mut spare);
        self.spare_texture = Some(spare);
        core::mem::swap(&mut self.current_initialized, &mut self.spare_initialized);
        Ok(())
    }

    pub(crate) fn execute(
        &mut self,
        commands: &ir::CommandBuffer<'_, '_>,
    ) -> Result<(), sgfx::Error> {
        self.session.executor().execute(commands)
    }

    pub(crate) fn import_shared_bgra_texture(
        &mut self,
        width: u32,
        height: u32,
        handle: Handle,
    ) -> Result<TextureId, Error> {
        if let Some(reusable) = take_reusable_import(&mut self.reusable_imports, width, height) {
            if let Err(error) = self
                .session
                .import_shared_bgra_texture(reusable.texture, handle)
            {
                self.reusable_imports.push(reusable);
                return Err(error.into());
            }
            return Ok(reusable.texture);
        }
        let texture = self
            .resources
            .define_texture(TextureDesc::new(
                TextureFormat::Bgra8Unorm,
                Extent2D::new(width, height)?,
                TextureUsage::SAMPLED | TextureUsage::COPY_SRC,
            )?)?
            .id();
        self.session.import_shared_bgra_texture(texture, handle)?;
        Ok(texture)
    }

    pub(crate) fn release_imported_texture(&mut self, texture: TextureId) -> Result<(), Error> {
        let reference = self.resources.texture_ref(texture)?;
        let descriptor = self.resources.texture(reference)?;
        let width = descriptor.extent().width();
        let height = descriptor.extent().height();
        self.session.release_imported_texture(texture)?;
        self.reusable_imports.push(ReusableImport {
            texture,
            width,
            height,
        });
        Ok(())
    }

    pub(crate) fn present(
        &mut self,
        display: &DisplaySurface,
        region: Option<DisplayPresentRegion>,
    ) -> Result<(), &'static str> {
        let presented_texture = self.texture;
        let image = self
            .session
            .image(presented_texture)
            .map_err(|_| "Failed to resolve mapped SGFX target")?;
        if self.spare_texture.is_some() {
            display
                .present_swapchain_image(image.shared_handle(), region)
                .map_err(|_| "Failed to present mapped SGFX swapchain target")?;
        } else {
            display
                .present_image(image.shared_handle(), region)
                .map_err(|_| "Failed to present mapped SGFX target")?;
        }
        self.finish_present()
            .map_err(|_| "Failed to advance mapped SGFX swapchain")?;
        self.presented_texture = Some(presented_texture);
        Ok(())
    }

    /// Read damaged regions from the most recently presented target.
    pub(crate) fn readback_bgra(
        &self,
        destination: &mut [u8],
        destination_stride: u32,
        damage: &[PixelRect],
    ) -> Result<(), &'static str> {
        let texture = self
            .presented_texture
            .ok_or("SGFX target has not presented a capturable frame")?;
        for rect in damage {
            self.session
                .readback_bgra(texture, destination, destination_stride, *rect)
                .map_err(|_| "Failed to read back the SGFX presentation target")?;
        }
        Ok(())
    }
}

fn union_pixel_rect(left: PixelRect, right: PixelRect) -> ir::Result<PixelRect> {
    let x = left.x().min(right.x());
    let y = left.y().min(right.y());
    let right_edge = left
        .x()
        .checked_add(left.width())
        .and_then(|edge| {
            edge.max(right.x().checked_add(right.width())?)
                .checked_sub(x)
        })
        .ok_or(ir::Error::Overflow)?;
    let bottom_edge = left
        .y()
        .checked_add(left.height())
        .and_then(|edge| {
            edge.max(right.y().checked_add(right.height())?)
                .checked_sub(y)
        })
        .ok_or(ir::Error::Overflow)?;
    PixelRect::new(x, y, right_edge, bottom_edge)
}

const fn supports_mapped_target(rendering: bool, presentation: bool, image_upload: bool) -> bool {
    rendering && presentation && image_upload
}

#[derive(Clone, Copy)]
pub(crate) struct SampledRect {
    pub(crate) texture: TextureId,
    pub(crate) texture_width: u32,
    pub(crate) texture_height: u32,
    pub(crate) destination: PixelRect,
    pub(crate) source: PixelRect,
    pub(crate) tint: ir::Color,
    pub(crate) ignore_source_alpha: bool,
    pub(crate) clip: Option<PixelRect>,
}

/// One opaque, unscaled texture rectangle that can bypass the 3D sampler.
#[derive(Clone, Copy)]
pub(crate) struct CopiedRect {
    pub(crate) texture: TextureId,
    pub(crate) destination: PixelRect,
    pub(crate) source: PixelRect,
    pub(crate) clip: Option<PixelRect>,
}

#[derive(Clone, Copy)]
pub(crate) enum Quad {
    Solid {
        destination: PixelRect,
        color: ir::Color,
        clip: Option<PixelRect>,
    },
    Sampled(SampledRect),
    Copy(CopiedRect),
}

/// One CPU-backed texture update recorded before scene composition.
pub(crate) struct TextureUpload<'a> {
    pub(crate) texture: TextureId,
    pub(crate) destination: PixelRect,
    pub(crate) stride: u32,
    pub(crate) bytes: &'a [u8],
}

/// Failure while recording or executing one quad-composition submission.
#[derive(Debug)]
pub(crate) enum QuadSubmitError {
    /// Portable IR recording rejected the requested frame.
    Recording(&'static str),
    /// The selected SGFX backend failed while executing valid recorded IR.
    Execution(sgfx::Error),
}

impl From<&'static str> for QuadSubmitError {
    fn from(error: &'static str) -> Self {
        Self::Recording(error)
    }
}

impl fmt::Display for QuadSubmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Recording(error) => formatter.write_str(error),
            Self::Execution(error) => write!(formatter, "SGFX execution failed: {error}"),
        }
    }
}

pub(crate) struct QuadRenderer {
    buffer: BufferId,
    sampler: SamplerId,
    solid_pipeline: RenderPipelineId,
    rgba_pipeline: RenderPipelineId,
    opaque_pipeline: RenderPipelineId,
    capacity: usize,
}

impl QuadRenderer {
    pub(crate) fn define(resources: &ResourceTable, capacity: usize) -> ir::Result<Self> {
        let vertex_count = capacity
            .checked_mul(QUAD_VERTEX_COUNT)
            .ok_or(ir::Error::Overflow)?;
        let buffer_size = u64::try_from(vertex_count)
            .ok()
            .and_then(|count| count.checked_mul(u64::from(QUAD_VERTEX_STRIDE)))
            .ok_or(ir::Error::Overflow)?;
        let buffer = resources
            .define_buffer(BufferDesc::new(
                buffer_size,
                BufferUsage::VERTEX | BufferUsage::COPY_DST,
            )?)?
            .id();
        let sampler = resources
            .define_sampler(SamplerDesc::new(
                FilterMode::Linear,
                FilterMode::Linear,
                AddressMode::ClampToEdge,
                AddressMode::ClampToEdge,
            ))?
            .id();
        let layout = VertexBufferLayout::new(
            QUAD_VERTEX_STRIDE,
            vec![
                VertexAttribute::new(0, VertexFormat::Float32x4, 0),
                VertexAttribute::new(1, VertexFormat::Float32x2, 16),
            ],
        )?;
        let raster = RasterState::new(ir::CullMode::None, ir::FrontFace::CounterClockwise);
        let define_pipeline = |fragment| -> ir::Result<RenderPipelineId> {
            Ok(resources
                .define_render_pipeline(RenderPipelineDesc::new(
                    TextureFormat::Bgra8Unorm,
                    PrimitiveTopology::TriangleList,
                    layout.clone(),
                    fragment,
                    BlendState::SOURCE_OVER_STRAIGHT_ALPHA,
                    raster,
                )?)?
                .id())
        };
        Ok(Self {
            buffer,
            sampler,
            solid_pipeline: define_pipeline(FragmentProgram::Solid)?,
            rgba_pipeline: define_pipeline(FragmentProgram::Texture(TextureSampleMode::Rgba))?,
            opaque_pipeline: define_pipeline(FragmentProgram::Texture(
                TextureSampleMode::RgbIgnoreAlpha,
            ))?,
            capacity,
        })
    }

    pub(crate) fn submit(
        &self,
        target: &mut MappedTarget,
        load: LoadOp,
        operations: &[Quad],
    ) -> Result<(), QuadSubmitError> {
        let area = PixelRect::new(0, 0, target.width, target.height)
            .map_err(|_| "Invalid SGFX target area")?;
        self.submit_region(target, area, load, operations)
    }

    /// Submit quads while limiting render-target work to one damaged region.
    pub(crate) fn submit_region(
        &self,
        target: &mut MappedTarget,
        area: PixelRect,
        load: LoadOp,
        operations: &[Quad],
    ) -> Result<(), QuadSubmitError> {
        self.submit_region_with_uploads(target, area, load, &[], operations)
    }

    /// Upload CPU-backed textures and compose one damaged target region.
    ///
    /// Keeping uploads in the first composition command buffer removes a
    /// synchronous executor round trip from CPU-rendered clients such as
    /// Wayland SHM applications.
    pub(crate) fn submit_region_with_uploads(
        &self,
        target: &mut MappedTarget,
        area: PixelRect,
        load: LoadOp,
        uploads: &[TextureUpload<'_>],
        operations: &[Quad],
    ) -> Result<(), QuadSubmitError> {
        if operations.len() > self.capacity {
            return Err(QuadSubmitError::Recording(
                "SGFX composition operation capacity exceeded",
            ));
        }
        if uploads.len().saturating_add(QUAD_BATCH_COMMAND_OVERHEAD) > ir::MAX_COMMANDS {
            return Err(QuadSubmitError::Recording(
                "SGFX texture upload command capacity exceeded",
            ));
        }
        let resources = Rc::clone(&target.resources);
        let mut vertices = Vec::new();
        vertices
            .try_reserve_exact(
                operations
                    .len()
                    .checked_mul(QUAD_VERTEX_COUNT)
                    .and_then(|count| count.checked_mul(QUAD_VERTEX_STRIDE as usize))
                    .ok_or("SGFX composition vertex size overflow")?,
            )
            .map_err(|_| "Failed to reserve SGFX composition vertices")?;
        for operation in operations {
            let (destination, source, texture_width, texture_height) = match operation {
                Quad::Solid { destination, .. } => (
                    *destination,
                    PixelRect::new(0, 0, 1, 1).map_err(|_| "Invalid solid quad")?,
                    1,
                    1,
                ),
                Quad::Sampled(rect) => (
                    rect.destination,
                    rect.source,
                    rect.texture_width,
                    rect.texture_height,
                ),
                Quad::Copy(_) => {
                    // Keep one fixed vertex slot per operation so later draw
                    // offsets remain stable across copy/render segmentation.
                    vertices.resize(
                        vertices
                            .len()
                            .checked_add(QUAD_VERTEX_COUNT * QUAD_VERTEX_STRIDE as usize)
                            .ok_or("SGFX composition vertex size overflow")?,
                        0,
                    );
                    continue;
                }
            };
            append_quad(
                &mut vertices,
                destination,
                source,
                target.width,
                target.height,
                texture_width,
                texture_height,
            );
        }

        let mut batch_start = 0usize;
        let mut first_batch = true;
        loop {
            let batch_capacity = if first_batch {
                (ir::MAX_COMMANDS
                    .saturating_sub(QUAD_BATCH_COMMAND_OVERHEAD)
                    .saturating_sub(uploads.len()))
                    / MAX_COMMANDS_PER_QUAD
            } else {
                MAX_QUADS_PER_COMMAND_BUFFER
            };
            if batch_capacity == 0 && !operations.is_empty() {
                return Err(QuadSubmitError::Recording(
                    "SGFX composition has no command capacity after texture uploads",
                ));
            }
            let batch_end = if operations.is_empty() {
                0
            } else {
                batch_start
                    .saturating_add(batch_capacity)
                    .min(operations.len())
            };
            let mut encoder = CommandEncoder::new(resources.as_ref());
            if first_batch {
                for upload in uploads {
                    encoder
                        .write_texture(
                            resources
                                .texture_ref(upload.texture)
                                .map_err(|_| "Invalid SGFX upload texture")?,
                            TextureWrite::new(upload.destination, upload.stride, upload.bytes)
                                .map_err(|_| "Invalid SGFX texture upload")?,
                        )
                        .map_err(|_| "Failed to record SGFX texture upload")?;
                }
            }
            if first_batch && !vertices.is_empty() {
                encoder
                    .write_buffer(
                        resources
                            .buffer_ref(self.buffer)
                            .map_err(|_| "Invalid quad buffer")?,
                        0,
                        &vertices,
                    )
                    .map_err(|_| "Failed to upload SGFX composition vertices")?;
            }
            self.record_operations(
                &mut encoder,
                resources.as_ref(),
                target.texture,
                area,
                if first_batch { load } else { LoadOp::Load },
                &operations[batch_start..batch_end],
                batch_start,
            )?;
            let commands = encoder
                .finish()
                .map_err(|_| "Failed to finish SGFX composition commands")?;
            target
                .execute(&commands)
                .map_err(QuadSubmitError::Execution)?;
            if batch_end == operations.len() {
                break;
            }
            batch_start = batch_end;
            first_batch = false;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn record_operations<'r, 'data>(
        &self,
        encoder: &mut CommandEncoder<'r, 'data>,
        resources: &'r ResourceTable,
        target: TextureId,
        area: PixelRect,
        load: LoadOp,
        operations: &[Quad],
        base_index: usize,
    ) -> Result<(), &'static str> {
        let mut segment_start = 0usize;
        let mut first_pass = true;
        for (index, operation) in operations.iter().enumerate() {
            let Quad::Copy(copy) = operation else {
                continue;
            };
            if first_pass || segment_start < index {
                self.record_pass(
                    encoder,
                    resources,
                    target,
                    area,
                    if first_pass { load } else { LoadOp::Load },
                    &operations[segment_start..index],
                    base_index.saturating_add(segment_start),
                )?;
                first_pass = false;
            }
            if let Some((source, destination)) = clipped_copy_rect(*copy, area)? {
                encoder
                    .copy_texture_to_texture(
                        resources
                            .texture_ref(copy.texture)
                            .map_err(|_| "Invalid copy source texture")?,
                        source,
                        resources
                            .texture_ref(target)
                            .map_err(|_| "Invalid SGFX target texture")?,
                        destination,
                    )
                    .map_err(|_| "Failed to record SGFX composition copy")?;
            }
            segment_start = index + 1;
        }
        if first_pass || segment_start < operations.len() {
            self.record_pass(
                encoder,
                resources,
                target,
                area,
                if first_pass { load } else { LoadOp::Load },
                &operations[segment_start..],
                base_index.saturating_add(segment_start),
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn record_pass<'r, 'data>(
        &self,
        encoder: &mut CommandEncoder<'r, 'data>,
        resources: &'r ResourceTable,
        target: TextureId,
        area: PixelRect,
        load: LoadOp,
        operations: &[Quad],
        base_index: usize,
    ) -> Result<(), &'static str> {
        let descriptor = RenderPassDesc::new(
            resources,
            resources
                .texture_ref(target)
                .map_err(|_| "Invalid SGFX target texture")?,
            area,
            load,
            StoreOp::Store,
        )
        .map_err(|_| "Invalid SGFX composition pass")?;
        let mut pass = encoder
            .begin_render_pass(descriptor)
            .map_err(|_| "Failed to begin SGFX composition pass")?;
        for (local_index, operation) in operations.iter().enumerate() {
            let requested_clip = match operation {
                Quad::Solid { clip, .. } => *clip,
                Quad::Sampled(rect) => rect.clip,
                Quad::Copy(_) => return Err("SGFX copy leaked into a render segment"),
            };
            let effective_clip = match requested_clip {
                Some(clip) => {
                    let Some(clip) = intersect_pixel_rect(clip, area)? else {
                        continue;
                    };
                    Some(clip)
                }
                None => None,
            };
            let index = base_index
                .checked_add(local_index)
                .ok_or("SGFX quad offset overflow")?;
            let byte_offset = u64::try_from(index * QUAD_VERTEX_COUNT)
                .ok()
                .and_then(|vertex| vertex.checked_mul(u64::from(QUAD_VERTEX_STRIDE)))
                .ok_or("SGFX quad offset overflow")?;
            pass.set_vertex_buffer(
                resources
                    .buffer_ref(self.buffer)
                    .map_err(|_| "Invalid quad buffer")?,
                byte_offset,
            )
            .map_err(|_| "Failed to bind SGFX composition vertices")?;
            match operation {
                Quad::Solid { color, .. } => {
                    pass.set_pipeline(
                        resources
                            .render_pipeline_ref(self.solid_pipeline)
                            .map_err(|_| "Invalid solid pipeline")?,
                    )
                    .map_err(|_| "Failed to bind solid pipeline")?;
                    pass.set_uniforms(DrawUniforms::new(Transform::identity(), *color))
                        .map_err(|_| "Failed to set solid uniforms")?;
                    pass.set_scissor(effective_clip)
                        .map_err(|_| "Failed to set solid scissor")?;
                }
                Quad::Sampled(rect) => {
                    let pipeline = if rect.ignore_source_alpha {
                        self.opaque_pipeline
                    } else {
                        self.rgba_pipeline
                    };
                    pass.set_pipeline(
                        resources
                            .render_pipeline_ref(pipeline)
                            .map_err(|_| "Invalid texture pipeline")?,
                    )
                    .map_err(|_| "Failed to bind texture pipeline")?;
                    pass.set_texture(
                        resources
                            .texture_ref(rect.texture)
                            .map_err(|_| "Invalid sampled texture")?,
                    )
                    .map_err(|_| "Failed to bind sampled texture")?;
                    pass.set_sampler(
                        resources
                            .sampler_ref(self.sampler)
                            .map_err(|_| "Invalid composition sampler")?,
                    )
                    .map_err(|_| "Failed to bind composition sampler")?;
                    pass.set_uniforms(DrawUniforms::new(Transform::identity(), rect.tint))
                        .map_err(|_| "Failed to set texture uniforms")?;
                    pass.set_scissor(effective_clip)
                        .map_err(|_| "Failed to set texture scissor")?;
                }
                Quad::Copy(_) => return Err("SGFX copy leaked into a render segment"),
            }
            pass.draw(QUAD_VERTEX_COUNT as u32, 0)
                .map_err(|_| "Failed to record SGFX quad")?;
        }
        pass.end()
            .map_err(|_| "Failed to end SGFX composition pass")
    }
}

fn clipped_copy_rect(
    copy: CopiedRect,
    render_area: PixelRect,
) -> Result<Option<(PixelRect, PixelRect)>, &'static str> {
    let Some(mut destination) = intersect_pixel_rect(copy.destination, render_area)? else {
        return Ok(None);
    };
    if let Some(clip) = copy.clip {
        let Some(clipped) = intersect_pixel_rect(destination, clip)? else {
            return Ok(None);
        };
        destination = clipped;
    }
    let source_x = copy
        .source
        .x()
        .checked_add(destination.x() - copy.destination.x())
        .ok_or("SGFX composition copy source overflow")?;
    let source_y = copy
        .source
        .y()
        .checked_add(destination.y() - copy.destination.y())
        .ok_or("SGFX composition copy source overflow")?;
    let source = PixelRect::new(
        source_x,
        source_y,
        destination.width(),
        destination.height(),
    )
    .map_err(|_| "Invalid SGFX composition copy source")?;
    Ok(Some((source, destination)))
}

fn intersect_pixel_rect(
    left: PixelRect,
    right: PixelRect,
) -> Result<Option<PixelRect>, &'static str> {
    let x = left.x().max(right.x());
    let y = left.y().max(right.y());
    let right_edge = left
        .x()
        .checked_add(left.width())
        .and_then(|left_edge| {
            right
                .x()
                .checked_add(right.width())
                .map(|right_edge| left_edge.min(right_edge))
        })
        .ok_or("SGFX composition rectangle overflow")?;
    let bottom_edge = left
        .y()
        .checked_add(left.height())
        .and_then(|left_edge| {
            right
                .y()
                .checked_add(right.height())
                .map(|right_edge| left_edge.min(right_edge))
        })
        .ok_or("SGFX composition rectangle overflow")?;
    if right_edge <= x || bottom_edge <= y {
        return Ok(None);
    }
    PixelRect::new(x, y, right_edge - x, bottom_edge - y)
        .map(Some)
        .map_err(|_| "Invalid SGFX composition intersection")
}

pub(crate) fn define_bgra_texture(
    resources: &ResourceTable,
    width: u32,
    height: u32,
) -> ir::Result<TextureId> {
    Ok(resources
        .define_texture(TextureDesc::new(
            TextureFormat::Bgra8Unorm,
            Extent2D::new(width, height)?,
            TextureUsage::SAMPLED | TextureUsage::COPY_SRC | TextureUsage::COPY_DST,
        )?)?
        .id())
}

pub(crate) fn upload_bgra(
    target: &mut MappedTarget,
    texture: TextureId,
    destination: PixelRect,
    stride: u32,
    bytes: &[u8],
) -> Result<(), &'static str> {
    let resources = Rc::clone(&target.resources);
    let mut encoder = CommandEncoder::new(resources.as_ref());
    encoder
        .write_texture(
            resources
                .texture_ref(texture)
                .map_err(|_| "Invalid SGFX upload texture")?,
            TextureWrite::new(destination, stride, bytes)
                .map_err(|_| "Invalid SGFX texture upload")?,
        )
        .map_err(|_| "Failed to record SGFX texture upload")?;
    let commands = encoder
        .finish()
        .map_err(|_| "Failed to finish SGFX texture upload")?;
    target
        .execute(&commands)
        .map_err(|_| "Failed to execute SGFX texture upload")
}

fn append_quad(
    bytes: &mut Vec<u8>,
    destination: PixelRect,
    source: PixelRect,
    target_width: u32,
    target_height: u32,
    texture_width: u32,
    texture_height: u32,
) {
    let left = destination.x() as f32 * 2.0 / target_width as f32 - 1.0;
    let right = (destination.x() + destination.width()) as f32 * 2.0 / target_width as f32 - 1.0;
    let top = 1.0 - destination.y() as f32 * 2.0 / target_height as f32;
    let bottom = 1.0 - (destination.y() + destination.height()) as f32 * 2.0 / target_height as f32;
    let u0 = source.x() as f32 / texture_width as f32;
    let u1 = (source.x() + source.width()) as f32 / texture_width as f32;
    let v0 = source.y() as f32 / texture_height as f32;
    let v1 = (source.y() + source.height()) as f32 / texture_height as f32;
    for vertex in [
        [left, top, 0.0, 1.0, u0, v0],
        [left, bottom, 0.0, 1.0, u0, v1],
        [right, bottom, 0.0, 1.0, u1, v1],
        [left, top, 0.0, 1.0, u0, v0],
        [right, bottom, 0.0, 1.0, u1, v1],
        [right, top, 0.0, 1.0, u1, v0],
    ] {
        for component in vertex {
            bytes.extend_from_slice(&component.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CopiedRect, MAX_COMMANDS_PER_QUAD, MAX_QUADS_PER_COMMAND_BUFFER,
        QUAD_BATCH_COMMAND_OVERHEAD, ReusableImport, clipped_copy_rect, supports_mapped_target,
        take_reusable_import,
    };
    use sgfx::ir::{Extent2D, ResourceTable, TextureDesc, TextureFormat, TextureUsage};

    #[test]
    fn quad_batches_fit_the_portable_command_limit() {
        let commands =
            MAX_QUADS_PER_COMMAND_BUFFER * MAX_COMMANDS_PER_QUAD + QUAD_BATCH_COMMAND_OVERHEAD;
        let next_commands = (MAX_QUADS_PER_COMMAND_BUFFER + 1) * MAX_COMMANDS_PER_QUAD
            + QUAD_BATCH_COMMAND_OVERHEAD;

        assert!(MAX_QUADS_PER_COMMAND_BUFFER > 0);
        assert!(commands <= sgfx::ir::MAX_COMMANDS);
        assert!(next_commands > sgfx::ir::MAX_COMMANDS);
    }

    #[test]
    fn mapped_target_requires_all_composition_capabilities() {
        assert!(supports_mapped_target(true, true, true));
        assert!(!supports_mapped_target(false, true, true));
        assert!(!supports_mapped_target(true, false, true));
        assert!(!supports_mapped_target(true, true, false));
    }

    #[test]
    fn released_import_slots_are_reused_only_for_an_exact_extent() {
        let resources = ResourceTable::new();
        let small = resources
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Bgra8Unorm,
                    Extent2D::new(64, 64).unwrap(),
                    TextureUsage::SAMPLED,
                )
                .unwrap(),
            )
            .unwrap()
            .id();
        let wide = resources
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Bgra8Unorm,
                    Extent2D::new(128, 64).unwrap(),
                    TextureUsage::SAMPLED,
                )
                .unwrap(),
            )
            .unwrap()
            .id();
        let mut imports = vec![
            ReusableImport {
                texture: small,
                width: 64,
                height: 64,
            },
            ReusableImport {
                texture: wide,
                width: 128,
                height: 64,
            },
        ];

        assert!(take_reusable_import(&mut imports, 32, 32).is_none());
        assert_eq!(imports.len(), 2);
        assert_eq!(
            take_reusable_import(&mut imports, 64, 64).unwrap().texture,
            small
        );
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].texture, wide);
    }

    #[test]
    fn clipped_copy_preserves_the_source_destination_offset() {
        let resources = ResourceTable::new();
        let texture = resources
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Bgra8Unorm,
                    Extent2D::new(128, 96).unwrap(),
                    TextureUsage::SAMPLED | TextureUsage::COPY_SRC,
                )
                .unwrap(),
            )
            .unwrap()
            .id();
        let copy = CopiedRect {
            texture,
            destination: sgfx::ir::PixelRect::new(20, 30, 80, 60).unwrap(),
            source: sgfx::ir::PixelRect::new(4, 6, 80, 60).unwrap(),
            clip: Some(sgfx::ir::PixelRect::new(35, 40, 40, 30).unwrap()),
        };
        let (source, destination) =
            clipped_copy_rect(copy, sgfx::ir::PixelRect::new(30, 35, 60, 45).unwrap())
                .unwrap()
                .unwrap();

        assert_eq!(
            destination,
            sgfx::ir::PixelRect::new(35, 40, 40, 30).unwrap()
        );
        assert_eq!(source, sgfx::ir::PixelRect::new(19, 16, 40, 30).unwrap());
    }

    #[test]
    fn copy_outside_render_area_is_skipped() {
        let resources = ResourceTable::new();
        let texture = resources
            .define_texture(
                TextureDesc::new(
                    TextureFormat::Bgra8Unorm,
                    Extent2D::new(16, 16).unwrap(),
                    TextureUsage::SAMPLED | TextureUsage::COPY_SRC,
                )
                .unwrap(),
            )
            .unwrap()
            .id();
        let copy = CopiedRect {
            texture,
            destination: sgfx::ir::PixelRect::new(0, 0, 8, 8).unwrap(),
            source: sgfx::ir::PixelRect::new(0, 0, 8, 8).unwrap(),
            clip: None,
        };

        assert!(
            clipped_copy_rect(copy, sgfx::ir::PixelRect::new(8, 8, 8, 8).unwrap())
                .unwrap()
                .is_none()
        );
    }
}
