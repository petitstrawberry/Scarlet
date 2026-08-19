//! Animated multi-pass showcase for the backend-neutral `sgfx` IR.

use std::process::ExitCode;
use std::rc::Rc;
use std::thread;
use std::time::Duration;
use std::vec::Vec;

use framebuffer::DisplaySurface;
use sgfx::{Device, SgfxImagePresentExt};
use sgfx::ir::{
    AddressMode, BlendState, BufferDesc, BufferUsage, Color, CommandEncoder, DrawUniforms, Error,
    Extent2D, FilterMode, FragmentProgram, IndexFormat, LoadOp, PixelRect, PrimitiveTopology,
    RasterState, RenderPassDesc, RenderPipelineDesc, ResourceTable, SamplerDesc, StoreOp,
    TextureDesc, TextureFormat, TextureSampleMode, TextureUsage, TextureWrite, Transform,
    VertexAttribute, VertexBufferLayout, VertexFormat,
};

const COLOR_VERTEX_STRIDE: usize = 32;
const TEXTURE_VERTEX_STRIDE: usize = 24;
const QUAD_VERTEX_COUNT: usize = 4;
const INDEX_COUNT: u32 = 6;
const MASK_SIZE: u32 = 64;
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

fn fail(message: &str, error: impl core::fmt::Debug) -> ExitCode {
    println!("sgfx_showcase: {message}: {error:?}");
    ExitCode::from(1)
}

fn reserved_bytes(capacity: usize) -> sgfx::ir::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| Error::OutOfMemory)?;
    Ok(bytes)
}

fn push_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_position(bytes: &mut Vec<u8>, x: f32, y: f32) {
    for component in [x, y, 0.0, 1.0] {
        push_f32(bytes, component);
    }
}

fn color_quad_bytes() -> sgfx::ir::Result<Vec<u8>> {
    let capacity = QUAD_VERTEX_COUNT
        .checked_mul(COLOR_VERTEX_STRIDE)
        .ok_or(Error::Overflow)?;
    let mut bytes = reserved_bytes(capacity)?;
    let vertices = [
        ([-0.78, -0.72], [0.02, 0.72, 0.82, 0.94]),
        ([0.78, -0.72], [0.96, 0.25, 0.30, 0.94]),
        ([0.78, 0.72], [1.0, 0.73, 0.24, 0.96]),
        ([-0.78, 0.72], [0.04, 0.28, 0.42, 0.94]),
    ];
    for (position, color) in vertices {
        push_position(&mut bytes, position[0], position[1]);
        for component in color {
            push_f32(&mut bytes, component);
        }
    }
    if bytes.len() != capacity {
        return Err(Error::InvalidValue);
    }
    Ok(bytes)
}

fn texture_quad_bytes() -> sgfx::ir::Result<Vec<u8>> {
    let capacity = QUAD_VERTEX_COUNT
        .checked_mul(TEXTURE_VERTEX_STRIDE)
        .ok_or(Error::Overflow)?;
    let mut bytes = reserved_bytes(capacity)?;
    let vertices = [
        ([-1.0, -1.0], [0.0, 1.0]),
        ([1.0, -1.0], [1.0, 1.0]),
        ([1.0, 1.0], [1.0, 0.0]),
        ([-1.0, 1.0], [0.0, 0.0]),
    ];
    for (position, uv) in vertices {
        push_position(&mut bytes, position[0], position[1]);
        push_f32(&mut bytes, uv[0]);
        push_f32(&mut bytes, uv[1]);
    }
    if bytes.len() != capacity {
        return Err(Error::InvalidValue);
    }
    Ok(bytes)
}

fn index_bytes() -> sgfx::ir::Result<Vec<u8>> {
    let mut bytes = reserved_bytes(INDEX_COUNT as usize * core::mem::size_of::<u16>())?;
    for index in [0_u16, 1, 2, 0, 2, 3] {
        bytes.extend_from_slice(&index.to_le_bytes());
    }
    Ok(bytes)
}

fn mask_bytes() -> sgfx::ir::Result<Vec<u8>> {
    let side = usize::try_from(MASK_SIZE).map_err(|_| Error::Overflow)?;
    let length = side.checked_mul(side).ok_or(Error::Overflow)?;
    let mut bytes = reserved_bytes(length)?;
    bytes.resize(length, 0);
    let center = MASK_SIZE as f32 * 0.5;
    for y in 0..MASK_SIZE {
        for x in 0..MASK_SIZE {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let radius = libm::sqrtf(dx * dx + dy * dy);
            let spoke = libm::sinf(libm::atan2f(dy, dx) * 8.0).abs();
            let ring = (radius - 19.0).abs() < 2.3;
            let core = radius < 9.0;
            let rays = radius > 11.0 && radius < 27.0 && spoke > 0.86;
            let coverage = if ring || core || rays { 255 } else { 0 };
            let offset = usize::try_from(y)
                .ok()
                .and_then(|row| row.checked_mul(side))
                .and_then(|row| {
                    usize::try_from(x)
                        .ok()
                        .and_then(|column| row.checked_add(column))
                })
                .ok_or(Error::Overflow)?;
            bytes[offset] = coverage;
        }
    }
    Ok(bytes)
}

fn attributes(values: &[(u32, VertexFormat, u32)]) -> sgfx::ir::Result<Vec<VertexAttribute>> {
    let mut attributes = Vec::new();
    attributes
        .try_reserve_exact(values.len())
        .map_err(|_| Error::OutOfMemory)?;
    for (location, format, offset) in values {
        attributes.push(VertexAttribute::new(*location, *format, *offset));
    }
    Ok(attributes)
}

fn transform_2d(
    scale_x: f32,
    scale_y: f32,
    angle: f32,
    x: f32,
    y: f32,
) -> sgfx::ir::Result<Transform> {
    let cosine = libm::cosf(angle);
    let sine = libm::sinf(angle);
    Transform::from_columns([
        cosine * scale_x,
        sine * scale_x,
        0.0,
        0.0,
        -sine * scale_y,
        cosine * scale_y,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        x,
        y,
        0.0,
        1.0,
    ])
}

fn inset_rect(width: u32, height: u32) -> sgfx::ir::Result<PixelRect> {
    let margin = (width.min(height) / 18).max(1);
    let horizontal = margin.checked_mul(2).ok_or(Error::Overflow)?;
    if width <= horizontal || height <= horizontal {
        PixelRect::new(0, 0, width, height)
    } else {
        PixelRect::new(margin, margin, width - horizontal, height - horizontal)
    }
}

fn main() -> ExitCode {
    macro_rules! attempt {
        ($expression:expr, $message:literal) => {
            match $expression {
                Ok(value) => value,
                Err(error) => return fail($message, error),
            }
        };
    }

    let display = attempt!(
        DisplaySurface::open_primary(),
        "failed to open primary display"
    );
    let display_info = attempt!(display.get_info(), "failed to query display");
    let device = attempt!(Device::open("/dev/gpu0"), "failed to open GPU");
    let capabilities = device.capabilities();
    if !capabilities.supports_rendering()
        || !capabilities.supports_presentation()
        || !capabilities.supports_image_upload()
    {
        println!("sgfx_showcase: required GPU capabilities are unavailable");
        return ExitCode::from(1);
    }
    let context = attempt!(device.create_context(), "failed to create context");
    let image = Rc::new(attempt!(
        context.create_image(display_info.width, display_info.height),
        "failed to create presentation image"
    ));
    let queue = attempt!(context.create_queue(), "failed to create queue");

    let offscreen_width = image.width().min(640).max(1);
    let offscreen_height = image.height().min(400).max(1);
    let screen_extent = attempt!(
        Extent2D::new(image.width(), image.height()),
        "invalid screen extent"
    );
    let offscreen_extent = attempt!(
        Extent2D::new(offscreen_width, offscreen_height),
        "invalid offscreen extent"
    );
    let mask_extent = attempt!(Extent2D::new(MASK_SIZE, MASK_SIZE), "invalid mask extent");
    let screen_area = attempt!(
        PixelRect::new(0, 0, image.width(), image.height()),
        "invalid screen area"
    );
    let offscreen_area = attempt!(
        PixelRect::new(0, 0, offscreen_width, offscreen_height),
        "invalid offscreen area"
    );
    let mask_area = attempt!(
        PixelRect::new(0, 0, MASK_SIZE, MASK_SIZE),
        "invalid mask area"
    );

    let color_vertices = attempt!(color_quad_bytes(), "failed to build color vertices");
    let texture_vertices = attempt!(texture_quad_bytes(), "failed to build texture vertices");
    let indices = attempt!(index_bytes(), "failed to build indices");
    let mask = attempt!(mask_bytes(), "failed to build alpha mask");
    let color_buffer_size = attempt!(
        u64::try_from(color_vertices.len()).map_err(|_| Error::Overflow),
        "color vertex buffer is too large"
    );
    let texture_buffer_size = attempt!(
        u64::try_from(texture_vertices.len()).map_err(|_| Error::Overflow),
        "texture vertex buffer is too large"
    );
    let index_buffer_size = attempt!(
        u64::try_from(indices.len()).map_err(|_| Error::Overflow),
        "index buffer is too large"
    );

    let resources = Rc::new(ResourceTable::new());
    let screen = attempt!(
        resources.define_texture(attempt!(
            TextureDesc::new(
                TextureFormat::Bgra8Unorm,
                screen_extent,
                TextureUsage::RENDER_ATTACHMENT | TextureUsage::PRESENT,
            ),
            "invalid screen descriptor"
        )),
        "failed to define screen texture"
    );
    let offscreen = attempt!(
        resources.define_texture(attempt!(
            TextureDesc::new(
                TextureFormat::Bgra8Unorm,
                offscreen_extent,
                TextureUsage::RENDER_ATTACHMENT | TextureUsage::SAMPLED | TextureUsage::COPY_SRC,
            ),
            "invalid offscreen descriptor"
        )),
        "failed to define offscreen texture"
    );
    let copied = attempt!(
        resources.define_texture(attempt!(
            TextureDesc::new(
                TextureFormat::Bgra8Unorm,
                offscreen_extent,
                TextureUsage::SAMPLED | TextureUsage::COPY_DST,
            ),
            "invalid copied texture descriptor"
        )),
        "failed to define copied texture"
    );
    let alpha_mask = attempt!(
        resources.define_texture(attempt!(
            TextureDesc::new(
                TextureFormat::R8Unorm,
                mask_extent,
                TextureUsage::SAMPLED | TextureUsage::COPY_DST,
            ),
            "invalid alpha-mask descriptor"
        )),
        "failed to define alpha-mask texture"
    );

    let color_buffer = attempt!(
        resources.define_buffer(attempt!(
            BufferDesc::new(
                color_buffer_size,
                BufferUsage::VERTEX | BufferUsage::COPY_DST,
            ),
            "invalid color buffer descriptor"
        )),
        "failed to define color buffer"
    );
    let texture_buffer = attempt!(
        resources.define_buffer(attempt!(
            BufferDesc::new(
                texture_buffer_size,
                BufferUsage::VERTEX | BufferUsage::COPY_DST,
            ),
            "invalid texture buffer descriptor"
        )),
        "failed to define texture buffer"
    );
    let index_buffer = attempt!(
        resources.define_buffer(attempt!(
            BufferDesc::new(
                index_buffer_size,
                BufferUsage::INDEX | BufferUsage::COPY_DST,
            ),
            "invalid index buffer descriptor"
        )),
        "failed to define index buffer"
    );

    let color_layout = attempt!(
        VertexBufferLayout::new(
            COLOR_VERTEX_STRIDE as u32,
            attempt!(
                attributes(&[
                    (0, VertexFormat::Float32x4, 0),
                    (1, VertexFormat::Float32x4, 16),
                ]),
                "failed to build color attributes"
            ),
        ),
        "invalid color vertex layout"
    );
    let texture_layout = attempt!(
        VertexBufferLayout::new(
            TEXTURE_VERTEX_STRIDE as u32,
            attempt!(
                attributes(&[
                    (0, VertexFormat::Float32x4, 0),
                    (1, VertexFormat::Float32x2, 16),
                ]),
                "failed to build texture attributes"
            ),
        ),
        "invalid texture vertex layout"
    );
    let raster = RasterState::new(
        sgfx::ir::CullMode::None,
        sgfx::ir::FrontFace::CounterClockwise,
    );
    let color_pipeline = attempt!(
        resources.define_render_pipeline(attempt!(
            RenderPipelineDesc::new(
                TextureFormat::Bgra8Unorm,
                PrimitiveTopology::TriangleList,
                color_layout.clone(),
                FragmentProgram::VertexColor,
                BlendState::SOURCE_OVER_STRAIGHT_ALPHA,
                raster,
            ),
            "invalid color pipeline"
        )),
        "failed to define color pipeline"
    );
    let solid_pipeline = attempt!(
        resources.define_render_pipeline(attempt!(
            RenderPipelineDesc::new(
                TextureFormat::Bgra8Unorm,
                PrimitiveTopology::TriangleList,
                color_layout,
                FragmentProgram::Solid,
                BlendState::SOURCE_OVER_STRAIGHT_ALPHA,
                raster,
            ),
            "invalid solid pipeline"
        )),
        "failed to define solid pipeline"
    );
    let texture_pipeline = attempt!(
        resources.define_render_pipeline(attempt!(
            RenderPipelineDesc::new(
                TextureFormat::Bgra8Unorm,
                PrimitiveTopology::TriangleList,
                texture_layout.clone(),
                FragmentProgram::Texture(TextureSampleMode::Rgba),
                BlendState::SOURCE_OVER_STRAIGHT_ALPHA,
                raster,
            ),
            "invalid texture pipeline"
        )),
        "failed to define texture pipeline"
    );
    let mask_pipeline = attempt!(
        resources.define_render_pipeline(attempt!(
            RenderPipelineDesc::new(
                TextureFormat::Bgra8Unorm,
                PrimitiveTopology::TriangleList,
                texture_layout,
                FragmentProgram::Texture(TextureSampleMode::AlphaMask),
                BlendState::SOURCE_OVER_STRAIGHT_ALPHA,
                raster,
            ),
            "invalid mask pipeline"
        )),
        "failed to define mask pipeline"
    );
    let sampler = attempt!(
        resources.define_sampler(SamplerDesc::new(
            FilterMode::Linear,
            FilterMode::Linear,
            AddressMode::ClampToEdge,
            AddressMode::ClampToEdge,
        )),
        "failed to define sampler"
    );

    let mut ir_resources = attempt!(
        context.create_ir_resources(Rc::clone(&resources)),
        "failed to create IR resource cache"
    );
    if let Err(error) = ir_resources.map_image(screen.id(), Rc::clone(&image)) {
        return fail("failed to map presentation image", error);
    }

    println!(
        "sgfx_showcase: animated {}x{} multi-pass IR scene",
        image.width(),
        image.height()
    );
    let white = attempt!(Color::rgba(1.0, 1.0, 1.0, 1.0), "invalid white color");
    let mut first_frame = true;
    let mut frame = 0_u32;
    loop {
        let phase = frame as f32 * 0.012;
        let mut encoder = CommandEncoder::new(&resources);
        if first_frame {
            if let Err(error) = encoder.write_buffer(color_buffer, 0, &color_vertices) {
                return fail("failed to upload color vertices", error);
            }
            if let Err(error) = encoder.write_buffer(texture_buffer, 0, &texture_vertices) {
                return fail("failed to upload texture vertices", error);
            }
            if let Err(error) = encoder.write_buffer(index_buffer, 0, &indices) {
                return fail("failed to upload indices", error);
            }
            let mask_write = attempt!(
                TextureWrite::new(mask_area, MASK_SIZE, &mask),
                "invalid mask upload"
            );
            if let Err(error) = encoder.write_texture(alpha_mask, mask_write) {
                return fail("failed to record mask upload", error);
            }
        }

        let offscreen_pass = attempt!(
            RenderPassDesc::new(
                &resources,
                offscreen,
                offscreen_area,
                LoadOp::Clear(attempt!(
                    Color::rgba(0.008, 0.035, 0.065, 1.0),
                    "invalid offscreen clear color"
                )),
                StoreOp::Store,
            ),
            "invalid offscreen pass"
        );
        {
            let mut pass = attempt!(
                encoder.begin_render_pass(offscreen_pass),
                "failed to begin offscreen pass"
            );
            if let Err(error) = pass.set_pipeline(color_pipeline) {
                return fail("failed to bind color pipeline", error);
            }
            if let Err(error) = pass.set_vertex_buffer(color_buffer, 0) {
                return fail("failed to bind color vertices", error);
            }
            if let Err(error) = pass.set_index_buffer(index_buffer, 0, IndexFormat::Uint16) {
                return fail("failed to bind indices", error);
            }
            if let Err(error) = pass.set_scissor(Some(attempt!(
                inset_rect(offscreen_width, offscreen_height),
                "invalid offscreen scissor"
            ))) {
                return fail("failed to set offscreen scissor", error);
            }
            let orbit = attempt!(
                transform_2d(0.92, 0.92, phase, 0.0, 0.0),
                "invalid orbit transform"
            );
            if let Err(error) = pass.set_uniforms(DrawUniforms::new(orbit, white)) {
                return fail("failed to set orbit uniforms", error);
            }
            if let Err(error) = pass.draw_indexed(INDEX_COUNT, 0, 0) {
                return fail("failed to record indexed orbit", error);
            }
            let echo = attempt!(
                transform_2d(0.42, 0.42, -phase * 1.7, 0.42, -0.34),
                "invalid echo transform"
            );
            let echo_tint = attempt!(Color::rgba(0.25, 0.92, 1.0, 0.72), "invalid echo tint");
            if let Err(error) = pass.set_uniforms(DrawUniforms::new(echo, echo_tint)) {
                return fail("failed to set echo uniforms", error);
            }
            if let Err(error) = pass.draw_indexed(INDEX_COUNT, 0, 0) {
                return fail("failed to record indexed echo", error);
            }
            if let Err(error) = pass.end() {
                return fail("failed to end offscreen pass", error);
            }
        }
        if let Err(error) =
            encoder.copy_texture_to_texture(offscreen, offscreen_area, copied, offscreen_area)
        {
            return fail("failed to record offscreen copy", error);
        }

        let screen_pass = attempt!(
            RenderPassDesc::new(
                &resources,
                screen,
                screen_area,
                LoadOp::Clear(attempt!(
                    Color::rgba(0.004, 0.008, 0.018, 1.0),
                    "invalid screen clear color"
                )),
                StoreOp::Store,
            ),
            "invalid screen pass"
        );
        {
            let mut pass = attempt!(
                encoder.begin_render_pass(screen_pass),
                "failed to begin screen pass"
            );
            if let Err(error) = pass.set_pipeline(texture_pipeline) {
                return fail("failed to bind texture pipeline", error);
            }
            if let Err(error) = pass.set_vertex_buffer(texture_buffer, 0) {
                return fail("failed to bind texture vertices", error);
            }
            if let Err(error) = pass.set_index_buffer(index_buffer, 0, IndexFormat::Uint16) {
                return fail("failed to bind screen indices", error);
            }
            if let Err(error) = pass.set_texture(copied) {
                return fail("failed to bind copied texture", error);
            }
            if let Err(error) = pass.set_sampler(sampler) {
                return fail("failed to bind sampler", error);
            }
            if let Err(error) = pass.set_scissor(Some(attempt!(
                inset_rect(image.width(), image.height()),
                "invalid screen scissor"
            ))) {
                return fail("failed to set screen scissor", error);
            }
            let main_panel = attempt!(
                transform_2d(0.72, 0.70, 0.025 * libm::sinf(phase), -0.10, 0.02),
                "invalid main-panel transform"
            );
            if let Err(error) = pass.set_uniforms(DrawUniforms::new(main_panel, white)) {
                return fail("failed to set main-panel uniforms", error);
            }
            if let Err(error) = pass.draw_indexed(INDEX_COUNT, 0, 0) {
                return fail("failed to draw copied panel", error);
            }

            if let Err(error) = pass.set_texture(offscreen) {
                return fail("failed to bind offscreen texture", error);
            }
            let thumbnail = attempt!(
                transform_2d(0.23, 0.23, -phase * 0.65, 0.67, -0.62),
                "invalid thumbnail transform"
            );
            let cool_tint = attempt!(Color::rgba(0.50, 0.92, 1.0, 0.82), "invalid thumbnail tint");
            if let Err(error) = pass.set_uniforms(DrawUniforms::new(thumbnail, cool_tint)) {
                return fail("failed to set thumbnail uniforms", error);
            }
            if let Err(error) = pass.draw_indexed(INDEX_COUNT, 0, 0) {
                return fail("failed to draw offscreen thumbnail", error);
            }

            if let Err(error) = pass.set_pipeline(mask_pipeline) {
                return fail("failed to bind mask pipeline", error);
            }
            if let Err(error) = pass.set_texture(alpha_mask) {
                return fail("failed to bind alpha mask", error);
            }
            if let Err(error) = pass.set_scissor(None) {
                return fail("failed to reset scissor", error);
            }
            let badge = attempt!(
                transform_2d(0.17, 0.17, phase * 1.9, -0.72, 0.62),
                "invalid badge transform"
            );
            let gold = attempt!(Color::rgba(1.0, 0.72, 0.22, 0.94), "invalid badge color");
            if let Err(error) = pass.set_uniforms(DrawUniforms::new(badge, gold)) {
                return fail("failed to set badge uniforms", error);
            }
            if let Err(error) = pass.draw_indexed(INDEX_COUNT, 0, 0) {
                return fail("failed to draw alpha-mask badge", error);
            }

            if let Err(error) = pass.set_pipeline(solid_pipeline) {
                return fail("failed to bind solid pipeline", error);
            }
            if let Err(error) = pass.set_vertex_buffer(color_buffer, 0) {
                return fail("failed to bind solid vertices", error);
            }
            let accent = attempt!(
                transform_2d(0.10, 0.018, 0.0, -0.72, 0.38),
                "invalid accent transform"
            );
            let coral = attempt!(Color::rgba(1.0, 0.24, 0.28, 0.88), "invalid accent color");
            if let Err(error) = pass.set_uniforms(DrawUniforms::new(accent, coral)) {
                return fail("failed to set accent uniforms", error);
            }
            if let Err(error) = pass.draw_indexed(INDEX_COUNT, 0, 0) {
                return fail("failed to draw solid accent", error);
            }
            if let Err(error) = pass.end() {
                return fail("failed to end screen pass", error);
            }
        }

        let commands = attempt!(encoder.finish(), "failed to finish IR commands");
        if let Err(error) = queue.submit_ir(&context, &mut ir_resources, &commands) {
            return fail("IR submission failed", error);
        }
        if let Err(error) = image.present(&display) {
            return fail("image present failed", error);
        }
        first_frame = false;
        frame = frame.wrapping_add(1);
        thread::sleep(FRAME_INTERVAL);
    }
}
