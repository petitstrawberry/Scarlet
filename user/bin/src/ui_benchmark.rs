//! Non-interactive ScarletUI renderer benchmark.
//!
//! The benchmark drives the normal ScarletUI application and SWS presentation
//! path. Renderer selection is owned by `SCARLET_UI_BACKEND`; this program only
//! chooses a workload and reports the backend that the platform actually used.
//!
//! Workload controls:
//! - `SCARLET_UI_BENCH_SCENE=paint|text|image|damage|retained|mixed`
//! - `SCARLET_UI_BENCH_DIRTY_MODE=full|partial`
//! - `SCARLET_UI_BENCH_WARMUP_FRAMES=<count>`
//! - `SCARLET_UI_BENCH_FRAMES=<count>`
//! - `SCARLET_UI_BENCH_COMMANDS=<workload item count>`

#![no_main]
#![no_std]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;

use scarlet_os::time::monotonic_time_ns;
use scarlet_ui::buffer::Buffer;
use scarlet_ui::element::{
    Element, ElementRenderObject, LayoutConstraints, RenderElement, UpdateResult,
};
use scarlet_ui::geometry::{Point, Rect, Size};
use scarlet_ui::renderer::{CompositorBackendKind, PaintContext, RendererBackendKind};
use scarlet_ui::prelude::*;
use scarlet_ui::{PlatformWindow, zstack};
use std::println;

const WINDOW_WIDTH: f32 = 1024.0;
const WINDOW_HEIGHT: f32 = 720.0;
const PARTIAL_WIDTH: f32 = 256.0;
const PARTIAL_HEIGHT: f32 = 192.0;
const DEFAULT_WARMUP_FRAMES: u64 = 120;
const DEFAULT_MEASURE_FRAMES: u64 = 600;
const DEFAULT_WORKLOAD_ITEMS: usize = 512;
const MAX_FRAMES: u64 = 100_000;
const MAX_WORKLOAD_ITEMS: usize = 1_024;
const WINDOW_KEY: &str = "main";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BenchmarkScene {
    Paint,
    Text,
    Image,
    Damage,
    Retained,
    Mixed,
}

impl BenchmarkScene {
    fn from_env() -> Self {
        match std::env::var("SCARLET_UI_BENCH_SCENE").as_deref() {
            Some("paint") => Self::Paint,
            Some("text") => Self::Text,
            Some("image") => Self::Image,
            Some("damage") => Self::Damage,
            Some("retained") => Self::Retained,
            Some("mixed") | None => Self::Mixed,
            Some(_) => Self::Mixed,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Paint => "paint",
            Self::Text => "text",
            Self::Image => "image",
            Self::Damage => "damage",
            Self::Retained => "retained",
            Self::Mixed => "mixed",
        }
    }

    const fn default_dirty_mode(self) -> DirtyMode {
        match self {
            Self::Damage | Self::Retained => DirtyMode::Partial,
            Self::Paint | Self::Text | Self::Image | Self::Mixed => DirtyMode::Full,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirtyMode {
    Full,
    Partial,
}

impl DirtyMode {
    fn from_env(scene: BenchmarkScene) -> Self {
        match std::env::var("SCARLET_UI_BENCH_DIRTY_MODE").as_deref() {
            Some("full") => Self::Full,
            Some("partial") => Self::Partial,
            Some(_) | None => scene.default_dirty_mode(),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Partial => "partial",
        }
    }

    const fn workload_size(self) -> Size {
        match self {
            Self::Full => Size::new(WINDOW_WIDTH, WINDOW_HEIGHT),
            Self::Partial => Size::new(PARTIAL_WIDTH, PARTIAL_HEIGHT),
        }
    }
}

#[derive(Clone, Copy)]
struct BenchmarkConfig {
    scene: BenchmarkScene,
    dirty_mode: DirtyMode,
    warmup_frames: u64,
    measure_frames: u64,
    workload_items: usize,
}

impl BenchmarkConfig {
    fn from_env() -> Self {
        let scene = BenchmarkScene::from_env();
        Self {
            scene,
            dirty_mode: DirtyMode::from_env(scene),
            warmup_frames: env_u64(
                "SCARLET_UI_BENCH_WARMUP_FRAMES",
                DEFAULT_WARMUP_FRAMES,
                0,
                MAX_FRAMES,
            ),
            measure_frames: env_u64(
                "SCARLET_UI_BENCH_FRAMES",
                DEFAULT_MEASURE_FRAMES,
                1,
                MAX_FRAMES,
            ),
            workload_items: env_usize(
                "SCARLET_UI_BENCH_COMMANDS",
                DEFAULT_WORKLOAD_ITEMS,
                1,
                MAX_WORKLOAD_ITEMS,
            ),
        }
    }
}

fn env_u64(name: &str, default: u64, min: u64, max: u64) -> u64 {
    std::env::var(name)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

fn env_usize(name: &str, default: usize, min: usize, max: usize) -> usize {
    std::env::var(name)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

fn requested_renderer_backend() -> &'static str {
    match std::env::var("SCARLET_UI_BACKEND").as_deref() {
        Some("cpu") => "cpu",
        Some("sgfx") => "sgfx",
        Some("auto") | None => "auto",
        Some(_) => "invalid",
    }
}

const fn renderer_backend_name(backend: Option<RendererBackendKind>) -> &'static str {
    match backend {
        Some(RendererBackendKind::Cpu) => "cpu",
        Some(RendererBackendKind::Sgfx) => "sgfx",
        None => "unknown",
    }
}

const fn compositor_backend_name(backend: CompositorBackendKind) -> &'static str {
    match backend {
        CompositorBackendKind::Cpu => "cpu",
        CompositorBackendKind::Sgfx => "sgfx",
        CompositorBackendKind::Unknown => "unknown",
    }
}

#[derive(Clone)]
struct UiBenchmarkApp {
    config: BenchmarkConfig,
    frame: State<u64>,
    scheduled_frames: u64,
    measure_started_ns: Option<u64>,
    backend: Option<RendererBackendKind>,
    compositor_backend: CompositorBackendKind,
    scale_milli: u32,
    finished: bool,
}

impl UiBenchmarkApp {
    fn new(config: BenchmarkConfig) -> Self {
        Self {
            config,
            frame: State::new(StateId::new(0), 0),
            scheduled_frames: 0,
            measure_started_ns: None,
            backend: None,
            compositor_backend: CompositorBackendKind::Unknown,
            scale_milli: 1_000,
            finished: false,
        }
    }

    fn content(&self) -> impl View + Clone {
        let background = BenchmarkSurface::background(self.config).repaint_boundary();
        let workload = BenchmarkSurface::workload(self.frame.clone(), self.config);
        zstack! {
            background,
            workload,
        }
    }

    fn finish(&mut self, end_ns: u64) {
        let start_ns = self.measure_started_ns.unwrap_or(end_ns);
        let total_ns = end_ns.saturating_sub(start_ns);
        let frames = self.config.measure_frames;
        let avg_ns = total_ns / frames.max(1);
        let fps_milli = if total_ns == 0 {
            0
        } else {
            frames
                .saturating_mul(1_000_000_000)
                .saturating_mul(1_000)
                / total_ns
        };

        println!(
            "UI_BENCH_RESULT {{\"version\":1,\"metric\":\"frame_submit\",\"requested_backend\":\"{}\",\"backend\":\"{}\",\"sws_backend\":\"{}\",\"scene\":\"{}\",\"dirty_mode\":\"{}\",\"scale_milli\":{},\"warmup_frames\":{},\"frames\":{},\"workload_items\":{},\"total_ns\":{},\"avg_ns\":{},\"fps\":{}.{:03}}}",
            requested_renderer_backend(),
            renderer_backend_name(self.backend),
            compositor_backend_name(self.compositor_backend),
            self.config.scene.name(),
            self.config.dirty_mode.name(),
            self.scale_milli,
            self.config.warmup_frames,
            frames,
            self.config.workload_items,
            total_ns,
            avg_ns,
            fps_milli / 1_000,
            fps_milli % 1_000,
        );
        self.finished = true;
        dismiss_window(WINDOW_KEY);
    }
}

impl View for UiBenchmarkApp {
    fn create_element(&self) -> Box<dyn Element> {
        self.content().create_element()
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        Vec::new()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Application for UiBenchmarkApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new(
            WINDOW_KEY,
            Window::new("ScarletUI Renderer Benchmark", self.content())
                .app_id("org.scarlet-os.scarlet-ui-benchmark")
                .size(Size::new(WINDOW_WIDTH, WINDOW_HEIGHT))
                .resizable(false)
                .movable(false)
                .decorated(false)
                .focus_on_create(false)
                .active_on_focus(false)
                .background_color(Color::rgb(12u8, 16u8, 24u8)),
        )
    }

    fn on_window_created(&mut self, _ctx: &WindowContext, window: &mut dyn PlatformWindow) {
        self.backend = Some(window.renderer_backend());
        self.compositor_backend = window.compositor_backend();
        self.scale_milli = window.output_scale_milli().max(1);
    }

    fn on_window_sync(&mut self, _ctx: &WindowContext, window: &mut dyn PlatformWindow) {
        self.backend = Some(window.renderer_backend());
        self.compositor_backend = window.compositor_backend();
        self.scale_milli = window.output_scale_milli().max(1);
    }

    fn on_idle(&mut self) {
        if self.finished {
            return;
        }

        let total_frames = self
            .config
            .warmup_frames
            .saturating_add(self.config.measure_frames);
        if self.scheduled_frames >= total_frames {
            self.finish(monotonic_time_ns());
            return;
        }

        if self.scheduled_frames == self.config.warmup_frames {
            self.measure_started_ns = Some(monotonic_time_ns());
        }

        self.scheduled_frames = self.scheduled_frames.saturating_add(1);
        self.frame.set(self.scheduled_frames);
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SurfaceRole {
    Background,
    Workload,
}

#[derive(Clone)]
struct BenchmarkSurface {
    frame: Option<State<u64>>,
    config: BenchmarkConfig,
    role: SurfaceRole,
    size: Size,
}

impl BenchmarkSurface {
    fn background(config: BenchmarkConfig) -> Self {
        Self {
            frame: None,
            config,
            role: SurfaceRole::Background,
            size: Size::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        }
    }

    fn workload(frame: State<u64>, config: BenchmarkConfig) -> Self {
        Self {
            frame: Some(frame),
            config,
            role: SurfaceRole::Workload,
            size: config.dirty_mode.workload_size(),
        }
    }

    fn frame_number(&self) -> u64 {
        self.frame.as_ref().map(State::get).unwrap_or(0)
    }
}

impl View for BenchmarkSurface {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            BenchmarkSurfaceRenderObject::new(self),
        ))
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        self.frame
            .as_ref()
            .map(|frame| vec![frame as &dyn Listenable])
            .unwrap_or_default()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct BenchmarkSurfaceRenderObject {
    config: BenchmarkConfig,
    role: SurfaceRole,
    frame: u64,
    size: Size,
    image: Buffer,
}

impl BenchmarkSurfaceRenderObject {
    fn new(view: &BenchmarkSurface) -> Self {
        Self {
            config: view.config,
            role: view.role,
            frame: view.frame_number(),
            size: view.size,
            image: benchmark_image(),
        }
    }

    fn paint_background<'a>(&'a self, ctx: &mut PaintContext<'a>, origin: Point) {
        let rect = Rect::new(origin, self.size);
        ctx.fill_rect(rect, Color::rgb(12u8, 16u8, 24u8));
        match self.config.scene {
            BenchmarkScene::Retained => self.emit_mixed(ctx, origin, self.config.workload_items, 0),
            BenchmarkScene::Damage => {
                self.emit_shapes(ctx, origin, self.config.workload_items.min(128), 0)
            }
            BenchmarkScene::Mixed => {
                self.emit_mixed(ctx, origin, self.config.workload_items / 2, 0)
            }
            BenchmarkScene::Paint | BenchmarkScene::Text | BenchmarkScene::Image => {}
        }
    }

    fn paint_workload<'a>(&'a self, ctx: &mut PaintContext<'a>, origin: Point) {
        match self.config.scene {
            BenchmarkScene::Paint => {
                self.emit_shapes(ctx, origin, self.config.workload_items, self.frame)
            }
            BenchmarkScene::Text => {
                self.emit_text(ctx, origin, self.config.workload_items, self.frame)
            }
            BenchmarkScene::Image => {
                self.emit_images(ctx, origin, self.config.workload_items, self.frame)
            }
            BenchmarkScene::Damage => {
                self.emit_mixed(ctx, origin, self.config.workload_items, self.frame)
            }
            BenchmarkScene::Retained => {
                self.emit_shapes(ctx, origin, self.config.workload_items.min(32), self.frame)
            }
            BenchmarkScene::Mixed => {
                self.emit_mixed(ctx, origin, self.config.workload_items, self.frame)
            }
        }
    }

    fn emit_shapes<'a>(
        &'a self,
        ctx: &mut PaintContext<'a>,
        origin: Point,
        count: usize,
        frame: u64,
    ) {
        ctx.push_rounded_clip(Rect::new(origin, self.size), 16.0);
        for index in 0..count.max(1) {
            let (rect, color) = command_rect(self.size, origin, index, frame);
            match index % 6 {
                0 => ctx.fill_rect(rect, color),
                1 => ctx.fill_triangle(
                    rect.origin,
                    Point::new(rect.origin.x + rect.size.width, rect.origin.y),
                    Point::new(
                        rect.origin.x + rect.size.width * 0.5,
                        rect.origin.y + rect.size.height,
                    ),
                    color,
                ),
                2 => ctx.stroke_rect(rect, 1.0 + (index % 4) as f32, color),
                3 => ctx.stroke_rounded_rect(rect, 6.0, 2.0, color),
                4 => ctx.draw_line(
                    rect.origin,
                    Point::new(
                        rect.origin.x + rect.size.width,
                        rect.origin.y + rect.size.height,
                    ),
                    2.0,
                    color,
                ),
                _ => {
                    ctx.set_opacity(1.0);
                    ctx.fill_rounded_rect(rect, 8.0, color);
                }
            }
        }
        ctx.pop_clip();
    }

    fn emit_text<'a>(
        &'a self,
        ctx: &mut PaintContext<'a>,
        origin: Point,
        count: usize,
        frame: u64,
    ) {
        const TEXT: [&str; 4] = [
            "ScarletUI benchmark",
            "CPU / SGFX renderer",
            "日本語テキスト描画",
            "Retained damage 0123456789",
        ];
        ctx.push_clip(Rect::new(origin, self.size));
        for index in 0..count.max(1) {
            let (rect, color) = command_rect(self.size, origin, index, frame);
            ctx.draw_text(
                rect.origin,
                TEXT[index % TEXT.len()],
                color,
                12.0 + (index % 4) as f32 * 2.0,
            );
        }
        ctx.pop_clip();
    }

    fn emit_images<'a>(
        &'a self,
        ctx: &mut PaintContext<'a>,
        origin: Point,
        count: usize,
        frame: u64,
    ) {
        let src = Rect::from_xywh(0.0, 0.0, 64.0, 64.0);
        ctx.push_rounded_clip(Rect::new(origin, self.size), 16.0);
        for index in 0..count.max(1) {
            let (rect, _) = command_rect(self.size, origin, index, frame);
            if index % 2 == 0 {
                ctx.draw_buffer_ref(rect, &self.image);
            } else {
                ctx.draw_buffer_rect_ref(rect, src, &self.image, 0.65 + (index % 4) as f32 * 0.1);
            }
        }
        ctx.pop_clip();
    }

    fn emit_mixed<'a>(
        &'a self,
        ctx: &mut PaintContext<'a>,
        origin: Point,
        count: usize,
        frame: u64,
    ) {
        let third = (count.max(3) / 3).max(1);
        self.emit_shapes(ctx, origin, third, frame);
        self.emit_text(ctx, origin, third, frame);
        self.emit_images(ctx, origin, count.saturating_sub(third * 2).max(1), frame);
    }
}

impl ElementRenderObject for BenchmarkSurfaceRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.size = constraints.constrain(self.size);
        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn render(&mut self) {}

    fn paint<'a>(&'a self, ctx: &mut PaintContext<'a>, origin: Point) -> bool {
        match self.role {
            SurfaceRole::Background => self.paint_background(ctx, origin),
            SurfaceRole::Workload => self.paint_workload(ctx, origin),
        }
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn update(&mut self, new_view: &dyn View) -> UpdateResult {
        let Some(view) = new_view.as_any().downcast_ref::<BenchmarkSurface>() else {
            return UpdateResult::Replaced;
        };
        self.frame = view.frame_number();
        UpdateResult::Updated
    }
}

fn command_rect(size: Size, origin: Point, index: usize, frame: u64) -> (Rect, Color) {
    let width = size.width.max(32.0) as u64;
    let height = size.height.max(32.0) as u64;
    let seed = (index as u64)
        .wrapping_mul(1_103_515_245)
        .wrapping_add(frame.wrapping_mul(12_345));
    let rect_width = 12 + seed % 44;
    let rect_height = 10 + (seed >> 7) % 38;
    let x_range = width.saturating_sub(rect_width).max(1);
    let y_range = height.saturating_sub(rect_height).max(1);
    let x = origin.x + (seed % x_range) as f32;
    let y = origin.y + ((seed >> 11) % y_range) as f32;
    let color = Color::rgba(
        (48 + seed % 192) as u8,
        (40 + (seed >> 8) % 200) as u8,
        (56 + (seed >> 16) % 184) as u8,
        (160 + (seed >> 24) % 96) as u8,
    );
    (
        Rect::from_xywh(x, y, rect_width as f32, rect_height as f32),
        color,
    )
}

fn benchmark_image() -> Buffer {
    const SIZE: u32 = 64;
    let mut buffer = Buffer::from_dimensions(SIZE, SIZE);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let checker = ((x / 8) + (y / 8)) % 2;
            let color = if checker == 0 {
                Color::rgba((x * 4) as u8, (y * 4) as u8, 220u8, 255u8)
            } else {
                Color::rgba(240u8, (x * 3) as u8, (y * 3) as u8, 192u8)
            };
            buffer.as_mut_slice()[(y * SIZE + x) as usize] = color.to_bgra();
        }
    }
    buffer
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    let config = BenchmarkConfig::from_env();
    println!(
        "UI_BENCH_CONFIG {{\"version\":1,\"scene\":\"{}\",\"dirty_mode\":\"{}\",\"warmup_frames\":{},\"frames\":{},\"workload_items\":{}}}",
        config.scene.name(),
        config.dirty_mode.name(),
        config.warmup_frames,
        config.measure_frames,
        config.workload_items,
    );

    let mut app = UiBenchmarkApp::new(config);
    match app.run() {
        Ok(()) if !app.finished => {
            println!(
                "UI_BENCH_ERROR {{\"version\":1,\"stage\":\"run\",\"code\":\"terminated_early\"}}"
            );
        }
        Ok(()) => {}
        Err(error) => {
            println!(
                "UI_BENCH_ERROR {{\"version\":1,\"stage\":\"run\",\"code\":\"application_error\"}}"
            );
            println!("[ui-benchmark] Application error: {}", error);
        }
    }
}
