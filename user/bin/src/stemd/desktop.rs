//! .desktop file parser and application registry
//!
//! This module implements parsing of XDG Desktop Entry files (.desktop files)
//! and manages the global application registry.
//!
//! # Desktop Entry Format
//!
//! ```text
//! [Desktop Entry]
//! Name=Application Name
//! Exec=/path/to/executable
//! Icon=app-icon
//! Type=Application
//! Terminal=false
//! ```

use std::fs::File;
use std::format;
use std::fs::list_directory;
use std::println;
use std::string::{String, ToString};
use std::vec::Vec;

/// Application definition from .desktop file
#[derive(Debug, Clone)]
pub struct DesktopEntry {
    pub app_id: String,
    pub name: String,
    pub exec: String,
    pub icon: Option<String>,
    pub terminal: bool,
}

/// .desktop file parser
pub struct DesktopParser {
    content: String,
}

impl DesktopParser {
    pub fn new(content: String) -> Self {
        Self { content }
    }

    /// Parse a .desktop file content
    pub fn parse(&self, filename: &str) -> Option<DesktopEntry> {
        // Extract app_id from filename (e.g., "foo.desktop" -> "foo")
        let app_id = filename.strip_suffix(".desktop")?.to_string();

        let lines: Vec<&str> = self.content.lines().collect();
        let mut in_desktop_entry = false;
        let mut name = None;
        let mut exec = None;
        let mut icon = None;
        let mut terminal = false;

        for line in lines {
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Check for [Desktop Entry] section
            if line == "[Desktop Entry]" {
                in_desktop_entry = true;
                continue;
            }

            // Stop if we hit another section
            if line.starts_with('[') && line != "[Desktop Entry]" {
                break;
            }

            if !in_desktop_entry {
                continue;
            }

            // Parse key=value
            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim();
                let value = line[eq_pos + 1..].trim();

                match key {
                    "Name" => name = Some(Self::unquote(value)),
                    "Exec" => exec = Some(Self::unquote(value)),
                    "Icon" => icon = Some(Self::unquote(value)),
                    "Terminal" => terminal = value == "true" || value == "1",
                    _ => {}
                }
            }
        }

        // Require at least Name and Exec
        let name = name?;
        let exec = exec?;

        Some(DesktopEntry {
            app_id,
            name,
            exec,
            icon,
            terminal,
        })
    }

    /// Remove surrounding quotes from a string
    fn unquote(s: &str) -> String {
        let s = s.trim();
        if ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
            && s.len() >= 2
        {
            return s[1..s.len() - 1].to_string();
        }
        s.to_string()
    }
}

// Global application registry
// Maps app_id to DesktopEntry
static mut APP_REGISTRY: Vec<DesktopEntry> = Vec::new();

/// Add an application to the registry
pub fn register_app(entry: DesktopEntry) {
    unsafe {
        // Remove existing entry with same app_id
        APP_REGISTRY.retain(|e| e.app_id != entry.app_id);
        APP_REGISTRY.push(entry);
    }
}

/// Look up an application by app_id
pub fn lookup_app(app_id: &str) -> Option<DesktopEntry> {
    unsafe {
        APP_REGISTRY.iter().find(|e| e.app_id == app_id).cloned()
    }
}

/// Load all .desktop files from a directory
pub fn load_desktop_files(dir_path: &str) -> Result<usize, &'static str> {
    let entries = list_directory(dir_path).map_err(|_| "Failed to list directory")?;
    let mut count = 0;

    for entry in entries {
        if entry.name == "." || entry.name == ".." {
            continue;
        }

        if entry.is_file() && entry.name.ends_with(".desktop") {
            let file_path = format!("{}/{}", dir_path, entry.name);

            match File::open(&file_path) {
                Ok(mut file) => {
                    let mut content = String::new();
                    let mut buffer = [0u8; 4096];

                    loop {
                        match file.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(n) => {
                                if let Ok(s) = core::str::from_utf8(&buffer[..n]) {
                                    content.push_str(s);
                                }
                            }
                            Err(_) => break,
                        }
                    }

                    let parser = DesktopParser::new(content);
                    if let Some(entry) = parser.parse(&entry.name) {
                        println!("stemd: Loaded app: {} ({})", entry.name, entry.app_id);
                        register_app(entry);
                        count += 1;
                    }
                }
                Err(_) => {
                    println!("stemd: Failed to read {}", file_path);
                }
            }
        }
    }

    Ok(count)
}
