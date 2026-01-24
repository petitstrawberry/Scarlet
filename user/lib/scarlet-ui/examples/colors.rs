//! Color Palette Example - Demonstrates color system
//!
//! Shows various colors and rectangles.

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use scarlet_ui::prelude::*;
use scarlet_ui::{View, Application, Window, Rectangle};
use scarlet_ui::geometry::Size;
use scarlet_ui::color::Color;

/// Color Palette Application
struct ColorPaletteApp;

impl View for ColorPaletteApp {
    fn create_element(&self) -> Box<dyn scarlet_ui::element::Element> {
        Window::new("Color Palette", Rectangle::new().fill(Color::WHITE))
            .create_element()
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn scarlet_ui::state::Listenable> {
        alloc::vec::Vec::new()
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

impl Application for ColorPaletteApp {
    fn body(&self) -> impl View {
        Window::new("Color Palette", Rectangle::new().fill(Color::WHITE))
            .size(Size::new(600.0, 400.0))
    }
}

#[no_mangle]
pub extern "C" fn main() {
    let mut app = ColorPaletteApp;
    let _ = app.run();
}
