//! Window View - Top-level window container with decorations
//!
//! Window is a View that provides window-level decorations including:
//! - Title bar with close, maximize, minimize buttons
//! - Window border with shadow
//! - Proper event handling for window controls
//! - Content area for child views

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;

use crate::buffer::Buffer;
use crate::color::{Color, ColorPalette};
use crate::element::{
    Element, ElementId, ElementRenderObject, LayoutConstraints, UpdateResult, WindowSizeLimits,
};
use crate::geometry::{Point, Rect, Size};
use crate::menu_model::MenuBarModel;
use crate::pipeline::{MountContext, PipelineId};
use crate::state::Listenable;
use crate::view::View;

/// Window types (matching sws_protocol::window_types)
pub mod window_type {
    pub const NORMAL: u32 = 0;
    pub const ALWAYS_ON_TOP: u32 = 1;
    pub const TASKBAR: u32 = 2;
    pub const DESKTOP: u32 = 3;
}

/// Window information
#[derive(Clone)]
pub struct WindowInfo {
    pub app_id: String,
    pub title: String,
    pub size: Size,
    pub window_type: u32,
    pub menu_bar: Option<MenuBarModel>,
    pub focus_on_create: bool,
    pub active_on_focus: bool,
    pub background_color: Color,
    pub opaque: bool,
}

impl WindowInfo {
    pub fn new(
        app_id: String,
        title: String,
        size: Size,
        window_type: u32,
        menu_bar: Option<MenuBarModel>,
        focus_on_create: bool,
        active_on_focus: bool,
        background_color: Color,
        opaque: bool,
    ) -> Self {
        Self {
            app_id,
            title,
            size,
            window_type,
            menu_bar,
            focus_on_create,
            active_on_focus,
            background_color,
            opaque,
        }
    }
}

/// Constants for window decorations (matching Scarlet_old design)
const TITLEBAR_HEIGHT: u32 = 32;
const CLOSE_BUTTON_SIZE: u32 = 18;
const CLOSE_BUTTON_MARGIN: u32 = 8;
const TITLEBAR_CONTROL_COUNT: u32 = 3;
const WINDOW_CORNER_RADIUS: u32 = 0;
const WINDOW_BORDER_WIDTH: u32 = 1;

/// Layout metrics for the content area inside a top-level window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowContentLayout {
    offset: Point,
    decoration_size: Size,
}

impl WindowContentLayout {
    /// Create content layout metrics for a window decoration mode.
    ///
    /// # Arguments
    ///
    /// * `decorated` - Whether the window has a titlebar and border.
    ///
    /// # Returns
    ///
    /// Content offset and total decoration size in ScarletUI logical pixels.
    pub const fn new(decorated: bool) -> Self {
        if decorated {
            let border_width = WINDOW_BORDER_WIDTH as f32;
            let titlebar_height = TITLEBAR_HEIGHT as f32;
            Self {
                offset: Point::new(border_width, titlebar_height),
                decoration_size: Size::new(border_width * 2.0, titlebar_height + border_width),
            }
        } else {
            Self {
                offset: Point::ZERO,
                decoration_size: Size::ZERO,
            }
        }
    }

    /// Get the content area's origin relative to the window origin.
    ///
    /// # Returns
    ///
    /// Content origin in ScarletUI logical pixels.
    pub const fn offset(&self) -> Point {
        self.offset
    }

    /// Get the total non-content size contributed by decorations.
    ///
    /// # Returns
    ///
    /// Horizontal and vertical decoration size in ScarletUI logical pixels.
    pub const fn decoration_size(&self) -> Size {
        self.decoration_size
    }
}

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
    movable: bool,
    decorated: bool,
    background_color: Color,
    opaque: bool,
    window_type: u32,
    menu_bar: Option<MenuBarModel>,
    focus_on_create: bool,
    active_on_focus: bool,
    scene_key: Option<String>,
    opens_at_launch: bool,
    content: V,
}

pub trait WindowViewInfo {
    /// Return window metadata used by the platform window.
    ///
    /// # Returns
    ///
    /// Window metadata such as title, size, type, menu bar, and background color.
    fn window_info(&self) -> WindowInfo;

    /// Return size limits advertised for the platform window.
    ///
    /// # Returns
    ///
    /// Minimum and maximum window sizes plus the resizable flag.
    fn window_size_limits(&self) -> WindowSizeLimits;

    /// Return whether the window draws client-side decorations.
    ///
    /// # Returns
    ///
    /// `true` when the window has a ScarletUI titlebar and border.
    fn is_decorated(&self) -> bool {
        true
    }

    /// Return the content view hosted by this window.
    ///
    /// # Returns
    ///
    /// The content view when this value represents an actual `Window`.
    fn content_view(&self) -> Option<&dyn View> {
        None
    }

    /// Return whether the window can be resized.
    ///
    /// # Returns
    ///
    /// `true` when resize actions should be accepted.
    fn is_resizable(&self) -> bool {
        false
    }

    /// Return whether the window can be moved.
    ///
    /// # Returns
    ///
    /// `true` when titlebar drag actions should be accepted.
    fn is_movable(&self) -> bool {
        true
    }
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
            movable: true,
            decorated: true,
            background_color: ColorPalette::light().window_background(),
            opaque: true,
            window_type: window_type::NORMAL,
            menu_bar: None,
            focus_on_create: true,
            active_on_focus: true,
            scene_key: None,
            opens_at_launch: true,
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

    /// Set whether the window is movable
    pub fn movable(mut self, movable: bool) -> Self {
        self.movable = movable;
        self
    }

    /// Set whether the window has decorations (title bar, borders)
    pub fn decorated(mut self, decorated: bool) -> Self {
        self.decorated = decorated;
        self
    }

    /// Set the background color.
    pub fn background_color(mut self, color: Color) -> Self {
        self.background_color = color;
        self
    }

    /// Set whether this window is fully opaque.
    pub fn opaque(mut self, opaque: bool) -> Self {
        self.opaque = opaque;
        self
    }

    /// Set the window type (NORMAL, TASKBAR, ALWAYS_ON_TOP)
    pub fn window_type(mut self, window_type: u32) -> Self {
        self.window_type = window_type;
        self
    }

    /// Set whether the window should request focus when created
    pub fn focus_on_create(mut self, focus_on_create: bool) -> Self {
        self.focus_on_create = focus_on_create;
        self
    }

    /// Set whether focusing this window should change the active app
    pub fn active_on_focus(mut self, active_on_focus: bool) -> Self {
        self.active_on_focus = active_on_focus;
        self
    }

    /// Set menu bar model for the window
    pub fn menu_bar(mut self, menu_bar: MenuBarModel) -> Self {
        self.menu_bar = Some(menu_bar);
        self
    }

    /// Set the scene key used when this window is declared directly as a scene.
    pub fn scene_key(mut self, key: impl Into<String>) -> Self {
        self.scene_key = Some(key.into());
        self
    }

    /// Set whether this direct window scene opens when the application launches.
    pub fn open_at_launch(mut self, opens_at_launch: bool) -> Self {
        self.opens_at_launch = opens_at_launch;
        self
    }

    pub(crate) fn scene_key_value(&self) -> Option<&str> {
        self.scene_key.as_deref()
    }

    pub(crate) fn opens_at_launch_value(&self) -> bool {
        self.opens_at_launch
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

    /// Check if the window is fully opaque.
    pub fn is_opaque(&self) -> bool {
        self.opaque
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
            movable: self.movable,
            decorated: self.decorated,
            background_color: self.background_color,
            opaque: self.opaque,
            window_type: self.window_type,
            menu_bar: self.menu_bar.clone(),
            focus_on_create: self.focus_on_create,
            active_on_focus: self.active_on_focus,
            scene_key: self.scene_key.clone(),
            opens_at_launch: self.opens_at_launch,
            content: self.content.clone(),
        }
    }
}

impl<V: View + Clone> WindowViewInfo for Window<V> {
    fn window_info(&self) -> WindowInfo {
        WindowInfo::new(
            self.app_id.clone(),
            self.title.clone(),
            self.size,
            self.window_type,
            self.menu_bar.clone(),
            self.focus_on_create,
            self.active_on_focus,
            self.background_color,
            self.opaque,
        )
    }

    fn window_size_limits(&self) -> WindowSizeLimits {
        WindowSizeLimits {
            min: self.min_size,
            max: self.max_size,
            resizable: self.resizable,
        }
    }

    fn is_decorated(&self) -> bool {
        self.decorated
    }

    fn content_view(&self) -> Option<&dyn View> {
        Some(&self.content)
    }

    fn is_resizable(&self) -> bool {
        self.resizable
    }

    fn is_movable(&self) -> bool {
        self.movable
    }
}

impl<V: View + Clone + 'static> View for Window<V> {
    fn create_element(&self) -> Box<dyn Element> {
        // Create WindowRenderObject for the background and border.
        let render_object = WindowRenderObject::new(
            self.title.clone(),
            self.size,
            self.decorated,
            self.background_color,
        );

        // Create child elements. The titlebar is a separate render element so
        // content repaints do not repaint window decorations.
        let mut children = Vec::new();
        if self.decorated {
            children.push(WindowTitleBarView::new(self.title.clone()).create_element());
        }
        children.push(self.content.create_element());

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

#[derive(Clone)]
struct WindowTitleBarView {
    title: String,
    focused: bool,
}

impl WindowTitleBarView {
    fn new(title: String) -> Self {
        Self {
            title,
            focused: true,
        }
    }
}

impl View for WindowTitleBarView {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(crate::element::RenderElement::new(
            self.clone(),
            WindowTitleBarRenderObject::new(self.title.clone(), self.focused),
        ))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct WindowTitleBarRenderObject {
    title: String,
    focused: bool,
    size: Size,
    buffer: Option<Buffer>,
    // Button hover states (0=none, 1=hover, 2=pressed)
    close_button_state: u8,
    maximize_button_state: u8,
    minimize_button_state: u8,
}

impl WindowTitleBarRenderObject {
    fn new(title: String, focused: bool) -> Self {
        Self {
            title,
            focused,
            size: Size::new(0.0, TITLEBAR_HEIGHT as f32),
            buffer: None,
            close_button_state: 0,
            maximize_button_state: 0,
            minimize_button_state: 0,
        }
    }

    fn update_button_states(&mut self, mouse_x: i32, mouse_y: i32, mouse_pressed: bool) -> bool {
        let old_close_state = self.close_button_state;
        let old_maximize_state = self.maximize_button_state;
        let old_minimize_state = self.minimize_button_state;

        let width = self.size.width as u32;
        self.close_button_state = Self::button_state(width, 0, mouse_x, mouse_y, mouse_pressed);
        self.maximize_button_state = Self::button_state(width, 1, mouse_x, mouse_y, mouse_pressed);
        self.minimize_button_state = Self::button_state(width, 2, mouse_x, mouse_y, mouse_pressed);

        old_close_state != self.close_button_state
            || old_maximize_state != self.maximize_button_state
            || old_minimize_state != self.minimize_button_state
    }

    fn button_state(
        width: u32,
        index_from_right: u32,
        mouse_x: i32,
        mouse_y: i32,
        mouse_pressed: bool,
    ) -> u8 {
        let rect = WindowRenderObject::control_button_rect_static(width, index_from_right);
        if rect.contains(Point::new(mouse_x as f32, mouse_y as f32)) {
            if mouse_pressed { 2 } else { 1 }
        } else {
            0
        }
    }
}

impl ElementRenderObject for WindowTitleBarRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let width = if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
            constraints.max_width.max(constraints.min_width)
        } else {
            self.size.width.max(constraints.min_width)
        };
        self.size = Size::new(width, TITLEBAR_HEIGHT as f32);

        let w = libm::ceilf(self.size.width) as u32;
        let h = libm::ceilf(self.size.height) as u32;
        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.logical_width() != w || b.logical_height() != h);
        if needs_resize {
            self.buffer = Some(Buffer::from_logical_dimensions(w, h));
        }

        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn render(&mut self) {
        if let Some(ref mut buffer) = self.buffer {
            use crate::graphics::Canvas;

            let mut canvas = Canvas::for_buffer(buffer);
            let width = canvas.width();
            let height = canvas.height();
            WindowRenderObject::draw_titlebar_canvas_with_states(
                &self.title,
                self.focused,
                &mut canvas,
                width,
                height,
                self.close_button_state,
                self.maximize_button_state,
                self.minimize_button_state,
            );
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

    fn update(&mut self, new_view: &dyn View) -> UpdateResult {
        let Some(titlebar) = new_view.as_any().downcast_ref::<WindowTitleBarView>() else {
            return UpdateResult::Replaced;
        };

        if self.title == titlebar.title && self.focused == titlebar.focused {
            return UpdateResult::NoChange;
        }

        self.title = titlebar.title.clone();
        self.focused = titlebar.focused;
        UpdateResult::Updated
    }
}

/// WindowRenderElement - Element for top-level Window behavior
///
/// This Element handles Window-specific rendering logic:
/// - Renders the window background and border
/// - Positions the titlebar and content elements
/// - Handles titlebar window-control events
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
    pipeline_id: PipelineId,
}

impl<C: View + Clone + WindowViewInfo> WindowRenderElement<C> {
    /// Create a new WindowRenderElement
    pub fn new(
        view: C,
        render_object: WindowRenderObject,
        children: Vec<Box<dyn Element>>,
    ) -> Self {
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
            pipeline_id: PipelineId::default(),
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

    fn titlebar_child_index(&self) -> Option<usize> {
        self.render_object.decorated.then_some(0)
    }

    fn content_child_index(&self) -> usize {
        if self.render_object.decorated { 1 } else { 0 }
    }

    fn titlebar_render_object_mut(&mut self) -> Option<&mut WindowTitleBarRenderObject> {
        let index = self.titlebar_child_index()?;
        self.children
            .get_mut(index)?
            .render_object_mut()?
            .as_any_mut()
            .downcast_mut::<WindowTitleBarRenderObject>()
    }

    fn update_titlebar_button_states(
        &mut self,
        mouse_x: i32,
        mouse_y: i32,
        mouse_pressed: bool,
    ) -> bool {
        self.titlebar_render_object_mut()
            .is_some_and(|titlebar| titlebar.update_button_states(mouse_x, mouse_y, mouse_pressed))
    }

    fn mark_titlebar_needs_paint(&self) {
        if let Some(index) = self.titlebar_child_index()
            && let Some(titlebar) = self.children.get(index)
        {
            crate::pipeline::mark_element_needs_paint(self.pipeline_id, titlebar.id());
        }
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
            if typed_view.is_decorated() != self.render_object.decorated {
                return UpdateResult::Replaced;
            }

            let window_info = typed_view.window_info();
            if let Some(index) = self.titlebar_child_index()
                && let Some(titlebar) = self.children.get_mut(index)
            {
                let titlebar_view = WindowTitleBarView::new(window_info.title.clone());
                if matches!(titlebar.update(&titlebar_view), UpdateResult::Updated) {
                    crate::pipeline::mark_element_needs_paint(self.pipeline_id, titlebar.id());
                }
            }

            if let Some(content_view) = typed_view.content_view() {
                let content_index = self.content_child_index();
                if let Some(content) = self.children.get_mut(content_index) {
                    match content.update(content_view) {
                        UpdateResult::NoChange => {}
                        UpdateResult::Updated => {
                            crate::pipeline::mark_element_needs_paint(
                                self.pipeline_id,
                                content.id(),
                            );
                        }
                        UpdateResult::Replaced => {
                            let old_constraints = content.last_layout_constraints();
                            let old_position = content.position();
                            let focused_path =
                                crate::element::focused_descendant_path(content.as_ref());
                            content.unmount();
                            let mut new_content = content_view.create_element();
                            let ctx = MountContext::new(self.pipeline_id);
                            new_content.mount(&ctx);
                            if let Some(constraints) = old_constraints {
                                new_content.layout(constraints);
                                new_content.set_position(old_position);
                            }
                            if let Some(path) = focused_path.as_deref() {
                                crate::element::restore_focus_at_path(new_content.as_mut(), path);
                            }
                            self.children[content_index] = new_content;
                            if old_constraints.is_some() {
                                crate::pipeline::mark_element_needs_paint(
                                    self.pipeline_id,
                                    self.children[content_index].id(),
                                );
                            } else {
                                crate::pipeline::mark_element_needs_layout(
                                    self.pipeline_id,
                                    self.children[content_index].id(),
                                );
                            }
                        }
                    }
                }
            }

            self.view = typed_view.clone();
            self.render_object
                .update_from_window_info(&window_info, typed_view.is_decorated())
        } else {
            UpdateResult::Replaced
        }
    }

    fn rebuild(&mut self) -> UpdateResult {
        UpdateResult::NoChange
    }

    fn mount(&mut self, ctx: &MountContext) {
        self.pipeline_id = ctx.pipeline_id();
        // Mount children
        for child in &mut self.children {
            child.mount(ctx);
        }
    }

    fn unmount(&mut self) {
        // Unmount children
        for child in &mut self.children {
            child.unmount();
        }
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
        // Render the window background and border. The titlebar and content are
        // independent children so their paint invalidation stays separate.
        self.render_object.render();
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
        if !matches!(event, crate::event::Event::Mouse(_)) {
            for child in self.children.iter_mut() {
                if child.handle_event(event, phase) {
                    return true;
                }
            }
            return false;
        }

        // Titlebar children are separate elements, so the parent window accepts
        // all dispatch phases and handles controls whenever the event is in
        // the titlebar band.
        if phase != crate::event::Phase::Target
            && phase != crate::event::Phase::Capture
            && phase != crate::event::Phase::Bubble
        {
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

                    if self.update_titlebar_button_states(local_x, local_y, mouse_pressed) {
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
                    self.update_titlebar_button_states(local_x, local_y, true);
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
                                self.pending_window_action =
                                    Some(crate::event::WindowEvent::CloseRequested);
                            }
                            2 if released_on_maximize => {
                                // Toggle maximize/restore
                                if self.maximized {
                                    self.pending_window_action =
                                        Some(crate::event::WindowEvent::RestoreRequested);
                                } else {
                                    self.pending_window_action =
                                        Some(crate::event::WindowEvent::MaximizeRequested);
                                }
                                self.maximized = !self.maximized;
                            }
                            3 if released_on_minimize => {
                                self.pending_window_action =
                                    Some(crate::event::WindowEvent::MinimizeRequested);
                            }
                            _ => {}
                        }
                    }

                    // Reset pressed state
                    self.pressed_button = 0;
                    self.update_titlebar_button_states(local_x, local_y, false);
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
            self.mark_titlebar_needs_paint();
        }

        handled
    }

    fn take_window_action(&mut self) -> Option<crate::event::WindowEvent> {
        core::mem::take(&mut self.pending_window_action)
    }

    fn get_window_info(&self) -> Option<WindowInfo> {
        Some(self.view.window_info())
    }

    fn get_window_size_limits(&self) -> Option<WindowSizeLimits> {
        Some(self.view.window_size_limits())
    }
}

/// WindowRenderObject - renders window with titlebar and background
///
/// This RenderObject owns a single buffer that contains:
/// - Window background (WHITE or custom)
/// - Window border (if decorated)
pub struct WindowRenderObject {
    size: Size,
    decorated: bool,
    background_color: Color,
    buffer: Option<Buffer>,
}

impl WindowRenderObject {
    pub fn new(_title: String, size: Size, decorated: bool, background_color: Color) -> Self {
        Self {
            size,
            decorated,
            background_color,
            buffer: None,
        }
    }

    /// Get the window background color.
    pub fn background_color(&self) -> Color {
        self.background_color
    }

    fn update_from_window_info(&mut self, info: &WindowInfo, decorated: bool) -> UpdateResult {
        let changed = self.size != info.size
            || self.decorated != decorated
            || self.background_color != info.background_color;

        self.size = info.size;
        self.decorated = decorated;
        self.background_color = info.background_color;

        if changed {
            UpdateResult::Updated
        } else {
            UpdateResult::NoChange
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
        let decorated = self.decorated;

        // Create or resize buffer
        let w = width as u32;
        let h = height as u32;
        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.logical_width() != w || b.logical_height() != h);
        if needs_resize {
            if crate::debug::is_enabled() {
                crate::logln!("[WindowRenderObject] Creating buffer: {}x{}", width, height);
            }
            self.buffer = Some(Buffer::from_logical_dimensions(w, h));
        }

        if let Some(ref mut buffer) = self.buffer {
            use crate::graphics::Canvas;
            let mut canvas = Canvas::for_buffer(buffer);
            let w = canvas.width();
            let h = canvas.height();

            // Fill background with the specified color, including explicit transparency.
            canvas.fill_rect(0, 0, w, h, self.background_color);

            // Draw border
            if decorated {
                Self::draw_border_canvas(&mut canvas, width as u32, height as u32);
            }
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
            crate::logln!(
                "[WindowRenderObject] draw_titlebar_canvas: width={}, title='{}'",
                width,
                title
            );
        }

        // Title bar base color (exact Scarlet_old: rgb(235, 235, 238))
        let base_color = Color::rgb(235u8, 235u8, 238u8);

        let close_rect = Self::control_button_rect_static(width, 0);
        let maximize_rect = Self::control_button_rect_static(width, 1);
        let minimize_rect = Self::control_button_rect_static(width, 2);

        if crate::debug::is_enabled() {
            crate::logln!(
                "[WindowRenderObject] close_rect: origin={:?}, size={:?}",
                close_rect.origin,
                close_rect.size
            );
        }

        // Button colors based on hover/pressed state
        let close_color = Self::get_button_color(close_button_state);
        let maximize_color = Self::get_button_color(maximize_button_state);
        let minimize_color = Self::get_button_color(minimize_button_state);

        // Draw titlebar with button colors
        for y in 0..TITLEBAR_HEIGHT {
            // No corner rounding (WINDOW_CORNER_RADIUS = 0)
            canvas.fill_rect(0, y as i32, width, 1, base_color);
            canvas.fill_rect(
                close_rect.origin.x as i32,
                y as i32,
                close_rect.size.width as u32,
                1,
                close_color,
            );
            canvas.fill_rect(
                maximize_rect.origin.x as i32,
                y as i32,
                maximize_rect.size.width as u32,
                1,
                maximize_color,
            );
            canvas.fill_rect(
                minimize_rect.origin.x as i32,
                y as i32,
                minimize_rect.size.width as u32,
                1,
                minimize_color,
            );
        }

        // Title text (exact Scarlet_old: rgb(20, 20, 24))
        let title_x: i32 = 10;
        let title_y: i32 = 7;
        let title_font_size: f32 = 18.0;
        let title_color = Color::rgb(20u8, 20u8, 24u8);
        let title_padding_right: f32 = 4.0;

        // minimize is the leftmost control button (index_from_right=2)
        let available_width = if minimize_rect.origin.x > title_x as f32 {
            (minimize_rect.origin.x - title_x as f32 - title_padding_right).max(0.0) as u32
        } else {
            0
        };

        let display_title = if available_width == 0 {
            alloc::string::String::new()
        } else {
            let full_width = crate::graphics::measure_text_sized(title, title_font_size).0;
            if full_width <= available_width {
                alloc::string::String::from(title)
            } else {
                let ellipsis = "...";
                let ellipsis_width =
                    crate::graphics::measure_text_sized(ellipsis, title_font_size).0;
                let max_text_width = available_width.saturating_sub(ellipsis_width);

                // binary search: find longest char prefix that fits within max_text_width
                let chars: Vec<char> = title.chars().collect();
                let mut lo = 0usize;
                let mut hi = chars.len();
                while lo < hi {
                    let mid = lo + (hi - lo) / 2;
                    let prefix: String = chars[..mid].iter().collect();
                    let pw = crate::graphics::measure_text_sized(&prefix, title_font_size).0;
                    if pw <= max_text_width {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                // lo is the first index where prefix doesn't fit; lo chars fit
                let cut = lo.min(chars.len());
                let mut result: alloc::string::String = chars[..cut].iter().collect();
                result.push_str(ellipsis);
                result
            }
        };

        canvas.draw_text_sized(
            title_x,
            title_y,
            &display_title,
            title_color,
            title_font_size,
        );

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
        canvas.draw_line(
            nx as i32 - nhalf,
            ny as i32,
            nx as i32 + nhalf,
            ny as i32,
            icon_color,
        );

        // Draw border at bottom of titlebar (matching slint-scarlet)
        let border_color = Color::rgb(180u8, 180u8, 185u8);
        canvas.draw_line(
            0,
            TITLEBAR_HEIGHT as i32 - 1,
            width as i32 - 1,
            TITLEBAR_HEIGHT as i32 - 1,
            border_color,
        );

        // The titlebar is composited above the window background, so it must
        // carry the border segments that overlap its own bounds.
        if width > 0 {
            let outer_border_color = Color::rgb(100u8, 100u8, 105u8);
            canvas.draw_line(0, 0, width as i32 - 1, 0, outer_border_color);
            canvas.draw_line(0, 0, 0, TITLEBAR_HEIGHT as i32 - 1, outer_border_color);
            canvas.draw_line(
                width as i32 - 1,
                0,
                width as i32 - 1,
                TITLEBAR_HEIGHT as i32 - 1,
                outer_border_color,
            );
        }
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
            crate::logln!(
                "[WindowRenderObject] draw_border_canvas: {}x{}",
                width,
                height
            );
        }

        // Match the titlebar outline: one physical-looking stroke, no extra inner highlight.
        let border_color = Color::rgb(100u8, 100u8, 105u8);
        if width == 0 || height == 0 {
            return;
        }

        canvas.draw_rect(0, 0, width, height, border_color);
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
            crate::logln!(
                "[WindowRenderObject::layout] START: constraints=({:?}, {:?}) -> ({:?}, {:?})",
                constraints.min_width,
                constraints.min_height,
                constraints.max_width,
                constraints.max_height
            );
        }

        let size = self.layout(constraints);
        if crate::debug::is_enabled() {
            crate::logln!(
                "[WindowRenderObject::layout] size={}x{}",
                size.width,
                size.height
            );
        }

        let content_layout = WindowContentLayout::new(self.decorated);
        let content_offset = content_layout.offset();
        let decoration_size = content_layout.decoration_size();
        let content_x = content_offset.x;
        let content_y = content_offset.y;
        let content_width = libm::ceilf(size.width - decoration_size.width).max(1.0);
        let content_height = libm::ceilf(size.height - decoration_size.height).max(1.0);

        if crate::debug::is_enabled() {
            crate::logln!(
                "[WindowRenderObject::layout] content_area: x={}, y={}, size={}x{}",
                content_x,
                content_y,
                content_width,
                content_height
            );
        }

        if self.decorated
            && let Some(titlebar) = children.get_mut(0)
        {
            titlebar.layout(LayoutConstraints::tight(size.width, TITLEBAR_HEIGHT as f32));
            titlebar.set_position(Point::ZERO);
        }

        let content_index = if self.decorated { 1 } else { 0 };
        if let Some(child) = children.get_mut(content_index) {
            let child_constraints = LayoutConstraints::loose(content_width, content_height);
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[WindowRenderObject::layout] child_constraints=({:?}, {:?}) -> ({:?}, {:?})",
                    child_constraints.min_width,
                    child_constraints.min_height,
                    child_constraints.max_width,
                    child_constraints.max_height
                );
            }
            let child_size = child.layout(child_constraints);
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[WindowRenderObject::layout] child size={}x{}",
                    child_size.width,
                    child_size.height
                );
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
            crate::logln!(
                "[WindowRenderObject] render: size={}x{}, decorated={}",
                self.size.width,
                self.size.height,
                self.decorated
            );
        }
        self.draw();
        if crate::debug::is_enabled() {
            crate::logln!(
                "[WindowRenderObject] render: complete, buffer={}",
                self.buffer.is_some()
            );
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
