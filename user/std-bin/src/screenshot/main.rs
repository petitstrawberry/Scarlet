//! SWS one-shot screenshot capture, region selection and arrow annotation.
#[cfg(target_os = "scarlet")]
mod capture;
mod model;
mod worker;
#[cfg(any(target_os = "scarlet", test))]
mod transport;
use model::{Pixels, Region};
use scarlet_ui::graphics::Canvas;
use scarlet_ui::platform::PlatformWindow;
use scarlet_ui::prelude::*;
use scarlet_ui::views::Either;
use scarlet_ui::{CanvasView, Event, MouseButton, MouseEvent, Select, hstack, vstack};
use scarlet_ui_macros::View;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::result::Result;
use std::sync::{Arc, Mutex};

enum ResultMessage {
    Capture(Result<Pixels, String>),
    Save(Result<String, String>),
}
#[derive(Default)]
struct Runtime {
    result: Option<ResultMessage>,
    minimize: bool,
    restore: bool,
    viewport: (u32, u32),
}

fn finish_worker_start(
    runtime: &Arc<Mutex<Runtime>>,
    started: std::io::Result<std::thread::JoinHandle<()>>,
    failed: impl FnOnce(String) -> ResultMessage,
) {
    if let Err(error) = started {
        runtime.lock().unwrap().result = Some(failed(format!("Cannot start worker: {error}")));
    }
}
#[derive(View, Clone)]
struct ScreenshotApp {
    pixels: State<Option<Arc<Pixels>>>,
    fixture: State<bool>,
    original: State<Option<Arc<Pixels>>>,
    selection: State<Option<Region>>,
    drag: State<Option<(u32, u32)>>,
    tool: State<usize>,
    canvas_focus: State<bool>,
    path: State<String>,
    status: State<String>,
    busy: State<bool>,
    dirty: State<bool>,
    confirm: State<Option<String>>,
    guard: State<usize>,
    runtime: State<Arc<Mutex<Runtime>>>,
}
impl ScreenshotApp {
    fn new() -> Self {
        let app = Self::default();
        app.canvas_focus.set(true);
        let base = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/root"));
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        app.path.set(
            base.join(format!("Screenshot-{seconds}.png"))
                .to_string_lossy()
                .into_owned(),
        );
        app.status.set("Ctrl+N: capture and hide this window briefly. Drag to select or draw. Ctrl+S: save PNG.".into());
        app
    }
    fn request_capture(&self) {
        if self.busy.get() {
            return;
        }
        if self.dirty.get() {
            self.guard.set(1);
            return;
        }
        self.start_capture();
    }
    fn start_capture(&self) {
        if self.busy.get() {
            return;
        }
        self.guard.set(0);
        self.busy.set(true);
        self.confirm.set(None);
        self.status.set("Capturing the screen…".into());
        self.runtime.get().lock().unwrap().minimize = true;
    }
    fn discard_guard(&self) {
        if self.busy.get() {
            return;
        }
        let action = self.guard.get();
        self.guard.set(0);
        self.dirty.set(false);
        if action == 1 {
            self.start_capture();
        } else if action == 2 {
            scarlet_ui::dismiss_window("main");
        }
    }
    fn install(&self, image: Pixels) {
        let image = Arc::new(image);
        self.selection.set(Some(image.full()));
        self.original.set(Some(image.clone()));
        self.pixels.set(Some(image));
        self.drag.set(None);
        self.dirty.set(true);
    }
    fn save(&self, replace: bool, path: String) {
        if self.busy.get() {
            return;
        }
        let Some(pixels) = self.pixels.get() else {
            return;
        };
        if path.trim().is_empty() {
            self.status.set("Choose a PNG destination path".into());
            return;
        }
        if !replace && model::destination_exists(Path::new(&path)) {
            self.confirm.set(Some(path));
            return;
        }
        let selected = self.selection.get().unwrap_or_else(|| pixels.full());
        self.busy.set(true);
        self.confirm.set(None);
        self.status.set("Saving PNG…".into());
        let runtime = self.runtime.get();
        let completed = runtime.clone();
        let started = worker::spawn("screenshot-save", move || {
            let result = pixels
                .crop(selected)
                .and_then(|image| model::save_png(&image, Path::new(&path), replace))
                .map(|()| format!("Saved {path}"));
            completed.lock().unwrap().result = Some(ResultMessage::Save(result));
        });
        finish_worker_start(&runtime, started, |error| ResultMessage::Save(Err(error)));
    }
    fn point(&self, x: i32, y: i32) -> Option<(u32, u32)> {
        let image = self.pixels.get()?;
        let (w, h) = self.runtime.get().lock().unwrap().viewport;
        let (scale, ox, oy) = fit(image.width, image.height, w, h)?;
        Some((
            ((x as f64 - ox) / scale)
                .floor()
                .clamp(0.0, (image.width - 1) as f64) as u32,
            ((y as f64 - oy) / scale)
                .floor()
                .clamp(0.0, (image.height - 1) as f64) as u32,
        ))
    }
    fn pointer(&self, event: &Event) -> bool {
        if self.busy.get() {
            return false;
        }
        match event {
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                x,
                y,
                ..
            }) => {
                self.drag.set(self.point(*x, *y));
                true
            }
            Event::Mouse(MouseEvent::Moved { x, y }) if self.drag.get().is_some() => {
                if self.tool.get() == 0 {
                    if let (Some(a), Some(b)) = (self.drag.get(), self.point(*x, *y)) {
                        self.selection.set(Some(Region::between(a, b)));
                        self.dirty.set(true);
                    }
                }
                true
            }
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                x,
                y,
                ..
            }) => {
                if let (Some(a), Some(b), Some(image)) =
                    (self.drag.get(), self.point(*x, *y), self.pixels.get())
                {
                    if self.tool.get() == 0 {
                        self.selection.set(Some(Region::between(a, b)));
                    } else {
                        let mut edited = (*image).clone();
                        edited.arrow(a, b);
                        self.pixels.set(Some(Arc::new(edited)));
                    }
                    self.dirty.set(true);
                    self.drag.set(None);
                    return true;
                }
                false
            }
            Event::Mouse(MouseEvent::ButtonCancelled { .. } | MouseEvent::Exited { .. }) => {
                self.drag.set(None);
                false
            }
            _ => false,
        }
    }
    fn shortcut(&self, event: &Event) -> bool {
        let Event::Keyboard(KeyEvent::Pressed { keycode, modifiers }) = event else {
            return false;
        };
        if !modifiers.primary() {
            return false;
        }
        match keycode {
            scarlet_ui::KeyCode::Char('s' | 'S') => self.save(false, self.path.get()),
            scarlet_ui::KeyCode::Char('n' | 'N') => self.request_capture(),
            _ => return false,
        }
        true
    }
    fn content(&self) -> impl View + Clone {
        let palette = ColorPalette::default();
        let secondary = Color::rgb(88, 94, 104);
        let capture = self.clone();
        let keys = self.clone();
        let save = self.clone();
        let full = self.clone();
        let reset = self.clone();
        let paint = self.clone();
        let pointer = self.clone();
        let yes = self.clone();
        let no = self.clone();
        let discard = self.clone();
        let keep = self.clone();
        let enabled = !self.busy.get();
        let has_image = self.pixels.get().is_some();
        let confirmation = if let Some(path) = self.confirm.get() {
            Either::A(vstack! {
                Text::new("Replace the existing PNG?").font_size(16.0),
                Text::new(wrap_feedback(&path)).font_size(14.0),
                hstack! {
                    Button::new("Cancel").on_click(move||no.confirm.set(None)),
                    Button::new("Replace PNG").enabled(enabled).on_click(move||yes.save(true,path.clone())),
                }.spacing(8.0),
            }.spacing(6.0).alignment(Alignment::Leading).padding(10.0).background(Color::rgb(255,245,234)))
        } else {
            Either::B(Spacer::new().frame_height(0.0))
        };
        let guard = if self.guard.get() != 0 {
            Either::A(hstack! {
                Text::new("Keep this screenshot's unsaved changes?").font_size(14.0), Spacer::new(),
                Button::new("Keep editing").on_click(move||keep.guard.set(0)),
                Button::new("Discard").enabled(enabled).on_click(move || discard.discard_guard()),
            }.spacing(8.0).padding(10.0).background(Color::rgb(255,245,234)))
        } else {
            Either::B(Spacer::new().frame_height(0.0))
        };
        let preview = if has_image {
            Either::A(
                CanvasView::new(
                    760.0,
                    440.0,
                    Rc::new(move |bytes, w, h| paint.preview(bytes, w, h)),
                )
                .on_event(move |e| pointer.pointer(e))
                .focusable(self.canvas_focus.clone())
                .frame(f32::INFINITY, f32::INFINITY)
                .border(
                    if self.canvas_focus.get() {
                        palette.primary()
                    } else {
                        Color::rgb(220, 224, 229)
                    },
                    1.0,
                ),
            )
        } else {
            Either::B(vstack! {
                Spacer::new(),
                Text::new("Start with a screen capture").font_size(24.0),
                Text::new(if cfg!(target_os = "scarlet") { "Capture screen hides this window briefly, then opens the image here." } else { "Live capture is available in Scarlet. This host can preview an explicit PNG fixture." }).font_size(14.0).color(secondary),
                Text::new("Select a region, add arrows, and save a PNG.").font_size(14.0).color(secondary),
                Spacer::new(),
            }.spacing(10.0).padding(16.0).frame(f32::INFINITY,f32::INFINITY).background(Color::rgb(244,246,248)))
        };
        let feedback = wrap_feedback(&self.status.get());
        let feedback_height = feedback.lines().count().max(1) as f32 * 19.0;
        let provenance = if self.fixture.get() {
            Either::A(
                Text::new("PNG fixture preview · live capture is not in use")
                    .font_size(14.0)
                    .color(secondary),
            )
        } else {
            Either::B(Spacer::new().frame_height(0.0))
        };
        vstack! {
            vstack! {
                hstack! {
                    Button::new(if enabled { "Capture screen" } else { "Working…" }).enabled(enabled).on_click(move||capture.request_capture()),
                    Select::new(vec!["Select region".into(),"Draw arrow".into()],self.tool.clone()).width(160.0).enabled(enabled&&has_image),
                    Button::new("Select whole image").enabled(enabled&&has_image).on_click(move||{if let Some(p)=full.pixels.get(){full.selection.set(Some(p.full()));full.dirty.set(true);}}),
                    Button::new("Reset edits").enabled(enabled&&has_image).on_click(move||{if let Some(p)=reset.original.get(){reset.selection.set(Some(p.full()));reset.pixels.set(Some(p));reset.dirty.set(true);}}),
                    Spacer::new(),
                    Text::new(self.selection.get().map(|r|format!("{} × {} px",r.width,r.height)).unwrap_or_default()).font_size(14.0).color(secondary),
                }.spacing(8.0),
                provenance,
            }.spacing(6.0).alignment(Alignment::Leading),
            preview,
            vstack! {
                hstack! {
                    Text::new("Save as PNG").font_size(16.0), Spacer::new(),
                    Text::new(self.selection.get().map(|r|format!("Selection at {}, {}",r.x,r.y)).unwrap_or_else(||"No image selected".into())).font_size(14.0).color(secondary),
                },
                hstack! { TextField::new(self.path.clone()).frame(f32::INFINITY,34.0), Button::new("Save PNG").enabled(enabled&&has_image).on_click(move||save.save(false,save.path.get())), }.spacing(8.0),
                confirmation, guard,
                Text::new(feedback).font_size(14.0).color(secondary).frame_height(feedback_height),
            }.spacing(8.0).alignment(Alignment::Leading),
        }.spacing(14.0).padding(20.0).frame(f32::INFINITY,f32::INFINITY).on_event_capture(move |event| keys.shortcut(event))
    }
    fn preview(&self, bytes: &mut [u8], w: u32, h: u32) {
        self.runtime.get().lock().unwrap().viewport = (w, h);
        for pixel in bytes.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[235, 230, 225, 255]);
        }
        let Some(image) = self.pixels.get() else {
            return;
        };
        let Some((scale, ox, oy)) = fit(image.width, image.height, w, h) else {
            return;
        };
        let dw = (image.width as f64 * scale).round() as u32;
        let dh = (image.height as f64 * scale).round() as u32;
        for y in 0..dh.min(h) {
            for x in 0..dw.min(w) {
                let sx = ((x as f64 / scale) as u32).min(image.width - 1);
                let sy = ((y as f64 / scale) as u32).min(image.height - 1);
                let dest = (((oy as u32 + y) as usize * w as usize) + (ox as u32 + x) as usize) * 4;
                let src = (sy as usize * image.width as usize + sx as usize) * 4;
                if dest + 4 <= bytes.len() {
                    bytes[dest..dest + 4].copy_from_slice(&image.bgra[src..src + 4]);
                }
            }
        }
        if let Some(r) = self.selection.get() {
            let mut canvas = Canvas::new(bytes, w, h);
            canvas.draw_rect(
                (ox + r.x as f64 * scale) as i32,
                (oy + r.y as f64 * scale) as i32,
                (r.width as f64 * scale).max(1.0) as u32,
                (r.height as f64 * scale).max(1.0) as u32,
                Color::rgb(30, 105, 215),
            );
        }
    }
}
fn fit(iw: u32, ih: u32, w: u32, h: u32) -> Option<(f64, f64, f64)> {
    if iw == 0 || ih == 0 || w == 0 || h == 0 {
        return None;
    }
    let scale = (w as f64 / iw as f64).min(h as f64 / ih as f64);
    Some((
        scale,
        ((w as f64 - iw as f64 * scale) / 2.0).floor(),
        ((h as f64 - ih as f64 * scale) / 2.0).floor(),
    ))
}
// Preserve complete failure and destination text, including unbroken paths.
fn wrap_feedback(text: &str) -> String {
    let mut output = String::new();
    let mut line = String::new();
    for original in text.split('\n') {
        if !output.is_empty() {
            output.push('\n');
        }
        for word in original.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if !line.is_empty() && scarlet_ui::measure_text_sized(&candidate, 14.0).0 as f32 > 690.0
            {
                output.push_str(&line);
                output.push('\n');
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            for ch in word.chars() {
                let mut candidate = line.clone();
                candidate.push(ch);
                if !line.is_empty()
                    && scarlet_ui::measure_text_sized(&candidate, 14.0).0 as f32 > 690.0
                {
                    output.push_str(&line);
                    output.push('\n');
                    line.clear();
                }
                line.push(ch);
            }
        }
        output.push_str(&line);
        line.clear();
    }
    output
}
impl Application for ScreenshotApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new(
            "main",
            Window::new("Screenshot", self.content())
                .app_id("org.scarlet-os.desktop.screenshot")
                .size(Size::new(860.0, 650.0))
                .min_size(Size::new(760.0, 540.0))
                .resizable(true),
        )
    }
    fn debug_logging(&self) -> bool {
        false
    }
    fn on_window_sync(&mut self, _: &WindowContext, window: &mut dyn PlatformWindow) {
        let shared = self.runtime.get();
        let (start, restore) = {
            let mut r = shared.lock().unwrap();
            (
                std::mem::take(&mut r.minimize),
                std::mem::take(&mut r.restore),
            )
        };
        if start {
            #[cfg(target_os = "scarlet")]
            {
                if let Err(e) = window.minimize() {
                    self.busy.set(false);
                    self.status.set(format!("Cannot hide capture window: {e}"));
                    return;
                }
                let completed = shared.clone();
                let started = worker::spawn("screenshot-capture", move || {
                    std::thread::sleep(std::time::Duration::from_millis(350));
                    let result = capture::capture();
                    completed.lock().unwrap().result = Some(ResultMessage::Capture(result));
                });
                finish_worker_start(&shared, started, |error| ResultMessage::Capture(Err(error)));
            }
            #[cfg(not(target_os = "scarlet"))]
            {
                shared.lock().unwrap().result=Some(ResultMessage::Capture(Err("Screen capture is available in Scarlet. On this host, open a PNG fixture with --preview FILE.".into())));
            }
        }
        if restore {
            let _ = window.restore();
            let _ = window.focus();
        }
    }
    fn on_idle(&mut self) {
        let result = self.runtime.get().lock().unwrap().result.take();
        if let Some(result) = result {
            self.busy.set(false);
            match result {
                ResultMessage::Capture(result) => {
                    self.runtime.get().lock().unwrap().restore = true;
                    match result {
                        Ok(image) => {
                            self.install(image);
                            self.fixture.set(false);
                            self.status.set(
                                "Captured. Select a region or annotate, then save PNG.".into(),
                            );
                        }
                        Err(error) => self.status.set(error),
                    }
                }
                ResultMessage::Save(result) => match result {
                    Ok(status) => {
                        self.dirty.set(false);
                        self.guard.set(0);
                        self.status.set(status);
                    }
                    Err(error) => self.status.set(format!("Save failed: {error}")),
                },
            }
        }
    }
    fn on_window_close_requested(&mut self, _: &WindowContext) -> bool {
        if self.busy.get() {
            self.status
                .set("Wait for the current operation to finish.".into());
            false
        } else if self.dirty.get() {
            self.guard.set(2);
            false
        } else {
            true
        }
    }
}
fn load_preview(path: &str) -> Result<Pixels, String> {
    let input = std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
    let mut decoder = png::Decoder::new(input);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let info = reader.info();
    model::byte_length(info.width, info.height)?;
    let mut bytes = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or("PNG output size unavailable")?
    ];
    let info = reader.next_frame(&mut bytes).map_err(|e| e.to_string())?;
    let channels = info.color_type.samples();
    let mut bgra = Vec::with_capacity(model::byte_length(info.width, info.height)?);
    for pixel in bytes[..info.buffer_size()].chunks_exact(channels) {
        let (r, g, b) = match info.color_type {
            png::ColorType::Rgb | png::ColorType::Rgba => (pixel[0], pixel[1], pixel[2]),
            png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha => {
                (pixel[0], pixel[0], pixel[0])
            }
            _ => return Err("Unsupported PNG colour type".into()),
        };
        bgra.extend_from_slice(&[b, g, r, 255]);
    }
    Pixels::new(info.width, info.height, bgra)
}
fn main() {
    let mut app = ScreenshotApp::new();
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--preview") {
        match args
            .get(2)
            .ok_or("--preview needs a PNG file".into())
            .and_then(|p| load_preview(p))
        {
            Ok(image) => {
                app.install(image);
                app.fixture.set(true);
                app.status
                    .set("Drag to select a region. Choose Draw arrow to annotate. Ctrl+S saves the selected region.".into());
            }
            Err(e) => {
                eprintln!("Screenshot: {e}");
                std::process::exit(1);
            }
        }
    }
    if let Err(e) = app.run() {
        eprintln!("Screenshot: {e}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use scarlet_ui::KeyCode;

    fn spawn_failure() -> std::io::Result<std::thread::JoinHandle<()>> {
        Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "thread limit"))
    }

    #[test]
    fn failed_save_worker_start_releases_busy_and_keeps_unsaved_image() {
        let mut app = ScreenshotApp::new();
        app.install(Pixels::new(2, 1, vec![0; 8]).unwrap());
        let pixels = app.pixels.get().unwrap();
        let selection = app.selection.get();
        app.guard.set(2);
        app.busy.set(true);
        finish_worker_start(&app.runtime.get(), spawn_failure(), |error| {
            ResultMessage::Save(Err(error))
        });
        app.on_idle();
        assert!(!app.busy.get());
        assert!(app.dirty.get());
        assert_eq!(app.guard.get(), 2);
        assert!(Arc::ptr_eq(&pixels, &app.pixels.get().unwrap()));
        assert_eq!(app.selection.get(), selection);
        assert!(app.status.get().contains("Save failed: Cannot start worker"));
    }

    #[test]
    fn save_worker_completes_png_and_only_then_clears_dirty_state() {
        let mut app = ScreenshotApp::new();
        let image = Pixels::new(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255]).unwrap();
        app.install(image.clone());
        let directory = std::env::temp_dir().join(format!("screenshot-worker-test-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("image.png");
        app.save(false, path.to_string_lossy().into_owned());
        assert!(app.busy.get());
        assert!(app.dirty.get());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.busy.get() {
            app.on_idle();
            assert!(std::time::Instant::now() < deadline, "PNG worker timed out");
            std::thread::yield_now();
        }
        assert!(!app.dirty.get());
        assert_eq!(load_preview(path.to_str().unwrap()).unwrap(), image);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn failed_capture_worker_start_restores_hidden_window_and_keeps_previous_image() {
        let mut app = ScreenshotApp::new();
        app.install(Pixels::new(2, 1, vec![0; 8]).unwrap());
        let pixels = app.pixels.get().unwrap();
        app.dirty.set(false);
        app.start_capture();
        app.runtime.get().lock().unwrap().minimize = false;
        finish_worker_start(&app.runtime.get(), spawn_failure(), |error| {
            ResultMessage::Capture(Err(error))
        });
        app.on_idle();
        assert!(!app.busy.get());
        assert!(app.runtime.get().lock().unwrap().restore);
        assert!(Arc::ptr_eq(&pixels, &app.pixels.get().unwrap()));
        assert!(!app.dirty.get());
        assert!(app.status.get().contains("Cannot start worker"));
    }
    #[test]
    fn capture_shortcut_reaches_app_from_focused_path_field() {
        use scarlet_ui::event::KeyModifiers;
        use scarlet_ui::pipeline::RenderingPipeline;
        let app = ScreenshotApp::new();
        let mut pipeline = RenderingPipeline::new();
        pipeline.set_root(
            Window::new("test", app.content())
                .decorated(false)
                .size(Size::new(860.0, 650.0))
                .create_element(),
        );
        pipeline.layout_initial();
        // The dirty guard is synchronous, so this does not access screen or files.
        app.dirty.set(true);
        let event = |keycode, modifiers| Event::Keyboard(KeyEvent::Pressed { keycode, modifiers });
        pipeline.handle_event(&event(KeyCode::Tab, KeyModifiers::empty()));
        pipeline.handle_event(&event(
            KeyCode::Char('n'),
            KeyModifiers {
                control: true,
                ..KeyModifiers::empty()
            },
        ));
        assert_eq!(app.guard.get(), 1);
        assert!(!app.busy.get());
    }
    #[test]
    fn pending_discard_cannot_start_or_close_during_save() {
        let app = ScreenshotApp::new();
        app.guard.set(1);
        app.dirty.set(true);
        app.busy.set(true);
        app.discard_guard();
        app.start_capture();
        assert_eq!(app.guard.get(), 1);
        assert!(app.dirty.get());
        assert!(!app.runtime.get().lock().unwrap().minimize);
    }
    #[test]
    fn fit_and_selection_use_image_coordinates() {
        assert_eq!(fit(100, 50, 300, 300), Some((3.0, 0.0, 75.0)));
        let app = ScreenshotApp::new();
        app.install(Pixels::new(100, 50, vec![0; 20000]).unwrap());
        app.runtime.get().lock().unwrap().viewport = (300, 300);
        assert_eq!(app.point(150, 150), Some((50, 25)));
        assert_eq!(app.point(-100, 1000), Some((0, 49)));
        assert!(fit(0, 1, 1, 1).is_none());
    }
}
