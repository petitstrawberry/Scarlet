//! Terminal buffer and screen management
//!
//! This module handles the terminal buffer, cursor position, and rendering.

use scarlet_std::string::String;
use crate::vtparser::VtAction;

/// Text attribute for terminal cells
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextAttr {
    pub fg_color: (u8, u8, u8),
    pub bg_color: (u8, u8, u8),
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub reverse: bool,
}

impl Default for TextAttr {
    fn default() -> Self {
        Self {
            fg_color: (0, 255, 0), // Default green
            bg_color: (20, 20, 20),
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            blink: false,
            reverse: false,
        }
    }
}

/// Terminal cell - single character with attributes
#[derive(Debug, Clone)]
pub struct TerminalCell {
    pub c: char,
    pub attr: TextAttr,
}

impl TerminalCell {
    pub fn new(c: char) -> Self {
        Self {
            c,
            attr: TextAttr::default(),
        }
    }

    pub fn with_attr(c: char, attr: TextAttr) -> Self {
        Self { c, attr }
    }
}

impl Default for TerminalCell {
    fn default() -> Self {
        Self::new(' ')
    }
}

/// Saved cursor state
#[derive(Debug, Clone)]
struct SavedCursor {
    x: usize,
    y: usize,
    attr: TextAttr,
}

/// Terminal buffer - holds all displayed text
pub struct TerminalBuffer {
    cells: scarlet_std::vec::Vec<TerminalCell>,
    width: usize,
    height: usize,
    cursor_x: usize,
    cursor_y: usize,
    attr: TextAttr,
    saved_cursor: Option<SavedCursor>,
    scroll_top: usize,
    scroll_bottom: usize,
    tab_stops: scarlet_std::vec::Vec<bool>,
}

impl TerminalBuffer {
    pub fn new(width: usize, height: usize) -> Self {
        let total_cells = width * height;
        let mut cells = scarlet_std::vec::Vec::with_capacity(total_cells);

        // Initialize with spaces
        for _ in 0..total_cells {
            cells.push(TerminalCell::default());
        }

        // Initialize tab stops every 8 columns
        let mut tab_stops = scarlet_std::vec::Vec::new();
        for i in 0..width {
            tab_stops.push(i % 8 == 0 && i != 0);
        }

        Self {
            cells,
            width,
            height,
            cursor_x: 0,
            cursor_y: 0,
            attr: TextAttr::default(),
            saved_cursor: None,
            scroll_top: 0,
            scroll_bottom: height,
            tab_stops,
        }
    }

    /// Write a character at the current cursor position
    pub fn write_char(&mut self, c: char) {
        if self.cursor_x >= self.width {
            self.line_feed();
        }

        let index = self.cursor_y * self.width + self.cursor_x;
        if index < self.cells.len() {
            self.cells[index] = TerminalCell::with_attr(c, self.attr);
        }

        self.cursor_x += 1;
    }

    /// Move to next line
    fn line_feed(&mut self) {
        self.cursor_x = 0;
        if self.cursor_y < self.scroll_bottom - 1 {
            self.cursor_y += 1;
        } else {
            // Scroll up within scroll region
            self.scroll_up(1);
        }
    }

    /// Carriage return - move to start of line
    pub fn carriage_return(&mut self) {
        self.cursor_x = 0;
    }

    /// Backspace - move cursor back
    pub fn backspace(&mut self) {
        if self.cursor_x > 0 {
            self.cursor_x -= 1;
        }
    }

    /// Tab - move to next tab stop
    pub fn tab(&mut self) {
        // Find next tab stop
        for i in (self.cursor_x + 1)..self.width {
            if self.tab_stops[i] {
                self.cursor_x = i;
                return;
            }
        }
        self.cursor_x = self.width - 1;
    }

    /// Scroll the buffer up by n lines
    fn scroll_up(&mut self, n: usize) {
        for _ in 0..n {
            // Shift lines up within scroll region
            for y in self.scroll_top..(self.scroll_bottom - 1) {
                for x in 0..self.width {
                    let src = y * self.width + x + self.width;
                    let dst = y * self.width + x;
                    if src < self.cells.len() && dst < self.cells.len() {
                        self.cells[dst] = self.cells[src].clone();
                    }
                }
            }

            // Clear bottom line
            let y = self.scroll_bottom - 1;
            for x in 0..self.width {
                let index = y * self.width + x;
                if index < self.cells.len() {
                    self.cells[index] = TerminalCell::default();
                }
            }
        }
    }

    /// Process a VT action
    pub fn process_action(&mut self, action: &VtAction) {
        match action {
            VtAction::None => {}
            VtAction::Print(c) => self.write_char(*c),
            VtAction::CarriageReturn => self.carriage_return(),
            VtAction::LineFeed => self.line_feed(),
            VtAction::Tab => self.tab(),
            VtAction::Backspace => self.backspace(),

            VtAction::CursorUp(n) => {
                self.cursor_y = self.cursor_y.saturating_sub(*n as usize);
                if self.cursor_y < self.scroll_top {
                    self.cursor_y = self.scroll_top;
                }
            }
            VtAction::CursorDown(n) => {
                self.cursor_y = (self.cursor_y + *n as usize).min(self.scroll_bottom - 1);
            }
            VtAction::CursorRight(n) => {
                self.cursor_x = (self.cursor_x + *n as usize).min(self.width - 1);
            }
            VtAction::CursorLeft(n) => {
                self.cursor_x = self.cursor_x.saturating_sub(*n as usize);
            }
            VtAction::CursorPosition(row, col) => {
                self.cursor_y = ((*row as usize).saturating_sub(1)).min(self.height - 1);
                self.cursor_x = ((*col as usize).saturating_sub(1)).min(self.width - 1);
            }

            VtAction::EraseLine(mode) => {
                let start = match mode {
                    0 => self.cursor_y * self.width + self.cursor_x,
                    1 => self.cursor_y * self.width,
                    2 => self.cursor_y * self.width,
                    _ => return,
                };
                let end = match mode {
                    0 => (self.cursor_y + 1) * self.width,
                    1 => self.cursor_y * self.width + self.cursor_x + 1,
                    2 => (self.cursor_y + 1) * self.width,
                    _ => return,
                };
                for i in start..end.min(self.cells.len()) {
                    self.cells[i] = TerminalCell::default();
                }
            }
            VtAction::EraseDisplay(mode) => {
                match mode {
                    0 => {
                        // Erase from cursor to end
                        for i in (self.cursor_y * self.width + self.cursor_x)..self.cells.len() {
                            self.cells[i] = TerminalCell::default();
                        }
                    }
                    1 => {
                        // Erase from start to cursor
                        for i in 0..=(self.cursor_y * self.width + self.cursor_x) {
                            self.cells[i] = TerminalCell::default();
                        }
                    }
                    2 => {
                        // Erase entire display
                        for cell in &mut self.cells {
                            *cell = TerminalCell::default();
                        }
                    }
                    _ => {}
                }
            }

            VtAction::SetAttributes(params) => {
                for param in params {
                    self.set_attribute(*param);
                }
            }
            VtAction::SetAttribute(param) => {
                self.set_attribute(*param);
            }

            VtAction::SaveCursor => {
                self.saved_cursor = Some(SavedCursor {
                    x: self.cursor_x,
                    y: self.cursor_y,
                    attr: self.attr,
                });
            }
            VtAction::RestoreCursor => {
                if let Some(saved) = &self.saved_cursor {
                    self.cursor_x = saved.x;
                    self.cursor_y = saved.y;
                    self.attr = saved.attr;
                }
            }

            _ => {
                // Other actions not yet implemented
            }
        }
    }

    fn set_attribute(&mut self, param: u16) {
        match param {
            0 => self.attr = TextAttr::default(),
            1 => self.attr.bold = true,
            2 => self.attr.dim = true,
            3 => self.attr.italic = true,
            4 => self.attr.underline = true,
            5 => self.attr.blink = true,
            7 => self.attr.reverse = true,
            21 => self.attr.bold = false,
            22 => self.attr.dim = false,
            23 => self.attr.italic = false,
            24 => self.attr.underline = false,
            25 => self.attr.blink = false,
            27 => self.attr.reverse = false,
            30..=37 => {
                // Set foreground color (dark)
                let color = param - 30;
                self.attr.fg_color = self.ansi_color(color, false);
            }
            38 => {
                // TODO: Set foreground color (256 color or RGB)
            }
            39 => {
                self.attr.fg_color = (0, 255, 0); // Default
            }
            40..=47 => {
                // Set background color (dark)
                let color = param - 40;
                self.attr.bg_color = self.ansi_color(color, false);
            }
            48 => {
                // TODO: Set background color (256 color or RGB)
            }
            49 => {
                self.attr.bg_color = (20, 20, 20); // Default
            }
            90..=97 => {
                // Set foreground color (bright)
                let color = param - 90;
                self.attr.fg_color = self.ansi_color(color, true);
            }
            100..=107 => {
                // Set background color (bright)
                let color = param - 100;
                self.attr.bg_color = self.ansi_color(color, true);
            }
            _ => {}
        }
    }

    fn ansi_color(&self, color: u16, bright: bool) -> (u8, u8, u8) {
        let intensity = if bright { 255 } else { 128 };
        match color {
            0 => (0, 0, 0),           // Black
            1 => (intensity, 0, 0),  // Red
            2 => (0, intensity, 0),  // Green
            3 => (intensity, intensity, 0), // Yellow
            4 => (0, 0, intensity),  // Blue
            5 => (intensity, 0, intensity), // Magenta
            6 => (0, intensity, intensity), // Cyan
            7 => (intensity, intensity, intensity), // White
            _ => (200, 200, 200),
        }
    }

    /// Get the buffer contents as a string for rendering
    pub fn to_string(&self) -> String {
        let mut result = scarlet_std::string::String::new();
        for y in 0..self.height {
            for x in 0..self.width {
                let index = y * self.width + x;
                if index < self.cells.len() {
                    result.push(self.cells[index].c);
                }
            }
            result.push('\n');
        }
        result
    }

    /// Get buffer dimensions
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Get cursor position
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_x, self.cursor_y)
    }
}

/// Terminal emulator state
pub struct TerminalEmulator {
    buffer: TerminalBuffer,
    parser: crate::vtparser::VtParser,
}

impl TerminalEmulator {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            buffer: TerminalBuffer::new(width, height),
            parser: crate::vtparser::VtParser::new(),
        }
    }

    /// Process input from PTY master and update buffer
    pub fn process_input(&mut self, data: &[u8]) {
        let actions = self.parser.parse_buffer(data);
        for action in &actions {
            self.buffer.process_action(action);
        }
    }

    /// Get current buffer contents for rendering
    pub fn contents(&self) -> String {
        self.buffer.to_string()
    }

    /// Get buffer size
    pub fn size(&self) -> (usize, usize) {
        self.buffer.size()
    }

    /// Resize the terminal
    pub fn resize(&mut self, width: usize, height: usize) {
        self.buffer = TerminalBuffer::new(width, height);
    }
}
