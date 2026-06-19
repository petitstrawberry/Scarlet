//! Select View - single-choice pull-down control.
//!
//! Select displays the current value and expands into a list of options when
//! clicked.

use crate::os::Mutex;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

use crate::buffer::Buffer;
use crate::color::{Color, ColorPalette};
use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::geometry::Size;
use crate::graphics;
use crate::state::State;
use crate::view::View;

static SELECT_EXPANDED_REGISTRY: Mutex<BTreeMap<crate::state::StateId, State<bool>>> =
    Mutex::new(BTreeMap::new());

fn select_expanded_state(selected_index: &State<usize>) -> State<bool> {
    let id = selected_index.id();
    let mut registry = SELECT_EXPANDED_REGISTRY.lock();
    if let Some(state) = registry.get(&id) {
        return state.clone();
    }
    let state: State<bool> = State::initial(crate::state::generate_state_id());
    registry.insert(id, state.clone());
    state
}

/// Select change callback type.
pub type SelectChangeCallback = Box<dyn Fn(usize) + 'static>;

/// Select View - displays a selected option and expands on click.
#[derive(Clone)]
pub struct Select {
    options: Vec<String>,
    selected_index: State<usize>,
    expanded: State<bool>,
    on_change: Option<Arc<dyn Fn(usize) + 'static>>,
    width: f32,
    row_height: f32,
    placeholder: String,
    max_visible_rows: usize,
}

impl Select {
    /// Create a new Select.
    ///
    /// # Arguments
    ///
    /// * `options` - Labels displayed by the select control.
    /// * `selected_index` - State containing the selected option index.
    ///
    /// # Returns
    ///
    /// A Select view bound to the supplied state.
    pub fn new(options: Vec<String>, selected_index: State<usize>) -> Self {
        let expanded = select_expanded_state(&selected_index);
        Self {
            options,
            selected_index,
            expanded,
            on_change: None,
            width: 260.0,
            row_height: 32.0,
            placeholder: String::from("Select…"),
            max_visible_rows: 8,
        }
    }

    /// Set the change callback.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function called with the newly selected option index.
    ///
    /// # Returns
    ///
    /// The updated Select view.
    pub fn on_change(mut self, callback: impl Fn(usize) + 'static) -> Self {
        self.on_change = Some(Arc::new(callback));
        self
    }

    /// Set the control width.
    ///
    /// # Arguments
    ///
    /// * `width` - Logical width in pixels.
    ///
    /// # Returns
    ///
    /// The updated Select view.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Set the row height.
    ///
    /// # Arguments
    ///
    /// * `row_height` - Logical row height in pixels.
    ///
    /// # Returns
    ///
    /// The updated Select view.
    pub fn row_height(mut self, row_height: f32) -> Self {
        self.row_height = row_height;
        self
    }

    /// Set placeholder text shown when no option is selected.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set the maximum number of visible rows before scrolling.
    pub fn max_visible_rows(mut self, max: usize) -> Self {
        self.max_visible_rows = max.max(1);
        self
    }

    /// Get the selected index state.
    ///
    /// # Returns
    ///
    /// State storing the selected option index.
    pub fn selected_index(&self) -> &State<usize> {
        &self.selected_index
    }

    /// Get the expanded state.
    ///
    /// # Returns
    ///
    /// State storing whether the option list is open.
    pub fn expanded(&self) -> &State<bool> {
        &self.expanded
    }

    pub fn option_count(&self) -> usize {
        self.options.len()
    }

    /// Invoke the change callback if present.
    ///
    /// # Arguments
    ///
    /// * `index` - Newly selected option index.
    pub fn invoke_on_change(&self, index: usize) {
        if let Some(callback) = self.on_change.as_ref() {
            callback(index);
        }
    }
}

impl View for Select {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            SelectRenderObject::new(
                self.options.clone(),
                self.selected_index.get(),
                self.expanded.get(),
                self.width,
                self.row_height,
                self.placeholder.clone(),
                self.max_visible_rows,
            ),
        ))
    }

    fn listenables(&self) -> Vec<&dyn crate::state::Listenable> {
        alloc::vec![&self.selected_index, &self.expanded]
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Select RenderObject - handles select rendering and hit testing.
pub struct SelectRenderObject {
    options: Vec<String>,
    selected_index: usize,
    expanded: bool,
    width: f32,
    row_height: f32,
    placeholder: String,
    max_visible_rows: usize,
    scroll_offset: usize,
    hovered_index: Option<usize>,
    size: Size,
    buffer: Option<Buffer>,
}

impl SelectRenderObject {
    /// Create a new SelectRenderObject.
    ///
    /// # Arguments
    ///
    /// * `options` - Labels shown by the select control.
    /// * `selected_index` - Currently selected option index.
    /// * `expanded` - Whether the option list starts open.
    /// * `width` - Preferred logical width in pixels.
    /// * `row_height` - Logical height of each row in pixels.
    ///
    /// # Returns
    ///
    /// A render object for a Select view.
    pub fn new(
        options: Vec<String>,
        selected_index: usize,
        expanded: bool,
        width: f32,
        row_height: f32,
        placeholder: String,
        max_visible_rows: usize,
    ) -> Self {
        let mut render_object = Self {
            options,
            selected_index,
            expanded,
            width,
            row_height,
            placeholder,
            max_visible_rows,
            scroll_offset: 0,
            hovered_index: None,
            size: Size::ZERO,
            buffer: None,
        };
        render_object.selected_index = render_object.clamped_selected_index();
        render_object
    }

    /// Set whether the list is expanded.
    ///
    /// # Arguments
    ///
    /// * `expanded` - Whether the option list should be shown.
    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    /// Return whether the list is expanded.
    ///
    /// # Returns
    ///
    /// `true` when the option list is open.
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    pub fn hovered_index(&self) -> Option<usize> {
        self.hovered_index
    }

    pub fn adjust_scroll(&mut self) {
        if self.options.is_empty() {
            return;
        }
        let visible = self.options.len().min(self.max_visible_rows);
        if visible == 0 {
            return;
        }
        let target = self.hovered_index.unwrap_or(self.selected_index);
        let ideal_start = target.saturating_sub(visible / 2);
        let max_start = self.options.len().saturating_sub(visible);
        self.scroll_offset = ideal_start.min(max_start);
    }

    pub fn typeahead(&mut self, c: char) -> bool {
        if !c.is_ascii_graphic() && c != ' ' {
            return false;
        }
        let lower = c.to_ascii_lowercase();
        let count = self.options.len();
        if count == 0 {
            return false;
        }
        let start = self.hovered_index.unwrap_or(self.selected_index);
        for offset in 0..count {
            let i = (start + 1 + offset) % count;
            if self.options[i]
                .chars()
                .next()
                .is_some_and(|fc| fc.to_ascii_lowercase() == lower)
            {
                self.hovered_index = Some(i);
                self.adjust_scroll();
                return true;
            }
        }
        false
    }

    /// Set the hovered option index.
    ///
    /// # Arguments
    ///
    /// * `index` - Hovered option index, or `None` when no option is hovered.
    pub fn set_hovered_index(&mut self, index: Option<usize>) {
        self.hovered_index = index;
    }

    /// Find an option index for a local Y coordinate in the expanded list.
    ///
    /// # Arguments
    ///
    /// * `y` - Local Y coordinate inside the select render object.
    ///
    /// # Returns
    ///
    /// Option index under the coordinate, or `None` outside option rows.
    pub fn option_index_at_y(&self, y: f32) -> Option<usize> {
        if !self.expanded || y < self.row_height {
            return None;
        }
        let index = ((y - self.row_height) / self.row_height) as usize + self.scroll_offset;
        if index < self.options.len() {
            Some(index)
        } else {
            None
        }
    }

    fn clamped_selected_index(&self) -> usize {
        if self.options.is_empty() {
            0
        } else {
            self.selected_index.min(self.options.len() - 1)
        }
    }

    fn popup_row_count(&self) -> usize {
        if self.expanded {
            self.options.len().min(self.max_visible_rows) + 1
        } else {
            1
        }
    }

    fn popup_height(&self) -> f32 {
        self.row_height.max(24.0) * self.popup_row_count() as f32
    }

    fn draw_label(
        canvas: &mut graphics::Canvas<'_>,
        label: &str,
        y: i32,
        width: u32,
        row_height: u32,
        color: Color,
    ) {
        let text_y = y + ((row_height as i32 - 15) / 2).max(0);
        let max_chars = ((width.saturating_sub(44) / 8) as usize).max(1);
        if label.chars().count() > max_chars {
            let clipped: String = label.chars().take(max_chars).collect();
            canvas.draw_text_sized(12, text_y, &clipped, color, 13.0);
        } else {
            canvas.draw_text_sized(12, text_y, label, color, 13.0);
        }
    }

    fn draw_chevron(canvas: &mut graphics::Canvas<'_>, x: i32, y: i32, color: Color) {
        canvas.draw_line(x, y, x + 5, y + 5, color);
        canvas.draw_line(x + 5, y + 5, x + 10, y, color);
    }

    fn draw_checkmark(canvas: &mut graphics::Canvas<'_>, w: u32, y: i32, row_h: u32, color: Color) {
        let cx = w.saturating_sub(22) as i32;
        let cy = y + ((row_h as i32 - 8) / 2).max(0) + 2;
        canvas.draw_line(cx, cy, cx + 3, cy + 3, color);
        canvas.draw_line(cx + 3, cy + 3, cx + 8, cy - 3, color);
    }

    fn draw_select(&mut self) {
        let w = libm::ceilf(self.size.width) as u32;
        let h = libm::ceilf(self.popup_height()) as u32;
        let row_h = libm::ceilf(self.row_height) as u32;
        let selected_index = self.clamped_selected_index();
        let selected_label = self.options.get(selected_index).cloned();
        let options = self.options.clone();
        let hovered_index = self.hovered_index;
        let opt_count = options.len();
        let visible = opt_count.min(self.max_visible_rows);
        let scroll_start = self.scroll_offset.min(opt_count.saturating_sub(visible));
        let scroll_end = (scroll_start + visible).min(opt_count);

        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.logical_width() != w || b.logical_height() != h);
        if needs_resize {
            self.buffer = Some(Buffer::from_logical_dimensions(w, h));
        }

        let Some(ref mut buffer) = self.buffer else {
            return;
        };

        let mut canvas = graphics::Canvas::for_buffer(buffer);
        let palette = ColorPalette::default();
        let background = palette.button_background();
        let popup_background = palette.surface();
        let selected_background = palette
            .primary()
            .with_opacity(0.13)
            .blend_over(popup_background);
        let hover_background = palette
            .surface_variant()
            .with_opacity(0.8)
            .blend_over(popup_background);
        let border = palette.border();
        let text = palette.text_primary();
        let subtle = palette.text_secondary();

        canvas.fill_rect(0, 0, w, h, Color::rgba(0.0, 0.0, 0.0, 0.0));
        canvas.fill_rect(0, 0, w, row_h, background);
        canvas.draw_rect(0, 0, w, row_h, border);

        if let Some(label) = selected_label {
            Self::draw_label(&mut canvas, &label, 0, w, row_h, text);
        } else {
            Self::draw_label(&mut canvas, &self.placeholder, 0, w, row_h, subtle);
        }
        Self::draw_chevron(
            &mut canvas,
            w.saturating_sub(24) as i32,
            ((row_h as i32 - 6) / 2).max(0),
            subtle,
        );

        if self.expanded {
            let active = hovered_index.or(Some(selected_index));
            let active_background = palette
                .primary()
                .with_opacity(0.2)
                .blend_over(popup_background);
            let mut y = row_h as i32;
            for index in scroll_start..scroll_end {
                let label = &options[index];
                let row_background = if active == Some(index) {
                    active_background
                } else {
                    popup_background
                };
                canvas.fill_rect(0, y, w, row_h, row_background);
                canvas.draw_line(0, y, w as i32, y, border.with_opacity(0.55));
                Self::draw_label(&mut canvas, label, y, w, row_h, text);
                if index == selected_index {
                    Self::draw_checkmark(&mut canvas, w, y, row_h, subtle);
                }
                y += row_h as i32;
            }
            canvas.draw_rect(0, 0, w, h, border);
        }
    }
}

impl ElementRenderObject for SelectRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        let mut width = self.width.max(constraints.min_width).max(120.0);
        if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
            width = width.min(constraints.max_width);
        }
        let height = self.row_height.max(24.0);
        self.size = Size { width, height };

        let w = libm::ceilf(width) as u32;
        let h = libm::ceilf(self.popup_height()) as u32;
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

    fn hit_test(&self, point: crate::geometry::Point) -> bool {
        let bounds = crate::geometry::Rect {
            origin: crate::geometry::Point::ZERO,
            size: Size {
                width: self.size.width,
                height: self.popup_height(),
            },
        };
        bounds.contains(point)
    }

    fn render(&mut self) {
        self.draw_select();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn clear_buffer(&mut self) {
        self.buffer = None;
    }

    fn update(&mut self, new_view: &dyn crate::view::View) -> crate::element::UpdateResult {
        if let Some(select) = new_view.as_any().downcast_ref::<Select>() {
            let new_options = select.options.clone();
            let new_selected_index = select.selected_index.get();
            let new_expanded = select.expanded.get();
            let new_width = select.width;
            let new_row_height = select.row_height;
            let new_placeholder = select.placeholder.clone();
            let new_max_visible_rows = select.max_visible_rows;

            if self.options != new_options
                || self.selected_index != new_selected_index
                || self.expanded != new_expanded
                || (self.width - new_width).abs() > 0.001
                || (self.row_height - new_row_height).abs() > 0.001
                || self.placeholder != new_placeholder
                || self.max_visible_rows != new_max_visible_rows
            {
                let just_expanded = !self.expanded && new_expanded;
                self.options = new_options;
                self.selected_index = new_selected_index;
                self.expanded = new_expanded;
                self.width = new_width;
                self.row_height = new_row_height;
                self.placeholder = new_placeholder;
                self.max_visible_rows = new_max_visible_rows;
                self.selected_index = self.clamped_selected_index();
                if just_expanded {
                    self.hovered_index = Some(self.selected_index);
                    self.scroll_offset = 0;
                    self.adjust_scroll();
                }
                crate::element::UpdateResult::Updated
            } else {
                crate::element::UpdateResult::NoChange
            }
        } else {
            crate::element::UpdateResult::Replaced
        }
    }
}
