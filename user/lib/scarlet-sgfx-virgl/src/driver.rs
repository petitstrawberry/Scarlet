//! Scarlet transport selection and dispatch for the application facade.

use alloc::{rc::Rc, vec::Vec};

use framebuffer::DisplaySurface;
use gpu_raw::Gpu as RawGpu;
#[cfg(feature = "std")]
use scarlet_os::handle::{Handle, HandleError, HandleResult};
#[cfg(not(feature = "std"))]
use std::handle::{Handle, HandleError, HandleResult};

use crate::{
    Capabilities, Color, PipelineDesc, PixelRect, SourceAlpha, VertexClip4Color3, Viewport, virgl,
};
#[cfg(feature = "std")]
use scarlet_os::ipc::SharedMemory;
#[cfg(not(feature = "std"))]
use std::ipc::SharedMemory;

const VIRTIO_GPU_BACKEND_ID: &[u8] = b"virtio-gpu";
const APPLE_AGX_BACKEND_ID: &[u8] = b"apple-agx";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendKind {
    Virgl,
    AppleAgx,
    Unknown,
}

fn classify_backend_id(backend_id: &[u8]) -> BackendKind {
    if backend_id == VIRTIO_GPU_BACKEND_ID {
        BackendKind::Virgl
    } else if backend_id == APPLE_AGX_BACKEND_ID {
        BackendKind::AppleAgx
    } else {
        BackendKind::Unknown
    }
}

pub(crate) enum Device {
    Virgl(Rc<virgl::Device>),
}

impl Device {
    pub(crate) fn open(path: &str) -> HandleResult<Self> {
        // Queue command bytes are backend-defined. Probe the stable backend ID
        // before selecting a lowerer so a non-VirGL device never receives
        // VirGL command streams merely because it exposes generic queues.
        let probe = RawGpu::open(path)?;
        let info = probe.query_info()?;
        match classify_backend_id(info.backend_id_bytes()) {
            BackendKind::Virgl => {
                let backend = Rc::new(virgl::Device::open(path)?);
                Ok(Self::Virgl(backend))
            }
            BackendKind::AppleAgx | BackendKind::Unknown => Err(HandleError::Unsupported),
        }
    }

    pub(crate) fn capabilities(&self) -> Capabilities {
        match self {
            Self::Virgl(device) => device.capabilities(),
        }
    }

    pub(crate) fn create_context(&self) -> HandleResult<Context> {
        match self {
            Self::Virgl(device) => Ok(Context::Virgl(device.create_context()?)),
        }
    }
}

pub(crate) enum Context {
    Virgl(virgl::Context),
}

impl Context {
    pub(crate) fn context_id(&self) -> i32 {
        match self {
            Self::Virgl(context) => context.context_id(),
        }
    }

    pub(crate) fn create_ir_resources(&self) -> HandleResult<IrResources> {
        match self {
            Self::Virgl(context) => Ok(IrResources::Virgl(context.create_ir_resources()?)),
        }
    }

    pub(crate) fn map_ir_image(
        &self,
        resources: &mut IrResources,
        texture: IrTextureSpec,
        image: &Image,
    ) -> HandleResult<()> {
        match (self, resources, image) {
            (Self::Virgl(context), IrResources::Virgl(resources), Image::Virgl(image)) => {
                context.map_ir_image(resources, texture, image)
            }
        }
    }

    pub(crate) fn create_image(&self, width: u32, height: u32) -> HandleResult<Image> {
        match self {
            Self::Virgl(context) => Ok(Image::Virgl(context.create_image(width, height)?)),
        }
    }

    pub(crate) fn create_shared_image(&self, width: u32, height: u32) -> HandleResult<Image> {
        match self {
            Self::Virgl(context) => Ok(Image::Virgl(context.create_shared_image(width, height)?)),
        }
    }

    pub(crate) fn create_sampled_bgra_texture(
        &self,
        width: u32,
        height: u32,
    ) -> HandleResult<Texture> {
        match self {
            Self::Virgl(context) => Ok(Texture::Virgl(
                context.create_sampled_bgra_texture(width, height)?,
            )),
        }
    }

    pub(crate) fn create_imported_bgra_texture(
        &self,
        shared_memory: &SharedMemory,
        width: u32,
        height: u32,
        shm_offset: usize,
        source_stride: u32,
    ) -> HandleResult<Texture> {
        match self {
            Self::Virgl(context) => Ok(Texture::Virgl(context.create_imported_bgra_texture(
                shared_memory,
                width,
                height,
                shm_offset,
                source_stride,
            )?)),
        }
    }

    pub(crate) fn import_shared_bgra_texture(
        &self,
        handle: Handle,
    ) -> HandleResult<(Texture, u32, u32)> {
        match self {
            Self::Virgl(context) => {
                let (texture, width, height) = context.import_shared_bgra_texture(handle)?;
                Ok((Texture::Virgl(texture), width, height))
            }
        }
    }

    pub(crate) fn upload_texture_bgra(
        &self,
        texture: &Texture,
        pixels: &[u8],
        source_stride: u32,
        damage: PixelRect,
    ) -> HandleResult<()> {
        match (self, texture) {
            (Self::Virgl(context), Texture::Virgl(texture)) => {
                context.upload_texture_bgra(texture, pixels, source_stride, damage)
            }
        }
    }

    pub(crate) fn transfer_imported_bgra_rect(
        &self,
        texture: &Texture,
        damage: PixelRect,
    ) -> HandleResult<()> {
        match (self, texture) {
            (Self::Virgl(context), Texture::Virgl(texture)) => {
                context.transfer_imported_bgra_rect(texture, damage)
            }
        }
    }

    pub(crate) fn release_texture(&self, texture: Texture) -> HandleResult<()> {
        match (self, texture) {
            (Self::Virgl(context), Texture::Virgl(texture)) => context.release_texture(texture),
        }
    }

    pub(crate) fn release_image(&self, image: Image) -> HandleResult<()> {
        match (self, image) {
            (Self::Virgl(context), Image::Virgl(image)) => context.release_image(image),
        }
    }

    pub(crate) fn create_pipeline(
        &self,
        image: &Image,
        description: PipelineDesc,
    ) -> HandleResult<Pipeline> {
        match (self, image) {
            (Self::Virgl(context), Image::Virgl(image)) => Ok(Pipeline::Virgl(
                context.create_pipeline(image, description)?,
            )),
        }
    }

    pub(crate) fn create_queue(&self) -> HandleResult<Queue> {
        match self {
            Self::Virgl(context) => Ok(Queue::Virgl(context.create_queue()?)),
        }
    }
}

pub(crate) enum Queue {
    Virgl(virgl::Queue),
}

impl Queue {
    pub(crate) fn context_id(&self) -> i32 {
        match self {
            Self::Virgl(queue) => queue.context_id(),
        }
    }

    pub(crate) fn submit_ir(
        &self,
        context: &Context,
        resources: &mut IrResources,
        image: &Image,
        submission: &IrSubmission,
    ) -> HandleResult<()> {
        match (self, context, resources, image) {
            (
                Self::Virgl(queue),
                Context::Virgl(context),
                IrResources::Virgl(resources),
                Image::Virgl(image),
            ) => queue.submit_ir(context, resources, image, submission),
        }
    }

    pub(crate) fn submit_ir_internal(
        &self,
        context: &Context,
        resources: &mut IrResources,
        target: IrTextureSpec,
        submission: &IrSubmission,
    ) -> HandleResult<()> {
        match (self, context, resources) {
            (Self::Virgl(queue), Context::Virgl(context), IrResources::Virgl(resources)) => {
                queue.submit_ir_internal(context, resources, target, submission)
            }
        }
    }

    pub(crate) fn upload_ir_texture(
        &self,
        context: &Context,
        resources: &mut IrResources,
        texture: IrTextureSpec,
        upload: &IrTextureUpload,
    ) -> HandleResult<()> {
        match (self, context, resources) {
            (Self::Virgl(queue), Context::Virgl(context), IrResources::Virgl(resources)) => {
                queue.upload_ir_texture(context, resources, texture, upload)
            }
        }
    }

    pub(crate) fn prepare_ir_buffer(
        &self,
        context: &Context,
        resources: &mut IrResources,
        buffer: IrBufferSpec,
        bytes: &[u8],
    ) -> HandleResult<()> {
        match (self, context, resources) {
            (Self::Virgl(queue), Context::Virgl(context), IrResources::Virgl(resources)) => {
                queue.prepare_ir_buffer(context, resources, buffer, bytes)
            }
        }
    }

    pub(crate) fn copy_ir_texture(
        &self,
        context: &Context,
        resources: &mut IrResources,
        copy: IrTextureCopy,
    ) -> HandleResult<()> {
        match (self, context, resources) {
            (Self::Virgl(queue), Context::Virgl(context), IrResources::Virgl(resources)) => {
                queue.copy_ir_texture(context, resources, copy)
            }
        }
    }

    pub(crate) fn submit(
        &self,
        image: &Image,
        viewport: Viewport,
        clear_color: Color,
        pipeline: &Pipeline,
        vertices: &[VertexClip4Color3],
    ) -> HandleResult<()> {
        match (self, image, pipeline) {
            (Self::Virgl(queue), Image::Virgl(image), Pipeline::Virgl(pipeline)) => {
                queue.submit(image, viewport, clear_color, pipeline, vertices)
            }
        }
    }

    pub(crate) fn submit_composition(
        &self,
        image: &Image,
        clear_color: Color,
        operations: &[CompositionOperation<'_>],
    ) -> HandleResult<()> {
        match (self, image) {
            (Self::Virgl(queue), Image::Virgl(image)) => {
                let mut backend_operations = Vec::new();
                backend_operations
                    .try_reserve(operations.len())
                    .map_err(|_| HandleError::OutOfResources)?;
                for operation in operations {
                    match operation {
                        CompositionOperation::Textured {
                            texture: Texture::Virgl(texture),
                            destination,
                            source,
                            opacity,
                            source_alpha,
                            clip,
                        } => backend_operations.push(virgl::CompositionOperation::Textured {
                            texture,
                            destination: *destination,
                            source: *source,
                            opacity: *opacity,
                            source_alpha: *source_alpha,
                            clip: *clip,
                        }),
                        CompositionOperation::Solid {
                            destination,
                            color,
                            clip,
                        } => backend_operations.push(virgl::CompositionOperation::Solid {
                            destination: *destination,
                            color: *color,
                            clip: *clip,
                        }),
                    }
                }
                queue.submit_composition(image, clear_color, &backend_operations)
            }
        }
    }
}

pub(crate) enum Image {
    Virgl(virgl::Image),
}

impl Image {
    pub(crate) fn context_id(&self) -> i32 {
        match self {
            Self::Virgl(image) => image.context_id(),
        }
    }

    pub(crate) fn shared_handle(&self) -> &Handle {
        match self {
            Self::Virgl(image) => image.shared_handle(),
        }
    }
}

/// Maximum canonical vertices accepted in one persistent IR submission.
// Keep the expanded 40-byte canonical vertex stream below VirGL's 64 KiB
// opaque command limit after the inline write and draw state are included.
pub(crate) const MAX_IR_VERTICES: usize = 1_440;

/// Canonical vertex representation shared by portable IR lowering and drivers.
#[derive(Clone, Copy)]
pub(crate) struct IrVertex {
    pub(crate) position: [f32; 4],
    pub(crate) secondary: [f32; 4],
    pub(crate) tertiary: [f32; 2],
}

/// Persistent canonical vertex-buffer requirements for one logical IR slot.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct IrBufferSpec {
    pub(crate) slot: usize,
    pub(crate) size: u64,
    pub(crate) revision: u64,
}

/// One draw's binding into a persistent canonical vertex buffer.
#[derive(Clone, Copy)]
pub(crate) struct IrVertexBufferBinding {
    pub(crate) buffer: IrBufferSpec,
    pub(crate) offset: u32,
}

/// Backend-neutral rectangle used by the private IR execution plan.
#[derive(Clone, Copy)]
pub(crate) struct IrRect {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Portable fragment operation lowered by the IR executor.
#[derive(Clone, Copy)]
pub(crate) enum IrFragmentProgram {
    Solid,
    VertexColor,
    TextureRgba,
    TextureRgbIgnoreAlpha,
    TextureAlphaMask,
    TextureVertexColorRgba,
    TextureVertexColorRgbIgnoreAlpha,
    TextureVertexColorAlphaMask,
}

/// Portable blend factor carried without backend protocol constants.
#[derive(Clone, Copy)]
pub(crate) enum IrBlendFactor {
    Zero,
    One,
    SourceAlpha,
    OneMinusSourceAlpha,
    DestinationAlpha,
    OneMinusDestinationAlpha,
}

/// Portable blend arithmetic operation carried without backend protocol constants.
#[derive(Clone, Copy)]
pub(crate) enum IrBlendOp {
    Add,
    Subtract,
    ReverseSubtract,
}

/// Independent portable blend component.
#[derive(Clone, Copy)]
pub(crate) struct IrBlendComponent {
    pub(crate) source_factor: IrBlendFactor,
    pub(crate) destination_factor: IrBlendFactor,
    pub(crate) operation: IrBlendOp,
}

/// Exact independent color and alpha blending state.
#[derive(Clone, Copy)]
pub(crate) struct IrBlendState {
    pub(crate) color: IrBlendComponent,
    pub(crate) alpha: IrBlendComponent,
}

/// Portable culling selection used by the IR executor.
#[derive(Clone, Copy)]
pub(crate) enum IrCullMode {
    None,
    Front,
    Back,
}

/// Portable front-face selection used by the IR executor.
#[derive(Clone, Copy)]
pub(crate) enum IrFrontFace {
    Clockwise,
    CounterClockwise,
}

/// Portable depth comparison used by the IR executor.
#[derive(Clone, Copy)]
pub(crate) enum IrCompareFunction {
    Never,
    Less,
    Equal,
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}

/// Portable depth-test state used by the IR executor.
#[derive(Clone, Copy)]
pub(crate) struct IrDepthState {
    pub(crate) compare: IrCompareFunction,
    pub(crate) write_enabled: bool,
}

/// Persistent pipeline slot and its immutable portable state.
#[derive(Clone, Copy)]
pub(crate) struct IrPipelineState {
    pub(crate) slot: usize,
    pub(crate) fragment: IrFragmentProgram,
    pub(crate) blend: IrBlendState,
    pub(crate) cull_mode: IrCullMode,
    pub(crate) front_face: IrFrontFace,
    pub(crate) depth: Option<IrDepthState>,
}

/// Portable sampler filtering mode.
#[derive(Clone, Copy)]
pub(crate) enum IrFilterMode {
    Nearest,
    Linear,
}

/// Portable sampler coordinate addressing mode.
#[derive(Clone, Copy)]
pub(crate) enum IrAddressMode {
    ClampToEdge,
    Repeat,
    MirrorRepeat,
}

/// Persistent sampler slot and its immutable portable state.
#[derive(Clone, Copy)]
pub(crate) struct IrSamplerState {
    pub(crate) slot: usize,
    pub(crate) min_filter: IrFilterMode,
    pub(crate) mag_filter: IrFilterMode,
    pub(crate) address_u: IrAddressMode,
    pub(crate) address_v: IrAddressMode,
}

/// Draw-uniform constants sent to the GPU without CPU vertex transformation.
#[derive(Clone, Copy)]
pub(crate) struct IrUniforms {
    pub(crate) transform: [f32; 16],
    pub(crate) color: [f32; 4],
}

/// One private non-indexed draw in an ordered IR submission.
pub(crate) struct IrDraw {
    pub(crate) start_vertex: usize,
    pub(crate) vertex_count: usize,
    pub(crate) vertex_buffer: Option<IrVertexBufferBinding>,
    pub(crate) pipeline: IrPipelineState,
    pub(crate) texture: Option<IrTextureSpec>,
    pub(crate) sampler: Option<IrSamplerState>,
    pub(crate) uniforms: IrUniforms,
    pub(crate) scissor: IrRect,
}

/// Converted BGRA texture upload retained until all stream validation succeeds.
pub(crate) struct IrTextureUpload {
    pub(crate) texture: IrTextureSpec,
    pub(crate) destination: IrRect,
    pub(crate) pixels: Vec<u8>,
}

/// Logical texture materialization requirements without backend identifiers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct IrTextureSpec {
    pub(crate) slot: usize,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) sampled: bool,
    pub(crate) render_attachment: bool,
    pub(crate) copy_destination: bool,
    pub(crate) present: bool,
    pub(crate) format: IrTextureFormat,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum IrTextureFormat {
    Bgra8,
    Rgba8,
    R8,
    Depth32Float,
}

/// One validated texture copy request without backend identifiers.
#[derive(Clone, Copy)]
pub(crate) struct IrTextureCopy {
    pub(crate) source: IrTextureSpec,
    pub(crate) source_rect: IrRect,
    pub(crate) destination: IrTextureSpec,
    pub(crate) destination_rect: IrRect,
}

/// Complete backend-neutral render submission for one mapped presentation target.
pub(crate) struct IrSubmission {
    pub(crate) clear_color: Option<[f32; 4]>,
    pub(crate) depth_attachment: Option<IrTextureSpec>,
    pub(crate) clear_depth: Option<f32>,
    pub(crate) render_area: IrRect,
    pub(crate) vertices: Vec<IrVertex>,
    pub(crate) draws: Vec<IrDraw>,
    pub(crate) texture_uploads: Vec<IrTextureUpload>,
}

/// Private persistent materialization cache owned by the creating context.
pub(crate) enum IrResources {
    Virgl(virgl::IrResources),
}

pub(crate) enum Texture {
    Virgl(virgl::Texture),
}

pub(crate) enum CompositionOperation<'a> {
    Textured {
        texture: &'a Texture,
        destination: PixelRect,
        source: PixelRect,
        opacity: f32,
        source_alpha: SourceAlpha,
        clip: Option<PixelRect>,
    },
    Solid {
        destination: PixelRect,
        color: Color,
        clip: Option<PixelRect>,
    },
}

impl Image {
    pub(crate) fn present(&self, display: &DisplaySurface) -> HandleResult<()> {
        match self {
            Self::Virgl(image) => image.present(display),
        }
    }
}

pub(crate) enum Pipeline {
    Virgl(virgl::Pipeline),
}

#[cfg(test)]
mod tests {
    use super::{BackendKind, classify_backend_id};

    #[test]
    fn selects_virgl_for_exact_virtio_backend_id() {
        assert_eq!(classify_backend_id(b"virtio-gpu"), BackendKind::Virgl);
        assert_eq!(
            classify_backend_id(b"virtio-gpu-extra"),
            BackendKind::Unknown
        );
    }

    #[test]
    fn reserves_apple_agx_without_falling_back_to_virgl() {
        assert_eq!(classify_backend_id(b"apple-agx"), BackendKind::AppleAgx);
    }

    #[test]
    fn rejects_unknown_and_empty_backend_ids() {
        assert_eq!(classify_backend_id(b""), BackendKind::Unknown);
        assert_eq!(classify_backend_id(b"software"), BackendKind::Unknown);
    }
}
