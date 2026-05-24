//! Minimal VT screen model for Scarlet Terminal.

use alloc::vec::Vec;

use scarlet_ui::{Color, TextGridBuffer, TextGridCell, TextGridCursor};

const SCROLLBACK_LIMIT: usize = 2000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParserState {
    Ground,
    Escape,
    Csi,
    Ss3,
}

/// Terminal screen state backed by a fixed text grid.
pub struct VtScreen {
    grid: TextGridBuffer,
    scrollback: Vec<Vec<TextGridCell>>,
    scrollback_offset: usize,
    columns: usize,
    rows: usize,
    cursor_column: usize,
    cursor_row: usize,
    saved_cursor_column: usize,
    saved_cursor_row: usize,
    foreground: Color,
    background: Color,
    bold: bool,
    inverse: bool,
    parser_state: ParserState,
    csi_params: [usize; 8],
    csi_count: usize,
    csi_value: Option<usize>,
}

impl VtScreen {
    /// Create a terminal screen.
    ///
    /// # Arguments
    ///
    /// * `columns` - Number of text columns.
    /// * `rows` - Number of text rows.
    ///
    /// # Returns
    ///
    /// A new [`VtScreen`].
    pub fn new(columns: usize, rows: usize) -> Self {
        let foreground = Color::rgb(230, 232, 235);
        let background = Color::rgb(12, 14, 18);
        Self {
            grid: TextGridBuffer::new(columns, rows, TextGridCell::blank(foreground, background)),
            scrollback: Vec::new(),
            scrollback_offset: 0,
            columns,
            rows,
            cursor_column: 0,
            cursor_row: 0,
            saved_cursor_column: 0,
            saved_cursor_row: 0,
            foreground,
            background,
            bold: false,
            inverse: false,
            parser_state: ParserState::Ground,
            csi_params: [0; 8],
            csi_count: 0,
            csi_value: None,
        }
    }

    /// Return a display grid with scrollback applied.
    ///
    /// # Returns
    ///
    /// Visible text grid.
    pub fn view_grid(&self) -> TextGridBuffer {
        if self.scrollback_offset == 0 {
            return self.grid.clone();
        }

        let foreground = Color::rgb(230, 232, 235);
        let background = Color::rgb(12, 14, 18);
        let blank = TextGridCell::blank(foreground, background);
        let mut view = TextGridBuffer::new(self.columns, self.rows, blank);
        let total_lines = self.scrollback.len().saturating_add(self.rows);
        let end = total_lines.saturating_sub(self.scrollback_offset);
        let start = end.saturating_sub(self.rows);

        for target_row in 0..self.rows {
            let source_line = start + target_row;
            if source_line < self.scrollback.len() {
                copy_line_to_grid(&self.scrollback[source_line], &mut view, target_row);
            } else {
                let source_row = source_line.saturating_sub(self.scrollback.len());
                for column in 0..self.columns {
                    if let Some(cell) = self.grid.cell(column, source_row) {
                        let _ = view.set_cell(column, target_row, cell);
                    }
                }
            }
        }

        view
    }

    /// Return the current cursor.
    ///
    /// # Returns
    ///
    /// Cursor position suitable for [`scarlet_ui::TextGrid`].
    pub fn cursor(&self) -> TextGridCursor {
        if self.scrollback_offset > 0 {
            return TextGridCursor {
                column: self.cursor_column,
                row: self.cursor_row,
                visible: false,
            };
        }
        TextGridCursor::new(self.cursor_column, self.cursor_row)
    }

    /// Scroll the display view through the saved scrollback.
    ///
    /// # Arguments
    ///
    /// * `lines` - Positive values scroll up into history; negative values scroll down.
    pub fn scroll_view(&mut self, lines: isize) {
        if lines > 0 {
            self.scrollback_offset = self
                .scrollback_offset
                .saturating_add(lines as usize)
                .min(self.scrollback.len());
        } else if lines < 0 {
            self.scrollback_offset = self.scrollback_offset.saturating_sub((-lines) as usize);
        }
    }

    /// Resize the terminal screen.
    ///
    /// # Arguments
    ///
    /// * `columns` - New number of columns.
    /// * `rows` - New number of rows.
    pub fn resize(&mut self, columns: usize, rows: usize) {
        let columns = columns.max(1);
        let rows = rows.max(1);
        if self.columns == columns && self.rows == rows {
            return;
        }

        self.scrollback_offset = 0;
        self.grid.resize(columns, rows, self.blank_cell());
        self.columns = columns;
        self.rows = rows;
        self.cursor_column = self.cursor_column.min(self.columns.saturating_sub(1));
        self.cursor_row = self.cursor_row.min(self.rows.saturating_sub(1));
        self.saved_cursor_column = self.saved_cursor_column.min(self.columns.saturating_sub(1));
        self.saved_cursor_row = self.saved_cursor_row.min(self.rows.saturating_sub(1));
    }

    /// Feed bytes from a PTY master into the screen model.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Bytes read from the PTY master.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.scrollback_offset = 0;
        for &byte in bytes {
            self.feed_byte(byte);
        }
    }

    fn feed_byte(&mut self, byte: u8) {
        match self.parser_state {
            ParserState::Ground => self.feed_ground(byte),
            ParserState::Escape => self.feed_escape(byte),
            ParserState::Csi => self.feed_csi(byte),
            ParserState::Ss3 => self.feed_ss3(byte),
        }
    }

    fn feed_ground(&mut self, byte: u8) {
        match byte {
            0x08 => self.backspace(),
            b'\t' => self.tab(),
            b'\r' => self.cursor_column = 0,
            b'\n' => self.line_feed(),
            0x1b => self.parser_state = ParserState::Escape,
            0x20..=0x7e => self.put_char(byte as char),
            _ => {}
        }
    }

    fn feed_escape(&mut self, byte: u8) {
        match byte {
            b'[' => {
                self.reset_csi();
                self.parser_state = ParserState::Csi;
            }
            b'O' => {
                self.parser_state = ParserState::Ss3;
            }
            b'7' | b's' => {
                self.save_cursor();
                self.parser_state = ParserState::Ground;
            }
            b'8' | b'u' => {
                self.restore_cursor();
                self.parser_state = ParserState::Ground;
            }
            b'c' => {
                self.clear_all();
                self.reset_attributes();
                self.parser_state = ParserState::Ground;
            }
            _ => self.parser_state = ParserState::Ground,
        }
    }

    fn feed_ss3(&mut self, byte: u8) {
        match byte {
            b'A' => self.move_cursor_up(1),
            b'B' => self.move_cursor_down(1),
            b'C' => self.move_cursor_right(1),
            b'D' => self.move_cursor_left(1),
            b'H' => self.set_cursor_column(0),
            b'F' => self.set_cursor_column(self.columns.saturating_sub(1)),
            _ => {}
        }
        self.parser_state = ParserState::Ground;
    }

    fn feed_csi(&mut self, byte: u8) {
        match byte {
            b'0'..=b'9' => {
                let digit = (byte - b'0') as usize;
                self.csi_value = Some(
                    self.csi_value
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(digit),
                );
            }
            b';' => self.push_csi_value(),
            b'?' => {}
            final_byte => {
                self.push_csi_value();
                self.handle_csi_final(final_byte);
                self.parser_state = ParserState::Ground;
            }
        }
    }

    fn reset_csi(&mut self) {
        self.csi_params = [0; 8];
        self.csi_count = 0;
        self.csi_value = None;
    }

    fn push_csi_value(&mut self) {
        if self.csi_count >= self.csi_params.len() {
            self.csi_value = None;
            return;
        }
        self.csi_params[self.csi_count] = self.csi_value.unwrap_or(0);
        self.csi_count += 1;
        self.csi_value = None;
    }

    fn param(&self, index: usize, default: usize) -> usize {
        if index < self.csi_count && self.csi_params[index] != 0 {
            self.csi_params[index]
        } else {
            default
        }
    }

    fn handle_csi_final(&mut self, final_byte: u8) {
        match final_byte {
            b'A' => self.move_cursor_up(self.param(0, 1)),
            b'B' => self.move_cursor_down(self.param(0, 1)),
            b'C' => self.move_cursor_right(self.param(0, 1)),
            b'D' => self.move_cursor_left(self.param(0, 1)),
            b'E' => {
                self.move_cursor_down(self.param(0, 1));
                self.cursor_column = 0;
            }
            b'F' => {
                self.move_cursor_up(self.param(0, 1));
                self.cursor_column = 0;
            }
            b'G' | b'`' => {
                let column = self.param(0, 1).saturating_sub(1);
                self.set_cursor_column(column);
            }
            b'H' | b'f' => {
                let row = self.param(0, 1).saturating_sub(1);
                let column = self.param(1, 1).saturating_sub(1);
                self.set_cursor(column, row);
            }
            b'd' => {
                let row = self.param(0, 1).saturating_sub(1);
                self.set_cursor_row(row);
            }
            b'J' => self.erase_display(self.param(0, 0)),
            b'K' => self.erase_line(self.param(0, 0)),
            b'X' => self.erase_chars(self.param(0, 1)),
            b'S' => self.scroll_up(self.param(0, 1)),
            b'T' => self.scroll_down(self.param(0, 1)),
            b's' => self.save_cursor(),
            b'u' => self.restore_cursor(),
            b'm' => self.apply_sgr(),
            _ => {}
        }
    }

    fn put_char(&mut self, ch: char) {
        if self.cursor_row >= self.rows {
            self.scroll_up(1);
            self.cursor_row = self.rows.saturating_sub(1);
        }

        let mut cell = TextGridCell::new(ch, self.foreground, self.background);
        cell.bold = self.bold;
        cell.inverse = self.inverse;
        let _ = self
            .grid
            .set_cell(self.cursor_column, self.cursor_row, cell);

        self.cursor_column += 1;
        if self.cursor_column >= self.columns {
            self.cursor_column = 0;
            self.line_feed();
        }
    }

    fn line_feed(&mut self) {
        if self.cursor_row + 1 >= self.rows {
            self.scroll_up(1);
        } else {
            self.cursor_row += 1;
        }
    }

    fn backspace(&mut self) {
        if self.cursor_column > 0 {
            self.cursor_column -= 1;
        }
    }

    fn tab(&mut self) {
        let next = ((self.cursor_column / 8) + 1).saturating_mul(8);
        self.cursor_column = next.min(self.columns.saturating_sub(1));
    }

    fn set_cursor(&mut self, column: usize, row: usize) {
        self.set_cursor_column(column);
        self.set_cursor_row(row);
    }

    fn set_cursor_column(&mut self, column: usize) {
        self.cursor_column = column.min(self.columns.saturating_sub(1));
    }

    fn set_cursor_row(&mut self, row: usize) {
        self.cursor_row = row.min(self.rows.saturating_sub(1));
    }

    fn save_cursor(&mut self) {
        self.saved_cursor_column = self.cursor_column;
        self.saved_cursor_row = self.cursor_row;
    }

    fn restore_cursor(&mut self) {
        self.cursor_column = self.saved_cursor_column.min(self.columns.saturating_sub(1));
        self.cursor_row = self.saved_cursor_row.min(self.rows.saturating_sub(1));
    }

    fn move_cursor_up(&mut self, count: usize) {
        self.cursor_row = self.cursor_row.saturating_sub(count);
    }

    fn move_cursor_down(&mut self, count: usize) {
        self.cursor_row = self
            .cursor_row
            .saturating_add(count)
            .min(self.rows.saturating_sub(1));
    }

    fn move_cursor_right(&mut self, count: usize) {
        self.cursor_column = self
            .cursor_column
            .saturating_add(count)
            .min(self.columns.saturating_sub(1));
    }

    fn move_cursor_left(&mut self, count: usize) {
        self.cursor_column = self.cursor_column.saturating_sub(count);
    }

    fn erase_display(&mut self, mode: usize) {
        match mode {
            0 => {
                self.erase_line(0);
                for row in self.cursor_row.saturating_add(1)..self.rows {
                    self.clear_row(row);
                }
            }
            1 => {
                for row in 0..self.cursor_row {
                    self.clear_row(row);
                }
                self.erase_line(1);
            }
            2 | 3 => self.clear_all(),
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: usize) {
        match mode {
            0 => self.clear_range(self.cursor_column, self.cursor_row, self.columns),
            1 => self.clear_range(0, self.cursor_row, self.cursor_column.saturating_add(1)),
            2 => self.clear_row(self.cursor_row),
            _ => {}
        }
    }

    fn erase_chars(&mut self, count: usize) {
        let end = self.cursor_column.saturating_add(count);
        self.clear_range(self.cursor_column, self.cursor_row, end);
    }

    fn clear_all(&mut self) {
        let blank = self.blank_cell();
        self.grid.clear(blank);
        self.cursor_column = 0;
        self.cursor_row = 0;
    }

    fn clear_row(&mut self, row: usize) {
        self.clear_range(0, row, self.columns);
    }

    fn clear_range(&mut self, start_column: usize, row: usize, end_column: usize) {
        let blank = self.blank_cell();
        for column in start_column.min(self.columns)..end_column.min(self.columns) {
            let _ = self.grid.set_cell(column, row, blank);
        }
    }

    fn scroll_up(&mut self, count: usize) {
        let count = count.min(self.rows);
        if count == 0 {
            return;
        }
        for row in 0..count {
            self.push_scrollback_row(row);
        }
        for row in 0..self.rows.saturating_sub(count) {
            for column in 0..self.columns {
                if let Some(cell) = self.grid.cell(column, row + count) {
                    let _ = self.grid.set_cell(column, row, cell);
                }
            }
        }
        for row in self.rows.saturating_sub(count)..self.rows {
            self.clear_row(row);
        }
    }

    fn scroll_down(&mut self, count: usize) {
        let count = count.min(self.rows);
        if count == 0 {
            return;
        }
        for row in (count..self.rows).rev() {
            for column in 0..self.columns {
                if let Some(cell) = self.grid.cell(column, row - count) {
                    let _ = self.grid.set_cell(column, row, cell);
                }
            }
        }
        for row in 0..count {
            self.clear_row(row);
        }
    }

    fn apply_sgr(&mut self) {
        if self.csi_count == 0 {
            self.reset_attributes();
            return;
        }

        for index in 0..self.csi_count {
            match self.csi_params[index] {
                0 => self.reset_attributes(),
                1 => self.bold = true,
                22 => self.bold = false,
                7 => self.inverse = true,
                27 => self.inverse = false,
                30..=37 => self.foreground = ansi_color(self.csi_params[index] - 30),
                39 => self.foreground = Color::rgb(230, 232, 235),
                40..=47 => self.background = ansi_color(self.csi_params[index] - 40),
                49 => self.background = Color::rgb(12, 14, 18),
                _ => {}
            }
        }
    }

    fn reset_attributes(&mut self) {
        self.foreground = Color::rgb(230, 232, 235);
        self.background = Color::rgb(12, 14, 18);
        self.bold = false;
        self.inverse = false;
    }

    fn blank_cell(&self) -> TextGridCell {
        TextGridCell::blank(self.foreground, self.background)
    }

    fn push_scrollback_row(&mut self, row: usize) {
        let mut line = Vec::new();
        for column in 0..self.columns {
            line.push(
                self.grid
                    .cell(column, row)
                    .unwrap_or_else(|| self.blank_cell()),
            );
        }
        self.scrollback.push(line);
        if self.scrollback.len() > SCROLLBACK_LIMIT {
            self.scrollback.remove(0);
        }
    }
}

fn copy_line_to_grid(line: &[TextGridCell], grid: &mut TextGridBuffer, row: usize) {
    for (column, cell) in line.iter().copied().enumerate().take(grid.columns()) {
        let _ = grid.set_cell(column, row, cell);
    }
}

fn ansi_color(index: usize) -> Color {
    match index {
        0 => Color::rgb(0, 0, 0),
        1 => Color::rgb(205, 49, 49),
        2 => Color::rgb(13, 188, 121),
        3 => Color::rgb(229, 229, 16),
        4 => Color::rgb(36, 114, 200),
        5 => Color::rgb(188, 63, 188),
        6 => Color::rgb(17, 168, 205),
        _ => Color::rgb(229, 229, 229),
    }
}
