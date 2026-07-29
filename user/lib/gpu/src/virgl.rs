//! Private command backend for the built-in vertex-color pipeline.

use alloc::{rc::Rc, vec::Vec};
use core::cell::Cell;

use framebuffer::DisplaySurface;
use gpu_raw::{
    GPU_DEVICE_STATE_READY, GPU_EXECUTION_SUPPORT_PRESENTATION, GPU_EXECUTION_SUPPORT_QUEUE,
    GPU_RESULT_SUCCESS, Gpu as RawGpu, GpuBuffer as RawBuffer, GpuContext as RawContext,
    GpuDialect as RawDialect, GpuImage as RawImage, GpuQueue as RawQueue,
};
use std::handle::{HandleError, HandleResult};

use crate::{
    Capabilities, Color, CullMode, FrontFace, PipelineDesc, PipelineKind, VertexClip4Color3,
    Viewport,
};

const VIRGL_CCMD_CREATE_OBJECT: u32 = 1;
const VIRGL_CCMD_BIND_OBJECT: u32 = 2;
const VIRGL_CCMD_SET_VIEWPORT_STATE: u32 = 4;
const VIRGL_CCMD_SET_FRAMEBUFFER_STATE: u32 = 5;
const VIRGL_CCMD_SET_VERTEX_BUFFERS: u32 = 6;
const VIRGL_CCMD_CLEAR: u32 = 7;
const VIRGL_CCMD_DRAW_VBO: u32 = 8;
const VIRGL_CCMD_RESOURCE_INLINE_WRITE: u32 = 9;
const VIRGL_CCMD_BIND_SHADER: u32 = 31;

const VIRGL_OBJECT_BLEND: u32 = 1;
const VIRGL_OBJECT_RASTERIZER: u32 = 2;
const VIRGL_OBJECT_SHADER: u32 = 4;
const VIRGL_OBJECT_VERTEX_ELEMENTS: u32 = 5;
const VIRGL_OBJECT_SURFACE: u32 = 8;

const VIRGL_FORMAT_B8G8R8A8_UNORM: u32 = 1;
const VIRGL_FORMAT_R32G32B32A32_FLOAT: u32 = 31;
const VIRGL_FORMAT_R32G32B32_FLOAT: u32 = 30;
const PIPE_SHADER_VERTEX: u32 = 0;
const PIPE_SHADER_FRAGMENT: u32 = 1;
const VIRGL_SHADER_TOKEN_COUNT_HINT: u32 = 300;
const PIPE_PRIM_TRIANGLES: u32 = 4;
const PIPE_CLEAR_COLOR0: u32 = 1 << 2;

const SURFACE_HANDLE: u32 = 1;
const VERTEX_SHADER_HANDLE: u32 = 2;
const FRAGMENT_SHADER_HANDLE: u32 = 3;
const VERTEX_ELEMENTS_HANDLE: u32 = 4;
const BLEND_HANDLE: u32 = 5;
const RASTERIZER_HANDLE: u32 = 6;
const VIRGL_RASTERIZER_DEPTH_CLIP: u32 = 1 << 1;
const VIRGL_RASTERIZER_CULL_FACE_SHIFT: u32 = 8;
const VIRGL_RASTERIZER_FRONT_CCW: u32 = 1 << 15;

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
        })
    }
}

pub(crate) struct Context {
    device: Rc<Device>,
    raw: RawContext,
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
        })
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
        Ok(Queue {
            raw: self.raw.create_queue()?,
            context_handle: self.handle_id(),
        })
    }

    fn handle_id(&self) -> i32 {
        self.raw.as_handle().as_raw()
    }
}

pub(crate) struct Queue {
    raw: RawQueue,
    context_handle: i32,
}

impl Queue {
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
            || vertices.len() % 3 != 0
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
        push_viewport(&mut commands, viewport, image.orientation);
        push_inline_write(&mut commands, pipeline.vertex_resource_id, vertices);
        push_clear_and_draw(&mut commands, clear_color, vertices.len());
        self.raw.submit(&commands)?;
        if needs_setup {
            pipeline.initialized.set(true);
        }
        Ok(())
    }
}

pub(crate) struct Image {
    raw: RawImage,
    resource_id: u32,
    context_handle: i32,
    orientation: FramebufferOrientation,
}

impl Image {
    pub(crate) fn present(&self, display: &DisplaySurface) -> HandleResult<()> {
        display.present_image(self.raw.as_handle(), None)
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

fn resource_id_from_token(token: u64) -> HandleResult<u32> {
    u32::try_from(token)
        .ok()
        .filter(|&resource_id| resource_id != 0)
        .ok_or(HandleError::SystemError(-1))
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
    push_dword(
        commands,
        (vertices.len() * core::mem::size_of::<VertexClip4Color3>()) as u32,
    );
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
