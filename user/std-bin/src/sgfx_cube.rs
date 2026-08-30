//! Standalone rotating colored cube sample.

use std::process::ExitCode;
use std::thread;
use std::time::Duration;
use std::vec::Vec;

use framebuffer::DisplaySurface;
use sgfx::ir::{
    BlendState, BufferDesc, BufferUsage, Color, CommandEncoder, DrawUniforms, FragmentProgram,
    LoadOp, PixelRect, PrimitiveTopology, RasterState, RenderPassDesc, RenderPipelineDesc, StoreOp,
    TextureFormat, Transform, VertexAttribute, VertexBufferLayout, VertexFormat,
};

mod sgfx_ir_support;

use sgfx_ir_support::MappedTarget;
const FACE_COUNT: usize = 6;
const VERTICES_PER_FACE: usize = 4;
const VERTEX_COUNT: usize = FACE_COUNT * 6;
const DEGREES_TO_RADIANS: f32 = core::f32::consts::PI / 180.0;

const KMSCUBE_POSITIONS: [[f32; 3]; FACE_COUNT * VERTICES_PER_FACE] = [
    [-1.0, -1.0, 1.0],
    [1.0, -1.0, 1.0],
    [-1.0, 1.0, 1.0],
    [1.0, 1.0, 1.0],
    [1.0, -1.0, -1.0],
    [-1.0, -1.0, -1.0],
    [1.0, 1.0, -1.0],
    [-1.0, 1.0, -1.0],
    [1.0, -1.0, 1.0],
    [1.0, -1.0, -1.0],
    [1.0, 1.0, 1.0],
    [1.0, 1.0, -1.0],
    [-1.0, -1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [-1.0, 1.0, -1.0],
    [-1.0, 1.0, 1.0],
    [-1.0, 1.0, 1.0],
    [1.0, 1.0, 1.0],
    [-1.0, 1.0, -1.0],
    [1.0, 1.0, -1.0],
    [-1.0, -1.0, -1.0],
    [1.0, -1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [1.0, -1.0, 1.0],
];

const KMSCUBE_COLORS: [[f32; 3]; FACE_COUNT * VERTICES_PER_FACE] = [
    [0.0, 0.0, 1.0],
    [1.0, 0.0, 1.0],
    [0.0, 1.0, 1.0],
    [1.0, 1.0, 1.0],
    [1.0, 0.0, 0.0],
    [0.0, 0.0, 0.0],
    [1.0, 1.0, 0.0],
    [0.0, 1.0, 0.0],
    [1.0, 0.0, 1.0],
    [1.0, 0.0, 0.0],
    [1.0, 1.0, 1.0],
    [1.0, 1.0, 0.0],
    [0.0, 0.0, 0.0],
    [0.0, 0.0, 1.0],
    [0.0, 1.0, 0.0],
    [0.0, 1.0, 1.0],
    [0.0, 1.0, 1.0],
    [1.0, 1.0, 1.0],
    [0.0, 1.0, 0.0],
    [1.0, 1.0, 0.0],
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0],
    [1.0, 0.0, 1.0],
];

#[derive(Clone, Copy)]
struct Matrix {
    columns: [[f32; 4]; 4],
}

impl Matrix {
    const fn identity() -> Self {
        Self {
            columns: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    fn multiply(left: Self, right: Self) -> Self {
        let mut result = Self::identity();
        for column in 0..4 {
            for row in 0..4 {
                result.columns[column][row] = left.columns[column][0] * right.columns[0][row]
                    + left.columns[column][1] * right.columns[1][row]
                    + left.columns[column][2] * right.columns[2][row]
                    + left.columns[column][3] * right.columns[3][row];
            }
        }
        result
    }

    fn translate(&mut self, x: f32, y: f32, z: f32) {
        for row in 0..4 {
            self.columns[3][row] +=
                self.columns[0][row] * x + self.columns[1][row] * y + self.columns[2][row] * z;
        }
    }

    fn rotate(&mut self, angle_degrees: f32, axis: [f32; 3]) {
        let [x, y, z] = axis;
        let radians = angle_degrees * DEGREES_TO_RADIANS;
        let sine = libm::sinf(radians);
        let cosine = libm::cosf(radians);
        let one_minus_cosine = 1.0 - cosine;
        let rotation = Self {
            columns: [
                [
                    one_minus_cosine * x * x + cosine,
                    one_minus_cosine * x * y - z * sine,
                    one_minus_cosine * z * x + y * sine,
                    0.0,
                ],
                [
                    one_minus_cosine * x * y + z * sine,
                    one_minus_cosine * y * y + cosine,
                    one_minus_cosine * y * z - x * sine,
                    0.0,
                ],
                [
                    one_minus_cosine * z * x - y * sine,
                    one_minus_cosine * y * z + x * sine,
                    one_minus_cosine * z * z + cosine,
                    0.0,
                ],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        *self = Self::multiply(rotation, *self);
    }

    fn frustum(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Self {
        let delta_x = right - left;
        let delta_y = top - bottom;
        let delta_z = far - near;
        Self {
            columns: [
                [2.0 * near / delta_x, 0.0, 0.0, 0.0],
                [0.0, 2.0 * near / delta_y, 0.0, 0.0],
                [
                    (right + left) / delta_x,
                    (top + bottom) / delta_y,
                    -(near + far) / delta_z,
                    -1.0,
                ],
                [0.0, 0.0, -2.0 * near * far / delta_z, 0.0],
            ],
        }
    }

    fn clip_position(self, position: [f32; 3]) -> [f32; 4] {
        let [x, y, z] = position;
        let clip_x = self.columns[0][0] * x
            + self.columns[1][0] * y
            + self.columns[2][0] * z
            + self.columns[3][0];
        let clip_y = self.columns[0][1] * x
            + self.columns[1][1] * y
            + self.columns[2][1] * z
            + self.columns[3][1];
        let clip_z = self.columns[0][2] * x
            + self.columns[1][2] * y
            + self.columns[2][2] * z
            + self.columns[3][2];
        let clip_w = self.columns[0][3] * x
            + self.columns[1][3] * y
            + self.columns[2][3] * z
            + self.columns[3][3];
        [clip_x, clip_y, clip_z, clip_w]
    }
}

fn build_vertices(frame: usize, width: u32, height: u32) -> Vec<u8> {
    let mut model_view = Matrix::identity();
    model_view.translate(0.0, 0.0, -8.0);
    model_view.rotate(45.0 + 0.25 * frame as f32, [1.0, 0.0, 0.0]);
    model_view.rotate(45.0 - 0.5 * frame as f32, [0.0, 1.0, 0.0]);
    model_view.rotate(10.0 + 0.15 * frame as f32, [0.0, 0.0, 1.0]);

    let aspect = height as f32 / width as f32;
    let projection = Matrix::frustum(-2.8, 2.8, -2.8 * aspect, 2.8 * aspect, 6.0, 10.0);
    let model_view_projection = Matrix::multiply(model_view, projection);

    let mut vertices = Vec::with_capacity(VERTEX_COUNT * 28);
    for face in 0..FACE_COUNT {
        let base = face * VERTICES_PER_FACE;
        for index in [base, base + 1, base + 2, base + 2, base + 1, base + 3] {
            for component in model_view_projection
                .clip_position(KMSCUBE_POSITIONS[index])
                .into_iter()
                .chain(KMSCUBE_COLORS[index])
            {
                vertices.extend_from_slice(&component.to_le_bytes());
            }
        }
    }
    vertices
}

fn main() -> ExitCode {
    let display = match DisplaySurface::open_primary() {
        Ok(display) => display,
        Err(error) => {
            println!("sgfx-cube: failed to open primary display: {:?}", error);
            return ExitCode::from(1);
        }
    };
    let display_info = match display.get_info() {
        Ok(info) => info,
        Err(error) => {
            println!("sgfx-cube: failed to query display: {:?}", error);
            return ExitCode::from(1);
        }
    };
    let mut target = match MappedTarget::open(display_info.width, display_info.height) {
        Ok(target) => target,
        Err(error) => {
            println!("sgfx-cube: failed to create mapped target: {:?}", error);
            return ExitCode::from(1);
        }
    };
    let resources = std::rc::Rc::clone(&target.resources);
    let vertex_buffer = match BufferDesc::new(
        (VERTEX_COUNT * 28) as u64,
        BufferUsage::VERTEX | BufferUsage::COPY_DST,
    )
    .and_then(|desc| resources.define_buffer(desc))
    {
        Ok(buffer) => buffer.id(),
        Err(error) => {
            println!("sgfx-cube: failed to define vertex buffer: {:?}", error);
            return ExitCode::from(1);
        }
    };
    let layout = match VertexBufferLayout::new(
        28,
        vec![
            VertexAttribute::new(0, VertexFormat::Float32x4, 0),
            VertexAttribute::new(1, VertexFormat::Float32x3, 16),
        ],
    ) {
        Ok(layout) => layout,
        Err(error) => {
            println!("sgfx-cube: failed to define vertex layout: {:?}", error);
            return ExitCode::from(1);
        }
    };
    let pipeline = match RenderPipelineDesc::new(
        TextureFormat::Bgra8Unorm,
        PrimitiveTopology::TriangleList,
        layout,
        FragmentProgram::VertexColor,
        BlendState::REPLACE,
        RasterState::new(
            sgfx::ir::CullMode::Back,
            sgfx::ir::FrontFace::CounterClockwise,
        ),
    )
    .and_then(|desc| resources.define_render_pipeline(desc))
    {
        Ok(pipeline) => pipeline,
        Err(error) => {
            println!("sgfx-cube: failed to create pipeline: {:?}", error);
            return ExitCode::from(1);
        }
    };
    let clear_color = Color::rgba(0.45, 0.45, 0.45, 1.0).expect("valid clear color");

    println!(
        "sgfx-cube: rendering {}x{} rotating cube",
        target.width, target.height
    );

    let mut frame = 0usize;
    loop {
        let vertices = build_vertices(frame, target.width, target.height);
        let mut encoder = CommandEncoder::new(resources.as_ref());
        if let Err(error) = encoder.write_buffer(
            resources
                .buffer_ref(vertex_buffer)
                .expect("defined vertex buffer"),
            0,
            &vertices,
        ) {
            println!("sgfx-cube: vertex upload failed: {:?}", error);
            return ExitCode::from(1);
        }
        let area = PixelRect::new(0, 0, target.width, target.height).expect("non-empty target");
        let desc = RenderPassDesc::new(
            resources.as_ref(),
            resources
                .texture_ref(target.texture)
                .expect("mapped target"),
            area,
            LoadOp::Clear(clear_color),
            StoreOp::Store,
        )
        .expect("valid cube render pass");
        let mut pass = encoder.begin_render_pass(desc).expect("valid cube pass");
        pass.set_pipeline(pipeline).expect("defined cube pipeline");
        pass.set_vertex_buffer(
            resources
                .buffer_ref(vertex_buffer)
                .expect("defined vertex buffer"),
            0,
        )
        .expect("valid cube vertex binding");
        pass.set_uniforms(DrawUniforms::new(
            Transform::identity(),
            Color::rgba(1.0, 1.0, 1.0, 1.0).expect("valid white"),
        ))
        .expect("valid cube uniforms");
        pass.draw(VERTEX_COUNT as u32, 0).expect("valid cube draw");
        pass.end().expect("valid cube pass end");
        let commands = encoder.finish().expect("valid cube commands");
        if let Err(error) = target.execute(&commands) {
            println!("sgfx-cube: draw failed: {:?}", error);
            return ExitCode::from(1);
        }
        if let Err(error) = target.present(&display, None) {
            println!("sgfx-cube: image present failed: {:?}", error);
            return ExitCode::from(1);
        }
        frame = frame.wrapping_add(1);
        thread::sleep(Duration::from_millis(16));
    }
}
