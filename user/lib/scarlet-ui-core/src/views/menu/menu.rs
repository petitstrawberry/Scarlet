//! Menu - Dropdown menu content
//!
//! Menu displays dropdown menu items vertically.

use crate::buffer::Buffer;
use crate::color::{Color, ColorPalette};
use crate::element::{Element, RenderElement};
use crate::geometry::Size;
use crate::graphics;
use crate::view::View;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

/// Menu item action
#[derive(Clone, Copy)]
pub enum MenuAction {
    /// Separator
    Separator,
    /// Submenu
    Submenu,
}

/// Menu item content
pub struct MenuItemContent {
    label: String,
    action: MenuAction,
    enabled: bool,
    shortcut: Option<String>,
    callback: Option<Arc<dyn Fn() + 'static>>,
}

impl Clone for MenuItemContent {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            action: self.action,
            enabled: self.enabled,
            shortcut: self.shortcut.clone(),
            callback: None, // Callbacks cannot be cloned
        }
    }
}

impl MenuItemContent {
    /// Create a new menu item
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: MenuAction::Separator,
            enabled: true,
            shortcut: None,
            callback: Some(Arc::new(|| {})),
        }
    }

    /// Set the action
    pub fn action(mut self, action: MenuAction) -> Self {
        self.action = action;
        self
    }

    /// Set the callback
    pub fn callback(mut self, callback: impl Fn() + 'static) -> Self {
        self.callback = Some(Arc::new(callback));
        self
    }

    /// Set whether the item is enabled
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the keyboard shortcut
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Create a separator
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            action: MenuAction::Separator,
            enabled: true,
            shortcut: None,
            callback: None,
        }
    }

    /// Get the callback
    pub fn get_callback(&self) -> Option<&Arc<dyn Fn() + 'static>> {
        self.callback.as_ref()
    }
}

/// Menu View - displays dropdown menu items vertically
#[derive(Clone)]
pub struct Menu {
    items: Vec<MenuItemContent>,
    item_height: f32,
    width: f32,
}

impl Menu {
    /// Create a new Menu with the given items
    pub fn new(items: Vec<MenuItemContent>) -> Self {
        Self {
            items,
            item_height: 24.0, // Standard menu item height
            width: 200.0,      // Default menu width
        }
    }

    /// Set the item height
    pub fn item_height(mut self, height: f32) -> Self {
        self.item_height = height;
        self
    }

    /// Set the menu width
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Get the menu items
    pub fn items(&self) -> &[MenuItemContent] {
        &self.items
    }
}

impl View for Menu {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            MenuRenderObject::new(self.items.clone(), self.item_height, self.width),
        ))
    }

    fn listenables(&self) -> Vec<&dyn crate::state::Listenable> {
        Vec::new()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Menu RenderObject - handles vertical layout of menu items
pub struct MenuRenderObject {
    items: Vec<MenuItemContent>,
    item_height: f32,
    width: f32,
    hovered_index: Option<usize>,
    size: Size,
    buffer: Option<Buffer>,
}

impl MenuRenderObject {
    /// Create a new MenuRenderObject
    pub fn new(items: Vec<MenuItemContent>, item_height: f32, width: f32) -> Self {
        let height = items
            .iter()
            .map(|item| {
                if matches!(item.action, MenuAction::Separator) {
                    1.0 // Separator height
                } else {
                    item_height
                }
            })
            .sum::<f32>()
            + 4.0; // Add padding

        Self {
            items,
            item_height,
            width,
            hovered_index: None,
            size: Size { width, height },
            buffer: None,
        }
    }

    /// Set the hovered item index
    pub fn set_hovered(&mut self, index: Option<usize>) {
        self.hovered_index = index;
    }

    /// Get the hovered item index
    pub fn hovered(&self) -> Option<usize> {
        self.hovered_index
    }

    /// Hit test - returns the item index at the given position
    pub fn hit_test(&self, x: f32, y: f32) -> Option<usize> {
        if x < 0.0 || x > self.width || y < 0.0 || y > self.size.height {
            return None;
        }

        let mut current_y = 2.0; // Top padding

        for (i, item) in self.items.iter().enumerate() {
            let item_h = if matches!(item.action, MenuAction::Separator) {
                1.0
            } else {
                self.item_height
            };

            if y >= current_y && y < current_y + item_h {
                return Some(i);
            }

            current_y += item_h;
        }

        None
    }

    /// Invoke the action for the given item
    pub fn invoke_item(&self, index: usize) {
        if let Some(item) = self.items.get(index) {
            if let Some(callback) = item.get_callback() {
                if item.enabled {
                    callback();
                }
            }
        }
    }

    /// Calculate total height
    fn calculate_height(&self) -> f32 {
        self.items
            .iter()
            .map(|item| {
                if matches!(item.action, MenuAction::Separator) {
                    1.0
                } else {
                    self.item_height
                }
            })
            .sum::<f32>()
            + 4.0 // Top and bottom padding
    }
}

impl crate::element::ElementRenderObject for MenuRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        let height = self.calculate_height();
        let width = self.width;

        self.size = Size { width, height };

        // Create buffer for the menu
        let w = libm::ceilf(width) as u32;
        let h = libm::ceilf(height) as u32;

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
        // Render menu to buffer
        if crate::debug::is_enabled() {
            crate::logln!(
                "[MenuRenderObject] render: buffer={}",
                self.buffer.is_some()
            );
        }

        let palette = ColorPalette::default();
        let bg_color = Color::rgb(1.0, 1.0, 1.0); // White background
        let border_color = palette.border();
        let text_color = palette.text_primary();
        let hover_color = palette.menu_hover();
        let separator_color = Color::rgb(0.784, 0.784, 0.784);

        if let Some(ref mut buffer) = self.buffer {
            let mut canvas = graphics::Canvas::for_buffer(buffer);
            let width = canvas.width();
            let height = canvas.height();

            // Fill background
            canvas.fill_rect(0, 0, width, height, bg_color);

            // Draw border
            canvas.draw_rect(0, 0, width, height, border_color);

            // Draw items
            let mut current_y: f32 = 2.0; // Top padding
            let font_size = 13.0;

            for (i, item) in self.items.iter().enumerate() {
                if matches!(item.action, MenuAction::Separator) {
                    // Draw separator line
                    let sep_y = current_y as i32;
                    canvas.draw_line(2, sep_y, (width as i32) - 2, sep_y, separator_color);
                    current_y += 1.0;
                } else {
                    // Draw hover background
                    if self.hovered_index == Some(i) {
                        canvas.fill_rect(
                            2,
                            current_y as i32,
                            width - 4,
                            self.item_height as u32,
                            hover_color,
                        );
                    }

                    // Draw text
                    let text_x = 8;
                    let text_y = current_y as i32 + ((self.item_height as i32 - 16) / 2).max(0);

                    let text_color = if item.enabled {
                        text_color
                    } else {
                        Color::rgb(0.471, 0.471, 0.471) // Disabled text color
                    };

                    canvas.draw_text_sized(text_x, text_y, &item.label, text_color, font_size);

                    // Draw shortcut if present
                    if let Some(ref shortcut) = item.shortcut {
                        let (shortcut_w, _) = graphics::measure_text_sized(shortcut, font_size);
                        let shortcut_x = (width as i32) - shortcut_w as i32 - 8;
                        canvas.draw_text_sized(shortcut_x, text_y, shortcut, text_color, font_size);
                    }

                    current_y += self.item_height;
                }
            }
        }
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn clear_buffer(&mut self) {
        self.buffer = None;
    }
}
