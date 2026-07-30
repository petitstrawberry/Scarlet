//! Visible advanced scene submitted through the backend-neutral `sgfx` IR.

use std::process::ExitCode;
use std::thread;
use std::time::Duration;
use std::vec::Vec;

use framebuffer::DisplaySurface;
use sgfx::Device;
use sgfx::ir::{
    BlendState, BufferDesc, BufferUsage, Color, CommandEncoder, DrawUniforms, Error, Extent2D,
    FragmentProgram, LoadOp, PixelRect, PrimitiveTopology, RasterState, RenderPassDesc,
    RenderPipelineDesc, ResourceTable, StoreOp, TextureDesc, TextureFormat, TextureUsage,
    Transform, VertexAttribute, VertexBufferLayout, VertexFormat,
};

const VERTEX_STRIDE: usize = 28;
const STAR_SEGMENTS: usize = 32;
const FRAME_HOLD_INTERVAL: Duration = Duration::from_secs(1);

fn fail(message: &str, error: impl core::fmt::Debug) -> ExitCode {
    println!("sgfx_showcase: {message}: {error:?}");
    ExitCode::from(1)
}

fn push_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_vertex(bytes: &mut Vec<u8>, x: f32, y: f32, color: [f32; 3]) {
    for component in [x, y, 0.0, 1.0] {
        push_f32(bytes, component);
    }
    for component in color {
        push_f32(bytes, component);
    }
}

fn push_triangle(bytes: &mut Vec<u8>, positions: [[f32; 2]; 3], colors: [[f32; 3]; 3]) {
    for index in 0..3 {
        push_vertex(
            bytes,
            positions[index][0],
            positions[index][1],
            colors[index],
        );
    }
}

fn push_panel(
    bytes: &mut Vec<u8>,
    left: f32,
    bottom: f32,
    right: f32,
    top: f32,
    colors: [[f32; 3]; 4],
) {
    push_triangle(
        bytes,
        [[left, bottom], [right, bottom], [left, top]],
        [colors[0], colors[1], colors[2]],
    );
    push_triangle(
        bytes,
        [[left, top], [right, bottom], [right, top]],
        [colors[2], colors[1], colors[3]],
    );
}

fn scene_vertices(width: u32, height: u32) -> sgfx::ir::Result<(Vec<u8>, u32)> {
    let maximum_vertices = STAR_SEGMENTS
        .checked_mul(3)
        .and_then(|value| value.checked_add(30))
        .ok_or(Error::Overflow)?;
    let capacity = maximum_vertices
        .checked_mul(VERTEX_STRIDE)
        .ok_or(Error::Overflow)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| Error::OutOfMemory)?;

    let aspect_scale = if width > height {
        height as f32 / width as f32
    } else {
        1.0
    };
    push_panel(
        &mut bytes,
        -0.96,
        -0.90,
        0.96,
        0.90,
        [
            [0.025, 0.055, 0.13],
            [0.08, 0.03, 0.17],
            [0.025, 0.16, 0.22],
            [0.18, 0.055, 0.22],
        ],
    );
    push_panel(
        &mut bytes,
        -0.88,
        -0.78,
        -0.50,
        0.72,
        [
            [0.04, 0.22, 0.30],
            [0.04, 0.08, 0.16],
            [0.14, 0.48, 0.56],
            [0.10, 0.18, 0.34],
        ],
    );
    push_panel(
        &mut bytes,
        0.54,
        -0.70,
        0.86,
        -0.18,
        [
            [0.50, 0.10, 0.30],
            [0.20, 0.04, 0.24],
            [0.96, 0.34, 0.38],
            [0.52, 0.08, 0.36],
        ],
    );

    let center = [0.16 * aspect_scale, 0.10];
    let outer_radius_x = 0.62 * aspect_scale;
    let outer_radius_y = 0.62;
    let inner_radius_x = 0.24 * aspect_scale;
    let inner_radius_y = 0.24;
    for segment in 0..STAR_SEGMENTS {
        let angle0 = core::f32::consts::TAU * segment as f32 / STAR_SEGMENTS as f32;
        let angle1 = core::f32::consts::TAU * (segment + 1) as f32 / STAR_SEGMENTS as f32;
        let radius0 = if segment % 2 == 0 {
            [outer_radius_x, outer_radius_y]
        } else {
            [inner_radius_x, inner_radius_y]
        };
        let radius1 = if (segment + 1) % 2 == 0 {
            [outer_radius_x, outer_radius_y]
        } else {
            [inner_radius_x, inner_radius_y]
        };
        let edge0 = [
            center[0] + libm::cosf(angle0) * radius0[0],
            center[1] + libm::sinf(angle0) * radius0[1],
        ];
        let edge1 = [
            center[0] + libm::cosf(angle1) * radius1[0],
            center[1] + libm::sinf(angle1) * radius1[1],
        ];
        let phase = segment as f32 / STAR_SEGMENTS as f32;
        let edge_color0 = [0.20 + phase * 0.76, 0.82 - phase * 0.42, 0.92];
        let edge_color1 = [0.96, 0.20 + phase * 0.55, 0.50 + phase * 0.42];
        push_triangle(
            &mut bytes,
            [center, edge0, edge1],
            [[1.0, 0.92, 0.64], edge_color0, edge_color1],
        );
    }

    let vertex_count = bytes.len() / VERTEX_STRIDE;
    if bytes.len() % VERTEX_STRIDE != 0 || vertex_count % 3 != 0 {
        return Err(Error::InvalidValue);
    }
    Ok((
        bytes,
        u32::try_from(vertex_count).map_err(|_| Error::Overflow)?,
    ))
}

fn main() -> ExitCode {
    let display = match DisplaySurface::open_primary() {
        Ok(display) => display,
        Err(error) => return fail("failed to open primary display", error),
    };
    let display_info = match display.get_info() {
        Ok(info) => info,
        Err(error) => return fail("failed to query display", error),
    };
    let device = match Device::open("/dev/gpu0") {
        Ok(device) => device,
        Err(error) => return fail("failed to open GPU", error),
    };
    let capabilities = device.capabilities();
    if !capabilities.supports_rendering() || !capabilities.supports_presentation() {
        println!("sgfx_showcase: rendering or presentation is unsupported");
        return ExitCode::from(1);
    }
    let context = match device.create_context() {
        Ok(context) => context,
        Err(error) => return fail("failed to create context", error),
    };
    let image = match context.create_image(display_info.width, display_info.height) {
        Ok(image) => image,
        Err(error) => return fail("failed to create render target", error),
    };
    let queue = match context.create_queue() {
        Ok(queue) => queue,
        Err(error) => return fail("failed to create queue", error),
    };
    let (vertex_bytes, vertex_count) = match scene_vertices(image.width(), image.height()) {
        Ok(scene) => scene,
        Err(error) => return fail("failed to build scene vertices", error),
    };

    let resources = ResourceTable::new();
    let extent = match Extent2D::new(image.width(), image.height()) {
        Ok(extent) => extent,
        Err(error) => return fail("invalid display extent", error),
    };
    let target_desc = match TextureDesc::new(
        TextureFormat::Bgra8Unorm,
        extent,
        TextureUsage::RENDER_ATTACHMENT | TextureUsage::PRESENT,
    ) {
        Ok(desc) => desc,
        Err(error) => return fail("invalid target descriptor", error),
    };
    let target = match resources.define_texture(target_desc) {
        Ok(target) => target,
        Err(error) => return fail("failed to define IR target", error),
    };
    let buffer_size = match u64::try_from(vertex_bytes.len()) {
        Ok(size) => size,
        Err(error) => return fail("vertex buffer size overflow", error),
    };
    let vertex_buffer_desc =
        match BufferDesc::new(buffer_size, BufferUsage::VERTEX | BufferUsage::COPY_DST) {
            Ok(desc) => desc,
            Err(error) => return fail("invalid vertex buffer descriptor", error),
        };
    let vertex_buffer = match resources.define_buffer(vertex_buffer_desc) {
        Ok(buffer) => buffer,
        Err(error) => return fail("failed to define IR vertex buffer", error),
    };
    let mut attributes = Vec::new();
    if attributes.try_reserve_exact(2).is_err() {
        return fail("failed to reserve vertex attributes", Error::OutOfMemory);
    }
    attributes.push(VertexAttribute::new(0, VertexFormat::Float32x4, 0));
    attributes.push(VertexAttribute::new(1, VertexFormat::Float32x3, 16));
    let layout = match VertexBufferLayout::new(VERTEX_STRIDE as u32, attributes) {
        Ok(layout) => layout,
        Err(error) => return fail("invalid vertex layout", error),
    };
    let pipeline_desc = match RenderPipelineDesc::new(
        TextureFormat::Bgra8Unorm,
        PrimitiveTopology::TriangleList,
        layout,
        FragmentProgram::VertexColor,
        BlendState::REPLACE,
        RasterState::new(
            sgfx::ir::CullMode::None,
            sgfx::ir::FrontFace::CounterClockwise,
        ),
    ) {
        Ok(desc) => desc,
        Err(error) => return fail("invalid pipeline descriptor", error),
    };
    let pipeline = match resources.define_render_pipeline(pipeline_desc) {
        Ok(pipeline) => pipeline,
        Err(error) => return fail("failed to define IR pipeline", error),
    };

    let mut encoder = CommandEncoder::new(&resources);
    if let Err(error) = encoder.write_buffer(vertex_buffer, 0, &vertex_bytes) {
        return fail("failed to record vertex upload", error);
    }
    let area = match PixelRect::new(0, 0, image.width(), image.height()) {
        Ok(area) => area,
        Err(error) => return fail("invalid render area", error),
    };
    let pass_desc = match RenderPassDesc::new(
        &resources,
        target,
        area,
        LoadOp::Clear(match Color::rgba(0.008, 0.012, 0.028, 1.0) {
            Ok(color) => color,
            Err(error) => return fail("invalid clear color", error),
        }),
        StoreOp::Store,
    ) {
        Ok(desc) => desc,
        Err(error) => return fail("invalid render pass", error),
    };
    let mut pass = match encoder.begin_render_pass(pass_desc) {
        Ok(pass) => pass,
        Err(error) => return fail("failed to begin IR render pass", error),
    };
    if let Err(error) = pass.set_pipeline(pipeline) {
        return fail("failed to bind IR pipeline", error);
    }
    if let Err(error) = pass.set_vertex_buffer(vertex_buffer, 0) {
        return fail("failed to bind IR vertex buffer", error);
    }
    let white = match Color::rgba(1.0, 1.0, 1.0, 1.0) {
        Ok(color) => color,
        Err(error) => return fail("invalid draw color", error),
    };
    if let Err(error) = pass.set_uniforms(DrawUniforms::new(Transform::identity(), white)) {
        return fail("failed to bind IR uniforms", error);
    }
    if let Err(error) = pass.draw(vertex_count, 0) {
        return fail("failed to record IR draw", error);
    }
    if let Err(error) = pass.end() {
        return fail("failed to end IR render pass", error);
    }
    let commands = match encoder.finish() {
        Ok(commands) => commands,
        Err(error) => return fail("failed to finish IR commands", error),
    };
    let present_target = match image.map_ir_present_target(&resources, target) {
        Ok(target) => target,
        Err(error) => return fail("failed to map IR presentation target", error),
    };
    if let Err(error) = queue.submit_ir(&context, present_target, &commands) {
        return fail("IR submission failed", error);
    }
    if let Err(error) = image.present(&display) {
        return fail("image present failed", error);
    }

    println!(
        "sgfx_showcase: displaying {} IR-derived vertices at {}x{}",
        vertex_count,
        image.width(),
        image.height()
    );
    loop {
        thread::sleep(FRAME_HOLD_INTERVAL);
    }
}
