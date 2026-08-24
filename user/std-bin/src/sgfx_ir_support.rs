//! Private helpers for driving the modern SGFX frontend from Scarlet binaries.

use std::rc::Rc;
use std::vec::Vec;
use std::{error, fmt};

use framebuffer::{DisplayPresentRegion, DisplaySurface};
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
            Self::UnsupportedCapabilities => formatter
                .write_str("SGFX device lacks rendering, presentation, or image-upload capability"),
        }
    }
}

impl error::Error for Error {}

/// One directly selected frontend context with a mapped presentation target.
pub(crate) struct MappedTarget {
    pub(crate) resources: Rc<ResourceTable>,
    pub(crate) texture: TextureId,
    pub(crate) width: u32,
    pub(crate) height: u32,
    session: MappedTargetSession,
}

impl MappedTarget {
    pub(crate) fn open(width: u32, height: u32) -> Result<Self, Error> {
        let instance = Instance::new()?;
        let device = instance.open_device("/dev/gpu0")?;
        let capabilities = device.capabilities();
        if !supports_mapped_target(
            capabilities.supports_rendering(),
            capabilities.supports_presentation(),
            capabilities.supports_image_upload(),
        ) {
            return Err(Error::UnsupportedCapabilities);
        }
        let context = device.create_context()?;
        Self::from_context(context, width, height)
    }

    fn from_context(context: Context, width: u32, height: u32) -> Result<Self, Error> {
        let resources = Rc::new(ResourceTable::new());
        let extent = Extent2D::new(width, height)?;
        let texture = resources
            .define_texture(TextureDesc::new(
                TextureFormat::Bgra8Unorm,
                extent,
                TextureUsage::RENDER_ATTACHMENT | TextureUsage::PRESENT,
            )?)?
            .id();
        let session = context.create_mapped_target_session(Rc::clone(&resources), &[texture])?;
        Ok(Self {
            resources,
            texture,
            width,
            height,
            session,
        })
    }

    pub(crate) fn execute(
        &mut self,
        commands: &ir::CommandBuffer<'_, '_>,
    ) -> Result<(), sgfx::Error> {
        self.session.executor().execute(commands)
    }

    pub(crate) fn present(
        &self,
        display: &DisplaySurface,
        region: Option<DisplayPresentRegion>,
    ) -> Result<(), &'static str> {
        let image = self
            .session
            .image(self.texture)
            .map_err(|_| "Failed to resolve mapped SGFX target")?;
        display
            .present_image(image.shared_handle(), region)
            .map_err(|_| "Failed to present mapped SGFX target")
    }
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

#[derive(Clone, Copy)]
pub(crate) enum Quad {
    Solid {
        destination: PixelRect,
        color: ir::Color,
        clip: Option<PixelRect>,
    },
    Sampled(SampledRect),
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
    ) -> Result<(), &'static str> {
        if operations.len() > self.capacity {
            return Err("SGFX composition operation capacity exceeded");
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

        let mut encoder = CommandEncoder::new(resources.as_ref());
        if !vertices.is_empty() {
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
        let area = PixelRect::new(0, 0, target.width, target.height)
            .map_err(|_| "Invalid SGFX target area")?;
        let descriptor = RenderPassDesc::new(
            resources.as_ref(),
            resources
                .texture_ref(target.texture)
                .map_err(|_| "Invalid SGFX target texture")?,
            area,
            load,
            StoreOp::Store,
        )
        .map_err(|_| "Invalid SGFX composition pass")?;
        {
            let mut pass = encoder
                .begin_render_pass(descriptor)
                .map_err(|_| "Failed to begin SGFX composition pass")?;
            for (index, operation) in operations.iter().enumerate() {
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
                    Quad::Solid { color, clip, .. } => {
                        pass.set_pipeline(
                            resources
                                .render_pipeline_ref(self.solid_pipeline)
                                .map_err(|_| "Invalid solid pipeline")?,
                        )
                        .map_err(|_| "Failed to bind solid pipeline")?;
                        pass.set_uniforms(DrawUniforms::new(Transform::identity(), *color))
                            .map_err(|_| "Failed to set solid uniforms")?;
                        pass.set_scissor(*clip)
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
                        pass.set_scissor(rect.clip)
                            .map_err(|_| "Failed to set texture scissor")?;
                    }
                }
                pass.draw(QUAD_VERTEX_COUNT as u32, 0)
                    .map_err(|_| "Failed to record SGFX quad")?;
            }
            pass.end()
                .map_err(|_| "Failed to end SGFX composition pass")?;
        }
        let commands = encoder
            .finish()
            .map_err(|_| "Failed to finish SGFX composition commands")?;
        target
            .execute(&commands)
            .map_err(|_| "Failed to execute SGFX composition")
    }
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
            TextureUsage::SAMPLED | TextureUsage::COPY_DST,
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
    use super::supports_mapped_target;

    #[test]
    fn mapped_target_requires_all_composition_capabilities() {
        assert!(supports_mapped_target(true, true, true));
        assert!(!supports_mapped_target(false, true, true));
        assert!(!supports_mapped_target(true, false, true));
        assert!(!supports_mapped_target(true, true, false));
    }
}
