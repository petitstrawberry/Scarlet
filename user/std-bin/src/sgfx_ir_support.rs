//! Private helpers for driving the modern SGFX frontend from Scarlet binaries.

use std::rc::Rc;
use std::vec::Vec;
use std::{error, fmt};

#[cfg(target_os = "scarlet")]
use framebuffer::{DisplayPresentRegion, DisplaySurface};
#[cfg(target_os = "scarlet")]
use scarlet_os::handle::Handle;
#[cfg(target_os = "scarlet")]
use scarlet_ui_renderer_sgfx::{FrameExecutor, FrameSubmissionError};
use sgfx::backend::CommandExecutor;
#[cfg(target_os = "scarlet")]
use sgfx::backend::{CommandSubmitter, CompletionStatus, SubmitError};
use sgfx::ir::{
    self, AddressMode, BlendState, BufferDesc, BufferId, BufferUsage, CommandEncoder, DrawUniforms,
    Extent2D, FilterMode, FragmentProgram, LoadOp, PixelRect, PrimitiveTopology, RasterState,
    RenderPassDesc, RenderPipelineDesc, RenderPipelineId, ResourceTable, SamplerDesc, SamplerId,
    StoreOp, TextureDesc, TextureFormat, TextureId, TextureSampleMode, TextureUsage, TextureWrite,
    Transform, VertexAttribute, VertexBufferLayout, VertexFormat,
};
#[cfg(target_os = "scarlet")]
use sgfx::{Context, Instance, MappedTargetSession};

#[cfg(target_os = "scarlet")]
#[path = "sgfx_presentation.rs"]
mod presentation;
#[cfg(target_os = "scarlet")]
use presentation::PresentationQueue;

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

#[cfg(target_os = "scarlet")]
struct TargetSlot {
    texture: TextureId,
    initialized: bool,
    damage: Option<PixelRect>,
    last_present: Option<u64>,
}

#[cfg(target_os = "scarlet")]
pub(crate) struct MappedTarget {
    pub(crate) resources: Rc<ResourceTable>,
    pub(crate) texture: TextureId,
    pub(crate) width: u32,
    pub(crate) height: u32,
    presented_texture: Option<TextureId>,
    slots: Vec<TargetSlot>,
    current_slot: usize,
    present_sequence: u64,
    presentation: Option<PresentationQueue>,
    pending_damage: Option<PixelRect>,
    session: MappedTargetSession,
    reusable_imports: Vec<ReusableImport>,
    tracked_submission: bool,
}

#[cfg(target_os = "scarlet")]
impl MappedTarget {
    pub(crate) fn supports_tracked_submission(&self) -> bool {
        self.tracked_submission
    }

    pub(crate) fn open(width: u32, height: u32) -> Result<Self, Error> {
        Self::open_with_target_count(width, height, 1, false)
    }

    /// Open a three-image presentation target for tear-free direct scanout.
    pub(crate) fn open_swapchain(width: u32, height: u32) -> Result<Self, Error> {
        // Readback is an optional remote-capture capability, not a prerequisite
        // for local GPU composition or presentation. Backends such as native
        // Adreno can therefore keep the desktop on the GPU while an attempted
        // capture reports its own unsupported readback operation.
        Self::open_with_target_count(width, height, 3, false)
    }

    fn open_with_target_count(
        width: u32,
        height: u32,
        target_count: usize,
        require_readback: bool,
    ) -> Result<Self, Error> {
        let instance = Instance::new()?;
        let device = instance.open_device("/dev/gpu0")?;
        println!("sgfx: selected backend {}", device.backend());
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
        if !matches!(target_count, 1 | 3) {
            return Err(ir::Error::InvalidValue.into());
        }
        let mut targets = Vec::with_capacity(target_count);
        let mut slots = Vec::with_capacity(target_count);
        for _ in 0..target_count {
            let texture = define_target()?;
            targets.push(texture);
            slots.push(TargetSlot {
                texture,
                initialized: false,
                damage: None,
                last_present: None,
            });
        }
        let texture = targets[0];
        let mut session = context.create_mapped_target_session(Rc::clone(&resources), &targets)?;
        let tracked_submission = session.executor().supports_async_submission();
        Ok(Self {
            resources,
            texture,
            width,
            height,
            presented_texture: None,
            slots,
            current_slot: 0,
            present_sequence: 0,
            presentation: None,
            pending_damage: None,
            session,
            reusable_imports: Vec::new(),
            tracked_submission,
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
        if self.slots.len() == 1 {
            return Ok(requested);
        }
        let slot = &self.slots[self.current_slot];
        if !slot.initialized {
            return Ok(full);
        }
        match slot.damage {
            Some(damage) => union_pixel_rect(damage, requested).map_err(Into::into),
            None => Ok(requested),
        }
    }

    fn finish_present(&mut self, sequence: u64) -> Result<(), Error> {
        let full = PixelRect::new(0, 0, self.width, self.height)?;
        let logical_damage = self.pending_damage.take().unwrap_or(full);
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if index == self.current_slot {
                slot.initialized = true;
                slot.damage = None;
                slot.last_present = Some(sequence);
            } else if slot.initialized {
                slot.damage = Some(match slot.damage {
                    Some(damage) => union_pixel_rect(damage, logical_damage)?,
                    None => logical_damage,
                });
            }
        }
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
        if self.slots.len() > 1 {
            if self.presentation.is_none() {
                let mut images = Vec::with_capacity(self.slots.len());
                for slot in &self.slots {
                    images.push(
                        self.session
                            .image(slot.texture)
                            .map_err(|_| "Failed to resolve SGFX swapchain image")?
                            .shared_handle()
                            .duplicate()
                            .map_err(|_| "Failed to retain SGFX swapchain image")?,
                    );
                }
                self.presentation = Some(PresentationQueue::new(display, images)?);
                println!(
                    "sgfx: asynchronous display presentation with {} images",
                    self.slots.len()
                );
            }
            let sequence = self
                .present_sequence
                .checked_add(1)
                .ok_or("SGFX presentation sequence exhausted")?;
            self.presentation
                .as_mut()
                .unwrap()
                .present(sequence, self.current_slot, region)?;
            self.present_sequence = sequence;
            self.finish_present(sequence)
                .map_err(|_| "Failed to track SGFX swapchain damage")?;
            let next = (self.current_slot + 1) % self.slots.len();
            if let Some(previous) = self.slots[next].last_present {
                // Completing image N's flip makes it the front. It is released
                // only when a LATER image has actually replaced it at scanout.
                self.presentation.as_mut().unwrap().wait_after(previous)?;
            }
            self.current_slot = next;
            self.texture = self.slots[next].texture;
        } else {
            let image = self
                .session
                .image(presented_texture)
                .map_err(|_| "Failed to resolve mapped SGFX target")?;
            display
                .present_image(image.shared_handle(), region)
                .map_err(|_| "Failed to present mapped SGFX target")?;
            self.pending_damage = None;
        }
        self.presented_texture = Some(presented_texture);
        Ok(())
    }

    /// Read the most recently composed and queued image. GPU work has retired;
    /// the independent display worker may still be waiting for its scanout flip.
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

#[cfg(target_os = "scarlet")]
impl Drop for MappedTarget {
    fn drop(&mut self) {
        // Join accepted display flips before releasing session images or
        // replacing this compositor with CPU composition after an error.
        self.presentation.take();
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
    /// Fractional source-texel offsets for separable linear-sampler kernels.
    /// UVs may extend beyond the texture; the sampler clamps at its edges.
    SampledOffset {
        rect: SampledRect,
        offset: [f32; 2],
    },
    Copy(CopiedRect),
}

/// One CPU-backed texture update recorded before scene composition.
pub(crate) struct TextureUpload<'a> {
    pub(crate) texture: TextureId,
    pub(crate) destination: PixelRect,
    pub(crate) stride: u32,
    pub(crate) bytes: &'a [u8],
}

/// Reusable linear capture for a backdrop filter. Native Maxwell presentation
/// and render targets are block-linear; copying the composed source into this
/// sampled-only texture keeps the blur path on the proven tiled-to-linear copy
/// and linear-sampler paths.
pub(crate) struct BackdropTextures {
    levels: Vec<(TextureId, u32, u32)>,
}

impl BackdropTextures {
    pub(crate) fn define(resources: &ResourceTable, width: u32, height: u32) -> ir::Result<Self> {
        let texture = resources
            .define_texture(TextureDesc::new(
                TextureFormat::Bgra8Unorm,
                Extent2D::new(width, height)?,
                TextureUsage::SAMPLED | TextureUsage::COPY_DST,
            )?)?
            .id();
        let levels = vec![(texture, width, height)];
        Ok(Self { levels })
    }
}

pub(crate) struct BackdropPass<'a> {
    pub(crate) textures: &'a BackdropTextures,
    /// Draw the scene below the chrome first; filter it, then draw the rest.
    pub(crate) split: usize,
    pub(crate) source: PixelRect,
    pub(crate) output: PixelRect,
    pub(crate) clips: Vec<PixelRect>,
    pub(crate) radius: u32,
}

/// Failure while recording or executing one quad-composition submission.
#[derive(Debug)]
pub(crate) enum QuadSubmitError<E = sgfx::Error> {
    /// Admission was busy and every accepted stream from this frame retired.
    /// Keep its inputs and retry the frame; no image has been published.
    Busy,
    /// Portable IR recording rejected the requested frame.
    Recording(&'static str),
    /// The selected SGFX backend failed while executing valid recorded IR.
    Execution(E),
}

impl<E> From<&'static str> for QuadSubmitError<E> {
    fn from(error: &'static str) -> Self {
        Self::Recording(error)
    }
}

impl<E: fmt::Display> fmt::Display for QuadSubmitError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => formatter.write_str("GPU admission is busy; frame safely discarded"),
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

    #[cfg(target_os = "scarlet")]
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
    #[cfg(target_os = "scarlet")]
    pub(crate) fn submit_region(
        &self,
        target: &mut MappedTarget,
        area: PixelRect,
        load: LoadOp,
        operations: &[Quad],
    ) -> Result<(), QuadSubmitError> {
        self.submit_region_with_uploads(target, area, load, &[], operations, &[])
    }

    /// Upload CPU-backed textures and compose one damaged target region.
    ///
    /// Keeping uploads in the first composition command buffer removes a
    /// synchronous executor round trip from CPU-rendered clients such as
    /// Wayland SHM applications.
    #[cfg(target_os = "scarlet")]
    pub(crate) fn submit_region_with_uploads(
        &self,
        target: &mut MappedTarget,
        area: PixelRect,
        load: LoadOp,
        uploads: &[TextureUpload<'_>],
        operations: &[Quad],
        backdrops: &[BackdropPass<'_>],
    ) -> Result<(), QuadSubmitError> {
        self.encode_scene_with_uploads(
            &mut target.session.executor(),
            Rc::clone(&target.resources),
            target.texture,
            target.width,
            target.height,
            area,
            load,
            uploads,
            operations,
            backdrops,
        )
    }

    /// Queue the complete composition and observe it once before presentation.
    #[cfg(target_os = "scarlet")]
    pub(crate) fn submit_region_with_uploads_tracked(
        &self,
        target: &mut MappedTarget,
        area: PixelRect,
        load: LoadOp,
        uploads: &[TextureUpload<'_>],
        operations: &[Quad],
        backdrops: &[BackdropPass<'_>],
    ) -> Result<(), QuadSubmitError<FrameSubmissionError<sgfx::Error, sgfx::Submission>>> {
        let mut executor = FrameExecutor::new(target.session.executor());
        let encoded = self.encode_scene_with_uploads(
            &mut executor,
            Rc::clone(&target.resources),
            target.texture,
            target.width,
            target.height,
            area,
            load,
            uploads,
            operations,
            backdrops,
        );
        if let Err(error) = encoded {
            if matches!(
                &error,
                QuadSubmitError::Execution(FrameSubmissionError::Submit(SubmitError::Busy))
            ) {
                return match executor.discard() {
                    Ok(CompletionStatus::Complete) => Err(QuadSubmitError::Busy),
                    Ok(_) => Err(QuadSubmitError::Execution(FrameSubmissionError::Pending)),
                    Err(error) => Err(QuadSubmitError::Execution(error)),
                };
            }
            return Err(error);
        }
        match executor.wait() {
            Ok(CompletionStatus::Complete) => Ok(()),
            Ok(_) => Err(QuadSubmitError::Execution(FrameSubmissionError::Pending)),
            Err(error) => Err(QuadSubmitError::Execution(error)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_scene_with_uploads<E: CommandExecutor>(
        &self,
        executor: &mut E,
        resources: Rc<ResourceTable>,
        texture: TextureId,
        width: u32,
        height: u32,
        area: PixelRect,
        load: LoadOp,
        uploads: &[TextureUpload<'_>],
        operations: &[Quad],
        backdrops: &[BackdropPass<'_>],
    ) -> Result<(), QuadSubmitError<E::Error>> {
        if backdrops.is_empty() {
            return self.encode_region_with_uploads(
                executor, resources, texture, width, height, area, load, uploads, operations,
            );
        }
        let white = ir::Color::rgba(1.0, 1.0, 1.0, 1.0).map_err(|_| "Invalid backdrop tint")?;
        let mut previous_split = 0;
        let mut first = 0;
        while first < backdrops.len() {
            let split = backdrops[first].split;
            if split < previous_split || split > operations.len() {
                return Err("Invalid SGFX backdrop split".into());
            }
            let end = first
                + backdrops[first..]
                    .iter()
                    .take_while(|b| b.split == split)
                    .count();
            self.encode_region_with_uploads(
                executor,
                Rc::clone(&resources),
                texture,
                width,
                height,
                area,
                if first == 0 { load } else { LoadOp::Load },
                if first == 0 { uploads } else { &[] },
                &operations[previous_split..split],
            )?;

            // Capture every material before writing any of them back to the
            // main target. Adjacent controls often have overlapping halos.
            for backdrop in &backdrops[first..end] {
                if backdrop.textures.levels.len() != 1 {
                    return Err("Invalid SGFX backdrop textures".into());
                }
                let (capture, sw, sh) = backdrop.textures.levels[0];
                let local = PixelRect::new(0, 0, sw, sh).map_err(|_| "Invalid backdrop source")?;
                self.encode_region_with_uploads(
                    executor,
                    Rc::clone(&resources),
                    capture,
                    sw,
                    sh,
                    local,
                    LoadOp::Load,
                    &[],
                    &[Quad::Copy(CopiedRect {
                        texture,
                        source: backdrop.source,
                        destination: local,
                        clip: None,
                    })],
                )?;
            }
            for backdrop in &backdrops[first..end] {
                let (capture, sw, sh) = backdrop.textures.levels[0];
                let local = PixelRect::new(0, 0, sw, sh).map_err(|_| "Invalid backdrop source")?;
                // A 5x5 binomial kernel approximates the three box passes used
                // by the CPU compositor. SOURCE_OVER can form an exact weighted
                // average when each successive alpha is weight/running_total;
                // the first tap is opaque and replaces the unfiltered pixels.
                let positions = [-2.0f32, -1.0, 0.0, 1.0, 2.0];
                let weights = [1.0f32, 4.0, 6.0, 4.0, 1.0];
                let step = backdrop.radius.max(1) as f32;
                let mut quads = Vec::with_capacity(25 + backdrop.clips.len() * 2);
                let mut total = 0.0f32;
                for (y, wy) in positions.into_iter().zip(weights) {
                    for (x, wx) in positions.into_iter().zip(weights) {
                        let weight = wx * wy;
                        total += weight;
                        quads.push(Quad::SampledOffset {
                            rect: SampledRect {
                                texture: capture,
                                texture_width: sw,
                                texture_height: sh,
                                destination: backdrop.source,
                                source: local,
                                tint: ir::Color::rgba(1.0, 1.0, 1.0, weight / total)
                                    .map_err(|_| "Invalid backdrop sample weight")?,
                                ignore_source_alpha: true,
                                clip: Some(backdrop.output),
                            },
                            offset: [x * step, y * step],
                        });
                    }
                }

                // The kernel uses one rectangular scissor. Restore the small
                // corner areas outside the rounded material from the clean
                // capture so blur never leaks through transparent corners.
                let output_right = backdrop
                    .output
                    .x()
                    .checked_add(backdrop.output.width())
                    .ok_or("Invalid backdrop output")?;
                for clip in &backdrop.clips {
                    if clip.x() > backdrop.output.x() {
                        let corner = PixelRect::new(
                            backdrop.output.x(),
                            clip.y(),
                            clip.x() - backdrop.output.x(),
                            clip.height(),
                        )
                        .map_err(|_| "Invalid backdrop corner")?;
                        quads.push(Quad::Sampled(SampledRect {
                            texture: capture,
                            texture_width: sw,
                            texture_height: sh,
                            destination: backdrop.source,
                            source: local,
                            tint: white,
                            ignore_source_alpha: true,
                            clip: Some(corner),
                        }));
                    }
                    let clip_right = clip
                        .x()
                        .checked_add(clip.width())
                        .ok_or("Invalid backdrop clip")?;
                    if clip_right < output_right {
                        let corner = PixelRect::new(
                            clip_right,
                            clip.y(),
                            output_right - clip_right,
                            clip.height(),
                        )
                        .map_err(|_| "Invalid backdrop corner")?;
                        quads.push(Quad::Sampled(SampledRect {
                            texture: capture,
                            texture_width: sw,
                            texture_height: sh,
                            destination: backdrop.source,
                            source: local,
                            tint: white,
                            ignore_source_alpha: true,
                            clip: Some(corner),
                        }));
                    }
                }
                self.encode_region_with_uploads(
                    executor,
                    Rc::clone(&resources),
                    texture,
                    width,
                    height,
                    backdrop.output,
                    LoadOp::Load,
                    &[],
                    &quads,
                )?;
            }
            previous_split = split;
            first = end;
        }
        self.encode_region_with_uploads(
            executor,
            resources,
            texture,
            width,
            height,
            area,
            LoadOp::Load,
            &[],
            &operations[previous_split..],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_region_with_uploads<E: CommandExecutor>(
        &self,
        executor: &mut E,
        resources: Rc<ResourceTable>,
        texture: TextureId,
        width: u32,
        height: u32,
        area: PixelRect,
        load: LoadOp,
        uploads: &[TextureUpload<'_>],
        operations: &[Quad],
    ) -> Result<(), QuadSubmitError<E::Error>> {
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
                Quad::Sampled(rect) | Quad::SampledOffset { rect, .. } => (
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
                width,
                height,
                texture_width,
                texture_height,
                match operation {
                    Quad::SampledOffset { offset, .. } => *offset,
                    _ => [0.0, 0.0],
                },
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
                texture,
                area,
                if first_batch { load } else { LoadOp::Load },
                &operations[batch_start..batch_end],
                batch_start,
            )?;
            let commands = encoder
                .finish()
                .map_err(|_| "Failed to finish SGFX composition commands")?;
            // A fully clipped segment may contain only an unused vertex
            // upload. Do not submit a draw-free stream to synchronous backends.
            if commands
                .commands()
                .iter()
                .any(|command| !matches!(command, ir::Command::WriteBuffer { .. }))
            {
                executor
                    .execute(&commands)
                    .map_err(QuadSubmitError::Execution)?;
            }
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
        // Copy-only backdrop captures and empty scene segments do not need a
        // render pass. VirGL rejects Load/Store passes without any draws; that
        // used to disable GPU composition as soon as a material was shown.
        if matches!(load, LoadOp::Load) {
            let mut has_draw = false;
            for operation in operations {
                let clip = match operation {
                    Quad::Solid { clip, .. } => *clip,
                    Quad::Sampled(rect) | Quad::SampledOffset { rect, .. } => rect.clip,
                    Quad::Copy(_) => return Err("SGFX copy leaked into a render segment"),
                };
                if clip.is_none() || intersect_pixel_rect(clip.unwrap(), area)?.is_some() {
                    has_draw = true;
                    break;
                }
            }
            if !has_draw {
                return Ok(());
            }
        }
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
                Quad::Sampled(rect) | Quad::SampledOffset { rect, .. } => rect.clip,
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
                Quad::Sampled(rect) | Quad::SampledOffset { rect, .. } => {
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

#[cfg(target_os = "scarlet")]
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
    offset: [f32; 2],
) {
    let left = destination.x() as f32 * 2.0 / target_width as f32 - 1.0;
    let right = (destination.x() + destination.width()) as f32 * 2.0 / target_width as f32 - 1.0;
    let top = 1.0 - destination.y() as f32 * 2.0 / target_height as f32;
    let bottom = 1.0 - (destination.y() + destination.height()) as f32 * 2.0 / target_height as f32;
    let u0 = (source.x() as f32 + offset[0]) / texture_width as f32;
    let u1 = ((source.x() + source.width()) as f32 + offset[0]) / texture_width as f32;
    let v0 = (source.y() as f32 + offset[1]) / texture_height as f32;
    let v1 = ((source.y() + source.height()) as f32 + offset[1]) / texture_height as f32;
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
