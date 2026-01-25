//! Toggle View - On/off switch control
//!
//! Toggle is a switch control that can be on or off.

use alloc::boxed::Box;
use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::Size;
use crate::color::Color;
use crate::buffer::Buffer;
use crate::graphics;
use crate::state::State;
use alloc::vec::Vec;

/// Toggle View - on/off switch
#[derive(Clone)]
pub struct Toggle {
    is_on: State<bool>,
}

impl Toggle {
    /// Create a new Toggle
    pub fn new(is_on: State<bool>) -> Self {
        Self { is_on }
    }

    /// Get the is_on state
    pub fn get_is_on(&self) -> &State<bool> {
        &self.is_on
    }
}

impl View for Toggle {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            ToggleRenderObject::new(self.is_on.get()),
        ))
    }

    fn listenables(&self) -> Vec<&dyn crate::state::Listenable> {
        alloc::vec![&self.is_on]
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Toggle RenderObject
///
/// Design matching iOS/Android switch:
/// - Width: 51px, Height: 31px
/// - On: Green background (#34C759)
/// - Off: Gray background (#767680)
/// - Thumb: White circle, 27px diameter
/// - Corner radius: 15.5px (half height)
pub struct ToggleRenderObject {
    is_on: bool,
    size: Size,
    buffer: Option<Buffer>,
}

impl ToggleRenderObject {
    /// Create a new ToggleRenderObject
    pub fn new(is_on: bool) -> Self {
        // iOS-style toggle dimensions
        const TOGGLE_WIDTH: f32 = 51.0;
        const TOGGLE_HEIGHT: f32 = 31.0;

        Self {
            is_on,
            size: Size::new(TOGGLE_WIDTH, TOGGLE_HEIGHT),
            buffer: None,
        }
    }

    /// Get is_on state
    pub fn get_is_on(&self) -> bool {
        self.is_on
    }

    /// Set is_on state
    pub fn set_is_on(&mut self, is_on: bool) {
        self.is_on = is_on;
    }

    /// Draw toggle using Canvas API (iOS-style design)
    fn draw_toggle(&mut self) {
        let width = libm::ceilf(self.size.width) as usize;
        let height = libm::ceilf(self.size.height) as usize;
        let needed = width * height;

        // Create or resize buffer
        if self.buffer.as_ref().map_or(true, |b| b.as_slice().len() < needed) {
            self.buffer = Some(Buffer::from_dimensions(width as u32, height as u32));
        }

        if let Some(ref mut buffer) = self.buffer {
            let mut canvas = graphics::Canvas::new(buffer.data_mut(), width as u32, height as u32);

            // iOS-style toggle colors
            let bg_color = if self.is_on {
                Color::rgb(52u8, 199u8, 89u8) // iOS green: #34C759
            } else {
                Color::rgb(118u8, 118u8, 128u8) // iOS gray: #767680
            };

            // Draw background (for now, just a regular rect)
            canvas.fill_rect(0, 0, width as u32, height as u32, bg_color);

            // Thumb position: on = right side, off = left side
            let thumb_diameter = self.size.height - 4.0; // 27px for 31px height
            let thumb_offset = if self.is_on {
                self.size.width - self.size.height // Right side
            } else {
                0.0 // Left side
            };
            let thumb_x = (thumb_offset + 2.0) as i32; // +2 for margin
            let thumb_y = 2.0 as i32; // +2 for margin
            let thumb_size = libm::ceilf(thumb_diameter) as u32;

            // Draw thumb (white circle)
            let thumb_color = Color::WHITE;
            canvas.fill_rect(thumb_x, thumb_y, thumb_size, thumb_size, thumb_color);
        }
    }
}

impl ElementRenderObject for ToggleRenderObject {
    fn layout(&mut self, _constraints: crate::element::LayoutConstraints) -> Size {
        // Toggle has fixed size (51x31), ignore constraints
        let width = self.size.width;  // 51.0
        let height = self.size.height; // 31.0

        self.size = Size { width, height };

        // Create buffer
        let w = libm::ceilf(width) as u32;
        let h = libm::ceilf(height) as u32;
        let needed = (w * h * 4) as usize;

        if self.buffer.as_ref().map_or(true, |b| b.data().len() < needed) {
            self.buffer = Some(Buffer::from_dimensions(w, h));
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
        self.draw_toggle();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn update(&mut self, new_view: &dyn crate::view::View) -> crate::element::UpdateResult {
        if let Some(toggle) = new_view.as_any().downcast_ref::<Toggle>() {
            let new_is_on = toggle.is_on.get();
            if self.is_on != new_is_on {
                self.is_on = new_is_on;
                crate::element::UpdateResult::Updated
            } else {
                crate::element::UpdateResult::NoChange
            }
        } else {
            crate::element::UpdateResult::Replaced
        }
    }
}
