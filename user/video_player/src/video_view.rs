//! A video plane plus small control buffers. Native frames never traverse a
//! full-window CPU canvas; mapped/software frames retain the existing renderer.
use super::*;
use scarlet_ui::element::UpdateResult;
use scarlet_ui::renderer::PaintContext;
use scarlet_ui::{Buffer, ElementRenderObject, LayoutConstraints, Point, Rect, RenderElement};

pub(super) fn view(
    frames: Arc<VideoFrameStore>,
    controls: Arc<ControlsOverlay>,
    signal: Arc<PaintSignal>,
) -> impl View + Clone {
    let key_controls = controls.clone();
    VideoView {
        frames,
        controls,
        signal: signal.clone(),
    }
    .on_key(move |event| handle_key_event(event, &key_controls, &signal))
}

#[derive(Clone)]
struct VideoView {
    frames: Arc<VideoFrameStore>,
    controls: Arc<ControlsOverlay>,
    signal: Arc<PaintSignal>,
}
impl View for VideoView {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            VideoRender {
                view: self.clone(),
                size: Size::new(DISPLAY_WIDTH as f32, DISPLAY_HEIGHT as f32),
                fallback: None,
                panel: None,
                debug: None,
                image: None,
                source: None,
            },
        ))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
struct VideoRender {
    view: VideoView,
    size: Size,
    fallback: Option<Buffer>,
    panel: Option<Buffer>,
    debug: Option<Buffer>,
    image: Option<Arc<scarlet_ui::SgfxTexture>>,
    source: Option<Arc<SharedVideoFrame>>,
}
fn buffer(slot: &mut Option<Buffer>, width: u32, height: u32) -> &mut Buffer {
    if slot.as_ref().is_none_or(|b| {
        b.logical_width() != width
            || b.logical_height() != height
            || b.scale_milli() != graphics::current_scale_milli()
    }) {
        *slot = Some(Buffer::from_logical_dimensions(width, height));
    }
    slot.as_mut().unwrap()
}
impl ElementRenderObject for VideoRender {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let width = if constraints.max_width.is_finite() {
            constraints.max_width
        } else {
            DISPLAY_WIDTH as f32
        };
        let height = if constraints.max_height.is_finite() {
            constraints.max_height
        } else {
            DISPLAY_HEIGHT as f32
        };
        self.size = Size::new(
            width.max(constraints.min_width),
            height.max(constraints.min_height),
        );
        self.size
    }
    fn size(&self) -> Size {
        self.size
    }
    fn handle_event(&mut self, event: &Event, phase: scarlet_ui::event::Phase) -> bool {
        if phase != scarlet_ui::event::Phase::Target {
            return false;
        }
        // Preserve local press/release coordinates, including touch taps that
        // have no preceding mouse move. The UI dispatcher owns pointer capture.
        let was_visible = self.view.controls.is_visible();
        let handled = handle_canvas_event(event, &self.view.controls, &self.view.signal);
        if handled || was_visible != self.view.controls.is_visible() {
            self.view.signal.notify();
            return true;
        }
        false
    }
    fn render(&mut self) {
        let width = (self.size.width as u32).max(1);
        let height = (self.size.height as u32).max(1);
        self.view.controls.update_canvas_size(width, height);
        let source = self.view.frames.data.lock().image.clone();
        let unchanged = match (&self.source, &source) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !unchanged {
            self.image = source.as_ref().and_then(|frame| {
                let handle = match frame.handle.duplicate() {
                    Ok(handle) => handle,
                    Err(error) => {
                        println!(
                            "[{}] shared image handle duplication failed: {:?}",
                            APP_NAME, error
                        );
                        return None;
                    }
                };
                match scarlet_ui::shared_nv12_texture(
                    handle,
                    frame.width,
                    frame.height,
                    frame.conversion,
                ) {
                    Ok(image) => Some(image),
                    Err(error) => {
                        println!("[{}] shared image adoption failed: {:?}", APP_NAME, error);
                        None
                    }
                }
            });
            self.source = source;
        }
        if self.image.is_none() {
            let target = buffer(&mut self.fallback, width, height);
            let (w, h) = (target.width(), target.height());
            draw_video_frame(
                target.data_mut(),
                w,
                h,
                &self.view.frames,
                &self.view.controls,
            );
            self.panel = None;
            self.debug = None;
            return;
        }
        self.fallback = None;
        let data = self.view.frames.data.lock();
        if self.view.controls.is_visible() {
            let panel_height = height.min(CONTROLS_MIN_HEIGHT.max(CONTROLS_PANEL_HEIGHT));
            let target = buffer(&mut self.panel, width, panel_height);
            let (w, h) = (target.width(), target.height());
            let pixels = target.data_mut();
            pixels.fill(0);
            draw_seek_bar(
                pixels,
                w,
                h,
                width,
                panel_height,
                &data,
                &self.view.controls,
                UiScale::current(),
            );
        } else {
            self.panel = None;
        }
        if self.view.controls.is_debug_visible() {
            let (dw, dh) = (width.min(360), height.min(86));
            let target = buffer(&mut self.debug, dw, dh);
            let (w, h) = (target.width(), target.height());
            let pixels = target.data_mut();
            pixels.fill(0);
            draw_debug_overlay(
                pixels,
                w,
                h,
                dw,
                dh,
                &data,
                &self.view.controls,
                UiScale::current(),
            );
        } else {
            self.debug = None;
        }
    }
    fn requires_buffer_render_for_paint(&self) -> bool {
        true
    }
    fn emits_paint_extension(&self) -> bool {
        true
    }
    fn paint<'a>(&'a self, ctx: &mut PaintContext<'a>, origin: Point) -> bool {
        let rect = Rect::new(origin, self.size);
        if let Some(image) = &self.image {
            ctx.fill_rect(rect, Color::BLACK);
            let (w, h) = fit_size(
                image.width(),
                image.height(),
                self.size.width as u32,
                self.size.height as u32,
            );
            let at = Point::new(
                origin.x + (self.size.width - w as f32) * 0.5,
                origin.y + (self.size.height - h as f32) * 0.5,
            );
            image.paint(ctx, Rect::new(at, Size::new(w as f32, h as f32)));
            if let Some(panel) = &self.panel {
                ctx.draw_buffer_ref(
                    Rect::new(
                        Point::new(
                            origin.x,
                            origin.y + self.size.height - panel.logical_height() as f32,
                        ),
                        Size::new(panel.logical_width() as f32, panel.logical_height() as f32),
                    ),
                    panel,
                );
            }
            if let Some(debug) = &self.debug {
                ctx.draw_buffer_ref(
                    Rect::new(
                        origin,
                        Size::new(debug.logical_width() as f32, debug.logical_height() as f32),
                    ),
                    debug,
                );
            }
        } else if let Some(fallback) = &self.fallback {
            ctx.draw_buffer_ref(rect, fallback);
        } else {
            ctx.fill_rect(rect, Color::BLACK);
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
        let Some(view) = new_view.as_any().downcast_ref::<VideoView>() else {
            return UpdateResult::Replaced;
        };
        self.view = view.clone();
        UpdateResult::Updated
    }
}
