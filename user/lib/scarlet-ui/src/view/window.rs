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
use crate::view::traits::{View, ViewChild};
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
/// └── content (user View)
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

    // Child views
    titlebar: Option<Box<dyn View>>,
    content: Option<Box<dyn View>>,

    // Child frames (for children() method)
    titlebar_frame: Rect,
    content_frame: Rect,

    // Cached children (for View trait)
    cached_children: Vec<ViewChild>,

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
            cached_children: Vec::new(),
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
}

impl View for Window {
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

        // Clear cached children
        self.cached_children.clear();

        // Layout titlebar at the top
        if let Some(ref mut titlebar) = self.titlebar {
            let titlebar_constraints = LayoutConstraints::new(
                self.width,
                self.width,
                TITLEBAR_HEIGHT,
                TITLEBAR_HEIGHT,
            );
            titlebar.layout(ctx, titlebar_constraints);
            self.cached_children.push(ViewChild::new(
                // Note: We can't clone the Box<dyn View>, so we create a placeholder
                // The actual child is stored in titlebar field
                Box::new(crate::view::Spacer::new()),
                self.titlebar_frame,
            ));
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
            self.cached_children.push(ViewChild::new(
                Box::new(crate::view::Spacer::new()),
                self.content_frame,
            ));
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

    fn children(&self) -> &[ViewChild] {
        &self.cached_children
    }

    fn children_mut(&mut self) -> &mut [ViewChild] {
        &mut self.cached_children
    }
}
