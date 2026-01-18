//! Context menu implementation
//!
//! A context menu is a popup menu that appears at a specific location
//! (typically where the user right-clicked) and contains a list of actions.

use super::traits::{View, Size};
use crate::graphics::{Canvas, Rect};
use crate::{Color};
use crate::event::{Event, EventKind, MouseButton};
use scarlet_std::vec::Vec;
use scarlet_std::string::String;
use scarlet_std::rc::Rc;

/// Action callback type for menu items
type MenuAction = Rc<dyn Fn() + Send + Sync>;

/// A single menu item in a context menu
pub struct MenuItem {
    text: String,
    action: MenuAction,
    is_separator: bool,
}

impl MenuItem {
    /// Create a new menu item with text and action
    pub fn new<F>(text: impl Into<String>, action: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        Self {
            text: text.into(),
            action: Rc::new(action),
            is_separator: false,
        }
    }

    /// Create a separator item (visual divider)
    pub fn separator() -> Self {
        Self {
            text: String::new(),
            action: Rc::new(|| {}),
            is_separator: true,
        }
    }

    /// Get the menu item text
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Execute the menu item action
    pub fn execute(&self) {
        (self.action)();
    }

    /// Check if this is a separator
    pub fn is_separator(&self) -> bool {
        self.is_separator
    }
}

/// Context menu - a popup menu with items
///
/// Context menus appear at specific screen coordinates and show a list
/// of actions. They're typically triggered by right-clicking.
pub struct ContextMenu {
    items: Vec<MenuItem>,
    is_visible: bool,
    position: (i32, i32),
    hovered_index: Option<usize>,
    item_height: u32,
    min_width: u32,
    padding: u32,
    background_color: Color,
    text_color: Color,
    hover_color: Color,
    separator_color: Color,
    cached_size: Size,
}

impl ContextMenu {
    /// Create a new context menu with the given items
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            is_visible: false,
            position: (0, 0),
            hovered_index: None,
            item_height: 28,
            min_width: 200,
            padding: 8,
            background_color: Color::rgb(50, 50, 50),
            text_color: Color::rgb(220, 220, 220),
            hover_color: Color::rgb(0, 122, 255),
            separator_color: Color::rgb(80, 80, 80),
            cached_size: Size::ZERO,
        }
    }

    /// Add a menu item
    pub fn item(mut self, item: MenuItem) -> Self {
        self.items.push(item);
        self
    }

    /// Add multiple menu items
    pub fn items(mut self, items: Vec<MenuItem>) -> Self {
        self.items.extend(items);
        self
    }

    /// Set menu visibility
    pub fn visible(mut self, visible: bool) -> Self {
        self.is_visible = visible;
        self
    }

    /// Set menu position (screen coordinates)
    pub fn position(mut self, x: i32, y: i32) -> Self {
        self.position = (x, y);
        self
    }

    /// Set item height
    pub fn item_height(mut self, height: u32) -> Self {
        self.item_height = height;
        self
    }

    /// Set minimum menu width
    pub fn min_width(mut self, width: u32) -> Self {
        self.min_width = width;
        self
    }

    /// Set padding around menu content
    pub fn padding(mut self, padding: u32) -> Self {
        self.padding = padding;
        self
    }

    /// Set background color
    pub fn background(mut self, color: Color) -> Self {
        self.background_color = color;
        self
    }

    /// Set text color
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    /// Set hover background color
    pub fn hover_color(mut self, color: Color) -> Self {
        self.hover_color = color;
        self
    }

    /// Show the menu at the specified position
    pub fn show(&mut self, x: i32, y: i32) {
        self.is_visible = true;
        self.position = (x, y);
        self.hovered_index = None;
    }

    /// Hide the menu
    pub fn hide(&mut self) {
        self.is_visible = false;
        self.hovered_index = None;
    }

    /// Check if menu is currently visible
    pub fn is_visible(&self) -> bool {
        self.is_visible
    }

    /// Get the menu position
    pub fn get_position(&self) -> (i32, i32) {
        self.position
    }
}

impl Default for ContextMenu {
    fn default() -> Self {
        Self::new()
    }
}

impl View for ContextMenu {
    fn layout(&mut self, available: Size) -> Size {
        if !self.is_visible || self.items.is_empty() {
            self.cached_size = Size::ZERO;
            return Size::ZERO;
        }

        // Width is max of min_width and available width
        let width = self.min_width.min(available.width);
        let separator_count = self.items.iter().filter(|i| i.is_separator()).count();
        let item_count = self.items.len();
        let separator_height = 3; // Thinner than regular items

        let height = (item_count as u32 * self.item_height
            - separator_count as u32 * (self.item_height - separator_height)
            + self.padding * 2)
            .min(available.height);

        let size = Size::new(width, height);
        self.cached_size = size;
        size
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        if !self.is_visible || self.items.is_empty() {
            return;
        }

        // Draw menu background
        canvas.fill_rect(
            frame.x,
            frame.y,
            frame.width,
            frame.height,
            self.background_color,
        );

        // Draw menu items
        let mut y = frame.y + self.padding as i32;
        for (index, item) in self.items.iter().enumerate() {
            if item.is_separator() {
                // Draw separator line
                let separator_y = y + self.item_height as i32 / 2;
                canvas.fill_rect(
                    frame.x + 4,
                    separator_y,
                    frame.width - 8,
                    1,
                    self.separator_color,
                );
                y += self.item_height as i32;
            } else {
                // Check if hovered
                let is_hovered = self.hovered_index == Some(index);

                // Draw hover background
                if is_hovered {
                    canvas.fill_rect(
                        frame.x + 2,
                        y + 2,
                        frame.width - 4,
                        self.item_height - 4,
                        self.hover_color,
                    );
                }

                // Draw text
                let text_x = frame.x + self.padding as i32 + 4;
                let text_y = y + (self.item_height as i32 - 14) / 2; // Center text vertically (assuming ~14px font)
                canvas.draw_text(text_x, text_y, &item.text, self.text_color);

                y += self.item_height as i32;
            }
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        if !self.is_visible {
            return false;
        }

        let x = event.x();
        let y = event.y();
        let local_x = x - frame.x;
        let local_y = y - frame.y;

        match event.kind {
            EventKind::MouseMove => {
                // Update hovered item
                let mut new_hovered = None;
                if local_x >= 0 && local_x < frame.width as i32
                    && local_y >= 0 && local_y < frame.height as i32
                {
                    let mut current_y = self.padding as i32;
                    for (index, item) in self.items.iter().enumerate() {
                        if !item.is_separator()
                            && local_y >= current_y
                            && local_y < current_y + self.item_height as i32
                        {
                            new_hovered = Some(index);
                            break;
                        }
                        current_y += self.item_height as i32;
                    }
                }
                self.hovered_index = new_hovered;
                false
            }

            EventKind::MouseDown { button: MouseButton::Left } => {
                // Check if clicked on an item
                if let Some(index) = self.hovered_index {
                    if index < self.items.len() {
                        self.items[index].execute();
                        self.hide();
                        event.stop_propagation();
                        return true;
                    }
                }

                // Click outside menu closes it
                if !frame.contains(x, y) {
                    self.hide();
                    return false;
                }

                false
            }

            _ => false,
        }
    }

    fn needs_draw(&self) -> bool {
        self.is_visible
    }

    fn set_needs_draw(&mut self) {
        // Context menu redraws when visible
    }

    fn clear_needs_draw(&mut self) {
        // No-op for context menu
    }
}
