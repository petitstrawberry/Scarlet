//! ScarletUI SGFX canvas showcase.

use std::any::Any;
use std::boxed::Box;
use std::sync::Arc;
use std::time::Instant;
use std::vec::Vec;

use scarlet_ui::element::{
    Element, ElementRenderObject, LayoutConstraints, RenderElement, UpdateResult,
};
use scarlet_ui::geometry::{Point, Size};
use scarlet_ui::renderer::PaintContext;
use scarlet_ui::prelude::*;
use scarlet_ui::{
    SgfxCanvas, SgfxCanvasDraw, SgfxCanvasFrame, SgfxCanvasHandle, SgfxCanvasVertex, SgfxMesh,
    SgfxTexture, hstack, vstack,
};

const WINDOW_WIDTH: f32 = 1024.0;
const WINDOW_HEIGHT: f32 = 720.0;
const HUD_HEIGHT: f32 = 48.0;
const WINDOW_CONTENT_LAYOUT: WindowContentLayout = WindowContentLayout::new(true);
const CONTENT_WIDTH: f32 =
    WINDOW_WIDTH - WINDOW_CONTENT_LAYOUT.decoration_size().width;
const CONTENT_HEIGHT: f32 =
    WINDOW_HEIGHT - WINDOW_CONTENT_LAYOUT.decoration_size().height;
const CANVAS_WIDTH: f32 = CONTENT_WIDTH;
const CANVAS_HEIGHT: f32 = CONTENT_HEIGHT - HUD_HEIGHT;
const STATS_INTERVAL_NS: u64 = 500_000_000;
const PARTICLE_COUNT: usize = 72;
const WINDOW_KEY: &str = "main";

#[derive(Clone, Copy, Debug)]
struct FpsStats {
    fps_milli: u64,
    frame_us: u64,
    draws: usize,
    triangles: usize,
}

impl FpsStats {
    const fn initial(draws: usize, triangles: usize) -> Self {
        Self {
            fps_milli: 0,
            frame_us: 0,
            draws,
            triangles,
        }
    }
}

#[derive(Clone)]
struct FpsHud {
    stats: State<FpsStats>,
}

impl FpsHud {
    fn new(stats: State<FpsStats>) -> Self {
        Self { stats }
    }
}

impl View for FpsHud {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            FpsHudRenderObject {
                stats: self.stats.clone(),
                size: Size::new(440.0, 42.0),
            },
        ))
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        vec![&self.stats]
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct FpsHudRenderObject {
    stats: State<FpsStats>,
    size: Size,
}

impl ElementRenderObject for FpsHudRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.size = constraints.constrain(self.size);
        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn render(&mut self) {}

    fn paint<'a>(&'a self, ctx: &mut PaintContext<'a>, origin: Point) -> bool {
        let stats = self.stats.get();
        let text = format!(
            "FPS {}.{:03}  frame {}.{:03} ms  draws {}  triangles {}",
            stats.fps_milli / 1_000,
            stats.fps_milli % 1_000,
            stats.frame_us / 1_000,
            stats.frame_us % 1_000,
            stats.draws,
            stats.triangles,
        );
        ctx.draw_text(
            Point::new(origin.x, origin.y + 27.0),
            text,
            Color::WHITE,
            17.0,
        );
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn update(&mut self, new_view: &dyn View) -> UpdateResult {
        let Some(hud) = new_view.as_any().downcast_ref::<FpsHud>() else {
            return UpdateResult::Replaced;
        };
        self.stats = hud.stats.clone();
        UpdateResult::Updated
    }
}

#[derive(Clone)]
struct SgfxShowcaseApp {
    canvas: SgfxCanvasHandle,
    frame: State<Arc<SgfxCanvasFrame>>,
    stats: State<FpsStats>,
    cube: Arc<SgfxMesh>,
    cube_texture: Arc<SgfxTexture>,
    gears: [Arc<SgfxMesh>; 3],
    particle: Arc<SgfxMesh>,
    frame_number: u64,
    animation_started_at: Instant,
    stats_started_at: Instant,
    stats_frames: u64,
}

impl SgfxShowcaseApp {
    fn new() -> Self {
        let cube = cube_mesh();
        let cube_texture = cube_texture();
        let gears = [
            gear_mesh(20, 0.22),
            gear_mesh(10, 0.28),
            gear_mesh(12, 0.26),
        ];
        let particle = particle_mesh();
        let initial = showcase_frame(0, 0.0, &cube, &cube_texture, &gears, &particle);
        let draws = initial.draw_count();
        let triangles = showcase_triangle_count();
        let now = Instant::now();
        Self {
            canvas: SgfxCanvasHandle::new(),
            frame: State::new(StateId::new(300), Arc::new(initial)),
            stats: State::new(StateId::new(301), FpsStats::initial(draws, triangles)),
            cube,
            cube_texture,
            gears,
            particle,
            frame_number: 0,
            animation_started_at: now,
            stats_started_at: now,
            stats_frames: 0,
        }
    }

    fn content(&self) -> impl View + Clone {
        vstack! {
            hstack! {
                Text::new("SgfxCanvas: kmscube / Mesa gears / mesh swarm")
                    .font_size(20.0),
                Spacer::new(),
                FpsHud::new(self.stats.clone()),
            }
            .frame(f32::INFINITY, HUD_HEIGHT),
            SgfxCanvas::from_state(
                self.canvas,
                CANVAS_WIDTH,
                CANVAS_HEIGHT,
                self.frame.clone(),
            ),
        }
        .frame(CONTENT_WIDTH, CONTENT_HEIGHT)
    }

    fn update_stats(&mut self, now: Instant, frame: &SgfxCanvasFrame) {
        self.stats_frames = self.stats_frames.saturating_add(1);
        let elapsed = u64::try_from(now.duration_since(self.stats_started_at).as_nanos())
            .unwrap_or(u64::MAX);
        if elapsed < STATS_INTERVAL_NS {
            return;
        }
        let fps_milli = self
            .stats_frames
            .saturating_mul(1_000_000_000)
            .saturating_mul(1_000)
            / elapsed.max(1);
        let frame_us = elapsed / self.stats_frames.max(1) / 1_000;
        self.stats.set(FpsStats {
            fps_milli,
            frame_us,
            draws: frame.draw_count(),
            triangles: showcase_triangle_count(),
        });
        self.stats_started_at = now;
        self.stats_frames = 0;
    }
}

impl View for SgfxShowcaseApp {
    fn create_element(&self) -> Box<dyn Element> {
        self.content().create_element()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Application for SgfxShowcaseApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new(
            WINDOW_KEY,
            Window::new("ScarletUI SGFX Canvas Showcase", self.content())
                .app_id("org.scarlet-os.scarlet-ui-sgfx-showcase")
                .size(Size::new(WINDOW_WIDTH, WINDOW_HEIGHT))
                .resizable(false)
                .background_color(Color::rgb(8u8, 12u8, 22u8)),
        )
    }

    fn on_idle(&mut self) {
        self.frame_number = self.frame_number.wrapping_add(1);
        let now = Instant::now();
        let elapsed_ns = u64::try_from(now.duration_since(self.animation_started_at).as_nanos())
            .unwrap_or(u64::MAX);
        let animation_seconds = elapsed_ns as f32 / 1_000_000_000.0;
        let frame = showcase_frame(
            self.frame_number,
            animation_seconds,
            &self.cube,
            &self.cube_texture,
            &self.gears,
            &self.particle,
        );
        self.update_stats(now, &frame);
        self.frame.set(Arc::new(frame));
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

fn showcase_frame(
    frame_number: u64,
    animation_seconds: f32,
    cube: &Arc<SgfxMesh>,
    cube_texture: &Arc<SgfxTexture>,
    gear_meshes: &[Arc<SgfxMesh>; 3],
    particle: &Arc<SgfxMesh>,
) -> SgfxCanvasFrame {
    let phase = animation_seconds;
    let mut frame = SgfxCanvasFrame::new(frame_number, Color::rgb(6u8, 11u8, 24u8));
    let projection = perspective(
        50.0 * core::f32::consts::PI / 180.0,
        CANVAS_WIDTH / CANVAS_HEIGHT,
        0.5,
        40.0,
    );
    let view_projection = matrix_mul(projection, translation(0.0, 0.0, -7.0));

    // Follow kmscube's three-axis motion and use a real perspective
    // object-to-clip transform so edges foreshorten instead of shearing.
    let cube_model = matrix_mul(
        translation(-2.18, 0.72, 0.0),
        matrix_mul(
            rotation_z(0.17 + phase * 0.31),
            matrix_mul(
                rotation_y(0.79 - phase * 0.74),
                matrix_mul(
                    rotation_x(0.79 + phase * 0.46),
                    scale(1.04, 1.04, 1.04),
                ),
            ),
        ),
    );
    frame = frame.draw(
        SgfxCanvasDraw::new(Arc::clone(cube), matrix_mul(view_projection, cube_model))
            .texture(Arc::clone(cube_texture)),
    );

    // Mesa's classic gears use one positive rotation and two counter-rotating
    // gears at twice the angular speed with fixed phase offsets.
    let gear_instances = [
        (
            1.02,
            0.57,
            0.00,
            0.92,
            phase * 1.35,
            Color::rgb(204u8, 51u8, 0u8),
        ),
        (
            2.25,
            0.27,
            -0.04,
            0.55,
            -phase * 2.70 - 0.157,
            Color::rgb(0u8, 204u8, 51u8),
        ),
        (
            1.38,
            -0.89,
            0.04,
            0.65,
            -phase * 2.70 - 0.436,
            Color::rgb(51u8, 102u8, 255u8),
        ),
    ];
    for (index, (x, y, z, size, angle, tint)) in gear_instances.into_iter().enumerate() {
        let model = matrix_mul(
            translation(x, y, z),
            matrix_mul(
                rotation_x(0.43),
                matrix_mul(
                    rotation_y(-0.18),
                    matrix_mul(rotation_z(angle), scale(size, size, size)),
                ),
            ),
        );
        frame = frame.draw(
            SgfxCanvasDraw::new(
                Arc::clone(&gear_meshes[index]),
                matrix_mul(view_projection, model),
            )
            .tint(tint),
        );
    }

    for index in 0..PARTICLE_COUNT {
        let seed = index as f32 * 0.754_877_7;
        let orbit = phase * (0.72 + (index % 9) as f32 * 0.045) + seed;
        let radius = 0.42 + (index % 13) as f32 * 0.055;
        let x = -1.72 + libm::cosf(orbit * 1.13) * radius;
        let y = -1.47 + libm::sinf(orbit * 0.91) * 0.48;
        let z = -0.15 + (index % 7) as f32 * 0.045;
        let size = 0.052 + (index % 5) as f32 * 0.011;
        let model = matrix_mul(
            translation(x, y, z),
            matrix_mul(rotation_z(-orbit * 1.7), scale(size, size, size)),
        );
        let tint = match index % 3 {
            0 => Color::rgb(85u8, 210u8, 255u8),
            1 => Color::rgb(255u8, 193u8, 82u8),
            _ => Color::rgb(190u8, 118u8, 255u8),
        };
        frame = frame.draw(
            SgfxCanvasDraw::new(Arc::clone(particle), matrix_mul(view_projection, model))
                .tint(tint),
        );
    }
    frame
}

const fn showcase_triangle_count() -> usize {
    cube_mesh_triangle_count()
        + gear_mesh_triangle_count(20)
        + gear_mesh_triangle_count(10)
        + gear_mesh_triangle_count(12)
        + PARTICLE_COUNT
}

const fn cube_mesh_triangle_count() -> usize {
    12
}

const fn gear_mesh_triangle_count(teeth: usize) -> usize {
    teeth * 4 * 8
}

fn cube_texture() -> Arc<SgfxTexture> {
    const CELL_SIZE: u32 = 64;
    const WIDTH: u32 = CELL_SIZE * 3;
    const HEIGHT: u32 = CELL_SIZE * 2;
    const FACE_COLORS: [[u8; 3]; 6] = [
        [222, 58, 76],
        [44, 116, 232],
        [241, 166, 42],
        [44, 181, 118],
        [149, 84, 214],
        [36, 178, 205],
    ];
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let column = x / CELL_SIZE;
            let row = y / CELL_SIZE;
            let face = (row * 3 + column) as usize;
            let local_x = x % CELL_SIZE;
            let local_y = y % CELL_SIZE;
            let border = local_x < 3
                || local_y < 3
                || local_x >= CELL_SIZE - 3
                || local_y >= CELL_SIZE - 3;
            let diagonal = (local_x + local_y + face as u32 * 7) % 24 < 3;
            let center_x = local_x as i32 - CELL_SIZE as i32 / 2;
            let center_y = local_y as i32 - CELL_SIZE as i32 / 2;
            let ring_distance = center_x * center_x + center_y * center_y;
            let marker_ring = (150..=250).contains(&ring_distance);
            let [base_red, base_green, base_blue] = FACE_COLORS[face];
            let checker = ((local_x / 8) + (local_y / 8)) % 2 == 0;
            let (red, green, blue) = if border || marker_ring {
                (244, 247, 255)
            } else if diagonal {
                (
                    base_red.saturating_add(28),
                    base_green.saturating_add(28),
                    base_blue.saturating_add(28),
                )
            } else if checker {
                (base_red, base_green, base_blue)
            } else {
                (
                    (u16::from(base_red) * 3 / 4) as u8,
                    (u16::from(base_green) * 3 / 4) as u8,
                    (u16::from(base_blue) * 3 / 4) as u8,
                )
            };
            pixels.extend_from_slice(&[red, green, blue, 255]);
        }
    }
    SgfxTexture::rgba8(WIDTH, HEIGHT, pixels)
}

fn cube_mesh() -> Arc<SgfxMesh> {
    let mut vertices = Vec::new();
    push_textured_face(
        &mut vertices,
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
        cube_face_uv(0),
    );
    push_textured_face(
        &mut vertices,
        [1.0, -1.0, -1.0],
        [-1.0, -1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [1.0, 1.0, -1.0],
        cube_face_uv(1),
    );
    push_textured_face(
        &mut vertices,
        [1.0, -1.0, 1.0],
        [1.0, -1.0, -1.0],
        [1.0, 1.0, -1.0],
        [1.0, 1.0, 1.0],
        cube_face_uv(2),
    );
    push_textured_face(
        &mut vertices,
        [-1.0, -1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [-1.0, 1.0, 1.0],
        [-1.0, 1.0, -1.0],
        cube_face_uv(3),
    );
    push_textured_face(
        &mut vertices,
        [-1.0, 1.0, 1.0],
        [1.0, 1.0, 1.0],
        [1.0, 1.0, -1.0],
        [-1.0, 1.0, -1.0],
        cube_face_uv(4),
    );
    push_textured_face(
        &mut vertices,
        [-1.0, -1.0, -1.0],
        [1.0, -1.0, -1.0],
        [1.0, -1.0, 1.0],
        [-1.0, -1.0, 1.0],
        cube_face_uv(5),
    );
    SgfxMesh::new(vertices)
}

fn cube_face_uv(face: usize) -> [f32; 4] {
    const CELL_SIZE: f32 = 64.0;
    const WIDTH: f32 = CELL_SIZE * 3.0;
    const HEIGHT: f32 = CELL_SIZE * 2.0;
    const INSET: f32 = 2.0;
    let column = (face % 3) as f32;
    let row = (face / 3) as f32;
    [
        (column * CELL_SIZE + INSET) / WIDTH,
        (row * CELL_SIZE + INSET) / HEIGHT,
        ((column + 1.0) * CELL_SIZE - INSET) / WIDTH,
        ((row + 1.0) * CELL_SIZE - INSET) / HEIGHT,
    ]
}

fn push_textured_face(
    vertices: &mut Vec<SgfxCanvasVertex>,
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    d: [f32; 3],
    uv: [f32; 4],
) {
    let [u0, v0, u1, v1] = uv;
    for (point, tex_coord) in [
        (a, [u0, v1]),
        (b, [u1, v1]),
        (c, [u1, v0]),
        (a, [u0, v1]),
        (c, [u1, v0]),
        (d, [u0, v0]),
    ] {
        vertices.push(
            SgfxCanvasVertex::new([point[0], point[1], point[2], 1.0], [1.0; 4])
                .with_tex_coord(tex_coord),
        );
    }
}

fn gear_mesh(teeth: usize, half_depth: f32) -> Arc<SgfxMesh> {
    let segments = teeth.saturating_mul(4);
    let mut rear_faces = Vec::new();
    let mut outer_walls = Vec::new();
    let mut inner_walls = Vec::new();
    let mut front_faces = Vec::new();
    let _ = rear_faces.try_reserve(segments.saturating_mul(6));
    let _ = outer_walls.try_reserve(segments.saturating_mul(6));
    let _ = inner_walls.try_reserve(segments.saturating_mul(6));
    let _ = front_faces.try_reserve(segments.saturating_mul(6));
    let tau = core::f32::consts::PI * 2.0;
    for segment in 0..segments {
        let a0 = tau * segment as f32 / segments as f32;
        let a1 = tau * (segment + 1) as f32 / segments as f32;
        let inner_radius = 0.34;
        let root_radius = 0.78;
        let tip_radius = 1.0;
        let outer0 = gear_outline_radius(segment, root_radius, tip_radius);
        let outer1 = gear_outline_radius(segment + 1, root_radius, tip_radius);
        let inner0_front = polar_point(inner_radius, a0, half_depth);
        let outer0_front = polar_point(outer0, a0, half_depth);
        let outer1_front = polar_point(outer1, a1, half_depth);
        let inner1_front = polar_point(inner_radius, a1, half_depth);
        let inner0_back = [inner0_front[0], inner0_front[1], -half_depth];
        let outer0_back = [outer0_front[0], outer0_front[1], -half_depth];
        let outer1_back = [outer1_front[0], outer1_front[1], -half_depth];
        let inner1_back = [inner1_front[0], inner1_front[1], -half_depth];

        push_solid_quad(
            &mut rear_faces,
            inner1_back,
            outer1_back,
            outer0_back,
            inner0_back,
            [0.34, 0.39, 0.49, 1.0],
        );
        let outer_color = match segment % 4 {
            1 => [0.78, 0.84, 0.94, 1.0],
            0 | 2 => [0.66, 0.72, 0.84, 1.0],
            _ => [0.56, 0.62, 0.74, 1.0],
        };
        push_solid_quad(
            &mut outer_walls,
            outer0_front,
            outer0_back,
            outer1_back,
            outer1_front,
            outer_color,
        );
        push_solid_quad(
            &mut inner_walls,
            inner1_front,
            inner1_back,
            inner0_back,
            inner0_front,
            [0.25, 0.29, 0.37, 1.0],
        );
        push_solid_quad(
            &mut front_faces,
            inner0_front,
            outer0_front,
            outer1_front,
            inner1_front,
            [1.0, 0.94, 0.78, 1.0],
        );
    }
    let mut vertices = Vec::with_capacity(
        rear_faces.len() + outer_walls.len() + inner_walls.len() + front_faces.len(),
    );
    // There is no depth attachment in the current portable SGFX IR. Emit the
    // convex surface groups back-to-front so the front annulus consistently
    // masks its bore and tooth walls while back-face culling removes hidden faces.
    vertices.extend(rear_faces);
    vertices.extend(outer_walls);
    vertices.extend(inner_walls);
    vertices.extend(front_faces);
    SgfxMesh::new(vertices)
}

fn gear_outline_radius(segment: usize, root_radius: f32, tip_radius: f32) -> f32 {
    match segment % 4 {
        1 | 2 => tip_radius,
        _ => root_radius,
    }
}

fn polar_point(radius: f32, angle: f32, z: f32) -> [f32; 3] {
    [libm::cosf(angle) * radius, libm::sinf(angle) * radius, z]
}

fn push_solid_quad(
    vertices: &mut Vec<SgfxCanvasVertex>,
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    d: [f32; 3],
    color: [f32; 4],
) {
    for point in [a, b, c, a, c, d] {
        vertices.push(SgfxCanvasVertex::new(
            [point[0], point[1], point[2], 1.0],
            color,
        ));
    }
}

fn particle_mesh() -> Arc<SgfxMesh> {
    SgfxMesh::new(vec![
        SgfxCanvasVertex::new([0.0, 1.0, 0.0, 1.0], [1.0, 1.0, 1.0, 0.92]),
        SgfxCanvasVertex::new([-0.72, -1.0, 0.0, 1.0], [1.0, 1.0, 1.0, 0.72]),
        SgfxCanvasVertex::new([0.72, -1.0, 0.0, 1.0], [1.0, 1.0, 1.0, 0.72]),
    ])
}

const fn identity() -> [f32; 16] {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

fn matrix_mul(left: [f32; 16], right: [f32; 16]) -> [f32; 16] {
    let mut result = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            for index in 0..4 {
                result[column * 4 + row] +=
                    left[index * 4 + row] * right[column * 4 + index];
            }
        }
    }
    result
}

fn translation(x: f32, y: f32, z: f32) -> [f32; 16] {
    let mut result = identity();
    result[12] = x;
    result[13] = y;
    result[14] = z;
    result
}

fn scale(x: f32, y: f32, z: f32) -> [f32; 16] {
    [
        x, 0.0, 0.0, 0.0, 0.0, y, 0.0, 0.0, 0.0, 0.0, z, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

fn perspective(vertical_fov: f32, aspect: f32, near: f32, far: f32) -> [f32; 16] {
    let focal_length = 1.0 / libm::tanf(vertical_fov * 0.5);
    let inverse_depth = 1.0 / (near - far);
    [
        focal_length / aspect,
        0.0,
        0.0,
        0.0,
        0.0,
        focal_length,
        0.0,
        0.0,
        0.0,
        0.0,
        (far + near) * inverse_depth,
        -1.0,
        0.0,
        0.0,
        2.0 * far * near * inverse_depth,
        0.0,
    ]
}

fn rotation_x(angle: f32) -> [f32; 16] {
    let cosine = libm::cosf(angle);
    let sine = libm::sinf(angle);
    [
        1.0, 0.0, 0.0, 0.0, 0.0, cosine, sine, 0.0, 0.0, -sine, cosine, 0.0, 0.0, 0.0, 0.0,
        1.0,
    ]
}

fn rotation_y(angle: f32) -> [f32; 16] {
    let cosine = libm::cosf(angle);
    let sine = libm::sinf(angle);
    [
        cosine, 0.0, -sine, 0.0, 0.0, 1.0, 0.0, 0.0, sine, 0.0, cosine, 0.0, 0.0, 0.0, 0.0,
        1.0,
    ]
}

fn rotation_z(angle: f32) -> [f32; 16] {
    let cosine = libm::cosf(angle);
    let sine = libm::sinf(angle);
    [
        cosine, sine, 0.0, 0.0, -sine, cosine, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0,
        1.0,
    ]
}

fn main() {
    println!("[ui-sgfx-showcase] starting");
    let mut app = SgfxShowcaseApp::new();
    match app.run() {
        Ok(()) => println!("[ui-sgfx-showcase] exited"),
        Err(error) => {
            println!("[ui-sgfx-showcase] Application error: {error}");
        }
    }
}
