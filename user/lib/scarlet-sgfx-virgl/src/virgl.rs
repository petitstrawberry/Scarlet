//! VirGL execution through Scarlet's GPU transport.

use alloc::{rc::Rc, vec::Vec};
use core::cell::Cell;

use framebuffer::DisplaySurface;
use gpu_raw::{
    GPU_DEVICE_STATE_READY, GPU_EXECUTION_SUPPORT_DEPTH, GPU_EXECUTION_SUPPORT_IMAGE_UPLOAD,
    GPU_EXECUTION_SUPPORT_PRESENTATION, GPU_EXECUTION_SUPPORT_QUEUE, GPU_IMAGE_FORMAT_BGRA8_UNORM,
    GPU_IMAGE_FORMAT_DEPTH32_FLOAT, GPU_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT,
    GPU_IMAGE_USAGE_PRESENTABLE, GPU_IMAGE_USAGE_RENDER_TARGET, GPU_IMAGE_USAGE_SAMPLED,
    GPU_IMAGE_USAGE_TRANSFER_DST, GPU_RESULT_SUCCESS, Gpu as RawGpu, GpuBuffer as RawBuffer,
    GpuContext as RawContext, GpuDialect as RawDialect, GpuImage as RawImage, GpuImageBgraRect,
    GpuQueue as RawQueue,
};
#[cfg(feature = "std")]
use scarlet_os::handle::{Handle, HandleError, HandleResult};
#[cfg(feature = "std")]
use scarlet_os::ipc::SharedMemory;
#[cfg(not(feature = "std"))]
use std::{
    handle::{Handle, HandleError, HandleResult},
    ipc::SharedMemory,
};

use crate::driver::{
    IrAddressMode, IrBlendFactor, IrBlendOp, IrBlendState, IrBufferSpec, IrCompareFunction,
    IrCullMode, IrDraw, IrFilterMode, IrFragmentProgram, IrFrontFace, IrPipelineState,
    IrSamplerState, IrSubmission, IrTextureCopy, IrTextureFormat, IrTextureSpec, IrTextureUpload,
    IrVertex, MAX_IR_VERTICES,
};
use crate::{
    Capabilities, Color, CullMode, FrontFace, MAX_COMPOSITION_OPERATIONS, PipelineDesc,
    PipelineKind, PixelRect, SourceAlpha, VertexClip4Color3, Viewport,
};

const VIRGL_CCMD_CREATE_OBJECT: u32 = 1;
const VIRGL_CCMD_BIND_OBJECT: u32 = 2;
const VIRGL_CCMD_SET_VIEWPORT_STATE: u32 = 4;
const VIRGL_CCMD_SET_FRAMEBUFFER_STATE: u32 = 5;
const VIRGL_CCMD_SET_VERTEX_BUFFERS: u32 = 6;
const VIRGL_CCMD_CLEAR: u32 = 7;
const VIRGL_CCMD_DRAW_VBO: u32 = 8;
const VIRGL_CCMD_RESOURCE_INLINE_WRITE: u32 = 9;
const VIRGL_CCMD_SET_SAMPLER_VIEWS: u32 = 10;
const VIRGL_CCMD_RESOURCE_COPY_REGION: u32 = 17;
const VIRGL_CCMD_SET_CONSTANT_BUFFER: u32 = 12;
const VIRGL_CCMD_SET_SCISSOR_STATE: u32 = 15;
const VIRGL_CCMD_BIND_SAMPLER_STATES: u32 = 18;
const VIRGL_CCMD_BIND_SHADER: u32 = 31;
const VIRGL_CCMD_CLEAR_SURFACE: u32 = 62;

const VIRGL_OBJECT_BLEND: u32 = 1;
const VIRGL_OBJECT_RASTERIZER: u32 = 2;
const VIRGL_OBJECT_DSA: u32 = 3;
const VIRGL_OBJECT_SHADER: u32 = 4;
const VIRGL_OBJECT_VERTEX_ELEMENTS: u32 = 5;
const VIRGL_OBJECT_SAMPLER_VIEW: u32 = 6;
const VIRGL_OBJECT_SAMPLER_STATE: u32 = 7;
const VIRGL_OBJECT_SURFACE: u32 = 8;

const VIRGL_FORMAT_B8G8R8A8_UNORM: u32 = 1;
const VIRGL_FORMAT_Z32_FLOAT: u32 = 18;
const VIRGL_FORMAT_R32G32B32A32_FLOAT: u32 = 31;
const VIRGL_FORMAT_R32G32B32_FLOAT: u32 = 30;
const VIRGL_FORMAT_R32G32_FLOAT: u32 = 29;
const PIPE_SHADER_VERTEX: u32 = 0;
const PIPE_SHADER_FRAGMENT: u32 = 1;
const VIRGL_SHADER_TOKEN_COUNT_HINT: u32 = 300;
const PIPE_PRIM_TRIANGLES: u32 = 4;
const PIPE_CLEAR_COLOR0: u32 = 1 << 2;
const PIPE_CLEAR_DEPTH: u32 = 1 << 0;

const SURFACE_HANDLE: u32 = 1;
const VERTEX_SHADER_HANDLE: u32 = 2;
const FRAGMENT_SHADER_HANDLE: u32 = 3;
const VERTEX_ELEMENTS_HANDLE: u32 = 4;
const BLEND_HANDLE: u32 = 5;
const RASTERIZER_HANDLE: u32 = 6;
const COMPOSITION_BLEND_HANDLE: u32 = 16;
const COMPOSITION_RASTERIZER_HANDLE: u32 = 17;
const COMPOSITION_VERTEX_SHADER_HANDLE: u32 = 18;
const COMPOSITION_TEXTURE_ALPHA_SHADER_HANDLE: u32 = 19;
const COMPOSITION_TEXTURE_OPAQUE_SHADER_HANDLE: u32 = 20;
const COMPOSITION_SOLID_SHADER_HANDLE: u32 = 21;
const COMPOSITION_VERTEX_ELEMENTS_HANDLE: u32 = 22;
const COMPOSITION_SAMPLER_STATE_HANDLE: u32 = 23;
const FIRST_DYNAMIC_OBJECT_HANDLE: u32 = 64;
const VIRGL_RASTERIZER_DEPTH_CLIP: u32 = 1 << 1;
const VIRGL_RASTERIZER_SCISSOR: u32 = 1 << 14;
const VIRGL_RASTERIZER_CULL_FACE_SHIFT: u32 = 8;
const VIRGL_RASTERIZER_FRONT_CCW: u32 = 1 << 15;
const VIRGL_BLEND_ENABLE: u32 = 1;
const VIRGL_BLEND_RGB_SRC_FACTOR_SHIFT: u32 = 4;
const VIRGL_BLEND_RGB_DST_FACTOR_SHIFT: u32 = 9;
const VIRGL_BLEND_ALPHA_SRC_FACTOR_SHIFT: u32 = 17;
const VIRGL_BLEND_ALPHA_DST_FACTOR_SHIFT: u32 = 22;
const VIRGL_BLEND_COLORMASK_SHIFT: u32 = 27;
const PIPE_BLENDFACTOR_ONE: u32 = 1;
const PIPE_BLENDFACTOR_SRC_ALPHA: u32 = 3;
const PIPE_BLENDFACTOR_DST_ALPHA: u32 = 4;
const PIPE_BLENDFACTOR_ZERO: u32 = 0x11;
const PIPE_BLENDFACTOR_INV_SRC_ALPHA: u32 = 19;
const PIPE_BLENDFACTOR_INV_DST_ALPHA: u32 = 20;
const PIPE_BLEND_ADD: u32 = 0;
const PIPE_BLEND_SUBTRACT: u32 = 1;
const PIPE_BLEND_REVERSE_SUBTRACT: u32 = 2;
const PIPE_TEX_WRAP_REPEAT: u32 = 0;
const PIPE_TEX_WRAP_CLAMP_TO_EDGE: u32 = 2;
const PIPE_TEX_WRAP_MIRROR_REPEAT: u32 = 4;
const PIPE_TEX_FILTER_NEAREST: u32 = 0;
const PIPE_TEX_FILTER_LINEAR: u32 = 1;
const PIPE_TEX_MIPFILTER_NONE: u32 = 2;
const PIPE_SWIZZLE_X: u32 = 0;
const PIPE_SWIZZLE_Y: u32 = 1;
const PIPE_SWIZZLE_Z: u32 = 2;
const PIPE_SWIZZLE_W: u32 = 3;
const IR_VERTEX_BUFFER_BYTES: usize = MAX_IR_VERTICES * 10 * core::mem::size_of::<f32>();
const IR_BUFFER_SLOTS: usize = 1_024;
const IR_TEXTURE_SLOTS: usize = 1_024;
const IR_SAMPLER_SLOTS: usize = 256;
const IR_PIPELINE_SLOTS: usize = 256;

#[derive(Clone, Copy)]
struct FramebufferOrientation {
    origin_upper_left: bool,
}

impl FramebufferOrientation {
    const UPPER_LEFT: Self = Self {
        origin_upper_left: true,
    };

    fn viewport_scale_y(self, height: u32) -> f32 {
        let scale = height as f32 / 2.0;
        if self.origin_upper_left {
            -scale
        } else {
            scale
        }
    }
}

const VERTEX_SHADER: &str = "VERT\n\
DCL IN[0]\n\
DCL IN[1]\n\
DCL OUT[0], POSITION\n\
DCL OUT[1], COLOR\n\
  0: MOV OUT[0], IN[0]\n\
  1: MOV OUT[1], IN[1]\n\
  2: END\n";

const FRAGMENT_SHADER: &str = "FRAG\n\
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
DCL IN[0], COLOR, PERSPECTIVE\n\
DCL OUT[0], COLOR\n\
  0: MOV OUT[0], IN[0]\n\
   1: END\n";

const COMPOSITION_VERTEX_SHADER: &str = "VERT\n\
DCL IN[0]\n\
DCL IN[1]\n\
DCL IN[2]\n\
DCL OUT[0], POSITION\n\
DCL OUT[1], GENERIC[0]\n\
DCL OUT[2], COLOR\n\
  0: MOV OUT[0], IN[0]\n\
  1: MOV OUT[1], IN[1]\n\
  2: MOV OUT[2], IN[2]\n\
  3: END\n";

const COMPOSITION_TEXTURE_ALPHA_SHADER: &str = "FRAG\n\
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
DCL IN[0], GENERIC[0], PERSPECTIVE\n\
DCL IN[1], COLOR, PERSPECTIVE\n\
DCL SAMP[0]\n\
DCL SVIEW[0], 2D, FLOAT\n\
DCL OUT[0], COLOR\n\
DCL TEMP[0]\n\
  0: TEX TEMP[0], IN[0], SAMP[0], 2D\n\
  1: MOV OUT[0].xyz, TEMP[0].xyzx\n\
  2: MUL OUT[0].w, TEMP[0].wwww, IN[1].wwww\n\
  3: END\n";

const COMPOSITION_TEXTURE_OPAQUE_SHADER: &str = "FRAG\n\
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
DCL IN[0], GENERIC[0], PERSPECTIVE\n\
DCL IN[1], COLOR, PERSPECTIVE\n\
DCL SAMP[0]\n\
DCL SVIEW[0], 2D, FLOAT\n\
DCL OUT[0], COLOR\n\
DCL TEMP[0]\n\
  0: TEX TEMP[0], IN[0], SAMP[0], 2D\n\
  1: MOV OUT[0].xyz, TEMP[0].xyzx\n\
  2: MOV OUT[0].w, IN[1].wwww\n\
  3: END\n";

const COMPOSITION_SOLID_SHADER: &str = "FRAG\n\
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
DCL IN[1], COLOR, PERSPECTIVE\n\
DCL OUT[0], COLOR\n\
   0: MOV OUT[0], IN[1]\n\
   1: END\n";

const IR_VERTEX_SHADER: &str = "VERT\n\
DCL IN[0]\n\
DCL IN[1]\n\
DCL IN[2]\n\
DCL CONST[0..3]\n\
DCL OUT[0], POSITION\n\
DCL OUT[1], COLOR\n\
DCL OUT[2], GENERIC[0]\n\
DCL TEMP[0]\n\
  0: MUL TEMP[0], CONST[0], IN[0].xxxx\n\
  1: MAD TEMP[0], CONST[1], IN[0].yyyy, TEMP[0]\n\
  2: MAD TEMP[0], CONST[2], IN[0].zzzz, TEMP[0]\n\
  3: MAD OUT[0], CONST[3], IN[0].wwww, TEMP[0]\n\
  4: MOV OUT[1], IN[1]\n\
  5: MOV OUT[2], IN[2]\n\
  6: END\n";

const IR_SOLID_FRAGMENT_SHADER: &str = "FRAG\n\
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
DCL CONST[0]\n\
DCL OUT[0], COLOR\n\
  0: MOV OUT[0], CONST[0]\n\
  1: END\n";

const IR_VERTEX_COLOR_FRAGMENT_SHADER: &str = "FRAG\n\
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
DCL IN[0], COLOR, PERSPECTIVE\n\
DCL CONST[0]\n\
DCL OUT[0], COLOR\n\
  0: MUL OUT[0], IN[0], CONST[0]\n\
  1: END\n";

const IR_TEXTURE_RGBA_FRAGMENT_SHADER: &str = "FRAG\n\
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
DCL IN[0], GENERIC[0], PERSPECTIVE\n\
DCL CONST[0]\n\
DCL SAMP[0]\n\
DCL SVIEW[0], 2D, FLOAT\n\
DCL OUT[0], COLOR\n\
DCL TEMP[0]\n\
  0: TEX TEMP[0], IN[0], SAMP[0], 2D\n\
  1: MUL OUT[0], TEMP[0], CONST[0]\n\
  2: END\n";

const IR_TEXTURE_RGB_IGNORE_ALPHA_FRAGMENT_SHADER: &str = "FRAG\n\
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
DCL IN[0], GENERIC[0], PERSPECTIVE\n\
DCL CONST[0]\n\
DCL SAMP[0]\n\
DCL SVIEW[0], 2D, FLOAT\n\
DCL OUT[0], COLOR\n\
DCL TEMP[0]\n\
  0: TEX TEMP[0], IN[0], SAMP[0], 2D\n\
  1: MUL OUT[0].xyz, TEMP[0].xyzx, CONST[0].xyzx\n\
  2: MOV OUT[0].w, CONST[0].wwww\n\
  3: END\n";

const IR_TEXTURE_ALPHA_MASK_FRAGMENT_SHADER: &str = "FRAG\n\
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
DCL IN[0], GENERIC[0], PERSPECTIVE\n\
DCL CONST[0]\n\
DCL SAMP[0]\n\
DCL SVIEW[0], 2D, FLOAT\n\
DCL OUT[0], COLOR\n\
DCL TEMP[0]\n\
  0: TEX TEMP[0], IN[0], SAMP[0], 2D\n\
  1: MOV OUT[0].xyz, CONST[0].xyzx\n\
  2: MUL OUT[0].w, TEMP[0].wwww, CONST[0].wwww\n\
  3: END\n";

const IR_TEXTURE_VERTEX_COLOR_RGBA_FRAGMENT_SHADER: &str = "FRAG\n\
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
DCL IN[0], GENERIC[0], PERSPECTIVE\n\
DCL IN[1], COLOR, PERSPECTIVE\n\
DCL CONST[0]\n\
DCL SAMP[0]\n\
DCL SVIEW[0], 2D, FLOAT\n\
DCL OUT[0], COLOR\n\
DCL TEMP[0]\n\
  0: TEX TEMP[0], IN[0], SAMP[0], 2D\n\
  1: MUL TEMP[0], TEMP[0], IN[1]\n\
  2: MUL OUT[0], TEMP[0], CONST[0]\n\
  3: END\n";

const IR_TEXTURE_VERTEX_COLOR_RGB_IGNORE_ALPHA_FRAGMENT_SHADER: &str = "FRAG\n\
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
DCL IN[0], GENERIC[0], PERSPECTIVE\n\
DCL IN[1], COLOR, PERSPECTIVE\n\
DCL CONST[0]\n\
DCL SAMP[0]\n\
DCL SVIEW[0], 2D, FLOAT\n\
DCL OUT[0], COLOR\n\
DCL TEMP[0]\n\
  0: TEX TEMP[0], IN[0], SAMP[0], 2D\n\
  1: MUL TEMP[0].xyz, TEMP[0].xyzx, IN[1].xyzx\n\
  2: MUL OUT[0].xyz, TEMP[0].xyzx, CONST[0].xyzx\n\
  3: MOV OUT[0].w, CONST[0].wwww\n\
  4: END\n";

const IR_TEXTURE_VERTEX_COLOR_ALPHA_MASK_FRAGMENT_SHADER: &str = "FRAG\n\
PROPERTY FS_COLOR0_WRITES_ALL_CBUFS 1\n\
DCL IN[0], GENERIC[0], PERSPECTIVE\n\
DCL IN[1], COLOR, PERSPECTIVE\n\
DCL CONST[0]\n\
DCL SAMP[0]\n\
DCL SVIEW[0], 2D, FLOAT\n\
DCL OUT[0], COLOR\n\
DCL TEMP[0]\n\
  0: TEX TEMP[0], IN[0], SAMP[0], 2D\n\
  1: MOV OUT[0].xyz, CONST[0].xyzx\n\
  2: MUL TEMP[0].w, TEMP[0].wwww, IN[1].wwww\n\
  3: MUL OUT[0].w, TEMP[0].wwww, CONST[0].wwww\n\
  4: END\n";

pub(crate) struct Device {
    raw: RawGpu,
    dialect: RawDialect,
    capabilities: Capabilities,
}

impl Device {
    pub(crate) fn open(path: &str) -> HandleResult<Self> {
        let raw = RawGpu::open(path)?;
        let info = raw.query_info()?;
        if info.result != GPU_RESULT_SUCCESS || info.device_state != GPU_DEVICE_STATE_READY {
            return Err(HandleError::Unsupported);
        }

        let capabilities = Capabilities {
            rendering: info.execution_support & GPU_EXECUTION_SUPPORT_QUEUE != 0,
            presentation: info.execution_support & GPU_EXECUTION_SUPPORT_PRESENTATION != 0,
            image_upload: info.execution_support & GPU_EXECUTION_SUPPORT_IMAGE_UPLOAD != 0,
            depth: info.execution_support & GPU_EXECUTION_SUPPORT_DEPTH != 0,
        };
        if !capabilities.supports_rendering() {
            return Err(HandleError::Unsupported);
        }

        let dialect = raw.query_dialect(0)?;
        Ok(Self {
            raw,
            dialect,
            capabilities,
        })
    }

    pub(crate) const fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    pub(crate) fn create_context(self: &Rc<Self>) -> HandleResult<Context> {
        Ok(Context {
            device: Rc::clone(self),
            raw: self.raw.create_context(&self.dialect)?,
            next_object_handle: Cell::new(FIRST_DYNAMIC_OBJECT_HANDLE),
        })
    }
}

pub(crate) struct Context {
    device: Rc<Device>,
    raw: RawContext,
    next_object_handle: Cell<u32>,
}

impl Context {
    pub(crate) fn create_image(&self, width: u32, height: u32) -> HandleResult<Image> {
        if width == 0 || height == 0 {
            return Err(HandleError::InvalidParameter);
        }

        let raw = self.device.raw.create_image(width, height)?;
        let resource_id = resource_id_from_token(self.raw.attach_image(&raw)?)?;
        Ok(Image {
            raw,
            resource_id,
            context_handle: self.handle_id(),
            orientation: FramebufferOrientation::UPPER_LEFT,
            width,
            height,
            composition_surface_handle: self.allocate_object_handle()?,
            composition_surface_initialized: Cell::new(false),
            ir_surface_handle: self.allocate_object_handle()?,
            ir_surface_initialized: Cell::new(false),
        })
    }

    pub(crate) fn create_shared_image(&self, width: u32, height: u32) -> HandleResult<Image> {
        if width == 0 || height == 0 {
            return Err(HandleError::InvalidParameter);
        }

        let raw = self.device.raw.create_image_with_usage(
            width,
            height,
            GPU_IMAGE_USAGE_RENDER_TARGET | GPU_IMAGE_USAGE_PRESENTABLE | GPU_IMAGE_USAGE_SAMPLED,
        )?;
        let resource_id = resource_id_from_token(self.raw.attach_image(&raw)?)?;
        Ok(Image {
            raw,
            resource_id,
            context_handle: self.handle_id(),
            orientation: FramebufferOrientation::UPPER_LEFT,
            width,
            height,
            composition_surface_handle: self.allocate_object_handle()?,
            composition_surface_initialized: Cell::new(false),
            ir_surface_handle: self.allocate_object_handle()?,
            ir_surface_initialized: Cell::new(false),
        })
    }

    pub(crate) fn create_sampled_bgra_texture(
        &self,
        width: u32,
        height: u32,
    ) -> HandleResult<Texture> {
        if width == 0 || height == 0 {
            return Err(HandleError::InvalidParameter);
        }
        if !self.device.capabilities.supports_image_upload() {
            return Err(HandleError::Unsupported);
        }
        let raw = self.device.raw.create_image_with_usage(
            width,
            height,
            GPU_IMAGE_USAGE_SAMPLED | GPU_IMAGE_USAGE_TRANSFER_DST,
        )?;
        let resource_id = resource_id_from_token(self.raw.attach_image(&raw)?)?;
        Ok(Texture {
            raw,
            resource_id,
            width,
            height,
            context_handle: self.handle_id(),
            sampler_view_handle: self.allocate_object_handle()?,
            sampler_view_initialized: Cell::new(false),
            ir_surface_handle: self.allocate_object_handle()?,
            ir_surface_initialized: Cell::new(false),
        })
    }

    pub(crate) fn create_imported_bgra_texture(
        &self,
        shared_memory: &SharedMemory,
        width: u32,
        height: u32,
        shm_offset: usize,
        source_stride: u32,
    ) -> HandleResult<Texture> {
        if width == 0 || height == 0 || !self.device.capabilities.supports_image_upload() {
            return Err(HandleError::InvalidParameter);
        }
        let shm_offset = u64::try_from(shm_offset).map_err(|_| HandleError::InvalidParameter)?;
        let raw = self.device.raw.create_imported_bgra_image(
            shared_memory,
            width,
            height,
            shm_offset,
            source_stride,
        )?;
        let resource_id = resource_id_from_token(self.raw.attach_image(&raw)?)?;
        Ok(Texture {
            raw,
            resource_id,
            width,
            height,
            context_handle: self.handle_id(),
            sampler_view_handle: self.allocate_object_handle()?,
            sampler_view_initialized: Cell::new(false),
            ir_surface_handle: self.allocate_object_handle()?,
            ir_surface_initialized: Cell::new(false),
        })
    }

    pub(crate) fn import_shared_bgra_texture(
        &self,
        handle: Handle,
    ) -> HandleResult<(Texture, u32, u32)> {
        let raw = RawImage::from_handle(handle)?;
        let info = raw.query()?;
        if info.format != gpu_raw::GPU_IMAGE_FORMAT_BGRA8_UNORM
            || info.usage & GPU_IMAGE_USAGE_SAMPLED == 0
            || info.width == 0
            || info.height == 0
        {
            return Err(HandleError::InvalidParameter);
        }
        let resource_id = resource_id_from_token(self.raw.attach_image(&raw)?)?;
        let texture = Texture {
            raw,
            resource_id,
            width: info.width,
            height: info.height,
            context_handle: self.handle_id(),
            sampler_view_handle: self.allocate_object_handle()?,
            sampler_view_initialized: Cell::new(false),
            ir_surface_handle: self.allocate_object_handle()?,
            ir_surface_initialized: Cell::new(false),
        };
        Ok((texture, info.width, info.height))
    }

    pub(crate) fn upload_texture_bgra(
        &self,
        texture: &Texture,
        pixels: &[u8],
        source_stride: u32,
        damage: PixelRect,
    ) -> HandleResult<()> {
        if texture.context_handle != self.handle_id()
            || !damage.is_within(texture.width, texture.height)
        {
            return Err(HandleError::InvalidParameter);
        }
        self.raw.upload_image_bgra(
            &texture.raw,
            pixels,
            source_stride,
            GpuImageBgraRect::new(damage.x(), damage.y(), damage.width(), damage.height()),
        )
    }

    fn create_ir_texture(&self, spec: IrTextureSpec) -> HandleResult<Texture> {
        if spec.width == 0 || spec.height == 0 || spec.present {
            return Err(HandleError::InvalidParameter);
        }
        let mut usage = 0;
        if spec.render_attachment {
            usage |= GPU_IMAGE_USAGE_RENDER_TARGET;
        }
        if spec.sampled {
            usage |= GPU_IMAGE_USAGE_SAMPLED;
        }
        if spec.copy_destination {
            usage |= GPU_IMAGE_USAGE_TRANSFER_DST;
        }
        if matches!(spec.format, IrTextureFormat::Depth32Float) {
            usage = GPU_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT;
        }
        if usage == 0 {
            return Err(HandleError::InvalidParameter);
        }
        let format = if matches!(spec.format, IrTextureFormat::Depth32Float) {
            GPU_IMAGE_FORMAT_DEPTH32_FLOAT
        } else {
            GPU_IMAGE_FORMAT_BGRA8_UNORM
        };
        let raw = self.device.raw.create_image_with_format_and_usage(
            format,
            spec.width,
            spec.height,
            usage,
        )?;
        let resource_id = resource_id_from_token(self.raw.attach_image(&raw)?)?;
        Ok(Texture {
            raw,
            resource_id,
            width: spec.width,
            height: spec.height,
            context_handle: self.handle_id(),
            sampler_view_handle: self.allocate_object_handle()?,
            sampler_view_initialized: Cell::new(false),
            ir_surface_handle: self.allocate_object_handle()?,
            ir_surface_initialized: Cell::new(false),
        })
    }

    pub(crate) fn transfer_imported_bgra_rect(
        &self,
        texture: &Texture,
        damage: PixelRect,
    ) -> HandleResult<()> {
        if texture.context_handle != self.handle_id()
            || !damage.is_within(texture.width, texture.height)
        {
            return Err(HandleError::InvalidParameter);
        }
        self.raw.transfer_imported_image_bgra(
            &texture.raw,
            GpuImageBgraRect::new(damage.x(), damage.y(), damage.width(), damage.height()),
        )
    }

    pub(crate) fn release_texture(&self, texture: Texture) -> HandleResult<()> {
        if texture.context_handle != self.handle_id() {
            return Err(HandleError::InvalidParameter);
        }
        self.raw.detach_image(&texture.raw)
    }

    pub(crate) fn release_image(&self, image: Image) -> HandleResult<()> {
        if image.context_handle != self.handle_id() {
            return Err(HandleError::InvalidParameter);
        }
        self.raw.detach_image(&image.raw)
    }

    pub(crate) fn create_pipeline(
        &self,
        image: &Image,
        description: PipelineDesc,
    ) -> HandleResult<Pipeline> {
        if image.context_handle != self.handle_id()
            || description.kind() != PipelineKind::ClipSpaceVertexColor
            || description.max_vertices() == 0
        {
            return Err(HandleError::InvalidParameter);
        }

        let vertex_bytes = description
            .max_vertices()
            .checked_mul(core::mem::size_of::<VertexClip4Color3>())
            .ok_or(HandleError::InvalidParameter)?;
        let raw_buffer = self.device.raw.create_buffer(vertex_bytes as u64, 0)?;
        let vertex_resource_id = resource_id_from_token(self.raw.attach_buffer(&raw_buffer)?)?;
        Ok(Pipeline {
            vertex_buffer: raw_buffer,
            target_resource_id: image.resource_id,
            vertex_resource_id,
            context_handle: self.handle_id(),
            max_vertices: description.max_vertices(),
            cull_mode: description.cull_mode(),
            front_face: description.front_face(),
            initialized: Cell::new(false),
        })
    }

    pub(crate) fn create_queue(&self) -> HandleResult<Queue> {
        let max_vertices = MAX_COMPOSITION_OPERATIONS
            .checked_mul(6)
            .ok_or(HandleError::InvalidParameter)?;
        let vertex_bytes = max_vertices
            .checked_mul(core::mem::size_of::<CompositionVertex>())
            .ok_or(HandleError::InvalidParameter)?;
        let raw_vertex_buffer = self.device.raw.create_buffer(
            u64::try_from(vertex_bytes).map_err(|_| HandleError::InvalidParameter)?,
            0,
        )?;
        let composition_vertex_resource_id =
            resource_id_from_token(self.raw.attach_buffer(&raw_vertex_buffer)?)?;
        Ok(Queue {
            raw: self.raw.create_queue()?,
            context_handle: self.handle_id(),
            composition_vertex_buffer: raw_vertex_buffer,
            composition_vertex_resource_id,
            composition_initialized: Cell::new(false),
        })
    }

    pub(crate) fn create_ir_resources(&self) -> HandleResult<IrResources> {
        let vertex_bytes =
            u64::try_from(IR_VERTEX_BUFFER_BYTES).map_err(|_| HandleError::InvalidParameter)?;
        let vertex_buffer = self.device.raw.create_buffer(vertex_bytes, 0)?;
        let vertex_resource_id = resource_id_from_token(self.raw.attach_buffer(&vertex_buffer)?)?;
        Ok(IrResources {
            context_handle: self.handle_id(),
            vertex_buffer,
            vertex_resource_id,
            buffers: empty_slots(IR_BUFFER_SLOTS)?,
            textures: empty_slots(IR_TEXTURE_SLOTS)?,
            texture_specs: empty_slots(IR_TEXTURE_SLOTS)?,
            samplers: empty_slots(IR_SAMPLER_SLOTS)?,
            pipelines: empty_slots(IR_PIPELINE_SLOTS)?,
            vertex_shader_handle: self.allocate_object_handle()?,
            solid_fragment_shader_handle: self.allocate_object_handle()?,
            vertex_color_fragment_shader_handle: self.allocate_object_handle()?,
            texture_rgba_fragment_shader_handle: self.allocate_object_handle()?,
            texture_rgb_ignore_alpha_fragment_shader_handle: self.allocate_object_handle()?,
            texture_alpha_mask_fragment_shader_handle: self.allocate_object_handle()?,
            texture_vertex_color_rgba_fragment_shader_handle: self.allocate_object_handle()?,
            texture_vertex_color_rgb_ignore_alpha_fragment_shader_handle: self
                .allocate_object_handle()?,
            texture_vertex_color_alpha_mask_fragment_shader_handle: self
                .allocate_object_handle()?,
            vertex_elements_handle: self.allocate_object_handle()?,
            initialized: Cell::new(false),
        })
    }

    pub(crate) fn map_ir_image(
        &self,
        resources: &mut IrResources,
        texture: IrTextureSpec,
        image: &Image,
    ) -> HandleResult<()> {
        if resources.context_handle != self.handle_id()
            || image.context_handle != self.handle_id()
            || texture.slot >= resources.textures.len()
            || texture.width != image.width
            || texture.height != image.height
            || !texture.render_attachment
            || !texture.present
        {
            return Err(HandleError::InvalidParameter);
        }
        let slot = resources
            .textures
            .get_mut(texture.slot)
            .ok_or(HandleError::InvalidParameter)?;
        if slot.is_some() {
            return Err(HandleError::InvalidParameter);
        }
        *slot = Some(IrTexture::Mapped(MappedIrTexture {
            resource_id: image.resource_id,
            width: image.width,
            height: image.height,
            sampler_view_handle: self.allocate_object_handle()?,
            sampler_view_initialized: Cell::new(false),
            surface_handle: image.ir_surface_handle,
            surface_initialized: Cell::new(image.ir_surface_initialized.get()),
        }));
        if let Some(spec) = resources.texture_specs.get_mut(texture.slot) {
            *spec = Some(texture);
        }
        Ok(())
    }

    pub(crate) fn context_id(&self) -> i32 {
        self.handle_id()
    }

    fn handle_id(&self) -> i32 {
        self.raw.as_handle().as_raw()
    }

    fn allocate_object_handle(&self) -> HandleResult<u32> {
        let handle = self.next_object_handle.get();
        let next_handle = handle.checked_add(1).ok_or(HandleError::OutOfResources)?;
        if handle == 0 {
            return Err(HandleError::OutOfResources);
        }
        self.next_object_handle.set(next_handle);
        Ok(handle)
    }
}

pub(crate) struct Queue {
    raw: RawQueue,
    context_handle: i32,
    composition_vertex_buffer: RawBuffer,
    composition_vertex_resource_id: u32,
    composition_initialized: Cell<bool>,
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

pub(crate) struct IrResources {
    context_handle: i32,
    vertex_buffer: RawBuffer,
    vertex_resource_id: u32,
    buffers: Vec<Option<IrBuffer>>,
    textures: Vec<Option<IrTexture>>,
    texture_specs: Vec<Option<IrTextureSpec>>,
    samplers: Vec<Option<IrSampler>>,
    pipelines: Vec<Option<IrPipeline>>,
    vertex_shader_handle: u32,
    solid_fragment_shader_handle: u32,
    vertex_color_fragment_shader_handle: u32,
    texture_rgba_fragment_shader_handle: u32,
    texture_rgb_ignore_alpha_fragment_shader_handle: u32,
    texture_alpha_mask_fragment_shader_handle: u32,
    texture_vertex_color_rgba_fragment_shader_handle: u32,
    texture_vertex_color_rgb_ignore_alpha_fragment_shader_handle: u32,
    texture_vertex_color_alpha_mask_fragment_shader_handle: u32,
    vertex_elements_handle: u32,
    initialized: Cell<bool>,
}

struct IrBuffer {
    _raw: RawBuffer,
    resource_id: u32,
    size: u64,
    uploaded_revision: Option<u64>,
}

struct IrSampler {
    handle: u32,
    state: IrSamplerState,
    initialized: Cell<bool>,
}

struct IrPipeline {
    blend_handle: u32,
    rasterizer_handle: u32,
    dsa_handle: Option<u32>,
    state: IrPipelineState,
    initialized: Cell<bool>,
}

enum IrTexture {
    Internal(Texture),
    Mapped(MappedIrTexture),
}

struct MappedIrTexture {
    resource_id: u32,
    width: u32,
    height: u32,
    sampler_view_handle: u32,
    sampler_view_initialized: Cell<bool>,
    surface_handle: u32,
    surface_initialized: Cell<bool>,
}

struct IrPassTarget {
    resource_id: u32,
    surface_handle: u32,
    surface_initialized: bool,
    width: u32,
    height: u32,
    orientation: FramebufferOrientation,
}

struct IrPassDepthTarget {
    surface_handle: u32,
    surface_initialized: bool,
}

impl IrTexture {
    fn resource_id(&self) -> u32 {
        match self {
            Self::Internal(texture) => texture.resource_id,
            Self::Mapped(texture) => texture.resource_id,
        }
    }

    fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::Internal(texture) => (texture.width, texture.height),
            Self::Mapped(texture) => (texture.width, texture.height),
        }
    }

    fn sampler_view_handle(&self) -> u32 {
        match self {
            Self::Internal(texture) => texture.sampler_view_handle,
            Self::Mapped(texture) => texture.sampler_view_handle,
        }
    }

    fn sampler_view_initialized(&self) -> bool {
        match self {
            Self::Internal(texture) => texture.sampler_view_initialized.get(),
            Self::Mapped(texture) => texture.sampler_view_initialized.get(),
        }
    }

    fn set_sampler_view_initialized(&self) {
        match self {
            Self::Internal(texture) => texture.sampler_view_initialized.set(true),
            Self::Mapped(texture) => texture.sampler_view_initialized.set(true),
        }
    }

    fn surface_handle(&self) -> u32 {
        match self {
            Self::Internal(texture) => texture.ir_surface_handle,
            Self::Mapped(texture) => texture.surface_handle,
        }
    }

    fn surface_initialized(&self) -> bool {
        match self {
            Self::Internal(texture) => texture.ir_surface_initialized.get(),
            Self::Mapped(texture) => texture.surface_initialized.get(),
        }
    }

    fn set_surface_initialized(&self) {
        match self {
            Self::Internal(texture) => texture.ir_surface_initialized.set(true),
            Self::Mapped(texture) => texture.surface_initialized.set(true),
        }
    }
}

impl Drop for IrResources {
    fn drop(&mut self) {
        let _ = &self.vertex_buffer;
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CompositionVertex {
    clip_position: [f32; 4],
    uv: [f32; 2],
    color: [f32; 4],
}

enum CompositionDrawKind<'a> {
    Textured {
        texture: &'a Texture,
        source_alpha: SourceAlpha,
    },
    Solid,
}

struct CompositionDraw<'a> {
    start_vertex: usize,
    clip: PixelRect,
    kind: CompositionDrawKind<'a>,
}

struct CompositionQuad {
    target_width: u32,
    target_height: u32,
    destination: PixelRect,
    source: PixelRect,
    source_width: u32,
    source_height: u32,
    color: [f32; 4],
}

impl Queue {
    pub(crate) fn context_id(&self) -> i32 {
        self.context_handle
    }

    pub(crate) fn prepare_ir_buffer(
        &self,
        context: &Context,
        resources: &mut IrResources,
        spec: IrBufferSpec,
        bytes: &[u8],
    ) -> HandleResult<()> {
        if self.context_handle != context.handle_id()
            || resources.context_handle != context.handle_id()
            || spec.size == 0
            || spec.size > u64::from(u32::MAX)
            || bytes.is_empty()
            || !bytes.len().is_multiple_of(core::mem::size_of::<u32>())
            || u64::try_from(bytes.len()).map_or(true, |length| length > spec.size)
        {
            return Err(HandleError::InvalidParameter);
        }
        let (resource_id, uploaded_revision) = {
            let buffer = ir_buffer(context, resources, spec)?;
            (buffer.resource_id, buffer.uploaded_revision)
        };
        if uploaded_revision == Some(spec.revision) {
            return Ok(());
        }

        const INLINE_WRITE_FIXED_BYTES: usize = 12 * core::mem::size_of::<u32>();
        let max_payload = (self.raw.max_opaque_command_size() as usize)
            .checked_sub(INLINE_WRITE_FIXED_BYTES)
            .map(|bytes| bytes & !(core::mem::size_of::<u32>() - 1))
            .filter(|bytes| *bytes > 0)
            .ok_or(HandleError::InvalidParameter)?;
        let mut offset = 0usize;
        while offset < bytes.len() {
            let end = offset.saturating_add(max_payload).min(bytes.len());
            let chunk = bytes
                .get(offset..end)
                .ok_or(HandleError::InvalidParameter)?;
            let mut commands = Vec::new();
            commands
                .try_reserve_exact(INLINE_WRITE_FIXED_BYTES.saturating_add(chunk.len()))
                .map_err(|_| HandleError::OutOfResources)?;
            push_ir_buffer_inline_write(
                &mut commands,
                resource_id,
                u32::try_from(offset).map_err(|_| HandleError::InvalidParameter)?,
                chunk,
            )?;
            self.raw.submit(&commands)?;
            offset = end;
        }
        let buffer = ir_buffer(context, resources, spec)?;
        buffer.uploaded_revision = Some(spec.revision);
        Ok(())
    }

    pub(crate) fn upload_ir_texture(
        &self,
        context: &Context,
        resources: &mut IrResources,
        spec: IrTextureSpec,
        upload: &IrTextureUpload,
    ) -> HandleResult<()> {
        if self.context_handle != context.handle_id()
            || resources.context_handle != context.handle_id()
            || upload.texture.slot != spec.slot
            || upload.texture.width != spec.width
            || upload.texture.height != spec.height
        {
            return Err(HandleError::InvalidParameter);
        }
        let texture = ir_texture(context, resources, spec)?;
        let IrTexture::Internal(texture) = texture else {
            return Err(HandleError::InvalidParameter);
        };
        context.upload_texture_bgra(
            texture,
            &upload.pixels,
            upload
                .destination
                .width
                .checked_mul(4)
                .ok_or(HandleError::InvalidParameter)?,
            ir_rect_to_pixel_rect(upload.destination)?,
        )
    }

    pub(crate) fn copy_ir_texture(
        &self,
        context: &Context,
        resources: &mut IrResources,
        copy: IrTextureCopy,
    ) -> HandleResult<()> {
        if self.context_handle != context.handle_id()
            || resources.context_handle != context.handle_id()
            || !ir_rect_is_within(copy.source_rect, copy.source.width, copy.source.height)
            || !ir_rect_is_within(
                copy.destination_rect,
                copy.destination.width,
                copy.destination.height,
            )
            || copy.source_rect.width != copy.destination_rect.width
            || copy.source_rect.height != copy.destination_rect.height
        {
            return Err(HandleError::InvalidParameter);
        }
        let source = ir_texture(context, resources, copy.source)?;
        let source_id = source.resource_id();
        let destination = ir_texture(context, resources, copy.destination)?;
        let destination_id = destination.resource_id();
        let mut commands = Vec::new();
        commands
            .try_reserve_exact(56)
            .map_err(|_| HandleError::OutOfResources)?;
        push_resource_copy(
            &mut commands,
            destination_id,
            copy.destination_rect,
            source_id,
            copy.source_rect,
        );
        self.raw.submit(&commands).map(|_| ())
    }

    pub(crate) fn submit_ir_internal(
        &self,
        context: &Context,
        resources: &mut IrResources,
        target: IrTextureSpec,
        submission: &IrSubmission,
    ) -> HandleResult<()> {
        if !target.render_attachment
            || self.context_handle != context.handle_id()
            || resources.context_handle != context.handle_id()
        {
            return Err(HandleError::InvalidParameter);
        }
        let pass_target = {
            let texture = ir_texture(context, resources, target)?;
            let (width, height) = texture.dimensions();
            if matches!(texture, IrTexture::Mapped(_))
                || width != target.width
                || height != target.height
            {
                return Err(HandleError::InvalidParameter);
            }
            IrPassTarget {
                resource_id: texture.resource_id(),
                surface_handle: texture.surface_handle(),
                surface_initialized: texture.surface_initialized(),
                width,
                height,
                orientation: FramebufferOrientation::UPPER_LEFT,
            }
        };
        self.submit_ir_target(context, resources, pass_target, submission)?;
        let texture = resources
            .textures
            .get(target.slot)
            .and_then(Option::as_ref)
            .ok_or(HandleError::InvalidParameter)?;
        texture.set_surface_initialized();
        Ok(())
    }

    pub(crate) fn submit_ir(
        &self,
        context: &Context,
        resources: &mut IrResources,
        image: &Image,
        submission: &IrSubmission,
    ) -> HandleResult<()> {
        self.submit_ir_target(
            context,
            resources,
            IrPassTarget {
                resource_id: image.resource_id,
                surface_handle: image.ir_surface_handle,
                surface_initialized: image.ir_surface_initialized.get(),
                width: image.width,
                height: image.height,
                orientation: image.orientation,
            },
            submission,
        )?;
        image.ir_surface_initialized.set(true);
        Ok(())
    }

    fn submit_ir_target(
        &self,
        context: &Context,
        resources: &mut IrResources,
        target: IrPassTarget,
        submission: &IrSubmission,
    ) -> HandleResult<()> {
        if self.context_handle != context.handle_id()
            || resources.context_handle != context.handle_id()
            || submission.vertices.len() > MAX_IR_VERTICES
            || submission.draws.is_empty()
            || !ir_rect_is_within(submission.render_area, target.width, target.height)
            || submission
                .clear_color
                .is_some_and(|color| !color.iter().all(|component| component.is_finite()))
        {
            return Err(HandleError::InvalidParameter);
        }

        let depth_target = if let Some(spec) = submission.depth_attachment {
            if !matches!(spec.format, IrTextureFormat::Depth32Float)
                || spec.width != target.width
                || spec.height != target.height
                || submission
                    .clear_depth
                    .is_some_and(|depth| !depth.is_finite() || !(0.0..=1.0).contains(&depth))
            {
                return Err(HandleError::InvalidParameter);
            }
            let texture = ir_texture(context, resources, spec)?;
            Some(IrPassDepthTarget {
                surface_handle: texture.surface_handle(),
                surface_initialized: texture.surface_initialized(),
            })
        } else {
            if submission.clear_depth.is_some() {
                return Err(HandleError::InvalidParameter);
            }
            None
        };

        let mut commands = Vec::new();
        commands
            .try_reserve(16 * 1024)
            .map_err(|_| HandleError::OutOfResources)?;
        let mut initialized_samplers = Vec::new();
        let mut initialized_pipelines = Vec::new();
        let mut initialized_views = Vec::new();
        if !resources.initialized.get() {
            push_ir_setup(&mut commands, resources);
        }
        if !target.surface_initialized {
            push_surface(&mut commands, target.surface_handle, target.resource_id);
        }
        if let (Some(depth_spec), Some(depth_target)) =
            (submission.depth_attachment, depth_target.as_ref())
            && !depth_target.surface_initialized
        {
            let texture = resources
                .textures
                .get(depth_spec.slot)
                .and_then(Option::as_ref)
                .ok_or(HandleError::InvalidParameter)?;
            push_depth_surface(
                &mut commands,
                depth_target.surface_handle,
                texture.resource_id(),
            );
        }
        for upload in &submission.texture_uploads {
            let texture = ir_texture(context, resources, IrTextureSpec { ..upload.texture })?;
            let view_is_pending = initialized_views.contains(&upload.texture.slot);
            if !texture.sampler_view_initialized() && !view_is_pending {
                push_sampler_view(
                    &mut commands,
                    texture.sampler_view_handle(),
                    texture.resource_id(),
                );
                initialized_views.push(upload.texture.slot);
            }
        }
        for draw in &submission.draws {
            validate_ir_draw(
                resources,
                target.width,
                target.height,
                draw,
                submission.vertices.len(),
            )?;
            let pipeline = ir_pipeline(context, resources, draw.pipeline)?;
            let pipeline_is_pending = initialized_pipelines.contains(&draw.pipeline.slot);
            if !pipeline.initialized.get() && !pipeline_is_pending {
                push_ir_pipeline(&mut commands, pipeline);
                initialized_pipelines.push(draw.pipeline.slot);
            }
            if let (Some(texture_spec), Some(sampler)) = (draw.texture, draw.sampler) {
                let texture = ir_texture(context, resources, texture_spec)?;
                let view_is_pending = initialized_views.contains(&texture_spec.slot);
                if !texture.sampler_view_initialized() && !view_is_pending {
                    push_sampler_view(
                        &mut commands,
                        texture.sampler_view_handle(),
                        texture.resource_id(),
                    );
                    initialized_views.push(texture_spec.slot);
                }
                let sampler = ir_sampler(context, resources, sampler)?;
                let sampler_is_pending = initialized_samplers.contains(&sampler.state.slot);
                if !sampler.initialized.get() && !sampler_is_pending {
                    push_ir_sampler(&mut commands, sampler);
                    initialized_samplers.push(sampler.state.slot);
                }
            }
        }

        push_ir_bind_pass_state(
            &mut commands,
            target.surface_handle,
            depth_target
                .as_ref()
                .map_or(0, |depth| depth.surface_handle),
            resources.vertex_resource_id,
            target.width,
            target.height,
            target.orientation,
            resources.vertex_shader_handle,
            resources.vertex_elements_handle,
        );
        let pass_scissor = ir_rect_to_pixel_rect(submission.render_area)?;
        push_ir_scissor(&mut commands, pass_scissor)?;
        let full_area = submission.render_area.x == 0
            && submission.render_area.y == 0
            && submission.render_area.width == target.width
            && submission.render_area.height == target.height;
        if full_area && (submission.clear_color.is_some() || submission.clear_depth.is_some()) {
            push_ir_clear(
                &mut commands,
                submission.clear_color,
                submission.clear_depth,
            );
        } else {
            if let Some(clear_color) = submission.clear_color {
                push_ir_clear_surface(
                    &mut commands,
                    target.surface_handle,
                    submission.render_area,
                    clear_color,
                )?;
            }
            if let (Some(clear_depth), Some(depth_target)) =
                (submission.clear_depth, depth_target.as_ref())
            {
                push_ir_clear_depth_surface(
                    &mut commands,
                    depth_target.surface_handle,
                    submission.render_area,
                    clear_depth,
                )?;
            }
        }
        if !submission.vertices.is_empty() {
            push_ir_inline_write(
                &mut commands,
                resources.vertex_resource_id,
                &submission.vertices,
            )?;
        }

        // Re-emitting the complete state for every draw used to make a pass
        // exceed the queue's 64 KiB opaque-command limit even when the vertex
        // payload itself fit below MAX_IR_VERTICES. Cache state that is purely
        // binding data, but reassert rasterizer and DSA state for every draw:
        // these affect face visibility and depth writes and must not be allowed
        // to leak across the backend's draw setup.
        let mut bound_blend = None;
        let mut bound_fragment_shader = None;
        let mut bound_sampler_view = None;
        let mut bound_sampler_state = None;
        let mut bound_vertex_buffer = Some((resources.vertex_resource_id, 0));
        let mut bound_scissor = Some((
            pass_scissor.x(),
            pass_scissor.y(),
            pass_scissor.width(),
            pass_scissor.height(),
        ));
        for draw in &submission.draws {
            let pipeline = resources
                .pipelines
                .get(draw.pipeline.slot)
                .and_then(Option::as_ref)
                .ok_or(HandleError::InvalidParameter)?;
            let vertex_buffer = if let Some(binding) = draw.vertex_buffer {
                let buffer = resources
                    .buffers
                    .get(binding.buffer.slot)
                    .and_then(Option::as_ref)
                    .filter(|buffer| {
                        buffer.size == binding.buffer.size
                            && buffer.uploaded_revision == Some(binding.buffer.revision)
                    })
                    .ok_or(HandleError::InvalidParameter)?;
                (buffer.resource_id, binding.offset)
            } else {
                (resources.vertex_resource_id, 0)
            };
            if bound_vertex_buffer != Some(vertex_buffer) {
                push_ir_vertex_buffer(&mut commands, vertex_buffer.0, vertex_buffer.1);
                bound_vertex_buffer = Some(vertex_buffer);
            }
            if bound_blend != Some(pipeline.blend_handle) {
                push_bind_object(&mut commands, VIRGL_OBJECT_BLEND, pipeline.blend_handle);
                bound_blend = Some(pipeline.blend_handle);
            }
            push_bind_object(
                &mut commands,
                VIRGL_OBJECT_RASTERIZER,
                pipeline.rasterizer_handle,
            );
            if let Some(dsa_handle) = pipeline.dsa_handle {
                push_bind_object(&mut commands, VIRGL_OBJECT_DSA, dsa_handle);
            }
            let fragment_shader = ir_fragment_shader_handle(resources, draw.pipeline.fragment);
            if bound_fragment_shader != Some(fragment_shader) {
                push_fragment_shader(&mut commands, fragment_shader);
                bound_fragment_shader = Some(fragment_shader);
            }
            push_constant_buffer(&mut commands, PIPE_SHADER_VERTEX, &draw.uniforms.transform)?;
            push_constant_buffer(&mut commands, PIPE_SHADER_FRAGMENT, &draw.uniforms.color)?;
            let scissor = ir_rect_to_pixel_rect(draw.scissor)?;
            let scissor_key = (scissor.x(), scissor.y(), scissor.width(), scissor.height());
            if bound_scissor != Some(scissor_key) {
                push_ir_scissor(&mut commands, scissor)?;
                bound_scissor = Some(scissor_key);
            }
            if let (Some(texture_spec), Some(sampler)) = (draw.texture, draw.sampler) {
                let texture = resources
                    .textures
                    .get(texture_spec.slot)
                    .and_then(Option::as_ref)
                    .ok_or(HandleError::InvalidParameter)?;
                let sampler = resources
                    .samplers
                    .get(sampler.slot)
                    .and_then(Option::as_ref)
                    .ok_or(HandleError::InvalidParameter)?;
                let sampler_view = texture.sampler_view_handle();
                if bound_sampler_view != Some(sampler_view) {
                    push_sampler_view_binding(&mut commands, sampler_view);
                    bound_sampler_view = Some(sampler_view);
                }
                if bound_sampler_state != Some(sampler.handle) {
                    push_sampler_state_binding(&mut commands, sampler.handle);
                    bound_sampler_state = Some(sampler.handle);
                }
            }
            push_draw(&mut commands, draw.start_vertex, draw.vertex_count)?;
        }
        if commands.len() > self.raw.max_opaque_command_size() as usize {
            return Err(HandleError::InvalidParameter);
        }

        for upload in &submission.texture_uploads {
            let texture = resources
                .textures
                .get(upload.texture.slot)
                .and_then(Option::as_ref)
                .ok_or(HandleError::InvalidParameter)?;
            let IrTexture::Internal(texture) = texture else {
                return Err(HandleError::InvalidParameter);
            };
            context.upload_texture_bgra(
                texture,
                &upload.pixels,
                upload
                    .destination
                    .width
                    .checked_mul(4)
                    .ok_or(HandleError::InvalidParameter)?,
                ir_rect_to_pixel_rect(upload.destination)?,
            )?;
        }
        self.raw.submit(&commands)?;
        if !resources.initialized.get() {
            resources.initialized.set(true);
        }
        for slot in initialized_samplers {
            if let Some(Some(sampler)) = resources.samplers.get(slot) {
                sampler.initialized.set(true);
            }
        }
        for slot in initialized_pipelines {
            if let Some(Some(pipeline)) = resources.pipelines.get(slot) {
                pipeline.initialized.set(true);
            }
        }
        for slot in initialized_views {
            if let Some(Some(texture)) = resources.textures.get(slot) {
                texture.set_sampler_view_initialized();
            }
        }
        if let Some(depth) = submission.depth_attachment
            && let Some(Some(texture)) = resources.textures.get(depth.slot)
        {
            texture.set_surface_initialized();
        }
        Ok(())
    }

    pub(crate) fn submit(
        &self,
        image: &Image,
        viewport: Viewport,
        clear_color: Color,
        pipeline: &Pipeline,
        vertices: &[VertexClip4Color3],
    ) -> HandleResult<()> {
        if image.context_handle != self.context_handle
            || pipeline.context_handle != self.context_handle
            || pipeline.target_resource_id != image.resource_id
            || viewport.width() == 0
            || viewport.height() == 0
            || vertices.is_empty()
            || vertices.len() > pipeline.max_vertices
            || !vertices.len().is_multiple_of(3)
        {
            return Err(HandleError::InvalidParameter);
        }

        let needs_setup = !pipeline.initialized.get();
        let mut commands = if needs_setup {
            build_setup_commands(
                pipeline.target_resource_id,
                pipeline.vertex_resource_id,
                pipeline.cull_mode,
                pipeline.front_face,
            )
        } else {
            Vec::with_capacity(2048)
        };
        push_legacy_bind_state(&mut commands, pipeline.vertex_resource_id);
        push_viewport(&mut commands, viewport, image.orientation);
        push_inline_write(&mut commands, pipeline.vertex_resource_id, vertices);
        push_clear_and_draw(&mut commands, clear_color, vertices.len());
        self.raw.submit(&commands)?;
        if needs_setup {
            pipeline.initialized.set(true);
        }
        Ok(())
    }

    pub(crate) fn submit_composition(
        &self,
        image: &Image,
        clear_color: Color,
        operations: &[CompositionOperation<'_>],
    ) -> HandleResult<()> {
        if image.context_handle != self.context_handle
            || !clear_color.is_finite_unit()
            || operations.len() > MAX_COMPOSITION_OPERATIONS
        {
            return Err(HandleError::InvalidParameter);
        }

        let max_vertices = MAX_COMPOSITION_OPERATIONS
            .checked_mul(6)
            .ok_or(HandleError::InvalidParameter)?;
        let mut vertices = Vec::new();
        vertices
            .try_reserve(max_vertices)
            .map_err(|_| HandleError::OutOfResources)?;
        let mut draws = Vec::new();
        draws
            .try_reserve(operations.len())
            .map_err(|_| HandleError::OutOfResources)?;
        for operation in operations {
            let start_vertex = vertices.len();
            match operation {
                CompositionOperation::Textured {
                    texture,
                    destination,
                    source,
                    opacity,
                    source_alpha,
                    clip,
                } => {
                    if texture.context_handle != self.context_handle
                        || !destination.is_within(image_width(image), image_height(image))
                        || !source.is_within(texture.width, texture.height)
                        || clip.is_some_and(|rect| {
                            !rect.is_within(image_width(image), image_height(image))
                        })
                        || !opacity.is_finite()
                        || !(0.0..=1.0).contains(opacity)
                    {
                        return Err(HandleError::InvalidParameter);
                    }
                    append_composition_quad(
                        &mut vertices,
                        CompositionQuad {
                            target_width: image_width(image),
                            target_height: image_height(image),
                            destination: *destination,
                            source: *source,
                            source_width: texture.width,
                            source_height: texture.height,
                            color: [1.0, 1.0, 1.0, *opacity],
                        },
                    )?;
                    draws.push(CompositionDraw {
                        start_vertex,
                        clip: clip.unwrap_or(PixelRect::new(
                            0,
                            0,
                            image_width(image),
                            image_height(image),
                        )),
                        kind: CompositionDrawKind::Textured {
                            texture,
                            source_alpha: *source_alpha,
                        },
                    });
                }
                CompositionOperation::Solid {
                    destination,
                    color,
                    clip,
                } => {
                    if !destination.is_within(image_width(image), image_height(image))
                        || !color.is_finite_unit()
                        || clip.is_some_and(|rect| {
                            !rect.is_within(image_width(image), image_height(image))
                        })
                    {
                        return Err(HandleError::InvalidParameter);
                    }
                    append_composition_quad(
                        &mut vertices,
                        CompositionQuad {
                            target_width: image_width(image),
                            target_height: image_height(image),
                            destination: *destination,
                            source: PixelRect::new(0, 0, 1, 1),
                            source_width: 1,
                            source_height: 1,
                            color: [color.red, color.green, color.blue, color.alpha],
                        },
                    )?;
                    draws.push(CompositionDraw {
                        start_vertex,
                        clip: clip.unwrap_or(PixelRect::new(
                            0,
                            0,
                            image_width(image),
                            image_height(image),
                        )),
                        kind: CompositionDrawKind::Solid,
                    });
                }
            }
        }

        let command_capacity = composition_command_capacity(vertices.len(), draws.len())?;
        if command_capacity > self.raw.max_opaque_command_size() as usize {
            return Err(HandleError::InvalidParameter);
        }
        let mut commands = Vec::new();
        commands
            .try_reserve(command_capacity)
            .map_err(|_| HandleError::OutOfResources)?;
        let needs_setup = !self.composition_initialized.get();
        if needs_setup {
            push_composition_setup(&mut commands);
        }
        let needs_surface = !image.composition_surface_initialized.get();
        if needs_surface {
            push_surface(
                &mut commands,
                image.composition_surface_handle,
                image.resource_id,
            );
        }
        push_composition_bind_state(
            &mut commands,
            image.composition_surface_handle,
            self.composition_vertex_resource_id,
        );
        push_viewport(
            &mut commands,
            Viewport::new(image_width(image), image_height(image)),
            image.orientation,
        );
        push_clear(&mut commands, clear_color);
        if !vertices.is_empty() {
            push_composition_inline_write(
                &mut commands,
                self.composition_vertex_resource_id,
                &vertices,
            )?;
        }

        let mut new_sampler_views: Vec<&Texture> = Vec::new();
        new_sampler_views
            .try_reserve(draws.len())
            .map_err(|_| HandleError::OutOfResources)?;
        for draw in &draws {
            push_scissor(&mut commands, draw.clip)?;
            match &draw.kind {
                CompositionDrawKind::Textured {
                    texture,
                    source_alpha,
                } => {
                    let sampler_view_is_new = !texture.sampler_view_initialized.get()
                        && !new_sampler_views
                            .iter()
                            .any(|pending| core::ptr::eq::<Texture>(*pending, *texture));
                    if sampler_view_is_new {
                        push_sampler_view(
                            &mut commands,
                            texture.sampler_view_handle,
                            texture.resource_id,
                        );
                        new_sampler_views.push(*texture);
                    }
                    push_fragment_shader(
                        &mut commands,
                        match *source_alpha {
                            SourceAlpha::Respect => COMPOSITION_TEXTURE_ALPHA_SHADER_HANDLE,
                            SourceAlpha::Ignore => COMPOSITION_TEXTURE_OPAQUE_SHADER_HANDLE,
                        },
                    );
                    push_sampler_view_binding(&mut commands, texture.sampler_view_handle);
                }
                CompositionDrawKind::Solid => {
                    push_fragment_shader(&mut commands, COMPOSITION_SOLID_SHADER_HANDLE);
                }
            }
            push_draw(&mut commands, draw.start_vertex, 6)?;
        }
        if commands.len() > self.raw.max_opaque_command_size() as usize {
            return Err(HandleError::InvalidParameter);
        }
        self.raw.submit(&commands)?;
        if needs_setup {
            self.composition_initialized.set(true);
        }
        if needs_surface {
            image.composition_surface_initialized.set(true);
        }
        for texture in new_sampler_views {
            texture.sampler_view_initialized.set(true);
        }
        Ok(())
    }
}

pub(crate) struct Image {
    raw: RawImage,
    resource_id: u32,
    context_handle: i32,
    orientation: FramebufferOrientation,
    width: u32,
    height: u32,
    composition_surface_handle: u32,
    composition_surface_initialized: Cell<bool>,
    ir_surface_handle: u32,
    ir_surface_initialized: Cell<bool>,
}

pub(crate) struct Texture {
    raw: RawImage,
    resource_id: u32,
    width: u32,
    height: u32,
    context_handle: i32,
    sampler_view_handle: u32,
    sampler_view_initialized: Cell<bool>,
    ir_surface_handle: u32,
    ir_surface_initialized: Cell<bool>,
}

impl Drop for Queue {
    fn drop(&mut self) {
        let _ = &self.composition_vertex_buffer;
    }
}

impl Image {
    pub(crate) fn context_id(&self) -> i32 {
        self.context_handle
    }

    pub(crate) fn present(&self, display: &DisplaySurface) -> HandleResult<()> {
        display.present_image(self.raw.as_handle(), None)
    }

    pub(crate) fn shared_handle(&self) -> &Handle {
        self.raw.as_handle()
    }
}

pub(crate) struct Pipeline {
    vertex_buffer: RawBuffer,
    target_resource_id: u32,
    vertex_resource_id: u32,
    context_handle: i32,
    max_vertices: usize,
    cull_mode: CullMode,
    front_face: FrontFace,
    initialized: Cell<bool>,
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        let _ = &self.vertex_buffer;
    }
}

fn empty_slots<T>(count: usize) -> HandleResult<Vec<Option<T>>> {
    let mut slots = Vec::new();
    slots
        .try_reserve_exact(count)
        .map_err(|_| HandleError::OutOfResources)?;
    for _ in 0..count {
        slots.push(None);
    }
    Ok(slots)
}

fn ir_buffer<'resources>(
    context: &Context,
    resources: &'resources mut IrResources,
    spec: IrBufferSpec,
) -> HandleResult<&'resources mut IrBuffer> {
    if spec.slot >= resources.buffers.len() || spec.size == 0 || spec.size > u64::from(u32::MAX) {
        return Err(HandleError::InvalidParameter);
    }
    let slot = resources
        .buffers
        .get_mut(spec.slot)
        .ok_or(HandleError::InvalidParameter)?;
    if slot.is_none() {
        let raw = context.device.raw.create_buffer(spec.size, 0)?;
        let resource_id = resource_id_from_token(context.raw.attach_buffer(&raw)?)?;
        *slot = Some(IrBuffer {
            _raw: raw,
            resource_id,
            size: spec.size,
            uploaded_revision: None,
        });
    }
    let buffer = slot.as_mut().ok_or(HandleError::InvalidParameter)?;
    if buffer.size != spec.size {
        return Err(HandleError::InvalidParameter);
    }
    Ok(buffer)
}

fn ir_texture<'resources>(
    context: &Context,
    resources: &'resources mut IrResources,
    spec: IrTextureSpec,
) -> HandleResult<&'resources IrTexture> {
    let known_spec = resources
        .texture_specs
        .get(spec.slot)
        .and_then(|known| *known);
    if let Some(known_spec) = known_spec {
        if known_spec != spec {
            return Err(HandleError::InvalidParameter);
        }
    } else if let Some(known_spec) = resources.texture_specs.get_mut(spec.slot) {
        *known_spec = Some(spec);
    }
    let texture_slot = resources
        .textures
        .get_mut(spec.slot)
        .ok_or(HandleError::InvalidParameter)?;
    if texture_slot.is_none() {
        *texture_slot = Some(IrTexture::Internal(context.create_ir_texture(spec)?));
    }
    let texture = texture_slot.as_ref().ok_or(HandleError::InvalidParameter)?;
    let (width, height) = texture.dimensions();
    if width != spec.width || height != spec.height {
        return Err(HandleError::InvalidParameter);
    }
    Ok(texture)
}

fn ir_sampler<'resources>(
    context: &Context,
    resources: &'resources mut IrResources,
    state: IrSamplerState,
) -> HandleResult<&'resources IrSampler> {
    let sampler_slot = resources
        .samplers
        .get_mut(state.slot)
        .ok_or(HandleError::InvalidParameter)?;
    if sampler_slot.is_none() {
        *sampler_slot = Some(IrSampler {
            handle: context.allocate_object_handle()?,
            state,
            initialized: Cell::new(false),
        });
    }
    let sampler = sampler_slot.as_ref().ok_or(HandleError::InvalidParameter)?;
    if !ir_sampler_states_equal(sampler.state, state) {
        return Err(HandleError::InvalidParameter);
    }
    Ok(sampler)
}

fn ir_pipeline<'resources>(
    context: &Context,
    resources: &'resources mut IrResources,
    state: IrPipelineState,
) -> HandleResult<&'resources IrPipeline> {
    let pipeline_slot = resources
        .pipelines
        .get_mut(state.slot)
        .ok_or(HandleError::InvalidParameter)?;
    if pipeline_slot.is_none() {
        *pipeline_slot = Some(IrPipeline {
            blend_handle: context.allocate_object_handle()?,
            rasterizer_handle: context.allocate_object_handle()?,
            dsa_handle: if state.depth.is_some() {
                Some(context.allocate_object_handle()?)
            } else {
                None
            },
            state,
            initialized: Cell::new(false),
        });
    }
    let pipeline = pipeline_slot
        .as_ref()
        .ok_or(HandleError::InvalidParameter)?;
    if !ir_pipeline_states_equal(pipeline.state, state) {
        return Err(HandleError::InvalidParameter);
    }
    Ok(pipeline)
}

fn ir_sampler_states_equal(left: IrSamplerState, right: IrSamplerState) -> bool {
    left.slot == right.slot
        && ir_filter_modes_equal(left.min_filter, right.min_filter)
        && ir_filter_modes_equal(left.mag_filter, right.mag_filter)
        && ir_address_modes_equal(left.address_u, right.address_u)
        && ir_address_modes_equal(left.address_v, right.address_v)
}

fn ir_pipeline_states_equal(left: IrPipelineState, right: IrPipelineState) -> bool {
    left.slot == right.slot
        && ir_fragment_programs_equal(left.fragment, right.fragment)
        && ir_blend_states_equal(left.blend, right.blend)
        && ir_cull_modes_equal(left.cull_mode, right.cull_mode)
        && ir_front_faces_equal(left.front_face, right.front_face)
        && match (left.depth, right.depth) {
            (None, None) => true,
            (Some(left), Some(right)) => {
                left.write_enabled == right.write_enabled
                    && core::mem::discriminant(&left.compare)
                        == core::mem::discriminant(&right.compare)
            }
            _ => false,
        }
}

fn ir_fragment_programs_equal(left: IrFragmentProgram, right: IrFragmentProgram) -> bool {
    core::mem::discriminant(&left) == core::mem::discriminant(&right)
}

fn ir_blend_states_equal(left: IrBlendState, right: IrBlendState) -> bool {
    ir_blend_components_equal(left.color, right.color)
        && ir_blend_components_equal(left.alpha, right.alpha)
}

fn ir_blend_components_equal(
    left: crate::driver::IrBlendComponent,
    right: crate::driver::IrBlendComponent,
) -> bool {
    ir_blend_factors_equal(left.source_factor, right.source_factor)
        && ir_blend_factors_equal(left.destination_factor, right.destination_factor)
        && ir_blend_ops_equal(left.operation, right.operation)
}

fn ir_blend_factors_equal(left: IrBlendFactor, right: IrBlendFactor) -> bool {
    core::mem::discriminant(&left) == core::mem::discriminant(&right)
}

fn ir_blend_ops_equal(left: IrBlendOp, right: IrBlendOp) -> bool {
    core::mem::discriminant(&left) == core::mem::discriminant(&right)
}

fn ir_cull_modes_equal(left: IrCullMode, right: IrCullMode) -> bool {
    core::mem::discriminant(&left) == core::mem::discriminant(&right)
}

fn ir_front_faces_equal(left: IrFrontFace, right: IrFrontFace) -> bool {
    core::mem::discriminant(&left) == core::mem::discriminant(&right)
}

fn ir_filter_modes_equal(left: IrFilterMode, right: IrFilterMode) -> bool {
    core::mem::discriminant(&left) == core::mem::discriminant(&right)
}

fn ir_address_modes_equal(left: IrAddressMode, right: IrAddressMode) -> bool {
    core::mem::discriminant(&left) == core::mem::discriminant(&right)
}

fn ir_rect_is_within(rect: crate::driver::IrRect, width: u32, height: u32) -> bool {
    rect.width != 0
        && rect.height != 0
        && rect
            .x
            .checked_add(rect.width)
            .is_some_and(|right| right <= width)
        && rect
            .y
            .checked_add(rect.height)
            .is_some_and(|bottom| bottom <= height)
}

fn ir_rect_to_pixel_rect(rect: crate::driver::IrRect) -> HandleResult<PixelRect> {
    if rect.width == 0
        || rect.height == 0
        || rect.x.checked_add(rect.width).is_none()
        || rect.y.checked_add(rect.height).is_none()
    {
        return Err(HandleError::InvalidParameter);
    }
    Ok(PixelRect::new(rect.x, rect.y, rect.width, rect.height))
}

fn validate_ir_draw(
    resources: &IrResources,
    target_width: u32,
    target_height: u32,
    draw: &IrDraw,
    vertices_len: usize,
) -> HandleResult<()> {
    let end = draw
        .start_vertex
        .checked_add(draw.vertex_count)
        .ok_or(HandleError::InvalidParameter)?;
    let vertex_range_valid = match draw.vertex_buffer {
        None => end <= vertices_len,
        Some(binding) => {
            let byte_end = end
                .checked_mul(40)
                .and_then(|bytes| bytes.checked_add(binding.offset as usize))
                .and_then(|bytes| u64::try_from(bytes).ok());
            binding.buffer.slot < resources.buffers.len()
                && byte_end.is_some_and(|end| end <= binding.buffer.size)
                && resources
                    .buffers
                    .get(binding.buffer.slot)
                    .and_then(Option::as_ref)
                    .is_some_and(|buffer| {
                        buffer.size == binding.buffer.size
                            && buffer.uploaded_revision == Some(binding.buffer.revision)
                    })
        }
    };
    if draw.vertex_count == 0
        || !draw.vertex_count.is_multiple_of(3)
        || !vertex_range_valid
        || !ir_rect_is_within(draw.scissor, target_width, target_height)
        || !draw
            .uniforms
            .transform
            .iter()
            .all(|value| value.is_finite())
        || !draw.uniforms.color.iter().all(|value| value.is_finite())
        || draw.pipeline.slot >= resources.pipelines.len()
    {
        return Err(HandleError::InvalidParameter);
    }
    match draw.pipeline.fragment {
        IrFragmentProgram::TextureRgba
        | IrFragmentProgram::TextureRgbIgnoreAlpha
        | IrFragmentProgram::TextureAlphaMask
        | IrFragmentProgram::TextureVertexColorRgba
        | IrFragmentProgram::TextureVertexColorRgbIgnoreAlpha
        | IrFragmentProgram::TextureVertexColorAlphaMask => {
            let Some(texture) = draw.texture else {
                return Err(HandleError::InvalidParameter);
            };
            if texture.width == 0
                || texture.height == 0
                || !texture.sampled
                || texture.slot >= resources.textures.len()
                || draw.sampler.is_none()
            {
                return Err(HandleError::InvalidParameter);
            }
        }
        IrFragmentProgram::Solid | IrFragmentProgram::VertexColor => {
            if draw.texture.is_some() || draw.sampler.is_some() {
                return Err(HandleError::InvalidParameter);
            }
        }
    }
    Ok(())
}

fn resource_id_from_token(token: u64) -> HandleResult<u32> {
    u32::try_from(token)
        .ok()
        .filter(|&resource_id| resource_id != 0)
        .ok_or(HandleError::SystemError(-1))
}

fn image_width(image: &Image) -> u32 {
    image.width
}

fn image_height(image: &Image) -> u32 {
    image.height
}

fn composition_command_capacity(vertex_count: usize, draw_count: usize) -> HandleResult<usize> {
    let vertex_dwords = vertex_count
        .checked_mul(10)
        .ok_or(HandleError::InvalidParameter)?;
    let vertex_command_bytes = vertex_dwords
        .checked_add(12)
        .and_then(|dwords| dwords.checked_mul(core::mem::size_of::<u32>()))
        .ok_or(HandleError::InvalidParameter)?;
    let draw_command_bytes = draw_count
        .checked_mul(50)
        .and_then(|dwords| dwords.checked_mul(core::mem::size_of::<u32>()))
        .ok_or(HandleError::InvalidParameter)?;
    (16 * 1024usize)
        .checked_add(vertex_command_bytes)
        .and_then(|bytes| bytes.checked_add(draw_command_bytes))
        .ok_or(HandleError::InvalidParameter)
}

fn append_composition_quad(
    vertices: &mut Vec<CompositionVertex>,
    quad: CompositionQuad,
) -> HandleResult<()> {
    let destination_right = quad
        .destination
        .x()
        .checked_add(quad.destination.width())
        .ok_or(HandleError::InvalidParameter)?;
    let destination_bottom = quad
        .destination
        .y()
        .checked_add(quad.destination.height())
        .ok_or(HandleError::InvalidParameter)?;
    let source_right = quad
        .source
        .x()
        .checked_add(quad.source.width())
        .ok_or(HandleError::InvalidParameter)?;
    let source_bottom = quad
        .source
        .y()
        .checked_add(quad.source.height())
        .ok_or(HandleError::InvalidParameter)?;
    if quad.target_width == 0
        || quad.target_height == 0
        || quad.source_width == 0
        || quad.source_height == 0
        || !quad
            .destination
            .is_within(quad.target_width, quad.target_height)
        || !quad.source.is_within(quad.source_width, quad.source_height)
    {
        return Err(HandleError::InvalidParameter);
    }
    vertices
        .try_reserve(6)
        .map_err(|_| HandleError::OutOfResources)?;
    let left = quad.destination.x() as f32 * 2.0 / quad.target_width as f32 - 1.0;
    let right = destination_right as f32 * 2.0 / quad.target_width as f32 - 1.0;
    let top = 1.0 - quad.destination.y() as f32 * 2.0 / quad.target_height as f32;
    let bottom = 1.0 - destination_bottom as f32 * 2.0 / quad.target_height as f32;
    let source_left = quad.source.x() as f32 / quad.source_width as f32;
    let source_right = source_right as f32 / quad.source_width as f32;
    let source_top = quad.source.y() as f32 / quad.source_height as f32;
    let source_bottom = source_bottom as f32 / quad.source_height as f32;
    let top_left = CompositionVertex {
        clip_position: [left, top, 0.0, 1.0],
        uv: [source_left, source_top],
        color: quad.color,
    };
    let bottom_left = CompositionVertex {
        clip_position: [left, bottom, 0.0, 1.0],
        uv: [source_left, source_bottom],
        color: quad.color,
    };
    let bottom_right = CompositionVertex {
        clip_position: [right, bottom, 0.0, 1.0],
        uv: [source_right, source_bottom],
        color: quad.color,
    };
    let top_right = CompositionVertex {
        clip_position: [right, top, 0.0, 1.0],
        uv: [source_right, source_top],
        color: quad.color,
    };
    vertices.extend_from_slice(&[
        top_left,
        bottom_left,
        bottom_right,
        top_left,
        bottom_right,
        top_right,
    ]);
    Ok(())
}

fn command_header(command: u32, object: u32, payload_dwords: u32) -> u32 {
    command | (object << 8) | (payload_dwords << 16)
}

fn push_dword(commands: &mut Vec<u8>, value: u32) {
    commands.extend_from_slice(&value.to_le_bytes());
}

fn push_float(commands: &mut Vec<u8>, value: f32) {
    push_dword(commands, value.to_bits());
}

fn push_bind_object(commands: &mut Vec<u8>, object: u32, handle: u32) {
    push_dword(commands, command_header(VIRGL_CCMD_BIND_OBJECT, object, 1));
    push_dword(commands, handle);
}

fn push_fragment_shader(commands: &mut Vec<u8>, handle: u32) {
    push_dword(commands, command_header(VIRGL_CCMD_BIND_SHADER, 0, 2));
    push_dword(commands, handle);
    push_dword(commands, PIPE_SHADER_FRAGMENT);
}

fn push_surface(commands: &mut Vec<u8>, handle: u32, resource_id: u32) {
    push_dword(
        commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_SURFACE, 5),
    );
    push_dword(commands, handle);
    push_dword(commands, resource_id);
    push_dword(commands, VIRGL_FORMAT_B8G8R8A8_UNORM);
    push_dword(commands, 0);
    push_dword(commands, 0);
}

fn push_depth_surface(commands: &mut Vec<u8>, handle: u32, resource_id: u32) {
    push_dword(
        commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_SURFACE, 5),
    );
    push_dword(commands, handle);
    push_dword(commands, resource_id);
    push_dword(commands, VIRGL_FORMAT_Z32_FLOAT);
    push_dword(commands, 0);
    push_dword(commands, 0);
}

fn push_composition_setup(commands: &mut Vec<u8>) {
    push_dword(
        commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_BLEND, 11),
    );
    push_dword(commands, COMPOSITION_BLEND_HANDLE);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(
        commands,
        VIRGL_BLEND_ENABLE
            | (PIPE_BLENDFACTOR_SRC_ALPHA << VIRGL_BLEND_RGB_SRC_FACTOR_SHIFT)
            | (PIPE_BLENDFACTOR_INV_SRC_ALPHA << VIRGL_BLEND_RGB_DST_FACTOR_SHIFT)
            | (PIPE_BLENDFACTOR_ONE << VIRGL_BLEND_ALPHA_SRC_FACTOR_SHIFT)
            | (PIPE_BLENDFACTOR_INV_SRC_ALPHA << VIRGL_BLEND_ALPHA_DST_FACTOR_SHIFT)
            | (0xf << VIRGL_BLEND_COLORMASK_SHIFT),
    );
    for _ in 0..7 {
        push_dword(commands, 0);
    }

    push_dword(
        commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_RASTERIZER, 9),
    );
    push_dword(commands, COMPOSITION_RASTERIZER_HANDLE);
    push_dword(
        commands,
        VIRGL_RASTERIZER_DEPTH_CLIP | VIRGL_RASTERIZER_SCISSOR,
    );
    push_float(commands, 1.0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_float(commands, 1.0);
    push_float(commands, 0.0);
    push_float(commands, 0.0);
    push_float(commands, 0.0);

    push_shader(
        commands,
        COMPOSITION_VERTEX_SHADER_HANDLE,
        PIPE_SHADER_VERTEX,
        COMPOSITION_VERTEX_SHADER,
    );
    push_shader(
        commands,
        COMPOSITION_TEXTURE_ALPHA_SHADER_HANDLE,
        PIPE_SHADER_FRAGMENT,
        COMPOSITION_TEXTURE_ALPHA_SHADER,
    );
    push_shader(
        commands,
        COMPOSITION_TEXTURE_OPAQUE_SHADER_HANDLE,
        PIPE_SHADER_FRAGMENT,
        COMPOSITION_TEXTURE_OPAQUE_SHADER,
    );
    push_shader(
        commands,
        COMPOSITION_SOLID_SHADER_HANDLE,
        PIPE_SHADER_FRAGMENT,
        COMPOSITION_SOLID_SHADER,
    );

    push_dword(
        commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_VERTEX_ELEMENTS, 13),
    );
    push_dword(commands, COMPOSITION_VERTEX_ELEMENTS_HANDLE);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, VIRGL_FORMAT_R32G32B32A32_FLOAT);
    push_dword(commands, 16);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, VIRGL_FORMAT_R32G32_FLOAT);
    push_dword(commands, 24);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, VIRGL_FORMAT_R32G32B32A32_FLOAT);

    push_dword(
        commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_SAMPLER_STATE, 9),
    );
    push_dword(commands, COMPOSITION_SAMPLER_STATE_HANDLE);
    push_dword(
        commands,
        PIPE_TEX_WRAP_CLAMP_TO_EDGE
            | (PIPE_TEX_WRAP_CLAMP_TO_EDGE << 3)
            | (PIPE_TEX_WRAP_CLAMP_TO_EDGE << 6)
            | (PIPE_TEX_FILTER_LINEAR << 9)
            | (PIPE_TEX_MIPFILTER_NONE << 11)
            | (PIPE_TEX_FILTER_LINEAR << 13),
    );
    push_float(commands, 0.0);
    push_float(commands, 0.0);
    push_float(commands, 0.0);
    for _ in 0..4 {
        push_dword(commands, 0);
    }
}

fn push_composition_bind_state(
    commands: &mut Vec<u8>,
    surface_handle: u32,
    vertex_resource_id: u32,
) {
    push_bind_object(commands, VIRGL_OBJECT_BLEND, COMPOSITION_BLEND_HANDLE);
    push_bind_object(
        commands,
        VIRGL_OBJECT_RASTERIZER,
        COMPOSITION_RASTERIZER_HANDLE,
    );
    push_dword(commands, command_header(VIRGL_CCMD_BIND_SHADER, 0, 2));
    push_dword(commands, COMPOSITION_VERTEX_SHADER_HANDLE);
    push_dword(commands, PIPE_SHADER_VERTEX);
    push_bind_object(
        commands,
        VIRGL_OBJECT_VERTEX_ELEMENTS,
        COMPOSITION_VERTEX_ELEMENTS_HANDLE,
    );
    push_dword(
        commands,
        command_header(VIRGL_CCMD_SET_FRAMEBUFFER_STATE, 0, 3),
    );
    push_dword(commands, 1);
    push_dword(commands, 0);
    push_dword(commands, surface_handle);
    push_dword(
        commands,
        command_header(VIRGL_CCMD_SET_VERTEX_BUFFERS, 0, 3),
    );
    push_dword(commands, core::mem::size_of::<CompositionVertex>() as u32);
    push_dword(commands, 0);
    push_dword(commands, vertex_resource_id);
    push_dword(
        commands,
        command_header(VIRGL_CCMD_BIND_SAMPLER_STATES, 0, 3),
    );
    push_dword(commands, PIPE_SHADER_FRAGMENT);
    push_dword(commands, 0);
    push_dword(commands, COMPOSITION_SAMPLER_STATE_HANDLE);
}

fn push_legacy_bind_state(commands: &mut Vec<u8>, vertex_resource_id: u32) {
    push_bind_object(commands, VIRGL_OBJECT_BLEND, BLEND_HANDLE);
    push_bind_object(commands, VIRGL_OBJECT_RASTERIZER, RASTERIZER_HANDLE);
    push_dword(commands, command_header(VIRGL_CCMD_BIND_SHADER, 0, 2));
    push_dword(commands, VERTEX_SHADER_HANDLE);
    push_dword(commands, PIPE_SHADER_VERTEX);
    push_fragment_shader(commands, FRAGMENT_SHADER_HANDLE);
    push_bind_object(
        commands,
        VIRGL_OBJECT_VERTEX_ELEMENTS,
        VERTEX_ELEMENTS_HANDLE,
    );
    push_dword(
        commands,
        command_header(VIRGL_CCMD_SET_FRAMEBUFFER_STATE, 0, 3),
    );
    push_dword(commands, 1);
    push_dword(commands, 0);
    push_dword(commands, SURFACE_HANDLE);
    push_dword(
        commands,
        command_header(VIRGL_CCMD_SET_VERTEX_BUFFERS, 0, 3),
    );
    push_dword(commands, core::mem::size_of::<VertexClip4Color3>() as u32);
    push_dword(commands, 0);
    push_dword(commands, vertex_resource_id);
}

fn push_sampler_view(commands: &mut Vec<u8>, handle: u32, resource_id: u32) {
    push_dword(
        commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_SAMPLER_VIEW, 6),
    );
    push_dword(commands, handle);
    push_dword(commands, resource_id);
    push_dword(commands, VIRGL_FORMAT_B8G8R8A8_UNORM);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(
        commands,
        PIPE_SWIZZLE_X | (PIPE_SWIZZLE_Y << 3) | (PIPE_SWIZZLE_Z << 6) | (PIPE_SWIZZLE_W << 9),
    );
}

fn push_sampler_view_binding(commands: &mut Vec<u8>, handle: u32) {
    push_dword(commands, command_header(VIRGL_CCMD_SET_SAMPLER_VIEWS, 0, 3));
    push_dword(commands, PIPE_SHADER_FRAGMENT);
    push_dword(commands, 0);
    push_dword(commands, handle);
}

fn push_resource_copy(
    commands: &mut Vec<u8>,
    destination_resource: u32,
    destination: crate::driver::IrRect,
    source_resource: u32,
    source: crate::driver::IrRect,
) {
    push_dword(
        commands,
        command_header(VIRGL_CCMD_RESOURCE_COPY_REGION, 0, 13),
    );
    push_dword(commands, destination_resource);
    push_dword(commands, 0);
    push_dword(commands, destination.x);
    push_dword(commands, destination.y);
    push_dword(commands, 0);
    push_dword(commands, source_resource);
    push_dword(commands, 0);
    push_dword(commands, source.x);
    push_dword(commands, source.y);
    push_dword(commands, 0);
    push_dword(commands, source.width);
    push_dword(commands, source.height);
    push_dword(commands, 1);
}

fn push_sampler_state_binding(commands: &mut Vec<u8>, handle: u32) {
    push_dword(
        commands,
        command_header(VIRGL_CCMD_BIND_SAMPLER_STATES, 0, 3),
    );
    push_dword(commands, PIPE_SHADER_FRAGMENT);
    push_dword(commands, 0);
    push_dword(commands, handle);
}

fn push_constant_buffer(
    commands: &mut Vec<u8>,
    shader_type: u32,
    values: &[f32],
) -> HandleResult<()> {
    let payload = u32::try_from(values.len())
        .ok()
        .and_then(|length| length.checked_add(2))
        .ok_or(HandleError::InvalidParameter)?;
    push_dword(
        commands,
        command_header(VIRGL_CCMD_SET_CONSTANT_BUFFER, 0, payload),
    );
    push_dword(commands, shader_type);
    push_dword(commands, 0);
    for value in values {
        push_float(commands, *value);
    }
    Ok(())
}

fn push_ir_setup(commands: &mut Vec<u8>, resources: &IrResources) {
    push_shader(
        commands,
        resources.vertex_shader_handle,
        PIPE_SHADER_VERTEX,
        IR_VERTEX_SHADER,
    );
    push_shader(
        commands,
        resources.solid_fragment_shader_handle,
        PIPE_SHADER_FRAGMENT,
        IR_SOLID_FRAGMENT_SHADER,
    );
    push_shader(
        commands,
        resources.vertex_color_fragment_shader_handle,
        PIPE_SHADER_FRAGMENT,
        IR_VERTEX_COLOR_FRAGMENT_SHADER,
    );
    push_shader(
        commands,
        resources.texture_rgba_fragment_shader_handle,
        PIPE_SHADER_FRAGMENT,
        IR_TEXTURE_RGBA_FRAGMENT_SHADER,
    );
    push_shader(
        commands,
        resources.texture_rgb_ignore_alpha_fragment_shader_handle,
        PIPE_SHADER_FRAGMENT,
        IR_TEXTURE_RGB_IGNORE_ALPHA_FRAGMENT_SHADER,
    );
    push_shader(
        commands,
        resources.texture_alpha_mask_fragment_shader_handle,
        PIPE_SHADER_FRAGMENT,
        IR_TEXTURE_ALPHA_MASK_FRAGMENT_SHADER,
    );
    push_shader(
        commands,
        resources.texture_vertex_color_rgba_fragment_shader_handle,
        PIPE_SHADER_FRAGMENT,
        IR_TEXTURE_VERTEX_COLOR_RGBA_FRAGMENT_SHADER,
    );
    push_shader(
        commands,
        resources.texture_vertex_color_rgb_ignore_alpha_fragment_shader_handle,
        PIPE_SHADER_FRAGMENT,
        IR_TEXTURE_VERTEX_COLOR_RGB_IGNORE_ALPHA_FRAGMENT_SHADER,
    );
    push_shader(
        commands,
        resources.texture_vertex_color_alpha_mask_fragment_shader_handle,
        PIPE_SHADER_FRAGMENT,
        IR_TEXTURE_VERTEX_COLOR_ALPHA_MASK_FRAGMENT_SHADER,
    );
    push_dword(
        commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_VERTEX_ELEMENTS, 13),
    );
    push_dword(commands, resources.vertex_elements_handle);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, VIRGL_FORMAT_R32G32B32A32_FLOAT);
    push_dword(commands, 16);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, VIRGL_FORMAT_R32G32B32A32_FLOAT);
    push_dword(commands, 32);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, VIRGL_FORMAT_R32G32_FLOAT);
}

fn push_ir_pipeline(commands: &mut Vec<u8>, pipeline: &IrPipeline) {
    push_dword(
        commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_BLEND, 11),
    );
    push_dword(commands, pipeline.blend_handle);
    push_dword(commands, 0);
    push_dword(commands, 0);
    let blend = pipeline.state.blend;
    push_dword(
        commands,
        VIRGL_BLEND_ENABLE
            | (ir_blend_op(blend.color.operation) << 1)
            | (ir_blend_factor(blend.color.source_factor) << VIRGL_BLEND_RGB_SRC_FACTOR_SHIFT)
            | (ir_blend_factor(blend.color.destination_factor) << VIRGL_BLEND_RGB_DST_FACTOR_SHIFT)
            | (ir_blend_op(blend.alpha.operation) << 14)
            | (ir_blend_factor(blend.alpha.source_factor) << VIRGL_BLEND_ALPHA_SRC_FACTOR_SHIFT)
            | (ir_blend_factor(blend.alpha.destination_factor)
                << VIRGL_BLEND_ALPHA_DST_FACTOR_SHIFT)
            | (0xf << VIRGL_BLEND_COLORMASK_SHIFT),
    );
    for _ in 0..7 {
        push_dword(commands, 0);
    }
    push_dword(
        commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_RASTERIZER, 9),
    );
    push_dword(commands, pipeline.rasterizer_handle);
    push_dword(
        commands,
        ir_rasterizer_flags(pipeline.state.cull_mode, pipeline.state.front_face),
    );
    push_float(commands, 1.0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_float(commands, 1.0);
    push_float(commands, 0.0);
    push_float(commands, 0.0);
    push_float(commands, 0.0);
    if let (Some(handle), Some(depth)) = (pipeline.dsa_handle, pipeline.state.depth) {
        push_ir_dsa(commands, handle, depth);
    }
}

fn push_ir_dsa(commands: &mut Vec<u8>, handle: u32, depth: crate::driver::IrDepthState) {
    push_dword(
        commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_DSA, 5),
    );
    push_dword(commands, handle);
    let flags =
        1 | (u32::from(depth.write_enabled) << 1) | (ir_compare_function(depth.compare) << 2);
    push_dword(commands, flags);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_float(commands, 0.0);
}

fn push_ir_sampler(commands: &mut Vec<u8>, sampler: &IrSampler) {
    push_dword(
        commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_SAMPLER_STATE, 9),
    );
    push_dword(commands, sampler.handle);
    let state = sampler.state;
    push_dword(
        commands,
        ir_address_mode(state.address_u)
            | (ir_address_mode(state.address_v) << 3)
            | (PIPE_TEX_WRAP_CLAMP_TO_EDGE << 6)
            | (ir_filter_mode(state.min_filter) << 9)
            | (PIPE_TEX_MIPFILTER_NONE << 11)
            | (ir_filter_mode(state.mag_filter) << 13),
    );
    push_float(commands, 0.0);
    push_float(commands, 0.0);
    push_float(commands, 0.0);
    for _ in 0..4 {
        push_dword(commands, 0);
    }
}

#[allow(clippy::too_many_arguments)]
fn push_ir_bind_pass_state(
    commands: &mut Vec<u8>,
    surface_handle: u32,
    depth_surface_handle: u32,
    vertex_resource_id: u32,
    width: u32,
    height: u32,
    orientation: FramebufferOrientation,
    vertex_shader_handle: u32,
    vertex_elements_handle: u32,
) {
    push_dword(commands, command_header(VIRGL_CCMD_BIND_SHADER, 0, 2));
    push_dword(commands, vertex_shader_handle);
    push_dword(commands, PIPE_SHADER_VERTEX);
    push_bind_object(
        commands,
        VIRGL_OBJECT_VERTEX_ELEMENTS,
        vertex_elements_handle,
    );
    push_dword(
        commands,
        command_header(VIRGL_CCMD_SET_FRAMEBUFFER_STATE, 0, 3),
    );
    push_dword(commands, 1);
    push_dword(commands, depth_surface_handle);
    push_dword(commands, surface_handle);
    push_dword(
        commands,
        command_header(VIRGL_CCMD_SET_VERTEX_BUFFERS, 0, 3),
    );
    push_dword(commands, 40);
    push_dword(commands, 0);
    push_dword(commands, vertex_resource_id);
    push_viewport(commands, Viewport::new(width, height), orientation);
}

fn push_ir_vertex_buffer(commands: &mut Vec<u8>, resource_id: u32, offset: u32) {
    push_dword(
        commands,
        command_header(VIRGL_CCMD_SET_VERTEX_BUFFERS, 0, 3),
    );
    push_dword(commands, 40);
    push_dword(commands, offset);
    push_dword(commands, resource_id);
}

fn push_ir_clear(commands: &mut Vec<u8>, color: Option<[f32; 4]>, depth: Option<f32>) {
    push_dword(commands, command_header(VIRGL_CCMD_CLEAR, 0, 8));
    push_dword(
        commands,
        if color.is_some() {
            PIPE_CLEAR_COLOR0
        } else {
            0
        } | if depth.is_some() { PIPE_CLEAR_DEPTH } else { 0 },
    );
    for component in color.unwrap_or([0.0; 4]) {
        push_float(commands, component);
    }
    let depth = f64::from(depth.unwrap_or(1.0)).to_bits();
    push_dword(commands, depth as u32);
    push_dword(commands, (depth >> 32) as u32);
    push_dword(commands, 0);
}

fn push_ir_clear_depth_surface(
    commands: &mut Vec<u8>,
    surface_handle: u32,
    area: crate::driver::IrRect,
    depth: f32,
) -> HandleResult<()> {
    push_dword(commands, command_header(VIRGL_CCMD_CLEAR_SURFACE, 0, 10));
    push_dword(commands, PIPE_CLEAR_DEPTH << 1);
    push_dword(commands, surface_handle);
    let depth = f64::from(depth).to_bits();
    push_dword(commands, depth as u32);
    push_dword(commands, (depth >> 32) as u32);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, area.x);
    push_dword(commands, area.y);
    push_dword(commands, area.width);
    push_dword(commands, area.height);
    Ok(())
}

fn push_ir_clear_surface(
    commands: &mut Vec<u8>,
    surface_handle: u32,
    area: crate::driver::IrRect,
    color: [f32; 4],
) -> HandleResult<()> {
    // VIRGL_CCMD_CLEAR disables scissoring. CLEAR_SURFACE carries an explicit
    // rectangle and therefore implements SGFX LoadOp::Clear's render-area
    // semantics without touching preserved pixels outside `area`. IR targets
    // already use an upper-left framebuffer orientation through the negative
    // viewport, so protocol rectangles stay in the same upper-left space.
    push_dword(commands, command_header(VIRGL_CCMD_CLEAR_SURFACE, 0, 10));
    push_dword(commands, PIPE_CLEAR_COLOR0 << 1);
    push_dword(commands, surface_handle);
    for component in color {
        push_float(commands, component);
    }
    push_dword(commands, area.x);
    push_dword(commands, area.y);
    push_dword(commands, area.width);
    push_dword(commands, area.height);
    Ok(())
}

fn push_ir_inline_write(
    commands: &mut Vec<u8>,
    resource_id: u32,
    vertices: &[IrVertex],
) -> HandleResult<()> {
    let components = vertices
        .len()
        .checked_mul(10)
        .ok_or(HandleError::InvalidParameter)?;
    let components = u32::try_from(components).map_err(|_| HandleError::InvalidParameter)?;
    let byte_len = vertices
        .len()
        .checked_mul(40)
        .and_then(|length| u32::try_from(length).ok())
        .ok_or(HandleError::InvalidParameter)?;
    push_dword(
        commands,
        command_header(VIRGL_CCMD_RESOURCE_INLINE_WRITE, 0, 11 + components),
    );
    push_dword(commands, resource_id);
    for _ in 0..7 {
        push_dword(commands, 0);
    }
    push_dword(commands, byte_len);
    push_dword(commands, 1);
    push_dword(commands, 1);
    for vertex in vertices {
        for component in vertex.position {
            push_float(commands, component);
        }
        for component in vertex.secondary {
            push_float(commands, component);
        }
        for component in vertex.tertiary {
            push_float(commands, component);
        }
    }
    Ok(())
}

fn push_ir_buffer_inline_write(
    commands: &mut Vec<u8>,
    resource_id: u32,
    offset: u32,
    bytes: &[u8],
) -> HandleResult<()> {
    if resource_id == 0
        || bytes.is_empty()
        || !bytes.len().is_multiple_of(core::mem::size_of::<u32>())
    {
        return Err(HandleError::InvalidParameter);
    }
    let components = u32::try_from(bytes.len() / core::mem::size_of::<u32>())
        .map_err(|_| HandleError::InvalidParameter)?;
    let byte_len = u32::try_from(bytes.len()).map_err(|_| HandleError::InvalidParameter)?;
    push_dword(
        commands,
        command_header(VIRGL_CCMD_RESOURCE_INLINE_WRITE, 0, 11 + components),
    );
    push_dword(commands, resource_id);
    // level, usage, stride, layer_stride
    for _ in 0..4 {
        push_dword(commands, 0);
    }
    // PIPE_BUFFER transfers use box.x as the byte offset and box.width as the
    // byte count. The remaining two origins and dimensions stay 0/1.
    push_dword(commands, offset);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, byte_len);
    push_dword(commands, 1);
    push_dword(commands, 1);
    commands.extend_from_slice(bytes);
    Ok(())
}

fn ir_fragment_shader_handle(resources: &IrResources, fragment: IrFragmentProgram) -> u32 {
    match fragment {
        IrFragmentProgram::Solid => resources.solid_fragment_shader_handle,
        IrFragmentProgram::VertexColor => resources.vertex_color_fragment_shader_handle,
        IrFragmentProgram::TextureRgba => resources.texture_rgba_fragment_shader_handle,
        IrFragmentProgram::TextureRgbIgnoreAlpha => {
            resources.texture_rgb_ignore_alpha_fragment_shader_handle
        }
        IrFragmentProgram::TextureAlphaMask => resources.texture_alpha_mask_fragment_shader_handle,
        IrFragmentProgram::TextureVertexColorRgba => {
            resources.texture_vertex_color_rgba_fragment_shader_handle
        }
        IrFragmentProgram::TextureVertexColorRgbIgnoreAlpha => {
            resources.texture_vertex_color_rgb_ignore_alpha_fragment_shader_handle
        }
        IrFragmentProgram::TextureVertexColorAlphaMask => {
            resources.texture_vertex_color_alpha_mask_fragment_shader_handle
        }
    }
}

fn ir_blend_factor(factor: IrBlendFactor) -> u32 {
    match factor {
        IrBlendFactor::Zero => PIPE_BLENDFACTOR_ZERO,
        IrBlendFactor::One => PIPE_BLENDFACTOR_ONE,
        IrBlendFactor::SourceAlpha => PIPE_BLENDFACTOR_SRC_ALPHA,
        IrBlendFactor::OneMinusSourceAlpha => PIPE_BLENDFACTOR_INV_SRC_ALPHA,
        IrBlendFactor::DestinationAlpha => PIPE_BLENDFACTOR_DST_ALPHA,
        IrBlendFactor::OneMinusDestinationAlpha => PIPE_BLENDFACTOR_INV_DST_ALPHA,
    }
}

fn ir_blend_op(operation: IrBlendOp) -> u32 {
    match operation {
        IrBlendOp::Add => PIPE_BLEND_ADD,
        IrBlendOp::Subtract => PIPE_BLEND_SUBTRACT,
        IrBlendOp::ReverseSubtract => PIPE_BLEND_REVERSE_SUBTRACT,
    }
}

fn ir_compare_function(compare: IrCompareFunction) -> u32 {
    match compare {
        IrCompareFunction::Never => 0,
        IrCompareFunction::Less => 1,
        IrCompareFunction::Equal => 2,
        IrCompareFunction::LessEqual => 3,
        IrCompareFunction::Greater => 4,
        IrCompareFunction::NotEqual => 5,
        IrCompareFunction::GreaterEqual => 6,
        IrCompareFunction::Always => 7,
    }
}

fn ir_filter_mode(filter: IrFilterMode) -> u32 {
    match filter {
        IrFilterMode::Nearest => PIPE_TEX_FILTER_NEAREST,
        IrFilterMode::Linear => PIPE_TEX_FILTER_LINEAR,
    }
}

fn ir_address_mode(address: IrAddressMode) -> u32 {
    match address {
        IrAddressMode::ClampToEdge => PIPE_TEX_WRAP_CLAMP_TO_EDGE,
        IrAddressMode::Repeat => PIPE_TEX_WRAP_REPEAT,
        IrAddressMode::MirrorRepeat => PIPE_TEX_WRAP_MIRROR_REPEAT,
    }
}

fn ir_rasterizer_flags(cull_mode: IrCullMode, front_face: IrFrontFace) -> u32 {
    let cull_face = match cull_mode {
        IrCullMode::None => 0,
        IrCullMode::Front => 1,
        IrCullMode::Back => 2,
    } << VIRGL_RASTERIZER_CULL_FACE_SHIFT;
    let front_ccw = if matches!(front_face, IrFrontFace::CounterClockwise) {
        VIRGL_RASTERIZER_FRONT_CCW
    } else {
        0
    };
    VIRGL_RASTERIZER_DEPTH_CLIP | VIRGL_RASTERIZER_SCISSOR | cull_face | front_ccw
}

fn push_scissor(commands: &mut Vec<u8>, clip: PixelRect) -> HandleResult<()> {
    let max_x = clip
        .x()
        .checked_add(clip.width())
        .ok_or(HandleError::InvalidParameter)?;
    let max_y = clip
        .y()
        .checked_add(clip.height())
        .ok_or(HandleError::InvalidParameter)?;
    if clip.x() > u16::MAX as u32
        || clip.y() > u16::MAX as u32
        || max_x > u16::MAX as u32
        || max_y > u16::MAX as u32
    {
        return Err(HandleError::InvalidParameter);
    }
    push_dword(commands, command_header(VIRGL_CCMD_SET_SCISSOR_STATE, 0, 3));
    push_dword(commands, 0);
    push_dword(commands, clip.x() | (clip.y() << 16));
    push_dword(commands, max_x | (max_y << 16));
    Ok(())
}

fn push_ir_scissor(commands: &mut Vec<u8>, clip: PixelRect) -> HandleResult<()> {
    // The IR viewport establishes an upper-left framebuffer orientation.
    // Converting Y here would mirror partial damage a second time.
    push_scissor(commands, clip)
}

fn push_clear(commands: &mut Vec<u8>, clear_color: Color) {
    push_dword(commands, command_header(VIRGL_CCMD_CLEAR, 0, 8));
    push_dword(commands, PIPE_CLEAR_COLOR0);
    push_float(commands, clear_color.red);
    push_float(commands, clear_color.green);
    push_float(commands, clear_color.blue);
    push_float(commands, clear_color.alpha);
    let depth = 1.0f64.to_bits();
    push_dword(commands, depth as u32);
    push_dword(commands, (depth >> 32) as u32);
    push_dword(commands, 0);
}

fn push_composition_inline_write(
    commands: &mut Vec<u8>,
    resource_id: u32,
    vertices: &[CompositionVertex],
) -> HandleResult<()> {
    let components = vertices
        .len()
        .checked_mul(10)
        .ok_or(HandleError::InvalidParameter)?;
    let components = u32::try_from(components).map_err(|_| HandleError::InvalidParameter)?;
    let byte_len = vertices
        .len()
        .checked_mul(core::mem::size_of::<CompositionVertex>())
        .and_then(|length| u32::try_from(length).ok())
        .ok_or(HandleError::InvalidParameter)?;
    push_dword(
        commands,
        command_header(VIRGL_CCMD_RESOURCE_INLINE_WRITE, 0, 11 + components),
    );
    push_dword(commands, resource_id);
    for _ in 0..7 {
        push_dword(commands, 0);
    }
    push_dword(commands, byte_len);
    push_dword(commands, 1);
    push_dword(commands, 1);
    for vertex in vertices {
        for component in vertex.clip_position {
            push_float(commands, component);
        }
        for component in vertex.uv {
            push_float(commands, component);
        }
        for component in vertex.color {
            push_float(commands, component);
        }
    }
    Ok(())
}

fn push_draw(commands: &mut Vec<u8>, start_vertex: usize, vertex_count: usize) -> HandleResult<()> {
    let start_vertex = u32::try_from(start_vertex).map_err(|_| HandleError::InvalidParameter)?;
    let vertex_count = u32::try_from(vertex_count).map_err(|_| HandleError::InvalidParameter)?;
    if vertex_count == 0 || vertex_count % 3 != 0 {
        return Err(HandleError::InvalidParameter);
    }
    push_dword(commands, command_header(VIRGL_CCMD_DRAW_VBO, 0, 12));
    push_dword(commands, start_vertex);
    push_dword(commands, vertex_count);
    push_dword(commands, PIPE_PRIM_TRIANGLES);
    push_dword(commands, 0);
    push_dword(commands, 1);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, u32::MAX);
    push_dword(commands, 0);
    Ok(())
}

fn push_shader(commands: &mut Vec<u8>, handle: u32, shader_type: u32, source: &str) {
    let source_bytes = source.as_bytes();
    let source_words = (source_bytes.len() + 1).div_ceil(core::mem::size_of::<u32>());
    push_dword(
        commands,
        command_header(
            VIRGL_CCMD_CREATE_OBJECT,
            VIRGL_OBJECT_SHADER,
            5 + source_words as u32,
        ),
    );
    push_dword(commands, handle);
    push_dword(commands, shader_type);
    push_dword(commands, (source_bytes.len() + 1) as u32);
    push_dword(commands, VIRGL_SHADER_TOKEN_COUNT_HINT);
    push_dword(commands, 0);
    commands.extend_from_slice(source_bytes);
    commands.resize(commands.len() + (source_words * 4 - source_bytes.len()), 0);
}

fn push_inline_write(commands: &mut Vec<u8>, resource_id: u32, vertices: &[VertexClip4Color3]) {
    let components = vertices.len() * 7;
    push_dword(
        commands,
        command_header(VIRGL_CCMD_RESOURCE_INLINE_WRITE, 0, 11 + components as u32),
    );
    push_dword(commands, resource_id);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, core::mem::size_of_val(vertices) as u32);
    push_dword(commands, 1);
    push_dword(commands, 1);
    for vertex in vertices {
        for component in vertex.clip_position {
            push_float(commands, component);
        }
        for component in vertex.color {
            push_float(commands, component);
        }
    }
}

fn push_viewport(commands: &mut Vec<u8>, viewport: Viewport, orientation: FramebufferOrientation) {
    push_dword(
        commands,
        command_header(VIRGL_CCMD_SET_VIEWPORT_STATE, 0, 7),
    );
    push_dword(commands, 0);
    push_float(commands, viewport.width() as f32 / 2.0);
    push_float(commands, orientation.viewport_scale_y(viewport.height()));
    push_float(commands, 0.5);
    push_float(commands, viewport.width() as f32 / 2.0);
    push_float(commands, viewport.height() as f32 / 2.0);
    push_float(commands, 0.5);
}

fn push_clear_and_draw(commands: &mut Vec<u8>, clear_color: Color, vertex_count: usize) {
    push_dword(commands, command_header(VIRGL_CCMD_CLEAR, 0, 8));
    push_dword(commands, PIPE_CLEAR_COLOR0);
    push_float(commands, clear_color.red);
    push_float(commands, clear_color.green);
    push_float(commands, clear_color.blue);
    push_float(commands, clear_color.alpha);
    let depth = 1.0f64.to_bits();
    push_dword(commands, depth as u32);
    push_dword(commands, (depth >> 32) as u32);
    push_dword(commands, 0);

    push_dword(commands, command_header(VIRGL_CCMD_DRAW_VBO, 0, 12));
    push_dword(commands, 0);
    push_dword(commands, vertex_count as u32);
    push_dword(commands, PIPE_PRIM_TRIANGLES);
    push_dword(commands, 0);
    push_dword(commands, 1);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, 0);
    push_dword(commands, u32::MAX);
    push_dword(commands, 0);
}

fn build_setup_commands(
    image_resource_id: u32,
    vertex_resource_id: u32,
    cull_mode: CullMode,
    front_face: FrontFace,
) -> Vec<u8> {
    let mut commands = Vec::with_capacity(2048);
    push_dword(
        &mut commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_SURFACE, 5),
    );
    push_dword(&mut commands, SURFACE_HANDLE);
    push_dword(&mut commands, image_resource_id);
    push_dword(&mut commands, VIRGL_FORMAT_B8G8R8A8_UNORM);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);

    push_dword(
        &mut commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_BLEND, 11),
    );
    push_dword(&mut commands, BLEND_HANDLE);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0xf << 27);
    for _ in 0..7 {
        push_dword(&mut commands, 0);
    }
    push_dword(
        &mut commands,
        command_header(VIRGL_CCMD_BIND_OBJECT, VIRGL_OBJECT_BLEND, 1),
    );
    push_dword(&mut commands, BLEND_HANDLE);

    push_dword(
        &mut commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_RASTERIZER, 9),
    );
    push_dword(&mut commands, RASTERIZER_HANDLE);
    push_dword(&mut commands, rasterizer_flags(cull_mode, front_face));
    push_float(&mut commands, 1.0);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);
    push_float(&mut commands, 1.0);
    push_float(&mut commands, 0.0);
    push_float(&mut commands, 0.0);
    push_float(&mut commands, 0.0);
    push_dword(
        &mut commands,
        command_header(VIRGL_CCMD_BIND_OBJECT, VIRGL_OBJECT_RASTERIZER, 1),
    );
    push_dword(&mut commands, RASTERIZER_HANDLE);

    push_shader(
        &mut commands,
        VERTEX_SHADER_HANDLE,
        PIPE_SHADER_VERTEX,
        VERTEX_SHADER,
    );
    push_dword(&mut commands, command_header(VIRGL_CCMD_BIND_SHADER, 0, 2));
    push_dword(&mut commands, VERTEX_SHADER_HANDLE);
    push_dword(&mut commands, PIPE_SHADER_VERTEX);
    push_shader(
        &mut commands,
        FRAGMENT_SHADER_HANDLE,
        PIPE_SHADER_FRAGMENT,
        FRAGMENT_SHADER,
    );
    push_dword(&mut commands, command_header(VIRGL_CCMD_BIND_SHADER, 0, 2));
    push_dword(&mut commands, FRAGMENT_SHADER_HANDLE);
    push_dword(&mut commands, PIPE_SHADER_FRAGMENT);

    push_dword(
        &mut commands,
        command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_VERTEX_ELEMENTS, 9),
    );
    push_dword(&mut commands, VERTEX_ELEMENTS_HANDLE);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, VIRGL_FORMAT_R32G32B32A32_FLOAT);
    push_dword(&mut commands, 16);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, VIRGL_FORMAT_R32G32B32_FLOAT);
    push_dword(
        &mut commands,
        command_header(VIRGL_CCMD_BIND_OBJECT, VIRGL_OBJECT_VERTEX_ELEMENTS, 1),
    );
    push_dword(&mut commands, VERTEX_ELEMENTS_HANDLE);

    push_dword(
        &mut commands,
        command_header(VIRGL_CCMD_SET_FRAMEBUFFER_STATE, 0, 3),
    );
    push_dword(&mut commands, 1);
    push_dword(&mut commands, 0);
    push_dword(&mut commands, SURFACE_HANDLE);

    push_dword(
        &mut commands,
        command_header(VIRGL_CCMD_SET_VERTEX_BUFFERS, 0, 3),
    );
    push_dword(
        &mut commands,
        core::mem::size_of::<VertexClip4Color3>() as u32,
    );
    push_dword(&mut commands, 0);
    push_dword(&mut commands, vertex_resource_id);
    commands
}

fn rasterizer_flags(cull_mode: CullMode, front_face: FrontFace) -> u32 {
    let cull_face = match cull_mode {
        CullMode::None => 0,
        CullMode::Front => 1,
        CullMode::Back => 2,
    } << VIRGL_RASTERIZER_CULL_FACE_SHIFT;
    let front_ccw = if matches!(front_face, FrontFace::CounterClockwise) {
        VIRGL_RASTERIZER_FRONT_CCW
    } else {
        0
    };
    VIRGL_RASTERIZER_DEPTH_CLIP | cull_face | front_ccw
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::IrDepthState;

    fn dwords(bytes: &[u8]) -> Vec<u32> {
        bytes
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().expect("four-byte command word")))
            .collect()
    }

    #[test]
    fn ir_color_only_packets_keep_depth_unbound_and_disabled() {
        let mut commands = Vec::new();
        push_ir_bind_pass_state(
            &mut commands,
            11,
            0,
            12,
            640,
            480,
            FramebufferOrientation::UPPER_LEFT,
            13,
            14,
        );
        push_ir_clear(&mut commands, Some([0.1, 0.2, 0.3, 1.0]), None);
        let words = dwords(&commands);
        let framebuffer = words
            .windows(4)
            .find(|packet| packet[0] == command_header(VIRGL_CCMD_SET_FRAMEBUFFER_STATE, 0, 3))
            .expect("framebuffer packet");
        assert_eq!(
            framebuffer,
            [
                command_header(VIRGL_CCMD_SET_FRAMEBUFFER_STATE, 0, 3),
                1,
                0,
                11
            ]
        );
        assert!(!words.windows(6).any(|packet| {
            packet[0] == command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_DSA, 5)
        }));
        let clear = words
            .windows(9)
            .find(|packet| packet[0] == command_header(VIRGL_CCMD_CLEAR, 0, 8))
            .expect("clear packet");
        assert_eq!(clear[1], PIPE_CLEAR_COLOR0);
    }

    #[test]
    fn ir_depth_packets_create_bind_test_write_and_clear_z32() {
        let mut commands = Vec::new();
        push_depth_surface(&mut commands, 21, 22);
        push_ir_bind_pass_state(
            &mut commands,
            11,
            21,
            12,
            640,
            480,
            FramebufferOrientation::UPPER_LEFT,
            13,
            14,
        );
        push_ir_dsa(
            &mut commands,
            15,
            IrDepthState {
                compare: IrCompareFunction::Less,
                write_enabled: true,
            },
        );
        push_ir_clear(&mut commands, Some([0.0; 4]), Some(1.0));
        let words = dwords(&commands);
        assert_eq!(
            &words[..6],
            &[
                command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_SURFACE, 5),
                21,
                22,
                VIRGL_FORMAT_Z32_FLOAT,
                0,
                0,
            ]
        );
        let framebuffer = words
            .windows(4)
            .find(|packet| packet[0] == command_header(VIRGL_CCMD_SET_FRAMEBUFFER_STATE, 0, 3))
            .expect("framebuffer packet");
        assert_eq!(framebuffer[2], 21);
        let dsa = words
            .windows(6)
            .find(|packet| {
                packet[0] == command_header(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_DSA, 5)
            })
            .expect("DSA packet");
        assert_eq!(dsa[2], 1 | (1 << 1) | (1 << 2));
        let clear = words
            .windows(9)
            .find(|packet| packet[0] == command_header(VIRGL_CCMD_CLEAR, 0, 8))
            .expect("clear packet");
        assert_eq!(clear[1], PIPE_CLEAR_COLOR0 | PIPE_CLEAR_DEPTH);
        assert_eq!(
            u64::from(clear[6]) | (u64::from(clear[7]) << 32),
            1.0f64.to_bits()
        );
    }

    #[test]
    fn persistent_buffer_upload_encodes_byte_offset_and_payload() {
        let mut commands = Vec::new();
        let payload = [0x11_u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        push_ir_buffer_inline_write(&mut commands, 17, 4_096, &payload).unwrap();
        let words = dwords(&commands);

        assert_eq!(
            words,
            [
                command_header(VIRGL_CCMD_RESOURCE_INLINE_WRITE, 0, 13),
                17,
                0,
                0,
                0,
                0,
                4_096,
                0,
                0,
                8,
                1,
                1,
                0x4433_2211,
                0x8877_6655,
            ]
        );
    }
}
