//! Standalone rotating colored cube sample.

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::vec::Vec;
use core::time::Duration;

use framebuffer::DisplaySurface;
use gpu::{
    Color, CullMode, Device, FrontFace, PipelineDesc, RenderPass, VertexClip4Color3, Viewport,
};
use std::println;

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

fn build_vertices(frame: usize, width: u32, height: u32) -> Vec<VertexClip4Color3> {
    let mut model_view = Matrix::identity();
    model_view.translate(0.0, 0.0, -8.0);
    model_view.rotate(45.0 + 0.25 * frame as f32, [1.0, 0.0, 0.0]);
    model_view.rotate(45.0 - 0.5 * frame as f32, [0.0, 1.0, 0.0]);
    model_view.rotate(10.0 + 0.15 * frame as f32, [0.0, 0.0, 1.0]);

    let aspect = height as f32 / width as f32;
    let projection = Matrix::frustum(-2.8, 2.8, -2.8 * aspect, 2.8 * aspect, 6.0, 10.0);
    let model_view_projection = Matrix::multiply(model_view, projection);

    let mut vertices = Vec::with_capacity(VERTEX_COUNT);
    for face in 0..FACE_COUNT {
        let base = face * VERTICES_PER_FACE;
        for index in [base, base + 1, base + 2, base + 2, base + 1, base + 3] {
            vertices.push(VertexClip4Color3::new(
                model_view_projection.clip_position(KMSCUBE_POSITIONS[index]),
                KMSCUBE_COLORS[index],
            ));
        }
    }
    vertices
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let display = match DisplaySurface::open_primary() {
        Ok(display) => display,
        Err(error) => {
            println!("gpu_cube: failed to open primary display: {:?}", error);
            return 1;
        }
    };
    let display_info = match display.get_info() {
        Ok(info) => info,
        Err(error) => {
            println!("gpu_cube: failed to query display: {:?}", error);
            return 1;
        }
    };
    let device = match Device::open("/dev/gpu0") {
        Ok(device) => device,
        Err(error) => {
            println!("gpu_cube: failed to open GPU: {:?}", error);
            return 1;
        }
    };
    let capabilities = device.capabilities();
    if !capabilities.supports_rendering() || !capabilities.supports_presentation() {
        println!("gpu_cube: rendering or presentation is unsupported");
        return 1;
    }
    let context = match device.create_context() {
        Ok(context) => context,
        Err(error) => {
            println!("gpu_cube: failed to create context: {:?}", error);
            return 1;
        }
    };
    let image = match context.create_image(display_info.width, display_info.height) {
        Ok(image) => image,
        Err(error) => {
            println!("gpu_cube: failed to create render target: {:?}", error);
            return 1;
        }
    };
    let pipeline = match context.create_pipeline(
        &image,
        PipelineDesc::clip_space_vertex_color(VERTEX_COUNT)
            .with_cull_mode(CullMode::Back)
            .with_front_face(FrontFace::Clockwise),
    ) {
        Ok(pipeline) => pipeline,
        Err(error) => {
            println!("gpu_cube: failed to create pipeline: {:?}", error);
            return 1;
        }
    };
    let queue = match context.create_queue() {
        Ok(queue) => queue,
        Err(error) => {
            println!("gpu_cube: failed to create queue: {:?}", error);
            return 1;
        }
    };
    let viewport = Viewport::new(image.width(), image.height());
    let clear_color = Color::rgba(0.45, 0.45, 0.45, 1.0);

    println!(
        "gpu_cube: rendering {}x{} rotating cube",
        image.width(),
        image.height()
    );

    let mut frame = 0usize;
    loop {
        let vertices = build_vertices(frame, image.width(), image.height());
        let mut render_pass = RenderPass::new(&image, viewport, clear_color);
        render_pass.draw_clip_space_vertex_color(&pipeline, &vertices);
        if let Err(error) = queue.submit(&render_pass) {
            println!("gpu_cube: draw failed: {:?}", error);
            return 1;
        }
        if let Err(error) = image.present(&display) {
            println!("gpu_cube: image present failed: {:?}", error);
            return 1;
        }
        frame = frame.wrapping_add(1);
        std::thread::sleep(Duration::from_millis(33));
    }
}
