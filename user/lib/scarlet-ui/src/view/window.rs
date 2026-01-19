//! Window - Root View with decorations
//!
//! Window is a special root View that manages window decorations (titlebar, buttons)
//! and user content. All decorations are built using ScarletUI controls.

extern crate alloc;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::Event;
use crate::graphics::Rect;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::View;
use crate::view::render::RenderObject;
use crate::view::Spacer;
use crate::{Button, Color, HStack, Text};
use sws_client::WindowSizeLimits;

/// Title bar height in pixels
const TITLEBAR_HEIGHT: u32 = 32;

/// Window type for compositor Z-order management
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    Normal,
    AlwaysOnTop,
    Taskbar,
    Desktop,
}

impl WindowKind {
    pub const fn to_protocol_value(self) -> u32 {
        match self {
            WindowKind::Normal => 0,
            WindowKind::AlwaysOnTop => 1,
            WindowKind::Taskbar => 2,
            WindowKind::Desktop => 3,
        }
    }
}

/// Window state for window operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Normal,
    Minimized,
    Maximized,
}

/// Window - Root View with decorations
///
/// Window structure:
/// ```
/// Window
/// ├── titlebar (HStack)
/// │   ├── Text(title)
/// │   ├── Spacer
/// │   ├── Button(minimize)
/// │   ├── Button(maximize)
/// │   └── Button(close)
/// └── content (user RenderObject)
/// ```
pub struct Window {
    // View identification
    id: ViewId,

    // Window properties
    title: String,
    app_id: Option<String>,
    width: u32,
    height: u32,

    // Window configuration
    size_limits: WindowSizeLimits,
    window_type: WindowKind,
    is_main_window: bool,

    // Window state
    state: WindowState,

    // Child views (user-facing)
    titlebar: Option<Box<dyn View>>,
    content: Option<Box<dyn View>>,

    // Child frames
    titlebar_frame: Rect,
    content_frame: Rect,

    // Window lifecycle flags
    close_requested: bool,
}

impl Window {
    pub fn new(title: &str, width: u32, height: u32) -> Self {
        let id = ViewId::new();

        // Initial frames
        let titlebar_frame = Rect::new(0, 0, width, TITLEBAR_HEIGHT);
        let content_frame = Rect::new(0, TITLEBAR_HEIGHT as i32, width, height - TITLEBAR_HEIGHT);

        Self {
            id,
            title: title.to_string(),
            app_id: None,
            width,
            height,
            size_limits: WindowSizeLimits::NONE,
            window_type: WindowKind::Normal,
            is_main_window: false,
            state: WindowState::Normal,
            titlebar: None,
            content: None,
            titlebar_frame,
            content_frame,
            close_requested: false,
        }
        .build_titlebar()
    }

    fn build_titlebar(mut self) -> Self {
        let title = self.title.clone();

        // Create titlebar: HStack { Text(title), Spacer, Button(minimize), Button(maximize), Button(close) }
        let mut titlebar = HStack::new()
            .spacing(8);

        // Title text
        let mut title_text = Text::new(title);
        title_text.set_color(Color::WHITE);
        titlebar = titlebar.child(title_text);

        titlebar = titlebar.child(Spacer::new());

        // Minimize button
        let mut minimize_btn = Button::new("−");
        minimize_btn.set_action(Arc::new(|| {
            // TODO: Trigger minimize
        }));

        // Maximize button
        let mut maximize_btn = Button::new("□");
        maximize_btn.set_action(Arc::new(|| {
            // TODO: Trigger maximize
        }));

        // Close button
        let mut close_btn = Button::new("×");
        close_btn.set_action(Arc::new(|| {
            // TODO: Trigger close
        }));

        titlebar = titlebar
            .child(minimize_btn)
            .child(maximize_btn)
            .child(close_btn);

        self.titlebar = Some(Box::new(titlebar));
        self
    }

    // Builder methods
    pub fn main_window(mut self) -> Self {
        self.is_main_window = true;
        self
    }

    pub fn app_id(mut self, id: &str) -> Self {
        self.app_id = Some(id.to_string());
        self
    }

    pub fn min_size(mut self, w: u32, h: u32) -> Self {
        self.size_limits.min_width = w;
        self.size_limits.min_height = h;
        self
    }

    pub fn max_size(mut self, w: u32, h: u32) -> Self {
        self.size_limits.max_width = w;
        self.size_limits.max_height = h;
        self
    }

    pub fn window_type(mut self, kind: WindowKind) -> Self {
        self.window_type = kind;
        self
    }

    pub fn background(self, _color: Color) -> Self {
        // TODO: Window background color
        self
    }

    pub fn content<V: View + 'static>(mut self, view: V) -> Self {
        self.content = Some(Box::new(view));
        self
    }

    // Getters
    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn title(&self) -> &str { &self.title }
    pub fn get_app_id(&self) -> Option<&str> { self.app_id.as_deref() }
    pub fn get_window_type(&self) -> WindowKind { self.window_type }
    pub fn get_size_limits(&self) -> WindowSizeLimits { self.size_limits }
    pub fn is_main_window(&self) -> bool { self.is_main_window }
    pub fn is_close_requested(&self) -> bool { self.close_requested }
    pub fn get_state(&self) -> WindowState { self.state }

    // Setters
    pub fn set_size(&mut self, w: u32, h: u32) {
        self.width = w;
        self.height = h;
        self.titlebar_frame = Rect::new(0, 0, w, TITLEBAR_HEIGHT);
        self.content_frame = Rect::new(0, TITLEBAR_HEIGHT as i32, w, h - TITLEBAR_HEIGHT);
    }

    pub fn titlebar_height() -> u32 { TITLEBAR_HEIGHT }

    pub fn content_frame(&self) -> Rect { self.content_frame }

    // Registry management
    pub fn build_view_registry(&mut self) {
        // TODO: Implement registry building
    }

    /// Create a builder for Window
    pub fn builder() -> WindowBuilder {
        WindowBuilder::new()
    }
}

/// Builder for Window
pub struct WindowBuilder {
    title: Option<String>,
    app_id: Option<String>,
    width: u32,
    height: u32,
    min_width: u32,
    min_height: u32,
    max_width: u32,
    max_height: u32,
    window_type: WindowKind,
    is_main_window: bool,
}

impl WindowBuilder {
    pub fn new() -> Self {
        Self {
            title: None,
            app_id: None,
            width: 800,
            height: 600,
            min_width: 0,
            min_height: 0,
            max_width: u32::MAX,
            max_height: u32::MAX,
            window_type: WindowKind::Normal,
            is_main_window: false,
        }
    }

    pub fn title(mut self, title: &str) -> Self {
        self.title = Some(title.to_string());
        self
    }

    pub fn app_id(mut self, id: &str) -> Self {
        self.app_id = Some(id.to_string());
        self
    }

    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn min_size(mut self, width: u32, height: u32) -> Self {
        self.min_width = width;
        self.min_height = height;
        self
    }

    pub fn max_size(mut self, width: u32, height: u32) -> Self {
        self.max_width = width;
        self.max_height = height;
        self
    }

    pub fn window_type(mut self, kind: WindowKind) -> Self {
        self.window_type = kind;
        self
    }

    pub fn main_window(mut self) -> Self {
        self.is_main_window = true;
        self
    }

    pub fn build(mut self) -> Window {
        let title = self.title.unwrap_or_else(|| String::from("Window"));

        let mut window = Window::new(&title, self.width, self.height);

        if let Some(app_id) = self.app_id {
            window = window.app_id(&app_id);
        }

        window = window.min_size(self.min_width, self.min_height);
        window = window.max_size(self.max_width, self.max_height);
        window = window.window_type(self.window_type);

        if self.is_main_window {
            window = window.main_window();
        }

        window
    }
}

impl Default for WindowBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::view::render::RenderObject for Window {
    fn id(&self) -> ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, _constraints: LayoutConstraints) -> Size {
        // Update frames
        self.titlebar_frame = Rect::new(0, 0, self.width, TITLEBAR_HEIGHT);
        self.content_frame = Rect::new(0, TITLEBAR_HEIGHT as i32, self.width, self.height - TITLEBAR_HEIGHT);

        // Layout titlebar at the top
        if let Some(ref mut titlebar) = self.titlebar {
            let titlebar_constraints = LayoutConstraints::new(
                self.width,
                self.width,
                TITLEBAR_HEIGHT,
                TITLEBAR_HEIGHT,
            );
            titlebar.layout(ctx, titlebar_constraints);
        }

        // Layout content below titlebar
        if let Some(ref mut content) = self.content {
            let content_constraints = LayoutConstraints::new(
                self.content_frame.width,
                self.content_frame.width,
                self.content_frame.height,
                self.content_frame.height,
            );
            content.layout(ctx, content_constraints);
        }

        Size::new(self.width, self.height)
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        // Draw background
        ctx.canvas.fill_rect(frame.x, frame.y, frame.width, frame.height, Color::rgb(40, 40, 40));

        // Draw titlebar background
        let titlebar_rect = Rect::new(frame.x, frame.y, frame.width, TITLEBAR_HEIGHT);
        ctx.canvas.fill_rect(titlebar_rect.x, titlebar_rect.y, titlebar_rect.width, titlebar_rect.height, Color::rgb(50, 50, 60));

        // Draw titlebar
        if let Some(ref titlebar) = self.titlebar {
            titlebar.draw(ctx, titlebar_rect);
        }

        // Draw content
        if let Some(ref content) = self.content {
            content.draw(ctx, self.content_frame);
        }

        // Draw border
        ctx.canvas.stroke(Rect::new(frame.x, frame.y, frame.width, frame.height), Color::rgb(80, 80, 90));
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        // Forward event to titlebar
        if let Some(ref mut titlebar) = self.titlebar {
            let mut title_ctx = EventCtx::new(titlebar.id(), event, ctx.tracker());
            if titlebar.event(&mut title_ctx, event) == ControlFlow::Stop {
                return ControlFlow::Stop;
            }
        }

        // Forward event to content
        if let Some(ref mut content) = self.content {
            let mut content_ctx = EventCtx::new(content.id(), event, ctx.tracker());
            if content.event(&mut content_ctx, event) == ControlFlow::Stop {
                return ControlFlow::Stop;
            }
        }

        ControlFlow::Continue
    }

    fn update(&mut self, ctx: &mut UpdateCtx) {
        if let Some(ref mut titlebar) = self.titlebar {
            titlebar.update(ctx);
        }

        if let Some(ref mut content) = self.content {
            content.update(ctx);
        }
    }
}

impl crate::view::traits::View for Window {
    fn body(&self) -> Option<&dyn crate::view::traits::View> {
        None
    }

    // as_any, id, layout, draw, event, update are inherited from RenderObject impl
}
