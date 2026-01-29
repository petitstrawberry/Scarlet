//! NavigationViewRenderObject - Runtime rendering and event handling for NavigationView
//!
//! This module provides the RenderObject for NavigationView which handles:
//! - Sidebar rendering with selection highlights
/// - Layout of sidebar and content areas
/// - Mouse event handling for item selection and hover

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::String;
use core::any::Any;
use libm;
use crate::element::{Element, ElementRenderObject, LayoutConstraints};
use crate::geometry::{Size, Point};
use crate::color::Color;
use crate::buffer::Buffer;
use crate::state::State;
use crate::graphics;
use crate::views::navigation::link::Icon;
use crate::color::ColorPalette;

/// NavigationView RenderObject - handles rendering and layout
///
/// This render object manages the navigation sidebar and content area layout.
pub struct NavigationViewRenderObject {
    /// Number of navigation links
    link_count: usize,
    /// Labels for each link
    labels: Vec<String>,
    /// Icons for each link
    icons: Vec<Icon>,
    /// Currently selected link index
    selected_index: State<usize>,
    /// Width of the sidebar (fixed)
    sidebar_width: f32,
    /// Currently hovered link index (if any)
    hovered_index: Option<usize>,
    /// Height of each navigation item
    item_height: f32,
    /// Total size of the NavigationView
    size: Size,
    /// Buffer for rendering the sidebar
    buffer: Option<Buffer>,
    /// Font size for labels
    font_size: f32,
    /// Icon size
    icon_size: u32,
    /// Padding for items
    item_padding: f32,
}

impl NavigationViewRenderObject {
    /// Create a new NavigationViewRenderObject
    pub fn new(
        labels: Vec<String>,
        icons: Vec<Icon>,
        selected_index: State<usize>,
        sidebar_width: f32,
    ) -> Self {
        let link_count = labels.len();
        Self {
            link_count,
            labels,
            icons,
            selected_index,
            sidebar_width,
            hovered_index: None,
            item_height: 40.0,
            size: Size::ZERO,
            buffer: None,
            font_size: 14.0,
            icon_size: 16,
            item_padding: 8.0,
        }
    }

    /// Get the currently hovered index
    pub fn hovered_index(&self) -> Option<usize> {
        self.hovered_index
    }

    /// Set the hovered index
    pub fn set_hovered_index(&mut self, index: Option<usize>) {
        self.hovered_index = index;
    }

    /// Get the selected index state reference
    pub fn selected_index(&self) -> &State<usize> {
        &self.selected_index
    }

    /// Get the sidebar width
    pub fn sidebar_width(&self) -> f32 {
        self.sidebar_width
    }

    /// Calculate the Y position for a given item index
    pub fn item_y(&self, index: usize) -> f32 {
        index as f32 * self.item_height
    }

    /// Get the index for a given Y position
    pub fn index_at_y(&self, y: f32) -> Option<usize> {
        if y >= 0.0 && y < self.link_count as f32 * self.item_height {
            Some((y / self.item_height) as usize)
        } else {
            None
        }
    }

    /// Get the number of links
    pub fn link_count(&self) -> usize {
        self.link_count
    }

    /// Render an icon at the given position
    fn render_icon(&self, canvas: &mut graphics::Canvas, icon: &Icon, x: i32, y: i32, color: Color) {
        let size = self.icon_size as i32;

        match icon {
            Icon::Home => {
                // Draw house shape
                // Roof (triangle)
                let center_x = x + size / 2;
                let roof_top = y + 2;
                let roof_bottom = y + size / 2 + 2;
                canvas.draw_line(center_x - size / 2, roof_bottom, center_x, roof_top, color);
                canvas.draw_line(center_x, roof_top, center_x + size / 2, roof_bottom, color);

                // Body (rectangle)
                let body_left = center_x - size / 3;
                let body_right = center_x + size / 3;
                let body_bottom = y + size - 2;
                canvas.draw_line(body_left, roof_bottom, body_left, body_bottom, color);
                canvas.draw_line(body_right, roof_bottom, body_right, body_bottom, color);
                canvas.draw_line(body_left, body_bottom, body_right, body_bottom, color);
            }
            Icon::Settings => {
                // Draw gear shape (simplified as circle with spokes)
                let center_x = x + size / 2;
                let center_y = y + size / 2;
                let radius = size / 2 - 2;

                // Draw circle (approximate with lines)
                let num_segments = 8;
                for i in 0..num_segments {
                    let angle1 = (i as f32 * 2.0 * core::f32::consts::PI) / num_segments as f32;
                    let angle2 = ((i + 1) as f32 * 2.0 * core::f32::consts::PI) / num_segments as f32;

                    let x1 = center_x as f32 + radius as f32 * libm::cosf(angle1);
                    let y1 = center_y as f32 + radius as f32 * libm::sinf(angle1);
                    let x2 = center_x as f32 + radius as f32 * libm::cosf(angle2);
                    let y2 = center_y as f32 + radius as f32 * libm::sinf(angle2);

                    canvas.draw_line(x1 as i32, y1 as i32, x2 as i32, y2 as i32, color);
                }

                // Draw spokes
                canvas.draw_line(center_x, center_y - radius, center_x, center_y + radius, color);
                canvas.draw_line(center_x - radius, center_y, center_x + radius, center_y, color);
            }
            Icon::Info => {
                // Draw circle with 'i'
                let center_x = x + size / 2;
                let center_y = y + size / 2;
                let radius = size / 2 - 2;

                // Draw circle
                let num_segments = 12;
                for i in 0..num_segments {
                    let angle1 = (i as f32 * 2.0 * core::f32::consts::PI) / num_segments as f32;
                    let angle2 = ((i + 1) as f32 * 2.0 * core::f32::consts::PI) / num_segments as f32;

                    let x1 = center_x as f32 + radius as f32 * libm::cosf(angle1);
                    let y1 = center_y as f32 + radius as f32 * libm::sinf(angle1);
                    let x2 = center_x as f32 + radius as f32 * libm::cosf(angle2);
                    let y2 = center_y as f32 + radius as f32 * libm::sinf(angle2);

                    canvas.draw_line(x1 as i32, y1 as i32, x2 as i32, y2 as i32, color);
                }

                // Draw 'i' dot (small circle)
                let dot_radius = 1;
                canvas.draw_line(center_x, center_y - 2, center_x + 1, center_y - 2, color);

                // Draw 'i' line
                canvas.draw_line(center_x, center_y, center_x, center_y + radius / 2, color);
            }
            Icon::Search => {
                // Draw magnifying glass
                let center_x = x + size / 2 - 2;
                let center_y = y + size / 2 - 2;
                let radius = size / 3;

                // Draw circle
                let num_segments = 12;
                for i in 0..num_segments {
                    let angle1 = (i as f32 * 2.0 * core::f32::consts::PI) / num_segments as f32;
                    let angle2 = ((i + 1) as f32 * 2.0 * core::f32::consts::PI) / num_segments as f32;

                    let x1 = center_x as f32 + radius as f32 * libm::cosf(angle1);
                    let y1 = center_y as f32 + radius as f32 * libm::sinf(angle1);
                    let x2 = center_x as f32 + radius as f32 * libm::cosf(angle2);
                    let y2 = center_y as f32 + radius as f32 * libm::sinf(angle2);

                    canvas.draw_line(x1 as i32, y1 as i32, x2 as i32, y2 as i32, color);
                }

                // Draw handle
                let handle_start_x = center_x + (radius as f32 * 0.707) as i32;
                let handle_start_y = center_y + (radius as f32 * 0.707) as i32;
                let handle_end_x = handle_start_x + 4;
                let handle_end_y = handle_start_y + 4;
                canvas.draw_line(handle_start_x, handle_start_y, handle_end_x, handle_end_y, color);
            }
            Icon::User => {
                // Draw person shape
                let center_x = x + size / 2;

                // Head (circle)
                let head_center_y = y + size / 3;
                let head_radius = size / 5;

                let num_segments = 8;
                for i in 0..num_segments {
                    let angle1 = (i as f32 * 2.0 * core::f32::consts::PI) / num_segments as f32;
                    let angle2 = ((i + 1) as f32 * 2.0 * core::f32::consts::PI) / num_segments as f32;

                    let x1 = center_x as f32 + head_radius as f32 * libm::cosf(angle1);
                    let y1 = head_center_y as f32 + head_radius as f32 * libm::sinf(angle1);
                    let x2 = center_x as f32 + head_radius as f32 * libm::cosf(angle2);
                    let y2 = head_center_y as f32 + head_radius as f32 * libm::sinf(angle2);

                    canvas.draw_line(x1 as i32, y1 as i32, x2 as i32, y2 as i32, color);
                }

                // Body (rounded rectangle, simplified as arc)
                let body_top = head_center_y + head_radius + 1;
                let body_bottom = y + size - 2;
                let body_width = size / 2 + 2;

                canvas.draw_line(center_x - body_width / 2, body_bottom, center_x + body_width / 2, body_bottom, color);
                canvas.draw_line(center_x - body_width / 2, body_top, center_x - body_width / 2, body_bottom, color);
                canvas.draw_line(center_x + body_width / 2, body_top, center_x + body_width / 2, body_bottom, color);

                // Shoulders (arc approximation)
                canvas.draw_line(center_x - body_width / 2, body_top, center_x - 2, body_top - 2, color);
                canvas.draw_line(center_x + body_width / 2, body_top, center_x + 2, body_top - 2, color);
            }
            Icon::File => {
                // Draw document shape
                let left = x + 3;
                let right = x + size - 3;
                let top = y + 2;
                let bottom = y + size - 2;

                // Rectangle
                canvas.draw_line(left, top, right, top, color);
                canvas.draw_line(right, top, right, bottom, color);
                canvas.draw_line(right, bottom, left, bottom, color);
                canvas.draw_line(left, bottom, left, top, color);

                // Folded corner
                canvas.draw_line(right - 3, top, right, top + 3, color);

                // Lines for text
                let line_spacing = 3;
                let mut line_y = top + 5;
                while line_y < bottom - 3 {
                    canvas.draw_line(left + 3, line_y, right - 3, line_y, color);
                    line_y += line_spacing;
                }
            }
            Icon::Folder => {
                // Draw folder shape
                let left = x + 2;
                let right = x + size - 2;
                let top = y + 4;
                let bottom = y + size - 2;

                // Tab
                let tab_right = left + (size / 3);
                canvas.draw_line(left, top - 2, tab_right, top - 2, color);
                canvas.draw_line(tab_right, top - 2, tab_right + 2, top, color);

                // Back
                canvas.draw_line(left, top - 2, left, bottom, color);
                canvas.draw_line(right, top, right, bottom, color);
                canvas.draw_line(left, bottom, right, bottom, color);

                // Front
                canvas.draw_line(left, top, right, top, color);
            }
        }
    }

    /// Render a single navigation item
    fn render_item(
        &self,
        canvas: &mut graphics::Canvas,
        y: i32,
        label: &str,
        icon: &Icon,
        is_selected: bool,
        is_hovered: bool,
    ) {
        let width = self.sidebar_width as i32;
        let height = self.item_height as i32;

        // Background
        let background_color = if is_selected {
            Color { r: 0.2, g: 0.4, b: 0.8, a: 1.0 }  // Blue for selected
        } else if is_hovered {
            Color { r: 0.9, g: 0.9, b: 0.9, a: 1.0 }  // Light gray for hover
        } else {
            Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }  // White for normal
        };

        canvas.fill_rect(0, y, width as u32, height as u32, background_color);

        // Icon
        let icon_x = (self.item_padding) as i32 + 4;
        let icon_y = y + (height - self.icon_size as i32) / 2;
        let icon_color = if is_selected {
            Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }
        } else {
            Color { r: 0.3, g: 0.3, b: 0.3, a: 1.0 }
        };
        self.render_icon(canvas, icon, icon_x, icon_y, icon_color);

        // Label
        let text_color = if is_selected {
            Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }
        } else {
            Color { r: 0.2, g: 0.2, b: 0.2, a: 1.0 }
        };

        let text_x = icon_x + self.icon_size as i32 + 8;
        let text_y = y + (height - (self.font_size * 1.2) as i32) / 2;
        canvas.draw_text_sized(text_x, text_y, label, text_color, self.font_size);

        // Bottom border
        let border_color = Color { r: 0.85, g: 0.85, b: 0.85, a: 1.0 };
        canvas.draw_line(0, y + height - 1, width, y + height - 1, border_color);
    }
}

impl ElementRenderObject for NavigationViewRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.size = Size {
            width: constraints.min_width.min(constraints.max_width),
            height: constraints.min_height.min(constraints.max_height),
        };
        self.size
    }

    fn layout_with_children(
        &mut self,
        constraints: LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        if crate::debug::is_enabled() {
            scarlet_std::println!(
                "[NavigationViewRenderObject::layout] constraints=({:?}, {:?}) -> ({:?}, {:?})",
                constraints.min_width, constraints.min_height, constraints.max_width, constraints.max_height
            );
        }

        // Expect exactly 2 children: sidebar and content
        if children.len() != 2 {
            scarlet_std::println!(
                "[NavigationViewRenderObject::layout] WARNING: Expected 2 children, got {}",
                children.len()
            );
        }

        // Layout sidebar (child 0) with fixed width
        let sidebar_constraints = LayoutConstraints::tight(
            self.sidebar_width,
            constraints.max_height,
        );
        let sidebar_height = if let Some(sidebar) = children.get_mut(0) {
            sidebar.layout(sidebar_constraints)
        } else {
            Size::new(self.sidebar_width, constraints.max_height)
        };

        // Layout content (child 1) with remaining width
        let content_width = constraints.max_width - self.sidebar_width;
        let content_constraints = LayoutConstraints::new(
            content_width, content_width,
            0.0, constraints.max_height,
        );
        let content_height = if let Some(content) = children.get_mut(1) {
            content.layout(content_constraints)
        } else {
            Size::new(content_width, constraints.max_height)
        };

        // Position sidebar at (0, 0)
        if let Some(sidebar) = children.get_mut(0) {
            sidebar.set_position(Point::ZERO);
        }

        // Position content at (sidebar_width, 0)
        if let Some(content) = children.get_mut(1) {
            content.set_position(Point::new(self.sidebar_width, 0.0));
        }

        // Total size is the full constraint
        self.size = Size::new(constraints.max_width, constraints.max_height);

        // Create buffer for sidebar only
        let sidebar_height_px = libm::ceilf(sidebar_height.height) as u32;
        let sidebar_width_px = libm::ceilf(self.sidebar_width) as u32;

        if crate::debug::is_enabled() {
            scarlet_std::println!(
                "[NavigationViewRenderObject::layout] sidebar size={}x{}, buffer needed={} bytes",
                sidebar_width_px, sidebar_height_px, sidebar_width_px * sidebar_height_px * 4
            );
        }

        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.width() != sidebar_width_px || b.height() != sidebar_height_px);

        if needs_resize {
            self.buffer = Some(Buffer::from_dimensions(sidebar_width_px, sidebar_height_px));
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
        if crate::debug::is_enabled() {
            scarlet_std::println!("[NavigationViewRenderObject::render] buffer={}", self.buffer.is_some());
        }

        if let Some(ref mut buffer) = self.buffer {
            let width = buffer.width();
            let height = buffer.height();
            let mut data = buffer.data_mut();
            let mut canvas = graphics::Canvas::new(&mut data, width, height);

            // Fill background
            let bg_color = Color { r: 0.95, g: 0.95, b: 0.97, a: 1.0 };
            canvas.fill_rect(0, 0, width, height, bg_color);

            // Render items
            // Clone values we need before the loop to avoid borrow issues
            let link_count = self.link_count;
            let selected = self.selected_index.get();
            let hovered = self.hovered_index;
            let sidebar_width = self.sidebar_width;
            let item_height = self.item_height;
            let font_size = self.font_size;
            let icon_size = self.icon_size;
            let item_padding = self.item_padding;

            for i in 0..link_count {
                let y = (i as f32 * item_height) as i32;
                let is_selected = selected == i;
                let is_hovered = hovered == Some(i);

                // Get label and icon for this item
                let label = self.labels.get(i).map(|s| s.as_str()).unwrap_or("Item");
                let icon = self.icons.get(i).copied().unwrap_or(Icon::Home);

                // Inline render_item to avoid borrow checker issues
                let width_px = sidebar_width as i32;
                let height_px = item_height as i32;

                // Background
                let background_color = if is_selected {
                    Color { r: 0.2, g: 0.4, b: 0.8, a: 1.0 }
                } else if is_hovered {
                    Color { r: 0.9, g: 0.9, b: 0.9, a: 1.0 }
                } else {
                    Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }
                };

                canvas.fill_rect(0, y, width_px as u32, height_px as u32, background_color);

                // Icon
                let icon_x = item_padding as i32 + 4;
                let icon_y = y + (height_px - icon_size as i32) / 2;
                let icon_color = if is_selected {
                    Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }
                } else {
                    Color { r: 0.3, g: 0.3, b: 0.3, a: 1.0 }
                };

                // Inline icon rendering (simplified)
                // For now, just skip icon rendering to avoid borrow issues
                // A full implementation would call render_icon here

                // Label
                let text_color = if is_selected {
                    Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }
                } else {
                    Color { r: 0.2, g: 0.2, b: 0.2, a: 1.0 }
                };

                let text_x = icon_x + icon_size as i32 + 8;
                let text_y = y + (height_px - (font_size * 1.2) as i32) / 2;
                canvas.draw_text_sized(text_x, text_y, label, text_color, font_size);

                // Bottom border
                let border_color = Color { r: 0.85, g: 0.85, b: 0.85, a: 1.0 };
                canvas.draw_line(0, y + height_px - 1, width_px - 1, y + height_px - 1, border_color);
            }

            // Draw separator line on the right edge
            let palette = ColorPalette::default();
            let border_color = palette.border();
            canvas.draw_line(width as i32 - 1, 0, width as i32 - 1, height as i32, border_color);
        }
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn clear_buffer(&mut self) {
        self.buffer = None;
    }

    fn hit_test(&self, point: Point) -> bool {
        // Check if point is within sidebar bounds
        if point.x >= 0.0 && point.x <= self.sidebar_width
            && point.y >= 0.0 && point.y <= self.size.height
        {
            true
        } else {
            false
        }
    }
}
