//! Line editor with cursor movement and history support
//!
//! This module provides a line editor for the Scarlet shell that supports:
//! - Cursor movement with arrow keys
//! - Character insertion and deletion at any position
//! - Home/End keys
//! - Command history navigation

#![allow(dead_code)]

extern crate scarlet_std as std;

use std::{print, string::String, vec::Vec};
use std::handle::Handle;

// TTY control opcodes (from kernel investigation)
const SCTL_TTY_SET_ECHO: u32 = 0x5354_0001;
const SCTL_TTY_SET_CANONICAL: u32 = 0x5354_0003;
const SCTL_TTY_FLUSH_INPUT: u32 = 0x5354_0009;

// Linux keycodes for special keys (from TTY device)
const KEY_UP: u32 = 103;
const KEY_DOWN: u32 = 108;
const KEY_LEFT: u32 = 105;
const KEY_RIGHT: u32 = 106;
const KEY_HOME: u32 = 102;
const KEY_END: u32 = 107;
const KEY_DELETE: u32 = 111;

/// Actions that can result from handling a key
#[derive(Debug, PartialEq)]
enum EditorAction {
    Continue,       // Continue editing
    Submit,         // Submit the line (Enter pressed)
    Interrupt,      // Ctrl-C pressed
    HistoryPrev,    // Up arrow
    HistoryNext,    // Down arrow
}

/// Line editor with cursor support
pub struct LineEditor {
    buffer: Vec<char>,
    cursor: usize,
    prompt: String,
    raw_mode_enabled: bool,
    stdin_handle: Option<Handle>,
}

impl LineEditor {
    /// Create a new line editor with the given prompt
    pub fn new(prompt: &str) -> Self {
        Self {
            buffer: Vec::new(),
            cursor: 0,
            prompt: String::from(prompt),
            raw_mode_enabled: false,
            stdin_handle: None,
        }
    }

    /// Enable or disable raw mode
    pub fn set_raw_mode(&mut self, enabled: bool) -> Result<(), ()> {
        // Use stdin handle (fd 0) for TTY control
        if self.stdin_handle.is_none() {
            // Create handle from stdin (file descriptor 0)
            self.stdin_handle = Some(unsafe { Handle::from_raw(0) });
        }

        if let Some(ref handle) = self.stdin_handle {
            // Set canonical mode (opposite of raw mode)
            let canonical_value = if enabled { 0 } else { 1 };
            if handle.control(SCTL_TTY_SET_CANONICAL, canonical_value).is_err() {
                return Err(());
            }

            // Set echo (off in raw mode, on in canonical mode)
            let echo_value = if enabled { 0 } else { 1 };
            if handle.control(SCTL_TTY_SET_ECHO, echo_value).is_err() {
                return Err(());
            }

            self.raw_mode_enabled = enabled;
            Ok(())
        } else {
            Err(())
        }
    }

    /// Read a line from the user
    pub fn read_line(&mut self) -> Result<String, ()> {
        // Clear buffer and reset cursor
        self.buffer.clear();
        self.cursor = 0;

        // Display prompt
        print!("{}", self.prompt);

        loop {
            let c = std::io::get_char();

            // In raw mode, we get keycodes; in canonical mode, we get ASCII
            let action = if self.raw_mode_enabled {
                self.handle_raw_key(c as u32)
            } else {
                self.handle_canonical_char(c)
            };

            match action {
                EditorAction::Continue => {
                    // Continue editing
                }
                EditorAction::Submit => {
                    // Line complete
                    print!("\n");
                    return Ok(self.buffer.iter().collect());
                }
                EditorAction::Interrupt => {
                    // Ctrl-C
                    print!("^C\n");
                    self.buffer.clear();
                    return Err(());
                }
                EditorAction::HistoryPrev | EditorAction::HistoryNext => {
                    // History navigation (handled externally)
                    // For now, just ignore
                }
            }
        }
    }

    /// Read a line from the user with history support
    pub fn read_line_with_history(&mut self, history: &mut crate::history::History) -> Result<String, ()> {
        // Clear buffer and reset cursor
        self.buffer.clear();
        self.cursor = 0;

        // Reset history navigation
        history.reset_navigation();

        // Display prompt
        print!("{}", self.prompt);

        loop {
            let c = std::io::get_char();

            // In raw mode, we get keycodes; in canonical mode, we get ASCII
            let action = if self.raw_mode_enabled {
                self.handle_raw_key(c as u32)
            } else {
                self.handle_canonical_char(c)
            };

            match action {
                EditorAction::Continue => {
                    // Continue editing
                }
                EditorAction::Submit => {
                    // Line complete
                    print!("\n");
                    history.reset_navigation();
                    return Ok(self.buffer.iter().collect());
                }
                EditorAction::Interrupt => {
                    // Ctrl-C
                    print!("^C\n");
                    self.buffer.clear();
                    history.reset_navigation();
                    return Err(());
                }
                EditorAction::HistoryPrev => {
                    // Navigate to previous history entry
                    let current_buffer = self.buffer_content();
                    if let Some(prev_cmd) = history.prev(&current_buffer) {
                        self.replace_buffer(prev_cmd);
                    }
                }
                EditorAction::HistoryNext => {
                    // Navigate to next history entry
                    if let Some(next_cmd) = history.next() {
                        self.replace_buffer(&next_cmd);
                    }
                }
            }
        }
    }

    /// Handle a character in canonical mode
    fn handle_canonical_char(&mut self, c: char) -> EditorAction {
        match c {
            '\n' => EditorAction::Submit,
            '\x7f' | '\x08' => {
                // Backspace
                if self.cursor > 0 {
                    self.buffer.remove(self.cursor - 1);
                    self.cursor -= 1;
                    // Echo backspace
                    print!("\x08 \x08");
                }
                EditorAction::Continue
            }
            '\x03' => {
                // Ctrl-C
                EditorAction::Interrupt
            }
            c if c >= '\x20' && c <= '\x7e' => {
                // Printable character
                self.buffer.insert(self.cursor, c);
                self.cursor += 1;
                print!("{}", c);
                EditorAction::Continue
            }
            _ => EditorAction::Continue,
        }
    }

    /// Handle a keycode in raw mode
    fn handle_raw_key(&mut self, key: u32) -> EditorAction {
        match key {
            // Enter key (0x0A in raw mode)
            10 => EditorAction::Submit,

            // Backspace (0x7F or 0x08)
            0x7f | 0x08 => {
                if self.cursor > 0 {
                    self.backspace();
                }
                EditorAction::Continue
            }

            // Ctrl-C (0x03)
            3 => EditorAction::Interrupt,

            // Arrow keys
            KEY_LEFT => {
                self.move_cursor_left();
                EditorAction::Continue
            }
            KEY_RIGHT => {
                self.move_cursor_right();
                EditorAction::Continue
            }
            KEY_UP => EditorAction::HistoryPrev,
            KEY_DOWN => EditorAction::HistoryNext,

            // Home/End
            KEY_HOME => {
                self.move_cursor_home();
                EditorAction::Continue
            }
            KEY_END => {
                self.move_cursor_end();
                EditorAction::Continue
            }

            // Delete key
            KEY_DELETE => {
                self.delete_char();
                EditorAction::Continue
            }

            // Printable characters (ASCII 0x20-0x7E)
            key if key >= 0x20 && key <= 0x7e => {
                self.insert_char(key as u8 as char);
                EditorAction::Continue
            }

            _ => EditorAction::Continue,
        }
    }

    /// Insert a character at the cursor position
    fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += 1;

        // Redraw from cursor to end
        self.redraw_from_cursor();
    }

    /// Delete character before cursor (backspace)
    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.buffer.remove(self.cursor - 1);
            self.cursor -= 1;

            // Move cursor left, redraw rest of line, move cursor back
            self.redraw_from_cursor();
        }
    }

    /// Delete character at cursor position (delete key)
    fn delete_char(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);

            // Redraw from cursor to end
            self.redraw_from_cursor();
        }
    }

    /// Move cursor left
    fn move_cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            // ANSI escape: move cursor left
            print!("\x1b[D");
        }
    }

    /// Move cursor right
    fn move_cursor_right(&mut self) {
        if self.cursor < self.buffer.len() {
            self.cursor += 1;
            // ANSI escape: move cursor right
            print!("\x1b[C");
        }
    }

    /// Move cursor to beginning of line
    fn move_cursor_home(&mut self) {
        while self.cursor > 0 {
            self.cursor -= 1;
            print!("\x1b[D");
        }
    }

    /// Move cursor to end of line
    fn move_cursor_end(&mut self) {
        while self.cursor < self.buffer.len() {
            self.cursor += 1;
            print!("\x1b[C");
        }
    }

    /// Redraw the line from cursor position to end
    fn redraw_from_cursor(&self) {
        // Save cursor position
        print!("\x1b[s");

        // Print from cursor to end
        for i in self.cursor..self.buffer.len() {
            print!("{}", self.buffer[i]);
        }

        // Clear to end of line
        print!("\x1b[K");

        // Restore cursor position
        print!("\x1b[u");
    }

    /// Redraw the entire line
    fn redraw_line(&self) {
        // Move to beginning of line
        print!("\r");

        // Print prompt and buffer
        print!("{}", self.prompt);
        for c in &self.buffer {
            print!("{}", c);
        }

        // Clear to end of line
        print!("\x1b[K");

        // Move cursor to correct position
        let cursor_col = self.prompt.len() + self.cursor;
        print!("\r\x1b[{}G", cursor_col + 1);
    }

    /// Replace the buffer with new content (for history navigation)
    pub fn replace_buffer(&mut self, new_content: &str) {
        self.buffer.clear();
        for c in new_content.chars() {
            self.buffer.push(c);
        }
        self.cursor = self.buffer.len();

        // Redraw entire line
        self.redraw_line();
    }

    /// Get current buffer content
    pub fn buffer_content(&self) -> String {
        self.buffer.iter().collect()
    }
}

impl Drop for LineEditor {
    fn drop(&mut self) {
        // Restore canonical mode when editor is dropped
        if self.raw_mode_enabled {
            let _ = self.set_raw_mode(false);
        }
    }
}
