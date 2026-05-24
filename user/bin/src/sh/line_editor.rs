//! Line editor with cursor movement and history support
//!
//! This module provides a line editor for the Scarlet shell that supports:
//! - Cursor movement with arrow keys
//! - Character insertion and deletion at any position
//! - Home/End keys
//! - Command history navigation

#![allow(dead_code)]

extern crate scarlet_std as std;

use std::handle::Handle;
use std::{
    print,
    string::String,
    tty::{KeyboardMode, ReadPolicy, Terminal},
    vec::Vec,
};

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
    Continue,    // Continue editing
    Submit,      // Submit the line (Enter pressed)
    Interrupt,   // Ctrl-C pressed
    HistoryPrev, // Up arrow
    HistoryNext, // Down arrow
}

/// Line editor with cursor support
pub struct LineEditor {
    buffer: Vec<char>,
    cursor: usize,
    prompt: String,
    raw_mode_enabled: bool,
    stdin_handle: Option<Handle>,
    saved_signal_chars_enabled: Option<bool>,
    // Escape sequence parsing state (0=none, 1=got ESC, 2=got ESC [, 4=got ESC O)
    esc_state: u8,
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
            saved_signal_chars_enabled: None,
            esc_state: 0,
        }
    }

    /// Enable or disable raw mode
    pub fn set_raw_mode(&mut self, enabled: bool) -> Result<(), ()> {
        // Use stdin handle (fd 0) for TTY control
        if self.stdin_handle.is_none() {
            // Create handle from stdin (file descriptor 0)
            self.stdin_handle = unsafe { Handle::from_raw(0) }.ok();
        }

        if let Some(ref handle) = self.stdin_handle {
            let terminal = Terminal::from_handle(handle);

            if terminal.set_canonical(!enabled).is_err() {
                return Err(());
            }
            if terminal.set_echo(!enabled).is_err() {
                return Err(());
            }
            if enabled && self.saved_signal_chars_enabled.is_none() {
                self.saved_signal_chars_enabled = terminal.signal_chars_enabled().ok();
            }
            let signal_chars_enabled = if enabled {
                false
            } else {
                self.saved_signal_chars_enabled.take().unwrap_or(true)
            };
            if terminal
                .set_signal_chars_enabled(signal_chars_enabled)
                .is_err()
            {
                return Err(());
            }

            // Use XLATE mode so all ASCII characters (including symbols) pass through
            if terminal.set_keyboard_mode(KeyboardMode::Xlate).is_err() {
                print!("DEBUG: Failed to set keyboard mode!\n");
                return Err(());
            }

            if terminal.set_read_policy(ReadPolicy::new(1, 0)).is_err() {
                if enabled {
                    print!("DEBUG: Failed to set read policy!\n");
                }
                return Err(());
            }
            if !enabled {
                print!("DEBUG: Canonical mode restored\n");
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
    pub fn read_line_with_history(
        &mut self,
        history: &mut crate::history::History,
    ) -> Result<String, ()> {
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
            c if ('\x20'..='\x7e').contains(&c) => {
                // Printable character
                self.buffer.insert(self.cursor, c);
                self.cursor += 1;
                print!("{}", c);
                EditorAction::Continue
            }
            _ => EditorAction::Continue,
        }
    }

    /// Handle a character in raw mode (XLATE mode - ASCII + escape sequences)
    fn handle_raw_key(&mut self, byte: u32) -> EditorAction {
        let ch = byte as u8 as char;

        // Handle escape sequences for arrow keys
        match (self.esc_state, byte as u8) {
            (0, 0x1B) => {
                // ESC pressed - start escape sequence
                self.esc_state = 1;
                return EditorAction::Continue;
            }
            (1, b'[') => {
                // ESC [ - CSI sequence
                self.esc_state = 2;
                return EditorAction::Continue;
            }
            (1, b'O') => {
                // ESC O - SS3 sequence
                self.esc_state = 4;
                return EditorAction::Continue;
            }
            (4, b'A') => {
                self.esc_state = 0;
                return EditorAction::HistoryPrev;
            }
            (4, b'B') => {
                self.esc_state = 0;
                return EditorAction::HistoryNext;
            }
            (4, b'C') => {
                self.esc_state = 0;
                self.move_cursor_right();
                return EditorAction::Continue;
            }
            (4, b'D') => {
                self.esc_state = 0;
                self.move_cursor_left();
                return EditorAction::Continue;
            }
            (4, b'H') => {
                self.esc_state = 0;
                self.move_cursor_home();
                return EditorAction::Continue;
            }
            (4, b'F') => {
                self.esc_state = 0;
                self.move_cursor_end();
                return EditorAction::Continue;
            }
            (2, b'A') => {
                // ESC [ A - Up arrow
                self.esc_state = 0;
                return EditorAction::HistoryPrev;
            }
            (2, b'B') => {
                // ESC [ B - Down arrow
                self.esc_state = 0;
                return EditorAction::HistoryNext;
            }
            (2, b'C') => {
                // ESC [ C - Right arrow
                self.esc_state = 0;
                self.move_cursor_right();
                return EditorAction::Continue;
            }
            (2, b'D') => {
                // ESC [ D - Left arrow
                self.esc_state = 0;
                self.move_cursor_left();
                return EditorAction::Continue;
            }
            (2, b'H') => {
                // ESC [ H - Home
                self.esc_state = 0;
                self.move_cursor_home();
                return EditorAction::Continue;
            }
            (2, b'F') => {
                // ESC [ F - End
                self.esc_state = 0;
                self.move_cursor_end();
                return EditorAction::Continue;
            }
            (2, b'1' | b'2' | b'3' | b'4' | b'5' | b'6' | b'7' | b'8') => {
                // ESC [ n - might be Home/End/Delete (ESC [ n ~)
                self.esc_state = byte as u8;
                return EditorAction::Continue;
            }
            (b'1', b'~') | (b'7', b'~') => {
                // ESC [ 1 ~ / ESC [ 7 ~ - Home
                self.esc_state = 0;
                self.move_cursor_home();
                return EditorAction::Continue;
            }
            (b'4', b'~') | (b'8', b'~') => {
                // ESC [ 4 ~ / ESC [ 8 ~ - End
                self.esc_state = 0;
                self.move_cursor_end();
                return EditorAction::Continue;
            }
            (b'3', b'~') => {
                // ESC [ 3 ~ - Delete
                self.esc_state = 0;
                self.delete_char();
                return EditorAction::Continue;
            }
            (b'2', b'~') | (b'5', b'~') | (b'6', b'~') => {
                // ESC [ 2 ~/5~/6~ - Insert/PageUp/PageDown. Consume for now.
                self.esc_state = 0;
                return EditorAction::Continue;
            }
            _ if self.esc_state != 0 => {
                // Invalid escape sequence, reset
                self.esc_state = 0;
                return EditorAction::Continue;
            }
            _ => {}
        }

        // Handle regular ASCII characters
        match ch {
            '\r' | '\n' => EditorAction::Submit,
            '\x03' => EditorAction::Interrupt, // Ctrl-C
            '\x7f' | '\x08' => {
                // Backspace/DEL
                if self.cursor > 0 {
                    self.backspace();
                }
                EditorAction::Continue
            }
            '\t' => {
                // Tab completion
                self.handle_tab_completion();
                EditorAction::Continue
            }
            c if (' '..='~').contains(&c) => {
                // Printable ASCII
                self.insert_char(c);
                EditorAction::Continue
            }
            _ => EditorAction::Continue,
        }
    }

    /// Insert a character at the cursor position
    fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);

        // In raw mode, manually output the character and everything after it
        // Print from current cursor position to end
        for i in self.cursor..self.buffer.len() {
            print!("{}", self.buffer[i]);
        }

        // If we inserted in the middle, move cursor back to correct position
        let chars_after = self.buffer.len() - self.cursor - 1;
        if chars_after > 0 {
            // Move cursor left by the number of characters we printed after insertion
            for _ in 0..chars_after {
                print!("\x1b[D");
            }
        }

        self.cursor += 1;
    }

    /// Delete character before cursor (backspace)
    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.buffer.remove(self.cursor - 1);
            self.cursor -= 1;

            // Move cursor left
            print!("\x1b[D");

            // Print remaining characters and clear to end
            for i in self.cursor..self.buffer.len() {
                print!("{}", self.buffer[i]);
            }
            print!(" \x1b[K"); // Space to clear the last char, then clear to EOL

            // Move cursor back to correct position
            let chars_after = self.buffer.len() - self.cursor;
            for _ in 0..=chars_after {
                print!("\x1b[D");
            }
        }
    }

    /// Delete character at cursor position (delete key)
    fn delete_char(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);

            // Print remaining characters and clear to end
            for i in self.cursor..self.buffer.len() {
                print!("{}", self.buffer[i]);
            }
            print!(" \x1b[K"); // Space to clear the last char, then clear to EOL

            // Move cursor back to correct position
            let chars_after = self.buffer.len() - self.cursor;
            for _ in 0..=chars_after {
                print!("\x1b[D");
            }
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
        // Clear and redraw the physical line, then move back to the logical cursor.
        print!("\r\x1b[2K");
        print!("{}", self.prompt);
        for c in &self.buffer {
            print!("{}", c);
        }

        let cursor_col = self.prompt.len() + self.cursor;
        print!("\r");
        if cursor_col > 0 {
            print!("\x1b[{}C", cursor_col);
        }
    }

    /// Handle tab completion
    fn handle_tab_completion(&mut self) {
        // Get the current line as a string
        let line: String = self.buffer.iter().collect();
        let words: Vec<&str> = line.split_whitespace().collect();

        // Find the word at cursor position
        let (word_start, word_to_complete) = self.get_word_at_cursor();

        if words.is_empty() || (words.len() == 1 && !line.ends_with(' ') && word_start == 0) {
            // First word - complete command
            self.complete_command(word_to_complete, word_start);
        } else {
            // Other words - complete filename
            self.complete_filename(word_to_complete, word_start);
        }
    }

    /// Get the word at the cursor position
    fn get_word_at_cursor(&self) -> (usize, String) {
        let line: String = self.buffer.iter().collect();
        let bytes = line.as_bytes();

        // Find word boundaries
        let mut start = self.cursor;
        while start > 0 && bytes[start - 1] != b' ' && bytes[start - 1] != b'\t' {
            start -= 1;
        }

        let mut end = self.cursor;
        while end < bytes.len() && bytes[end] != b' ' && bytes[end] != b'\t' {
            end += 1;
        }

        let word = String::from(&line[start..end]);
        (start, word)
    }

    /// Complete command name from PATH
    fn complete_command(&mut self, prefix: String, word_start: usize) {
        let mut matches = Vec::new();

        // Get PATH and search for matching executables
        if let Some(path) = std::env::var("PATH") {
            for dir in path.split(':') {
                if let Ok(entries) = std::fs::list_directory(dir) {
                    for entry in entries {
                        let name = String::from(&entry.name[..]);
                        if name.starts_with(&prefix) {
                            matches.push(name);
                        }
                    }
                }
            }
        }

        matches.sort();
        matches.dedup();

        self.apply_completion(matches, prefix.len(), word_start);
    }

    /// Complete filename from current directory or specified path
    fn complete_filename(&mut self, prefix: String, word_start: usize) {
        let mut matches = Vec::new();

        // Check if prefix contains a path separator
        let (dir_part, file_part) = if let Some(last_slash) = prefix.rfind('/') {
            // Has path separator - extract directory and filename parts
            let dir = if last_slash == 0 {
                // Absolute path starting with /
                String::from("/")
            } else {
                String::from(&prefix[..=last_slash])
            };
            let file = String::from(&prefix[last_slash + 1..]);
            (dir, file)
        } else {
            // No path separator - use current directory
            (String::from("."), prefix.clone())
        };

        // List files in the directory
        if let Ok(entries) = std::fs::list_directory(&dir_part) {
            for entry in entries {
                let name = String::from(&entry.name[..]);

                // Skip . and .. unless explicitly typed
                if (name == "." || name == "..") && !file_part.starts_with('.') {
                    continue;
                }

                if name.starts_with(&file_part) {
                    // Build full completion path
                    let mut completion = if dir_part == "." {
                        // Current directory - just use filename
                        name.clone()
                    } else {
                        // Other directory - include directory part
                        let mut full_path = dir_part.clone();
                        if !full_path.ends_with('/') {
                            full_path.push('/');
                        }
                        full_path.push_str(&name);
                        full_path
                    };

                    // Add trailing / for directories
                    if entry.file_type == 1 {
                        // Directory
                        completion.push('/');
                    }
                    matches.push(completion);
                }
            }
        }

        matches.sort();

        self.apply_completion(matches, prefix.len(), word_start);
    }

    /// Apply completion based on matches
    fn apply_completion(&mut self, matches: Vec<String>, prefix_len: usize, word_start: usize) {
        if matches.is_empty() {
            // No matches - beep or do nothing
            return;
        }

        if matches.len() == 1 {
            // Single match - complete it
            let _completion = &matches[0][prefix_len..];

            // Remove old word and insert new completion
            for _ in 0..prefix_len {
                if word_start < self.buffer.len() {
                    self.buffer.remove(word_start);
                }
            }

            // Insert completion
            let completion_chars: Vec<char> = matches[0].chars().collect();
            for (i, ch) in completion_chars.iter().enumerate() {
                self.buffer.insert(word_start + i, *ch);
            }

            self.cursor = word_start + completion_chars.len();

            // Redraw line
            self.redraw_line();
        } else {
            // Multiple matches - show them
            print!("\n");
            for (i, m) in matches.iter().enumerate() {
                print!("{}  ", m);
                if (i + 1) % 5 == 0 {
                    print!("\n");
                }
            }
            if !matches.len().is_multiple_of(5) {
                print!("\n");
            }

            // Find common prefix
            let common_prefix = self.find_common_prefix(&matches);
            if common_prefix.len() > prefix_len {
                // Complete to common prefix
                let completion = &common_prefix[prefix_len..];
                for ch in completion.chars() {
                    self.buffer.insert(self.cursor, ch);
                    self.cursor += 1;
                }
            }

            // Redraw prompt and line
            print!("{}", self.prompt);
            for c in &self.buffer {
                print!("{}", c);
            }
        }
    }

    /// Find common prefix of strings
    fn find_common_prefix(&self, strings: &[String]) -> String {
        if strings.is_empty() {
            return String::new();
        }

        let first = &strings[0];
        let mut prefix_len = first.len();

        for s in &strings[1..] {
            let mut i = 0;
            while i < prefix_len && i < s.len() && first.chars().nth(i) == s.chars().nth(i) {
                i += 1;
            }
            prefix_len = i;
        }

        first.chars().take(prefix_len).collect()
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
