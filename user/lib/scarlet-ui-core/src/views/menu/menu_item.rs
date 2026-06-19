//! MenuItem - Individual menu bar item
//!
//! MenuItem represents a single menu item in the menu bar (e.g., "File", "Edit").
//! When clicked, it can show a dropdown menu.

use crate::buffer::Buffer;
use crate::color::{Color, ColorPalette};
use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::geometry::Size;
use crate::graphics;
use crate::view::View;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::any::Any;

/// Menu item click callback type
pub type MenuItemCallback = Box<dyn Fn() + 'static>;

/// MenuItem View - displays a clickable menu bar item
#[derive(Clone)]
pub struct MenuItem {
    label: String,
    on_click: Option<Arc<dyn Fn() + 'static>>,
    on_hover: Option<Arc<dyn Fn() + 'static>>,
    font_size: f32,
    padding: f32,
    selected: bool,
}

impl MenuItem {
    /// Create a new MenuItem with the given label
    pub fn new(label: impl Into<String>) -> Self {
        let label_str = label.into();
        Self {
            label: label_str,
            on_click: None,
            on_hover: None,
            font_size: 18.0,
            padding: 8.0,
            selected: false,
        }
    }

    /// Set the click callback
    pub fn on_click(mut self, callback: impl Fn() + 'static) -> Self {
        self.on_click = Some(Arc::new(callback));
        self
    }

    /// Set the hover callback
    pub fn on_hover(mut self, callback: impl Fn() + 'static) -> Self {
        self.on_hover = Some(Arc::new(callback));
        self
    }

    /// Set the font size
    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    /// Set the padding
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = padding;
        self
    }

    /// Set the selected state
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Get the menu item label
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Get the font size
    pub fn get_font_size(&self) -> f32 {
        self.font_size
    }

    /// Get the padding
    pub fn get_padding(&self) -> f32 {
        self.padding
    }

    /// Invoke the click callback if present
    pub fn invoke_on_click(&self) {
        if let Some(callback) = self.on_click.as_ref() {
            callback();
        }
    }

    /// Invoke the hover callback if present
    pub fn invoke_on_hover(&self) {
        if let Some(callback) = self.on_hover.as_ref() {
            callback();
        }
    }

    /// Get selected state
    pub fn is_selected(&self) -> bool {
        self.selected
    }
}

impl View for MenuItem {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            MenuItemRenderObject::new(
                self.label.clone(),
                self.font_size,
                self.padding,
                self.selected,
            ),
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        alloc::vec::Vec::new()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// MenuItem RenderObject - handles menu item rendering and interaction
pub struct MenuItemRenderObject {
    label: String,
    font_size: f32,
    padding: f32,
    selected: bool,
    hovered: bool,
    pressed: bool,
    size: Size,
    buffer: Option<Buffer>,
}

impl MenuItemRenderObject {
    /// Create a new MenuItemRenderObject
    pub fn new(label: String, font_size: f32, padding: f32, selected: bool) -> Self {
        Self {
            label,
            font_size,
            padding,
            selected,
            hovered: false,
            pressed: false,
            size: Size::ZERO,
            buffer: None,
        }
    }

    /// Estimate menu item size based on label
    fn estimate_size(&self) -> Size {
        let (text_w, text_h) = graphics::measure_text_sized(&self.label, self.font_size);
        let width = text_w as f32 + self.padding * 2.0;
        let height = text_h as f32 + self.padding * 2.0;

        Size { width, height }
    }

    pub fn set_hovered(&mut self, hovered: bool) {
        self.hovered = hovered;
    }

    pub fn set_pressed(&mut self, pressed: bool) {
        self.pressed = pressed;
    }

    pub fn set_selected(&mut self, selected: bool) {
        self.selected = selected;
    }

    pub fn is_pressed(&self) -> bool {
        self.pressed
    }

    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    fn current_background(&self) -> Color {
        let palette = ColorPalette::default();
        if self.pressed || self.selected {
            palette.menu_active()
        } else if self.hovered {
            palette.menu_hover()
        } else {
            Color::rgba(0.0, 0.0, 0.0, 0.0) // Transparent
        }
    }
}

impl ElementRenderObject for MenuItemRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        let intrinsic = self.estimate_size();

        if crate::debug::is_enabled() {
            crate::logln!(
                "[MenuItemRenderObject] layout: label='{}' intrinsic={:?}, constraints={:?}",
                self.label,
                intrinsic,
                constraints
            );
        }

        // For menu items, use the intrinsic size, but constrain within bounds
        let mut width = intrinsic.width;
        let mut height = intrinsic.height;

        // Apply min/max constraints
        if constraints.min_width.is_finite() && constraints.min_width > 0.0 {
            width = width.max(constraints.min_width);
        }
        if constraints.min_height.is_finite() && constraints.min_height > 0.0 {
            height = height.max(constraints.min_height);
        }
        if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
            width = width.min(constraints.max_width);
        }
        if constraints.max_height.is_finite() && constraints.max_height > 0.0 {
            height = height.min(constraints.max_height);
        }

        self.size = Size { width, height };

        // Create buffer for this menu item
        let w = libm::ceilf(width) as u32;
        let h = libm::ceilf(height) as u32;

        if crate::debug::is_enabled() {
            crate::logln!(
                "[MenuItemRenderObject] layout: final size={}x{}, buffer needed={} bytes",
                w,
                h,
                w * h * 4
            );
        }

        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.logical_width() != w || b.logical_height() != h);
        if needs_resize {
            self.buffer = Some(Buffer::from_logical_dimensions(w, h));
        }

        self.size
    }

    fn update(&mut self, new_view: &dyn crate::view::View) -> crate::element::UpdateResult {
        if let Some(view) = new_view.as_any().downcast_ref::<MenuItem>() {
            self.label = view.label.clone();
            self.font_size = view.font_size;
            self.padding = view.padding;
            self.selected = view.selected;
            crate::element::UpdateResult::Updated
        } else {
            crate::element::UpdateResult::Replaced
        }
    }

    fn size(&self) -> Size {
        self.size
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn render(&mut self) {
        // Render menu item to buffer
        if crate::debug::is_enabled() {
            crate::logln!(
                "[MenuItemRenderObject] render: label='{}', buffer={}",
                self.label,
                self.buffer.is_some()
            );
        }
        let background = self.current_background();
        let palette = ColorPalette::default();
        let text_color = palette.text_primary();

        if let Some(ref mut buffer) = self.buffer {
            let mut canvas = graphics::Canvas::for_buffer(buffer);
            let width = canvas.width();
            let height = canvas.height();

            // Clear to avoid blending text on top of previous frames.
            canvas.fill_rect(0, 0, width, height, Color::TRANSPARENT);

            // Fill background (only if hovered or pressed)
            if self.hovered || self.pressed {
                canvas.fill_rect(0, 0, width, height, background);
            }

            // Draw text centered
            let (text_w, _text_h) = graphics::measure_text_sized(&self.label, self.font_size);
            let x = ((width as i32) - (text_w as i32)) / 2;
            let y = ((height as i32) - (self.font_size as i32 * 6 / 5)) / 2;

            canvas.draw_text_sized(x.max(0), y.max(0), &self.label, text_color, self.font_size);
        }
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn clear_buffer(&mut self) {
        self.buffer = None;
    }
}
