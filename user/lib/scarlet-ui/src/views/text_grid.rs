//! TextGrid View - fixed-cell text rendering for terminal-like surfaces.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;

use crate::buffer::Buffer;
use crate::color::Color;
use crate::element::{
    Element, ElementRenderObject, LayoutConstraints, RenderElement, UpdateResult,
};
use crate::geometry::{Point, Size};
use crate::graphics::{self, FontStack};
use crate::state::State;
use crate::view::View;

/// A single fixed-cell text grid cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextGridCell {
    /// Character drawn in the cell.
    pub ch: char,
    /// Foreground text color.
    pub foreground: Color,
    /// Cell background color.
    pub background: Color,
    /// Whether the cell should be drawn in bold style.
    pub bold: bool,
    /// Whether the foreground should be rendered with faint intensity.
    pub faint: bool,
    /// Whether the cell should be rendered in italic style.
    pub italic: bool,
    /// Whether the cell should draw an underline.
    pub underline: bool,
    /// Optional underline color override.
    pub underline_color: Option<Color>,
    /// Optional underline thickness override in pixels.
    pub underline_thickness: u8,
    /// Whether foreground and background should be swapped.
    pub inverse: bool,
    /// Whether the cell should draw a strike-through line.
    pub strikethrough: bool,
}

impl TextGridCell {
    /// Create a text cell with explicit colors.
    ///
    /// # Arguments
    ///
    /// * `ch` - Character to render.
    /// * `foreground` - Foreground text color.
    /// * `background` - Background cell color.
    ///
    /// # Returns
    ///
    /// A new [`TextGridCell`].
    pub const fn new(ch: char, foreground: Color, background: Color) -> Self {
        Self {
            ch,
            foreground,
            background,
            bold: false,
            faint: false,
            italic: false,
            underline: false,
            underline_color: None,
            underline_thickness: 0,
            inverse: false,
            strikethrough: false,
        }
    }

    /// Create a blank cell with explicit colors.
    ///
    /// # Arguments
    ///
    /// * `foreground` - Foreground text color.
    /// * `background` - Background cell color.
    ///
    /// # Returns
    ///
    /// A blank [`TextGridCell`].
    pub const fn blank(foreground: Color, background: Color) -> Self {
        Self::new(' ', foreground, background)
    }
}

impl Default for TextGridCell {
    fn default() -> Self {
        Self::blank(Color::WHITE, Color::BLACK)
    }
}

fn dim_color(color: Color) -> Color {
    Color::rgba_f32(color.r * 0.55, color.g * 0.55, color.b * 0.55, color.a)
}

fn font_stack_id(font_stack: Option<&FontStack>) -> Option<usize> {
    font_stack.map(FontStack::cache_id)
}

/// Return the fixed-grid cell width for a Unicode scalar value.
///
/// This follows the terminal convention that East Asian wide/fullwidth glyphs
/// occupy two cells. Combining marks are intentionally left as one cell for now
/// because `TextGridBuffer` stores one scalar per cell and has no grapheme
/// clustering layer yet.
///
/// # Arguments
///
/// * `ch` - Character to classify.
///
/// # Returns
///
/// Number of fixed grid cells occupied by `ch`.
pub fn text_grid_cell_width(ch: char) -> usize {
    let code = ch as u32;
    if matches!(
        code,
        0x1100..=0x115f
            | 0x2329..=0x232a
            | 0x2e80..=0xa4cf
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe10..=0xfe19
            | 0xfe30..=0xfe6f
            | 0xff00..=0xff60
            | 0xffe0..=0xffe6
            | 0x20000..=0x3fffd
    ) {
        2
    } else {
        1
    }
}

/// Fixed-size text grid contents.
#[derive(Clone, Debug, PartialEq)]
pub struct TextGridBuffer {
    columns: usize,
    rows: usize,
    cells: Vec<TextGridCell>,
}

impl TextGridBuffer {
    /// Create a text grid buffer.
    ///
    /// # Arguments
    ///
    /// * `columns` - Number of columns.
    /// * `rows` - Number of rows.
    /// * `default_cell` - Initial value for all cells.
    ///
    /// # Returns
    ///
    /// A new [`TextGridBuffer`].
    pub fn new(columns: usize, rows: usize, default_cell: TextGridCell) -> Self {
        let len = columns.saturating_mul(rows);
        Self {
            columns,
            rows,
            cells: alloc::vec![default_cell; len],
        }
    }

    /// Return the number of columns.
    ///
    /// # Returns
    ///
    /// Column count.
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// Return the number of rows.
    ///
    /// # Returns
    ///
    /// Row count.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Return the cell slice in row-major order.
    ///
    /// # Returns
    ///
    /// Borrowed row-major cells.
    pub fn cells(&self) -> &[TextGridCell] {
        &self.cells
    }

    /// Return a mutable cell slice in row-major order.
    ///
    /// # Returns
    ///
    /// Mutable row-major cells.
    pub fn cells_mut(&mut self) -> &mut [TextGridCell] {
        &mut self.cells
    }

    /// Return a cell by position.
    ///
    /// # Arguments
    ///
    /// * `column` - Cell column.
    /// * `row` - Cell row.
    ///
    /// # Returns
    ///
    /// The cell if the position is inside the grid.
    pub fn cell(&self, column: usize, row: usize) -> Option<TextGridCell> {
        self.index(column, row)
            .and_then(|index| self.cells.get(index).copied())
    }

    /// Set a cell by position.
    ///
    /// # Arguments
    ///
    /// * `column` - Cell column.
    /// * `row` - Cell row.
    /// * `cell` - New cell value.
    ///
    /// # Returns
    ///
    /// `true` when the position was inside the grid.
    pub fn set_cell(&mut self, column: usize, row: usize, cell: TextGridCell) -> bool {
        let Some(index) = self.index(column, row) else {
            return false;
        };
        if let Some(slot) = self.cells.get_mut(index) {
            *slot = cell;
            true
        } else {
            false
        }
    }

    /// Fill all cells with a value.
    ///
    /// # Arguments
    ///
    /// * `cell` - New value for every cell.
    pub fn clear(&mut self, cell: TextGridCell) {
        self.cells.fill(cell);
    }

    /// Resize the grid while preserving cells that still fit.
    ///
    /// # Arguments
    ///
    /// * `columns` - New column count.
    /// * `rows` - New row count.
    /// * `default_cell` - Fill value for newly-created cells.
    pub fn resize(&mut self, columns: usize, rows: usize, default_cell: TextGridCell) {
        if self.columns == columns && self.rows == rows {
            return;
        }

        let mut new_cells = alloc::vec![default_cell; columns.saturating_mul(rows)];
        let copy_columns = self.columns.min(columns);
        let copy_rows = self.rows.min(rows);
        for row in 0..copy_rows {
            let old_start = row.saturating_mul(self.columns);
            let new_start = row.saturating_mul(columns);
            new_cells[new_start..new_start + copy_columns]
                .copy_from_slice(&self.cells[old_start..old_start + copy_columns]);
        }

        self.columns = columns;
        self.rows = rows;
        self.cells = new_cells;
    }

    /// Write text into a row starting at a column.
    ///
    /// # Arguments
    ///
    /// * `column` - Starting column.
    /// * `row` - Target row.
    /// * `text` - Text to write.
    /// * `foreground` - Foreground color.
    /// * `background` - Background color.
    pub fn write_text(
        &mut self,
        column: usize,
        row: usize,
        text: &str,
        foreground: Color,
        background: Color,
    ) {
        let mut target_column = column;
        for ch in text.chars() {
            let width = text_grid_cell_width(ch);
            if target_column >= self.columns {
                break;
            }
            if width == 2 && target_column + 1 >= self.columns {
                break;
            }

            let _ = self.set_cell(
                target_column,
                row,
                TextGridCell::new(ch, foreground, background),
            );
            if width == 2 {
                let _ = self.set_cell(
                    target_column + 1,
                    row,
                    TextGridCell::new('\0', foreground, background),
                );
            }
            target_column = target_column.saturating_add(width);
        }
    }

    fn index(&self, column: usize, row: usize) -> Option<usize> {
        if column < self.columns && row < self.rows {
            Some(row.saturating_mul(self.columns).saturating_add(column))
        } else {
            None
        }
    }
}

impl Default for TextGridBuffer {
    fn default() -> Self {
        Self::new(80, 24, TextGridCell::default())
    }
}

/// Text-grid cursor position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextGridCursor {
    /// Cursor column.
    pub column: usize,
    /// Cursor row.
    pub row: usize,
    /// Whether the cursor is visible.
    pub visible: bool,
}

impl TextGridCursor {
    /// Create a visible cursor position.
    ///
    /// # Arguments
    ///
    /// * `column` - Cursor column.
    /// * `row` - Cursor row.
    ///
    /// # Returns
    ///
    /// A visible cursor.
    pub const fn new(column: usize, row: usize) -> Self {
        Self {
            column,
            row,
            visible: true,
        }
    }
}

/// Fixed-cell text grid view.
#[derive(Clone)]
pub struct TextGrid {
    buffer: State<TextGridBuffer>,
    cell_width: f32,
    cell_height: f32,
    font_size: f32,
    font_stack: Option<FontStack>,
    background_color: Color,
    cursor: Option<TextGridCursor>,
    cursor_color: Color,
}

impl TextGrid {
    /// Create a text grid from state.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Reactive grid contents.
    ///
    /// # Returns
    ///
    /// A new [`TextGrid`].
    pub fn new(buffer: State<TextGridBuffer>) -> Self {
        Self {
            buffer,
            cell_width: 9.0,
            cell_height: 18.0,
            font_size: 16.0,
            font_stack: None,
            background_color: Color::BLACK,
            cursor: None,
            cursor_color: Color::rgba_f32(1.0, 1.0, 1.0, 0.65),
        }
    }

    /// Set fixed cell metrics in pixels.
    ///
    /// # Arguments
    ///
    /// * `width` - Cell width.
    /// * `height` - Cell height.
    ///
    /// # Returns
    ///
    /// Updated view.
    pub fn cell_size(mut self, width: f32, height: f32) -> Self {
        self.cell_width = width.max(1.0);
        self.cell_height = height.max(1.0);
        self
    }

    /// Set font size in pixels.
    ///
    /// # Arguments
    ///
    /// * `font_size` - Font size in pixels.
    ///
    /// # Returns
    ///
    /// Updated view.
    pub fn font_size(mut self, font_size: f32) -> Self {
        self.font_size = font_size.max(1.0);
        self
    }

    /// Set a view-local font stack.
    ///
    /// When unset, [`TextGrid`] uses the system-wide ScarletUI default font
    /// stack.
    ///
    /// # Arguments
    ///
    /// * `font_stack` - Font stack to use for this grid.
    ///
    /// # Returns
    ///
    /// Updated view.
    pub fn font_stack(mut self, font_stack: FontStack) -> Self {
        self.font_stack = Some(font_stack);
        self
    }

    /// Set the view background color.
    ///
    /// This color is painted across the full text-grid view before cell
    /// backgrounds and glyphs are rendered.
    ///
    /// # Arguments
    ///
    /// * `color` - Background color for the whole view.
    ///
    /// # Returns
    ///
    /// Updated view.
    pub fn background_color(mut self, color: Color) -> Self {
        self.background_color = color;
        self
    }

    /// Set cursor position.
    ///
    /// # Arguments
    ///
    /// * `cursor` - Cursor position, or `None` to hide it.
    ///
    /// # Returns
    ///
    /// Updated view.
    pub fn cursor(mut self, cursor: Option<TextGridCursor>) -> Self {
        self.cursor = cursor;
        self
    }

    /// Set cursor color.
    ///
    /// # Arguments
    ///
    /// * `color` - Cursor overlay color.
    ///
    /// # Returns
    ///
    /// Updated view.
    pub fn cursor_color(mut self, color: Color) -> Self {
        self.cursor_color = color;
        self
    }

    /// Return the backing grid state.
    ///
    /// # Returns
    ///
    /// Borrowed grid state.
    pub fn buffer(&self) -> &State<TextGridBuffer> {
        &self.buffer
    }
}

impl View for TextGrid {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            TextGridRenderObject::new(
                self.buffer.get(),
                self.cell_width,
                self.cell_height,
                self.font_size,
                self.font_stack.clone(),
                self.background_color,
                self.cursor,
                self.cursor_color,
            ),
        ))
    }

    fn listenables(&self) -> Vec<&dyn crate::state::Listenable> {
        alloc::vec![&self.buffer]
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Render object for [`TextGrid`].
pub struct TextGridRenderObject {
    grid: TextGridBuffer,
    cell_width: f32,
    cell_height: f32,
    font_size: f32,
    font_stack: Option<FontStack>,
    background_color: Color,
    cursor: Option<TextGridCursor>,
    cursor_color: Color,
    size: Size,
    buffer: Option<Buffer>,
    full_repaint: bool,
    dirty_cells: Vec<usize>,
}

impl TextGridRenderObject {
    /// Create a text-grid render object.
    ///
    /// # Arguments
    ///
    /// * `grid` - Initial grid contents.
    /// * `cell_width` - Cell width in pixels.
    /// * `cell_height` - Cell height in pixels.
    /// * `font_size` - Font size in pixels.
    /// * `font_stack` - Optional view-local font stack.
    /// * `background_color` - Background color for the whole view.
    /// * `cursor` - Optional cursor.
    /// * `cursor_color` - Cursor overlay color.
    ///
    /// # Returns
    ///
    /// A new [`TextGridRenderObject`].
    pub fn new(
        grid: TextGridBuffer,
        cell_width: f32,
        cell_height: f32,
        font_size: f32,
        font_stack: Option<FontStack>,
        background_color: Color,
        cursor: Option<TextGridCursor>,
        cursor_color: Color,
    ) -> Self {
        Self {
            grid,
            cell_width: cell_width.max(1.0),
            cell_height: cell_height.max(1.0),
            font_size: font_size.max(1.0),
            font_stack,
            background_color,
            cursor,
            cursor_color,
            size: Size::ZERO,
            buffer: None,
            full_repaint: true,
            dirty_cells: Vec::new(),
        }
    }

    fn mark_cursor_dirty(&mut self, cursor: Option<TextGridCursor>) {
        let Some(cursor) = cursor else {
            return;
        };
        if !cursor.visible {
            return;
        }
        if let Some(index) = self.grid.index(cursor.column, cursor.row) {
            self.dirty_cells.push(index);
        }
    }

    fn repaint_cell(&self, canvas: &mut graphics::Canvas<'_>, index: usize) {
        let columns = self.grid.columns();
        if columns == 0 {
            return;
        }

        let column = index % columns;
        let row = index / columns;
        let x = libm::floorf(column as f32 * self.cell_width) as i32;
        let y = libm::floorf(row as f32 * self.cell_height) as i32;
        if x >= canvas.width() as i32 || y >= canvas.height() as i32 {
            return;
        }

        let Some(cell) = self.grid.cells().get(index).copied() else {
            return;
        };
        if cell.ch == '\0' {
            return;
        }

        let cell_span =
            text_grid_cell_width(cell.ch).min(self.grid.columns().saturating_sub(column).max(1));
        let w = libm::ceilf(self.cell_width * cell_span as f32) as u32;
        let h = libm::ceilf(self.cell_height) as u32;
        let (mut foreground, background) = if cell.inverse {
            (cell.background, cell.foreground)
        } else {
            (cell.foreground, cell.background)
        };
        if cell.faint {
            foreground = dim_color(foreground);
        }

        canvas.fill_rect(x, y, w, h, background);

        if cell.ch != ' ' && cell.ch != '\0' {
            let mut encoded = [0u8; 4];
            let text = cell.ch.encode_utf8(&mut encoded);
            let baseline_adjust = ((self.cell_height - self.font_size) / 2.0).max(0.0);
            if let Some(font_stack) = &self.font_stack {
                canvas.draw_text_sized_with_font_stack(
                    x,
                    y + libm::floorf(baseline_adjust) as i32,
                    text,
                    foreground,
                    self.font_size,
                    font_stack,
                );
            } else {
                canvas.draw_text_sized(
                    x,
                    y + libm::floorf(baseline_adjust) as i32,
                    text,
                    foreground,
                    self.font_size,
                );
            }
            if cell.bold {
                if let Some(font_stack) = &self.font_stack {
                    canvas.draw_text_sized_with_font_stack(
                        x + 1,
                        y + libm::floorf(baseline_adjust) as i32,
                        text,
                        foreground,
                        self.font_size,
                        font_stack,
                    );
                } else {
                    canvas.draw_text_sized(
                        x + 1,
                        y + libm::floorf(baseline_adjust) as i32,
                        text,
                        foreground,
                        self.font_size,
                    );
                }
            }
        }
        let line_thickness = (libm::ceilf(self.font_size / 12.0) as u32).max(1);
        if cell.underline {
            let underline_thickness = line_thickness.max(cell.underline_thickness as u32);
            let line_y = y + h.saturating_sub(underline_thickness).saturating_sub(1) as i32;
            canvas.fill_rect(
                x,
                line_y.max(y),
                w,
                underline_thickness,
                cell.underline_color.unwrap_or(foreground),
            );
        }
        if cell.strikethrough {
            let line_y = y + (h / 2) as i32;
            canvas.fill_rect(x, line_y, w, line_thickness, foreground);
        }
    }

    fn grid_pixel_width(&self) -> u32 {
        libm::ceilf(self.grid.columns() as f32 * self.cell_width).max(0.0) as u32
    }

    fn grid_pixel_height(&self) -> u32 {
        libm::ceilf(self.grid.rows() as f32 * self.cell_height).max(0.0) as u32
    }

    fn repaint_remainder(&self, canvas: &mut graphics::Canvas<'_>, width: u32, height: u32) {
        let background = self.background_color;
        let grid_width = self.grid_pixel_width().min(width);
        let grid_height = self.grid_pixel_height().min(height);

        if grid_width < width {
            canvas.fill_rect(grid_width as i32, 0, width - grid_width, height, background);
        }
        if grid_height < height {
            canvas.fill_rect(
                0,
                grid_height as i32,
                grid_width,
                height - grid_height,
                background,
            );
        }
    }

    fn visible_columns(&self, width: u32) -> usize {
        let columns = libm::ceilf(width as f32 / self.cell_width).max(0.0) as usize;
        columns.min(self.grid.columns())
    }

    fn visible_rows(&self, height: u32) -> usize {
        let rows = libm::ceilf(height as f32 / self.cell_height).max(0.0) as usize;
        rows.min(self.grid.rows())
    }

    fn repaint_cursor(&self, canvas: &mut graphics::Canvas<'_>) {
        let Some(cursor) = self.cursor else {
            return;
        };
        if !cursor.visible || self.grid.index(cursor.column, cursor.row).is_none() {
            return;
        }

        let x = libm::floorf(cursor.column as f32 * self.cell_width) as i32;
        let y = libm::floorf(cursor.row as f32 * self.cell_height) as i32;
        if x >= canvas.width() as i32 || y >= canvas.height() as i32 {
            return;
        }

        let w = libm::ceilf(self.cell_width).max(1.0) as u32;
        let h = libm::ceilf(self.cell_height).max(1.0) as u32;
        let cursor_height = (h / 5).max(2);
        canvas.fill_rect(
            x,
            y + h.saturating_sub(cursor_height) as i32,
            w,
            cursor_height,
            self.cursor_color,
        );
    }
}

impl ElementRenderObject for TextGridRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let natural_width = self.grid.columns() as f32 * self.cell_width;
        let natural_height = self.grid.rows() as f32 * self.cell_height;

        let width = if constraints.is_tight_width()
            && constraints.max_width.is_finite()
            && constraints.max_width > 0.0
        {
            constraints.max_width.max(1.0)
        } else if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
            natural_width
                .min(constraints.max_width)
                .max(constraints.min_width)
        } else {
            natural_width.max(constraints.min_width).max(1.0)
        };
        let height = if constraints.is_tight_height()
            && constraints.max_height.is_finite()
            && constraints.max_height > 0.0
        {
            constraints.max_height.max(1.0)
        } else if constraints.max_height.is_finite() && constraints.max_height > 0.0 {
            natural_height
                .min(constraints.max_height)
                .max(constraints.min_height)
        } else {
            natural_height.max(constraints.min_height).max(1.0)
        };

        self.size = Size { width, height };
        let w = libm::ceilf(width).max(1.0) as u32;
        let h = libm::ceilf(height).max(1.0) as u32;
        let needs_resize = self.buffer.as_ref().map_or(true, |buffer| {
            buffer.logical_width() != w || buffer.logical_height() != h
        });
        if needs_resize {
            self.buffer = Some(Buffer::from_logical_dimensions(w, h));
            self.full_repaint = true;
        }

        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn hit_test(&self, point: Point) -> bool {
        point.x >= 0.0 && point.y >= 0.0 && point.x < self.size.width && point.y < self.size.height
    }

    fn render(&mut self) {
        let Some(mut buffer) = self.buffer.take() else {
            return;
        };

        let width = buffer.logical_width();
        let height = buffer.logical_height();
        {
            let mut canvas = graphics::Canvas::for_buffer(&mut buffer);
            if self.full_repaint {
                canvas.fill_rect(0, 0, width, height, self.background_color);
                let visible_columns = self.visible_columns(width);
                let visible_rows = self.visible_rows(height);
                let columns = self.grid.columns();
                for row in 0..visible_rows {
                    let row_start = row.saturating_mul(columns);
                    for column in 0..visible_columns {
                        self.repaint_cell(&mut canvas, row_start + column);
                    }
                }
                self.full_repaint = false;
                self.dirty_cells.clear();
            } else {
                let mut dirty_cells = core::mem::take(&mut self.dirty_cells);
                dirty_cells.sort_unstable();
                dirty_cells.dedup();
                for index in dirty_cells {
                    self.repaint_cell(&mut canvas, index);
                }
            }
            self.repaint_remainder(&mut canvas, width, height);
            self.repaint_cursor(&mut canvas);
        }

        self.buffer = Some(buffer);
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn clear_buffer(&mut self) {
        self.buffer = None;
        self.full_repaint = true;
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn update(&mut self, new_view: &dyn View) -> UpdateResult {
        let Some(text_grid) = new_view.as_any().downcast_ref::<TextGrid>() else {
            return UpdateResult::Replaced;
        };

        let mut changed = false;
        let new_grid = text_grid.buffer.get();
        if self.cell_width != text_grid.cell_width
            || self.cell_height != text_grid.cell_height
            || self.font_size != text_grid.font_size
        {
            self.cell_width = text_grid.cell_width;
            self.cell_height = text_grid.cell_height;
            self.font_size = text_grid.font_size;
            self.full_repaint = true;
            changed = true;
        }
        if font_stack_id(self.font_stack.as_ref()) != font_stack_id(text_grid.font_stack.as_ref()) {
            self.font_stack = text_grid.font_stack.clone();
            self.full_repaint = true;
            changed = true;
        }
        if self.background_color != text_grid.background_color {
            self.background_color = text_grid.background_color;
            self.full_repaint = true;
            changed = true;
        }
        if self.cursor_color != text_grid.cursor_color {
            self.cursor_color = text_grid.cursor_color;
            self.mark_cursor_dirty(self.cursor);
            changed = true;
        }
        if self.cursor != text_grid.cursor {
            self.mark_cursor_dirty(self.cursor);
            self.mark_cursor_dirty(text_grid.cursor);
            self.cursor = text_grid.cursor;
            changed = true;
        }

        if self.grid.columns() != new_grid.columns() || self.grid.rows() != new_grid.rows() {
            self.grid = new_grid;
            self.full_repaint = true;
            changed = true;
        } else if self.grid != new_grid {
            let old_cells = self.grid.cells();
            for (index, new_cell) in new_grid.cells().iter().enumerate() {
                if old_cells.get(index) != Some(new_cell) {
                    self.dirty_cells.push(index);
                }
            }
            self.grid = new_grid;
            changed = true;
        }

        if changed {
            UpdateResult::Updated
        } else {
            UpdateResult::NoChange
        }
    }
}
