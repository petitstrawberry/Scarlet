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
use crate::element::{Element, ElementId, ElementRenderObject, LayoutConstraints, UpdateResult, WindowSizeLimits};
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
    min_size: Option<Size>,
    max_size: Option<Size>,
    resizable: bool,
    decorated: bool,
    content: V,
}

pub trait WindowViewInfo {
    fn window_info(&self) -> (String, String, Size);
    fn window_size_limits(&self) -> WindowSizeLimits;
}

impl<V: View> Window<V> {
    /// Create a new Window with content
    pub fn new(title: impl Into<String>, content: V) -> Self {
        let title_str = title.into();
        Self {
            app_id: String::from("com.example.scarletui"),
            title: title_str,
            size: Size::new(800.0, 600.0),
            min_size: None,
            max_size: None,
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

    /// Set the minimum window size
    pub fn min_size(mut self, size: Size) -> Self {
        self.min_size = Some(size);
        self
    }

    /// Set the maximum window size
    pub fn max_size(mut self, size: Size) -> Self {
        self.max_size = Some(size);
        self
    }

    /// Set both minimum and maximum window sizes
    pub fn size_limits(mut self, min: Size, max: Size) -> Self {
        self.min_size = Some(min);
        self.max_size = Some(max);
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

    /// Get the minimum window size
    pub fn get_min_size(&self) -> Option<Size> {
        self.min_size
    }

    /// Get the maximum window size
    pub fn get_max_size(&self) -> Option<Size> {
        self.max_size
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
            min_size: self.min_size,
            max_size: self.max_size,
            resizable: self.resizable,
            decorated: self.decorated,
            content: self.content.clone(),
        }
    }
}

impl<V: View + Clone> WindowViewInfo for Window<V> {
    fn window_info(&self) -> (String, String, Size) {
        (self.app_id.clone(), self.title.clone(), self.size)
    }

    fn window_size_limits(&self) -> WindowSizeLimits {
        WindowSizeLimits {
            min: self.min_size,
            max: self.max_size,
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
pub struct WindowRenderElement<C: View + Clone + WindowViewInfo> {
    id: ElementId,
    view: C,
    render_object: WindowRenderObject,
    children: Vec<Box<dyn Element>>,
    position: Point,
    pending_window_action: Option<crate::event::WindowEvent>,
    // Track which button is currently pressed (0=none, 1=close, 2=maximize, 3=minimize, 4=titlebar)
    pressed_button: u8,
    // Track last mouse position to detect changes
    last_mouse_x: i32,
    last_mouse_y: i32,
    last_mouse_pressed: bool,
    // Track maximized state for toggle
    maximized: bool,
}

impl<C: View + Clone + WindowViewInfo> WindowRenderElement<C> {
    /// Create a new WindowRenderElement
    pub fn new(view: C, render_object: WindowRenderObject, children: Vec<Box<dyn Element>>) -> Self {
        Self {
            id: ElementId::generate(),
            view,
            render_object,
            children,
            position: Point::ZERO,
            pending_window_action: None,
            pressed_button: 0,
            last_mouse_x: -1,
            last_mouse_y: -1,
            last_mouse_pressed: false,
            maximized: false,
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

impl<C: View + Clone + WindowViewInfo> Element for WindowRenderElement<C> {
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
        self.render_object
            .layout_with_children(constraints, &mut self.children)
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

    fn clear_buffers(&mut self) {
        self.render_object.clear_buffer();
        for child in self.children.iter_mut() {
            child.clear_buffers();
        }
    }

    fn render_object(&self) -> Option<&dyn ElementRenderObject> {
        Some(&self.render_object)
    }

    fn render_object_mut(&mut self) -> Option<&mut dyn ElementRenderObject> {
        Some(&mut self.render_object)
    }

    fn handle_event(&mut self, event: &crate::event::Event, phase: crate::event::Phase) -> bool {
        // Only handle mouse events in target phase
        if phase != crate::event::Phase::Target {
            return false;
        }

        // Only handle events on decorated windows
        if !self.render_object.decorated {
            return false;
        }

        let mut needs_repaint = false;
        let mut handled = false;

        match event {
            crate::event::Event::Mouse(crate::event::MouseEvent::Moved { x, y }) => {
                // SWS coordinates are already window-relative
                let local_x = *x;
                let local_y = *y;

                // Only update if position or pressed state changed
                if local_x != self.last_mouse_x || local_y != self.last_mouse_y {
                    let mouse_pressed = self.pressed_button != 0;

                    // Store old states to check for changes
                    let old_close_state = self.render_object.close_button_state;
                    let old_maximize_state = self.render_object.maximize_button_state;
                    let old_minimize_state = self.render_object.minimize_button_state;

                    // Update button states
                    self.render_object.update_button_states(local_x, local_y, mouse_pressed);

                    // Check if any button state changed
                    if old_close_state != self.render_object.close_button_state
                        || old_maximize_state != self.render_object.maximize_button_state
                        || old_minimize_state != self.render_object.minimize_button_state
                    {
                        needs_repaint = true;
                    }

                    self.last_mouse_x = local_x;
                    self.last_mouse_y = local_y;
                }
            }
            crate::event::Event::Mouse(crate::event::MouseEvent::ButtonPressed {
                x,
                y,
                button: crate::event::MouseButton::Left,
            }) => {
                // SWS coordinates are already window-relative
                let local_x = *x;
                let local_y = *y;

                // Check if click is in titlebar
                let width = self.render_object.size.width as u32;
                let titlebar_height = TITLEBAR_HEIGHT as i32;

                if local_y >= 0 && local_y < titlebar_height {
                    // Determine which button was pressed
                    let close_rect = self.render_object.close_button_rect(width);
                    let maximize_rect = self.render_object.maximize_button_rect(width);
                    let minimize_rect = self.render_object.minimize_button_rect(width);

                    if close_rect.contains(crate::geometry::Point {
                        x: local_x as f32,
                        y: local_y as f32,
                    }) {
                        self.pressed_button = 1; // close
                    } else if maximize_rect.contains(crate::geometry::Point {
                        x: local_x as f32,
                        y: local_y as f32,
                    }) {
                        self.pressed_button = 2; // maximize
                    } else if minimize_rect.contains(crate::geometry::Point {
                        x: local_x as f32,
                        y: local_y as f32,
                    }) {
                        self.pressed_button = 3; // minimize
                    } else {
                        // Clicked on titlebar (not buttons) - request interactive move immediately
                        self.pressed_button = 0;
                        self.pending_window_action = Some(crate::event::WindowEvent::MoveRequested);
                        handled = true;

                        // Update last mouse position
                        self.last_mouse_x = local_x;
                        self.last_mouse_y = local_y;
                        self.last_mouse_pressed = true;

                        // Don't update button states for titlebar clicks
                        return handled;
                    }

                    // Update button states (pressed = true)
                    self.render_object.update_button_states(local_x, local_y, true);
                    needs_repaint = true;
                    handled = true;

                    // Update last mouse position
                    self.last_mouse_x = local_x;
                    self.last_mouse_y = local_y;
                    self.last_mouse_pressed = true;
                }
            }
            crate::event::Event::Mouse(crate::event::MouseEvent::ButtonReleased {
                x,
                y,
                button: crate::event::MouseButton::Left,
            }) => {
                // Only handle if we had a button pressed
                if self.pressed_button != 0 {
                    // SWS coordinates are already window-relative
                    let local_x = *x;
                    let local_y = *y;

                    // Check which button we're releasing on
                    let width = self.render_object.size.width as u32;
                    let titlebar_height = TITLEBAR_HEIGHT as i32;

                    if local_y >= 0 && local_y < titlebar_height {
                        let close_rect = self.render_object.close_button_rect(width);
                        let maximize_rect = self.render_object.maximize_button_rect(width);
                        let minimize_rect = self.render_object.minimize_button_rect(width);

                        let released_on_close = close_rect.contains(crate::geometry::Point {
                            x: local_x as f32,
                            y: local_y as f32,
                        });
                        let released_on_maximize = maximize_rect.contains(crate::geometry::Point {
                            x: local_x as f32,
                            y: local_y as f32,
                        });
                        let released_on_minimize = minimize_rect.contains(crate::geometry::Point {
                            x: local_x as f32,
                            y: local_y as f32,
                        });

                        // Only trigger action if released on the same button that was pressed
                        match self.pressed_button {
                            1 if released_on_close => {
                                self.pending_window_action = Some(crate::event::WindowEvent::CloseRequested);
                            }
                            2 if released_on_maximize => {
                                // Toggle maximize/restore
                                if self.maximized {
                                    self.pending_window_action = Some(crate::event::WindowEvent::RestoreRequested);
                                } else {
                                    self.pending_window_action = Some(crate::event::WindowEvent::MaximizeRequested);
                                }
                                self.maximized = !self.maximized;
                            }
                            3 if released_on_minimize => {
                                self.pending_window_action = Some(crate::event::WindowEvent::MinimizeRequested);
                            }
                            _ => {}
                        }
                    }

                    // Reset pressed state
                    self.pressed_button = 0;
                    self.render_object.update_button_states(local_x, local_y, false);
                    needs_repaint = true;

                    // Update last mouse position
                    self.last_mouse_x = local_x;
                    self.last_mouse_y = local_y;
                    self.last_mouse_pressed = false;
                    handled = true;
                }
            }
            _ => {}
        }

        // Mark for repaint if button states changed
        if needs_repaint {
            crate::pipeline::mark_element_needs_paint(self.id());
        }

        handled
    }

    fn take_window_action(&mut self) -> Option<crate::event::WindowEvent> {
        core::mem::take(&mut self.pending_window_action)
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
    // Button hover states (0=none, 1=hover, 2=pressed)
    close_button_state: u8,
    maximize_button_state: u8,
    minimize_button_state: u8,
}

impl WindowRenderObject {
    pub fn new(title: String, size: Size, decorated: bool) -> Self {
        Self {
            title,
            size,
            decorated,
            focused: true,
            buffer: None,
            close_button_state: 0,
            maximize_button_state: 0,
            minimize_button_state: 0,
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

    /// Update button hover/pressed states based on mouse position
    pub fn update_button_states(&mut self, mouse_x: i32, mouse_y: i32, mouse_pressed: bool) {
        if !self.decorated {
            self.close_button_state = 0;
            self.maximize_button_state = 0;
            self.minimize_button_state = 0;
            return;
        }

        let width = self.size.width as u32;

        // Update close button state
        let close_rect = self.close_button_rect(width);
        self.close_button_state = if close_rect.contains(crate::geometry::Point {
            x: mouse_x as f32,
            y: mouse_y as f32,
        }) {
            if mouse_pressed { 2 } else { 1 }
        } else {
            0
        };

        // Update maximize button state
        let maximize_rect = self.maximize_button_rect(width);
        self.maximize_button_state = if maximize_rect.contains(crate::geometry::Point {
            x: mouse_x as f32,
            y: mouse_y as f32,
        }) {
            if mouse_pressed { 2 } else { 1 }
        } else {
            0
        };

        // Update minimize button state
        let minimize_rect = self.minimize_button_rect(width);
        self.minimize_button_state = if minimize_rect.contains(crate::geometry::Point {
            x: mouse_x as f32,
            y: mouse_y as f32,
        }) {
            if mouse_pressed { 2 } else { 1 }
        } else {
            0
        };
    }

    /// Get button color based on state
    fn get_button_color(state: u8) -> Color {
        match state {
            0 => Color::rgb(235u8, 235u8, 238u8), // normal
            1 => Color::rgb(210u8, 210u8, 213u8), // hover
            2 => Color::rgb(190u8, 190u8, 193u8), // pressed
            _ => Color::rgb(235u8, 235u8, 238u8),
        }
    }

    /// Draw the window background and titlebar using Canvas
    fn draw(&mut self) {
        let width = libm::ceilf(self.size.width) as usize;
        let height = libm::ceilf(self.size.height) as usize;
        let title = self.title.clone();
        let decorated = self.decorated;
        let focused = self.focused;

        // Copy button states before borrowing buffer
        let close_state = self.close_button_state;
        let maximize_state = self.maximize_button_state;
        let minimize_state = self.minimize_button_state;

        // Create or resize buffer
        let w = width as u32;
        let h = height as u32;
        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.width() != w || b.height() != h);
        if needs_resize {
            if crate::debug::is_enabled() {
                scarlet_std::println!("[WindowRenderObject] Creating buffer: {}x{}", width, height);
            }
            self.buffer = Some(Buffer::from_dimensions(w, h));
        }

        if let Some(ref mut buffer) = self.buffer {
            use crate::graphics::Canvas;
            let mut canvas = Canvas::new(buffer.data_mut(), w, h);

            // Fill entire background with white
            canvas.fill_rect(0, 0, w, h, Color::WHITE);

            // Draw titlebar if decorated
            if decorated {
                Self::draw_titlebar_canvas_with_states(
                    &title,
                    focused,
                    &mut canvas,
                    width as u32,
                    height as u32,
                    close_state,
                    maximize_state,
                    minimize_state,
                );
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
    fn draw_titlebar_canvas_with_states(
        title: &str,
        _focused: bool,
        canvas: &mut crate::graphics::Canvas,
        width: u32,
        _height: u32,
        close_button_state: u8,
        maximize_button_state: u8,
        minimize_button_state: u8,
    ) {
        if crate::debug::is_enabled() {
            scarlet_std::println!("[WindowRenderObject] draw_titlebar_canvas: width={}, title='{}'", width, title);
        }

        // Title bar base color (exact Scarlet_old: rgb(235, 235, 238))
        let base_color = Color::rgb(235u8, 235u8, 238u8);

        let close_rect = Self::control_button_rect_static(width, 0);
        let maximize_rect = Self::control_button_rect_static(width, 1);
        let minimize_rect = Self::control_button_rect_static(width, 2);

        if crate::debug::is_enabled() {
            scarlet_std::println!("[WindowRenderObject] close_rect: origin={:?}, size={:?}", close_rect.origin, close_rect.size);
        }

        // Button colors based on hover/pressed state
        let close_color = Self::get_button_color(close_button_state);
        let maximize_color = Self::get_button_color(maximize_button_state);
        let minimize_color = Self::get_button_color(minimize_button_state);

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
        if crate::debug::is_enabled() {
            scarlet_std::println!("[WindowRenderObject] draw_border_canvas: {}x{}", width, height);
        }

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

    fn layout_with_children(
        &mut self,
        constraints: LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        if crate::debug::is_enabled() {
            scarlet_std::println!("[WindowRenderObject::layout] START: constraints=({:?}, {:?}) -> ({:?}, {:?})",
                constraints.min_width, constraints.min_height, constraints.max_width, constraints.max_height);
        }

        let size = self.layout(constraints);
        if crate::debug::is_enabled() {
            scarlet_std::println!("[WindowRenderObject::layout] size={}x{}", size.width, size.height);
        }

        let border_width = if self.decorated {
            WINDOW_BORDER_WIDTH as f32
        } else {
            0.0
        };

        let titlebar_height = if self.decorated {
            TITLEBAR_HEIGHT as f32
        } else {
            0.0
        };

        let content_x = border_width;
        let content_y = titlebar_height;
        let content_width = libm::ceilf(size.width - border_width * 2.0).max(1.0);
        let content_height = libm::ceilf(size.height - titlebar_height - border_width).max(1.0);

        if crate::debug::is_enabled() {
            scarlet_std::println!("[WindowRenderObject::layout] content_area: x={}, y={}, size={}x{}",
                content_x, content_y, content_width, content_height);
        }

        for child in children {
            let child_constraints = LayoutConstraints::loose(content_width, content_height);
            if crate::debug::is_enabled() {
                scarlet_std::println!("[WindowRenderObject::layout] child_constraints=({:?}, {:?}) -> ({:?}, {:?})",
                    child_constraints.min_width, child_constraints.min_height, child_constraints.max_width, child_constraints.max_height);
            }
            let child_size = child.layout(child_constraints);
            if crate::debug::is_enabled() {
                scarlet_std::println!("[WindowRenderObject::layout] child size={}x{}", child_size.width, child_size.height);
            }
            child.set_position(Point::new(content_x, content_y));
        }

        size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn render(&mut self) {
        if crate::debug::is_enabled() {
            scarlet_std::println!("[WindowRenderObject] render: size={}x{}, decorated={}",
                self.size.width, self.size.height, self.decorated);
        }
        self.draw();
        if crate::debug::is_enabled() {
            scarlet_std::println!("[WindowRenderObject] render: complete, buffer={}",
                self.buffer.is_some());
        }
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn clear_buffer(&mut self) {
        self.buffer = None;
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
