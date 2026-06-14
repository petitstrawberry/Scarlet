//! Button View - Interactive button with callback
//!
//! Button displays a label and triggers an action when clicked.

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

/// Button click callback type
pub type ButtonCallback = Box<dyn Fn() + 'static>;

/// Button View - displays a clickable button
#[derive(Clone)]
pub struct Button {
    label: String,
    on_click: Option<Arc<dyn Fn() + 'static>>,
    background_color: Color,
    border_color: Color,
    text_color: Color,
    font_size: f32,
    padding: f32,
}

impl Button {
    /// Create a new Button with the given label
    pub fn new(label: impl Into<String>) -> Self {
        let label_str = label.into();
        let palette = ColorPalette::default();
        Self {
            label: label_str,
            on_click: None,
            background_color: palette.button_background(),
            border_color: palette.border(),
            text_color: palette.text_primary(),
            font_size: 15.0,
            padding: 10.0,
        }
    }

    /// Set the click callback
    pub fn on_click(mut self, callback: impl Fn() + 'static) -> Self {
        self.on_click = Some(Arc::new(callback));
        self
    }

    /// Set the background color
    pub fn background_color(mut self, color: Color) -> Self {
        self.background_color = color;
        self
    }

    /// Set the text color
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    /// Set the border color
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = color;
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

    /// Get the button label
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Get the background color
    pub fn get_background_color(&self) -> Color {
        self.background_color
    }

    /// Get the text color
    pub fn get_text_color(&self) -> Color {
        self.text_color
    }

    /// Get the border color
    pub fn get_border_color(&self) -> Color {
        self.border_color
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
}

impl View for Button {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            ButtonRenderObject::new(
                self.label.clone(),
                self.background_color,
                self.border_color,
                self.text_color,
                self.font_size,
                self.padding,
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

/// Button RenderObject - handles button rendering and interaction
pub struct ButtonRenderObject {
    label: String,
    background_color: Color,
    border_color: Color,
    text_color: Color,
    font_size: f32,
    padding: f32,
    hovered: bool,
    pressed: bool,
    size: Size,
    buffer: Option<Buffer>,
}

impl ButtonRenderObject {
    /// Create a new ButtonRenderObject
    pub fn new(
        label: String,
        background_color: Color,
        border_color: Color,
        text_color: Color,
        font_size: f32,
        padding: f32,
    ) -> Self {
        Self {
            label,
            background_color,
            border_color,
            text_color,
            font_size,
            padding,
            hovered: false,
            pressed: false,
            size: Size::ZERO,
            buffer: None,
        }
    }

    /// Estimate button size based on label
    fn estimate_size(&self) -> Size {
        let char_width = self.font_size * 0.6;

        let text_width = self.label.len() as f32 * char_width;
        let width = text_width + self.padding * 2.0;
        let height = self.font_size * 1.2 + self.padding * 2.0;

        Size { width, height }
    }

    pub fn set_hovered(&mut self, hovered: bool) {
        self.hovered = hovered;
    }

    pub fn set_pressed(&mut self, pressed: bool) {
        self.pressed = pressed;
    }

    pub fn is_pressed(&self) -> bool {
        self.pressed
    }

    fn shade_color(color: Color, factor: f32) -> Color {
        let clamp = |v: f32| v.clamp(0.0, 1.0);
        Color {
            r: clamp(color.r * factor),
            g: clamp(color.g * factor),
            b: clamp(color.b * factor),
            a: color.a,
        }
    }

    fn current_background(&self) -> Color {
        if self.pressed {
            Self::shade_color(self.background_color, 0.92)
        } else if self.hovered {
            Self::shade_color(self.background_color, 0.97)
        } else {
            self.background_color
        }
    }

    fn current_border(&self) -> Color {
        if self.pressed {
            Self::shade_color(self.border_color, 0.90)
        } else if self.hovered {
            Self::shade_color(self.border_color, 0.96)
        } else {
            self.border_color
        }
    }

    fn highlight_color(&self, background: Color) -> Color {
        Self::shade_color(background, 1.06)
    }
}

impl ElementRenderObject for ButtonRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        let intrinsic = self.estimate_size();

        if crate::debug::is_enabled() {
            crate::logln!(
                "[ButtonRenderObject] layout: label='{}' intrinsic={:?}, constraints={:?}",
                self.label,
                intrinsic,
                constraints
            );
        }

        // For buttons, use the intrinsic size, but constrain within bounds
        // Buttons should NOT expand to fill min_width/min_height
        let mut width = intrinsic.width;
        let mut height = intrinsic.height;

        // Apply max constraints (don't exceed maximum)
        if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
            width = width.min(constraints.max_width);
        }
        if constraints.max_height.is_finite() && constraints.max_height > 0.0 {
            height = height.min(constraints.max_height);
        }

        // Don't expand to min - buttons should stay at their intrinsic size

        self.size = Size { width, height };

        // Create buffer for this button
        let w = libm::ceilf(width) as u32;
        let h = libm::ceilf(height) as u32;

        if crate::debug::is_enabled() {
            crate::logln!(
                "[ButtonRenderObject] layout: final size={}x{}, buffer needed={} bytes",
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
        // Render button to buffer
        if crate::debug::is_enabled() {
            crate::logln!(
                "[ButtonRenderObject] render: label='{}', buffer={}",
                self.label,
                self.buffer.is_some()
            );
        }
        let background = self.current_background();
        let border = self.current_border();
        let highlight = self.highlight_color(background);
        if let Some(ref mut buffer) = self.buffer {
            let mut canvas = graphics::Canvas::for_buffer(buffer);
            let width = canvas.width();
            let height = canvas.height();

            // Fill background
            canvas.fill_rect(0, 0, width, height, background);

            // Subtle top highlight for a modern surface feel
            if !self.pressed && height > 2 {
                canvas.draw_line(1, 1, (width as i32) - 2, 1, highlight);
            }

            // Border
            canvas.draw_rect(0, 0, width, height, border);

            // Draw text centered
            let (text_w, _text_h) = graphics::measure_text_sized(&self.label, self.font_size);
            let x = ((width as i32) - (text_w as i32)) / 2;
            let y = ((height as i32) - (self.font_size as i32 * 6 / 5)) / 2;

            canvas.draw_text_sized(
                x.max(0),
                y.max(0),
                &self.label,
                self.text_color,
                self.font_size,
            );
        }
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn clear_buffer(&mut self) {
        self.buffer = None;
    }
}
