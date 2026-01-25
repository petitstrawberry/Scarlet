//! Window View - Top-level window container with decorations
//!
//! Window is a View that provides window-level decorations including:
//! - Title bar with close, maximize, minimize buttons
//! - Window border with shadow
//! - Proper event handling for window controls
//! - Content area for child views

use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use core::any::Any;

use crate::view::View;
use crate::element::{Element, ElementId, ElementRenderObject, LayoutConstraints, UpdateResult};
use crate::geometry::{Size, Rect, Point};
use crate::color::Color;
use crate::buffer::Buffer;
use crate::state::Listenable;

/// Constants for window decorations (matching Scarlet_old design)
const TITLEBAR_HEIGHT: u32 = 32;
const CLOSE_BUTTON_SIZE: u32 = 18;
const CLOSE_BUTTON_MARGIN: u32 = 8;
const TITLEBAR_CONTROL_COUNT: u32 = 3;
const WINDOW_CORNER_RADIUS: u32 = 0;
const WINDOW_BORDER_WIDTH: u32 = 2; // 1 outer border + 1 inner highlight

/// Window View - top-level window container
///
/// Window provides window-level properties like title, size, and decorations.
/// The content is a single View (use VStack/HStack for multiple children).
pub struct Window<V: View> {
    app_id: String,
    title: String,
    size: Size,
    resizable: bool,
    decorated: bool,
    content: V,
}

impl<V: View> Window<V> {
    /// Create a new Window with content
    pub fn new(title: impl Into<String>, content: V) -> Self {
        let title_str = title.into();
        Self {
            app_id: String::from("com.example.scarletui"),
            title: title_str,
            size: Size::new(800.0, 600.0),
            resizable: true,
            decorated: true,
            content,
        }
    }

    /// Set the application ID
    pub fn app_id(mut self, app_id: impl Into<String>) -> Self {
        self.app_id = app_id.into();
        self
    }

    /// Set the window size
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Set whether the window is resizable
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Set whether the window has decorations (title bar, borders)
    pub fn decorated(mut self, decorated: bool) -> Self {
        self.decorated = decorated;
        self
    }

    /// Get the application ID
    pub fn get_app_id(&self) -> &str {
        &self.app_id
    }

    /// Get the window title
    pub fn get_title(&self) -> &str {
        &self.title
    }

    /// Get the window size
    pub fn get_window_size(&self) -> Size {
        self.size
    }

    /// Check if the window is resizable
    pub fn is_resizable(&self) -> bool {
        self.resizable
    }

    /// Check if the window is decorated
    pub fn is_decorated(&self) -> bool {
        self.decorated
    }

}

impl<V: View + Clone> Clone for Window<V> {
    fn clone(&self) -> Self {
        Self {
            app_id: self.app_id.clone(),
            title: self.title.clone(),
            size: self.size,
            resizable: self.resizable,
            decorated: self.decorated,
            content: self.content.clone(),
        }
    }
}

impl<V: View + Clone + 'static> View for Window<V> {
    fn create_element(&self) -> Box<dyn Element> {
        // Create WindowRenderObject with titlebar included
        let render_object = WindowRenderObject::new(
            self.title.clone(),
            self.size,
            self.decorated,
        );

        // Create child element from content
        let children = alloc::vec![self.content.create_element()];

        Box::new(WindowRenderElement::new(
            self.clone(),
            render_object,
            children,
        ))
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        self.content.listenables()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// WindowRenderElement - Element for Window that handles child buffer compositing
///
/// This Element handles Window-specific rendering logic:
/// - Renders window decorations (titlebar, borders)
/// - Renders child elements
/// - Composites child buffers below the titlebar
pub struct WindowRenderElement<C: View + Clone> {
    id: ElementId,
    view: C,
    render_object: WindowRenderObject,
    children: Vec<Box<dyn Element>>,
    position: Point,
}

impl<C: View + Clone> WindowRenderElement<C> {
    /// Create a new WindowRenderElement
    pub fn new(view: C, render_object: WindowRenderObject, children: Vec<Box<dyn Element>>) -> Self {
        Self {
            id: ElementId::generate(),
            view,
            render_object,
            children,
            position: Point::ZERO,
        }
    }

    /// Get the Window view
    pub fn view(&self) -> &C {
        &self.view
    }

    /// Get mutable reference to the view
    pub fn view_mut(&mut self) -> &mut C {
        &mut self.view
    }

    /// Get the WindowRenderObject
    pub fn render_object(&self) -> &WindowRenderObject {
        &self.render_object
    }

    /// Get mutable reference to the WindowRenderObject
    pub fn render_object_mut(&mut self) -> &mut WindowRenderObject {
        &mut self.render_object
    }
}

impl<C: View + Clone> Element for WindowRenderElement<C> {
    fn id(&self) -> ElementId {
        self.id
    }

    fn type_name(&self) -> &str {
        "WindowRenderElement"
    }

    fn type_name_debug(&self) -> alloc::string::String {
        alloc::format!("WindowRenderElement<{}>", core::any::type_name::<C>())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn children(&self) -> &[Box<dyn Element>] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Box<dyn Element>] {
        &mut self.children
    }

    fn update(&mut self, new_view: &dyn View) -> UpdateResult {
        if let Some(typed_view) = new_view.as_any().downcast_ref::<C>() {
            self.view = typed_view.clone();
            self.render_object.update(new_view)
        } else {
            UpdateResult::Replaced
        }
    }

    fn rebuild(&mut self) -> UpdateResult {
        UpdateResult::NoChange
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        scarlet_std::println!("[WindowRenderElement::layout] START: constraints=({:?}, {:?}) -> ({:?}, {:?})",
            constraints.min_width, constraints.min_height, constraints.max_width, constraints.max_height);
        // Layout the window
        let size = self.render_object.layout(constraints);
        scarlet_std::println!("[WindowRenderElement::layout] render_object size={}x{}", size.width, size.height);

        // Calculate content area (inside border and below titlebar)
        let border_width = if self.render_object.decorated {
            WINDOW_BORDER_WIDTH as f32
        } else {
            0.0
        };

        let titlebar_height = if self.render_object.decorated {
            TITLEBAR_HEIGHT as f32
        } else {
            0.0
        };

        // Content area: inside border (left, right, bottom), below titlebar
        // Use ceilf to ensure we round up to avoid fractional pixels
        let content_x = border_width;
        let content_y = titlebar_height;
        let content_width = libm::ceilf(size.width - border_width * 2.0).max(1.0);
        let content_height = libm::ceilf(size.height - titlebar_height - border_width).max(1.0);

        scarlet_std::println!("[WindowRenderElement::layout] content_area: x={}, y={}, size={}x{}",
            content_x, content_y, content_width, content_height);

        for child in &mut self.children {
            let child_constraints = LayoutConstraints::loose(content_width, content_height);
            scarlet_std::println!("[WindowRenderElement::layout] child_constraints=({:?}, {:?}) -> ({:?}, {:?})",
                child_constraints.min_width, child_constraints.min_height, child_constraints.max_width, child_constraints.max_height);
            let child_size = child.layout(child_constraints);
            scarlet_std::println!("[WindowRenderElement::layout] child size={}x{}", child_size.width, child_size.height);
            child.set_position(Point::new(content_x, content_y));
        }

        size
    }

    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, position: Point) {
        self.position = position;
    }

    fn bounds(&self) -> Rect {
        Rect {
            origin: self.position,
            size: self.render_object.size(),
        }
    }

    fn hit_test(&self, point: Point) -> bool {
        let local_point = Point {
            x: point.x - self.position.x,
            y: point.y - self.position.y,
        };
        self.render_object.hit_test(local_point)
    }

    fn render(&mut self) {
        // Render window decorations (background, titlebar, borders)
        self.render_object.render();

        // Render children and composite their buffers below the titlebar
        let mut child_buffers: Vec<Option<&Buffer>> = Vec::new();
        for child in &mut self.children {
            child.render();
            child_buffers.push(child.get_buffer());
        }

        // Composite child buffers into window buffer
        let buffers: Vec<&Buffer> = child_buffers
            .into_iter()
            .filter_map(|b| b)
            .collect();
        self.render_object.composite_children(&buffers);
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.render_object.get_buffer()
    }

    fn render_object(&self) -> Option<&dyn ElementRenderObject> {
        Some(&self.render_object)
    }

    fn render_object_mut(&mut self) -> Option<&mut dyn ElementRenderObject> {
        Some(&mut self.render_object)
    }

    fn handle_event(&mut self, _event: &crate::event::Event, _phase: crate::event::Phase) -> bool {
        // TODO: Handle window control button events
        false
    }
}

/// WindowRenderObject - renders window with titlebar and background
///
/// This RenderObject owns a single buffer that contains:
/// - Window background (WHITE or custom)
/// - Titlebar with buttons (if decorated)
pub struct WindowRenderObject {
    title: String,
    size: Size,
    decorated: bool,
    focused: bool,
    buffer: Option<Buffer>,
}

impl WindowRenderObject {
    pub fn new(title: String, size: Size, decorated: bool) -> Self {
        Self {
            title,
            size,
            decorated,
            focused: true,
            buffer: None,
        }
    }

    /// Get close button rect (matching Scarlet_old)
    fn close_button_rect(&self, width: u32) -> Rect {
        self.control_button_rect(width, 0)
    }

    fn maximize_button_rect(&self, width: u32) -> Rect {
        self.control_button_rect(width, 1)
    }

    fn minimize_button_rect(&self, width: u32) -> Rect {
        self.control_button_rect(width, 2)
    }

    /// Get control button rects (matching Scarlet_old)
    fn control_button_rect(&self, width: u32, index_from_right: u32) -> Rect {
        if width < TITLEBAR_CONTROL_COUNT {
            return Rect::zero();
        }

        let base_seg_w = CLOSE_BUTTON_SIZE + CLOSE_BUTTON_MARGIN * 2;
        let seg_w = if width >= base_seg_w * TITLEBAR_CONTROL_COUNT {
            base_seg_w
        } else {
            (width / TITLEBAR_CONTROL_COUNT).max(1)
        };
        let total_w = seg_w.saturating_mul(TITLEBAR_CONTROL_COUNT).min(width);
        let right_x0 = (width - total_w) as i32;
        let x = right_x0 + (total_w as i32) - (seg_w as i32) * (index_from_right as i32 + 1);
        Rect::from_xywh(x as f32, 0.0, seg_w as f32, TITLEBAR_HEIGHT as f32)
    }

    /// Draw the window background and titlebar using Canvas
    fn draw(&mut self) {
        let width = libm::ceilf(self.size.width) as usize;
        let height = libm::ceilf(self.size.height) as usize;
        let needed = width * height;
        let title = self.title.clone();
        let decorated = self.decorated;
        let focused = self.focused;

        // Create or resize buffer
        if self.buffer.as_ref().map_or(true, |b| b.as_slice().len() < needed) {
            scarlet_std::println!("[WindowRenderObject] Creating buffer: {}x{}", width, height);
            self.buffer = Some(Buffer::from_dimensions(width as u32, height as u32));
        }

        if let Some(ref mut buffer) = self.buffer {
            use crate::graphics::Canvas;
            let mut canvas = Canvas::new(buffer.data_mut(), width as u32, height as u32);

            // Fill entire background with white
            canvas.fill_rect(0, 0, width as u32, height as u32, Color::WHITE);

            // Draw titlebar if decorated
            if decorated {
                Self::draw_titlebar_canvas(&title, focused, &mut canvas, width as u32, height as u32);
            }

            // Draw border
            if decorated {
                Self::draw_border_canvas(&mut canvas, width as u32, height as u32);
            }
        }
    }

    /// Composite child buffers into the window buffer
    ///
    /// Children are rendered inside the border, below the titlebar
    pub fn composite_children(&mut self, child_buffers: &[&Buffer]) {
        if self.buffer.is_none() {
            return;
        }

        let buffer = self.buffer.as_mut().unwrap();

        let border_offset = if self.decorated {
            WINDOW_BORDER_WIDTH as i32
        } else {
            0
        };

        let titlebar_height = if self.decorated {
            TITLEBAR_HEIGHT as i32
        } else {
            0
        };

        for child_buffer in child_buffers {
            // Composite child inside border, below titlebar
            buffer.composite(
                child_buffer,
                border_offset,
                titlebar_height,
                1.0, // Full opacity
            );
        }
    }

    /// Draw titlebar using Canvas API (exact Scarlet_old design)
    fn draw_titlebar_canvas(title: &str, _focused: bool, canvas: &mut crate::graphics::Canvas, width: u32, _height: u32) {
        scarlet_std::println!("[WindowRenderObject] draw_titlebar_canvas: width={}, title='{}'", width, title);

        // Title bar base color (exact Scarlet_old: rgb(235, 235, 238))
        let base_color = Color::rgb(235u8, 235u8, 238u8);

        let close_rect = Self::control_button_rect_static(width, 0);
        let maximize_rect = Self::control_button_rect_static(width, 1);
        let minimize_rect = Self::control_button_rect_static(width, 2);

        scarlet_std::println!("[WindowRenderObject] close_rect: origin={:?}, size={:?}", close_rect.origin, close_rect.size);

        // Button colors (matching Scarlet_old: base, hover=210, pressed=190)
        let close_color = base_color; // TODO: add hover/pressed state
        let maximize_color = base_color; // TODO: add hover/pressed state
        let minimize_color = base_color; // TODO: add hover/pressed state

        // Draw titlebar with button colors
        for y in 0..TITLEBAR_HEIGHT {
            // No corner rounding (WINDOW_CORNER_RADIUS = 0)
            canvas.fill_rect(0, y as i32, width, 1, base_color);
            canvas.fill_rect(close_rect.origin.x as i32, y as i32, close_rect.size.width as u32, 1, close_color);
            canvas.fill_rect(maximize_rect.origin.x as i32, y as i32, maximize_rect.size.width as u32, 1, maximize_color);
            canvas.fill_rect(minimize_rect.origin.x as i32, y as i32, minimize_rect.size.width as u32, 1, minimize_color);
        }

        // Title text (exact Scarlet_old: rgb(20, 20, 24))
        canvas.draw_text_sized(10, 7, title, Color::rgb(20u8, 20u8, 24u8), 18.0);

        // Draw button icons (exact Scarlet_old design)
        let icon_color = Color::rgb(30u8, 30u8, 34u8);

        // Close button: X mark (double-stroke lines)
        let cx = close_rect.origin.x + close_rect.size.width / 2.0;
        let cy = close_rect.origin.y + close_rect.size.height / 2.0;
        let size: i32 = 10;
        let half = size / 2;
        let x0 = cx as i32 - half;
        let x1 = cx as i32 + half - 1;
        let y0 = cy as i32 - half;
        let y1 = cy as i32 + half - 1;
        canvas.draw_line(x0, y0, x1, y1, icon_color);
        canvas.draw_line(x1, y0, x0, y1, icon_color);

        // Maximize button: square outline
        let mx = maximize_rect.origin.x + maximize_rect.size.width / 2.0;
        let my = maximize_rect.origin.y + maximize_rect.size.height / 2.0;
        let msize: i32 = 10;
        let mhalf = msize / 2;
        let mx0 = mx as i32 - mhalf;
        let my0 = my as i32 - mhalf;
        canvas.draw_rect(mx0, my0, msize as u32, msize as u32, icon_color);

        // Minimize button: horizontal line
        let nx = minimize_rect.origin.x + minimize_rect.size.width / 2.0;
        let ny = minimize_rect.origin.y + minimize_rect.size.height / 2.0 + 3.0;
        let nsize: i32 = 12;
        let nhalf = nsize / 2;
        canvas.draw_line(nx as i32 - nhalf, ny as i32, nx as i32 + nhalf, ny as i32, icon_color);
    }

    /// Static helper for button rect calculation
    fn control_button_rect_static(width: u32, index_from_right: u32) -> Rect {
        if width < TITLEBAR_CONTROL_COUNT {
            return Rect::zero();
        }

        let base_seg_w = CLOSE_BUTTON_SIZE + CLOSE_BUTTON_MARGIN * 2;
        let seg_w = if width >= base_seg_w * TITLEBAR_CONTROL_COUNT {
            base_seg_w
        } else {
            (width / TITLEBAR_CONTROL_COUNT).max(1)
        };
        let total_w = seg_w.saturating_mul(TITLEBAR_CONTROL_COUNT).min(width);
        let right_x0 = (width - total_w) as i32;
        let x = right_x0 + (total_w as i32) - (seg_w as i32) * (index_from_right as i32 + 1);
        Rect::from_xywh(x as f32, 0.0, seg_w as f32, TITLEBAR_HEIGHT as f32)
    }

    /// Draw window border (exact Scarlet_old design)
    fn draw_border_canvas(canvas: &mut crate::graphics::Canvas, width: u32, height: u32) {
        scarlet_std::println!("[WindowRenderObject] draw_border_canvas: {}x{}", width, height);

        // Modern border with subtle shadow effect
        // Outer border: rgb(100, 100, 105)
        let border_color = Color::rgb(100u8, 100u8, 105u8);
        if width == 0 || height == 0 {
            return;
        }

        canvas.draw_rect(0, 0, width, height, border_color);

        // Inner highlight for depth: rgb(90, 90, 95)
        if width > 2 && height > 2 {
            canvas.draw_rect(
                1,
                1,
                width.saturating_sub(2),
                height.saturating_sub(2),
                Color::rgb(90u8, 90u8, 95u8),
            );
        }
    }
}

impl ElementRenderObject for WindowRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let width = if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
            constraints.max_width.max(constraints.min_width)
        } else {
            self.size.width
        };

        let height = if constraints.max_height.is_finite() && constraints.max_height > 0.0 {
            constraints.max_height.max(constraints.min_height)
        } else {
            self.size.height
        };

        self.size = Size { width, height };
        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn render(&mut self) {
        scarlet_std::println!("[WindowRenderObject] render: size={}x{}, decorated={}",
            self.size.width, self.size.height, self.decorated);
        self.draw();
        scarlet_std::println!("[WindowRenderObject] render: complete, buffer={}",
            self.buffer.is_some());
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn update(&mut self, _new_view: &dyn View) -> UpdateResult {
        UpdateResult::NoChange
    }
}
