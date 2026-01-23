//! Button View - Interactive button with callback
//!
//! Button displays a label and triggers an action when clicked.

use alloc::string::String;
use alloc::boxed::Box;
use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::Size;
use crate::color::Color;

/// Button click callback type
pub type ButtonCallback = Box<dyn Fn() + 'static>;

/// Button View - displays a clickable button
pub struct Button {
    label: String,
    on_click: Option<ButtonCallback>,
    background_color: Color,
    text_color: Color,
    font_size: f32,
    padding: f32,
}

impl Button {
    /// Create a new Button with the given label
    pub fn new(label: impl Into<String>) -> Self {
        let label_str = label.into();
        Self {
            label: label_str,
            on_click: None,
            background_color: Color::rgb(200.0, 200.0, 200.0),
            text_color: Color::BLACK,
            font_size: 16.0,
            padding: 8.0,
        }
    }

    /// Set the click callback
    pub fn on_click(mut self, callback: impl Fn() + 'static) -> Self {
        self.on_click = Some(Box::new(callback));
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

    /// Get the font size
    pub fn get_font_size(&self) -> f32 {
        self.font_size
    }

    /// Get the padding
    pub fn get_padding(&self) -> f32 {
        self.padding
    }
}

impl View for Button {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            ButtonRenderObject::new(self.label.clone(), self.background_color, self.text_color),
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
    text_color: Color,
    size: Size,
}

impl ButtonRenderObject {
    /// Create a new ButtonRenderObject
    pub fn new(label: String, background_color: Color, text_color: Color) -> Self {
        Self {
            label,
            background_color,
            text_color,
            size: Size::ZERO,
        }
    }

    /// Estimate button size based on label
    fn estimate_size(&self) -> Size {
        let font_size = 16.0; // Default font size
        let char_width = font_size * 0.6;
        let padding = 8.0;

        let text_width = self.label.len() as f32 * char_width;
        let width = text_width + padding * 2.0;
        let height = font_size * 1.2 + padding * 2.0;

        Size { width, height }
    }
}

impl ElementRenderObject for ButtonRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        let intrinsic = self.estimate_size();

        let width = if constraints.max_width > 0.0 {
            intrinsic.width.min(constraints.max_width).max(constraints.min_width)
        } else {
            intrinsic.width
        };

        let height = if constraints.max_height > 0.0 {
            intrinsic.height.min(constraints.max_height).max(constraints.min_height)
        } else {
            intrinsic.height
        };

        self.size = Size { width, height };
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
}
