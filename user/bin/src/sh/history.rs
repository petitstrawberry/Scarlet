//! Command history management for the shell
//!
//! This module provides functionality to:
//! - Store command history in memory
//! - Navigate through history with up/down arrows
//! - Persist history to a file
//! - Load history from a file on startup

#![allow(dead_code)]

extern crate scarlet_std as std;

use std::fs::{File, OpenOptions};
use std::{format, string::String, vec::Vec};

/// Command history manager
pub struct History {
    /// Stored command entries
    entries: Vec<String>,

    /// Maximum number of entries to keep
    max_size: usize,

    /// Current position in history for navigation
    /// None = at the end (current line), Some(index) = at that position
    current_index: Option<usize>,

    /// Temporary storage for the current line when navigating history
    current_line_backup: Option<String>,
}

impl History {
    /// Create a new history manager with the specified maximum size
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_size,
            current_index: None,
            current_line_backup: None,
        }
    }

    /// Add a command to history
    pub fn add(&mut self, cmd: String) {
        // Don't add empty commands or duplicates of the last command
        if cmd.trim().is_empty() {
            return;
        }

        if let Some(last) = self.entries.last()
            && last == &cmd
        {
            return;
        }

        self.entries.push(cmd);

        // Trim to max size if needed
        if self.entries.len() > self.max_size {
            self.entries.remove(0);
        }

        // Reset navigation state
        self.reset_navigation();
    }

    /// Navigate to the previous command in history
    /// Returns the command if available, None otherwise
    pub fn prev(&mut self, current_line: &str) -> Option<&String> {
        if self.entries.is_empty() {
            return None;
        }

        match self.current_index {
            None => {
                // First time navigating back from current line
                // Save current line for later restore
                self.current_line_backup = Some(String::from(current_line));

                // Go to last entry
                self.current_index = Some(self.entries.len() - 1);
                Some(&self.entries[self.entries.len() - 1])
            }
            Some(idx) => {
                if idx > 0 {
                    // Go to previous entry
                    self.current_index = Some(idx - 1);
                    Some(&self.entries[idx - 1])
                } else {
                    // Already at the oldest entry
                    None
                }
            }
        }
    }

    /// Navigate to the next command in history
    /// Returns the command if available, None if at the end
    pub fn next(&mut self) -> Option<String> {
        match self.current_index {
            None => {
                // Already at the end
                None
            }
            Some(idx) => {
                if idx < self.entries.len() - 1 {
                    // Go to next entry
                    self.current_index = Some(idx + 1);
                    Some(self.entries[idx + 1].clone())
                } else {
                    // Reached the end, restore current line
                    self.current_index = None;
                    self.current_line_backup.take()
                }
            }
        }
    }

    /// Reset navigation state
    pub fn reset_navigation(&mut self) {
        self.current_index = None;
        self.current_line_backup = None;
    }

    /// Load history from a file
    pub fn load_from_file(&mut self, path: &str) -> Result<(), ()> {
        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return Err(()), // File doesn't exist or can't be opened
        };

        let mut content = String::new();
        let mut buffer = [0u8; 1024];

        loop {
            match file.read(&mut buffer) {
                Ok(0) => break, // EOF
                Ok(bytes_read) => {
                    if let Ok(text) = std::str::from_utf8(&buffer[..bytes_read]) {
                        content.push_str(text);
                    } else {
                        return Err(()); // Invalid UTF-8
                    }
                }
                Err(_) => return Err(()),
            }
        }

        // Parse lines
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                self.entries.push(String::from(trimmed));
            }
        }

        // Trim to max size if needed
        while self.entries.len() > self.max_size {
            self.entries.remove(0);
        }

        Ok(())
    }

    /// Save history to a file
    pub fn save_to_file(&self, path: &str) -> Result<(), ()> {
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(true);

        let mut file = match opts.open(path) {
            Ok(f) => f,
            Err(_) => return Err(()),
        };

        for entry in &self.entries {
            let line = format!("{}\n", entry);
            if file.write(line.as_bytes()).is_err() {
                return Err(());
            }
        }

        Ok(())
    }

    /// Get the number of entries in history
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if history is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
