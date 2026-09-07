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
//!
//! Launcher activation focuses an existing window by default. Set
//! `X-Scarlet-NewInstance=true` to start a new process on every activation.

use std::format;
use std::fs::{self, File};
use std::io::Read;
use std::println;
use std::string::{String, ToString};
use std::sync::Mutex;
use std::{vec, vec::Vec};

/// Application definition from .desktop file
#[derive(Debug, Clone)]
pub struct DesktopEntry {
    pub app_id: String,
    pub name: String,
    pub exec: String,
    pub icon: Option<String>,
    pub terminal: bool,
    /// Start a new process for each activation instead of focusing an existing
    /// window. Set by `X-Scarlet-NewInstance`; defaults to false.
    pub new_instance: bool,
    pub mime_types: Vec<String>,
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
        let mut new_instance = false;
        let mut mime_types = Vec::new();

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
                    "X-Scarlet-NewInstance" => new_instance = value == "true" || value == "1",
                    "MimeType" => {
                        for mime_type in value.split(';') {
                            let mime_type = Self::unquote(mime_type.trim());
                            if !mime_type.is_empty() {
                                mime_types.push(mime_type);
                            }
                        }
                    }
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
            new_instance,
            mime_types,
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
// Thread-safe using Mutex
static APP_REGISTRY: Mutex<Vec<DesktopEntry>> = Mutex::new(Vec::new());

/// Add an application to the registry
pub fn register_app(entry: DesktopEntry) {
    let mut registry = APP_REGISTRY.lock().expect("stemd mutex poisoned");
    // Remove existing entry with same app_id
    registry.retain(|e| e.app_id != entry.app_id);
    registry.push(entry);
}

/// Look up an application by app_id
pub fn lookup_app(app_id: &str) -> Option<DesktopEntry> {
    println!("stemd: lookup_app called for app_id={}", app_id);
    let registry = APP_REGISTRY.lock().expect("stemd mutex poisoned");
    let result = registry.iter().find(|e| e.app_id == app_id).cloned();
    println!(
        "stemd: lookup_app returning {:?}",
        result.as_ref().map(|e| e.name.as_str())
    );
    result
}

/// Return a snapshot of all applications registered from desktop entries.
pub fn list_apps() -> Vec<DesktopEntry> {
    APP_REGISTRY.lock().expect("stemd mutex poisoned").clone()
}

/// Look up the first registered application advertising a MIME type.
///
/// Exact MIME type matches take precedence over `type/*` and `*/*` matches.
pub fn lookup_app_for_mime(mime_type: &str) -> Option<DesktopEntry> {
    if let Some(app_id) = default_app_id_for_mime(mime_type)
        && let Some(entry) = lookup_app(&app_id)
    {
        return Some(entry);
    }

    lookup_registered_app_for_mime(mime_type)
}

fn lookup_registered_app_for_mime(mime_type: &str) -> Option<DesktopEntry> {
    let registry = APP_REGISTRY.lock().expect("stemd mutex poisoned");
    let wildcard = mime_type
        .split_once('/')
        .map(|(kind, _)| format!("{kind}/*"));

    registry
        .iter()
        .find(|entry| {
            entry
                .mime_types
                .iter()
                .any(|candidate| candidate == mime_type)
        })
        .cloned()
        .or_else(|| {
            wildcard.as_deref().and_then(|wildcard| {
                registry
                    .iter()
                    .find(|entry| {
                        entry
                            .mime_types
                            .iter()
                            .any(|candidate| candidate == wildcard)
                    })
                    .cloned()
            })
        })
        .or_else(|| {
            registry
                .iter()
                .find(|entry| entry.mime_types.iter().any(|candidate| candidate == "*/*"))
                .cloned()
        })
}

fn default_app_id_for_mime(mime_type: &str) -> Option<String> {
    for path in mimeapps_paths() {
        if let Some(app_id) = default_app_id_from_file(&path, mime_type) {
            return Some(app_id);
        }
    }
    None
}

fn mimeapps_paths() -> Vec<String> {
    let config_home = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or(String::from("/root"));
        format!("{home}/.config")
    });
    let config_dirs = std::env::var("XDG_CONFIG_DIRS").unwrap_or(String::from("/etc/xdg"));

    let mut paths = vec![format!("{config_home}/mimeapps.list")];
    for directory in config_dirs
        .split(':')
        .filter(|directory| !directory.is_empty())
    {
        paths.push(format!("{directory}/mimeapps.list"));
    }
    paths.push(String::from("/etc/mimeapps.list"));
    paths
}

fn default_app_id_from_file(path: &str, mime_type: &str) -> Option<String> {
    let Ok(mut file) = File::open(path) else {
        return None;
    };

    let mut content = String::new();
    let mut buffer = [0u8; 4096];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => {
                let Ok(chunk) = core::str::from_utf8(&buffer[..length]) else {
                    return None;
                };
                content.push_str(chunk);
            }
            Err(_) => return None,
        }
    }

    let mut in_default_applications = false;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_default_applications = line == "[Default Applications]";
            continue;
        }
        if !in_default_applications {
            continue;
        }

        let Some(separator) = line.find('=') else {
            continue;
        };
        if line[..separator].trim() != mime_type {
            continue;
        }

        for desktop_id in line[separator + 1..].split(';') {
            let desktop_id = desktop_id.trim();
            if desktop_id.is_empty() {
                continue;
            }
            return Some(
                desktop_id
                    .strip_suffix(".desktop")
                    .unwrap_or(desktop_id)
                    .to_string(),
            );
        }
    }

    None
}

/// Infer the common MIME type for a local path from its filename extension.
pub fn mime_type_for_path(path: &str) -> Option<&'static str> {
    let extension = path.rsplit_once('.')?.1;

    if matches_extension(extension, &["txt", "text", "log", "md", "rst"]) {
        return Some("text/plain");
    }
    if matches_extension(extension, &["json"]) {
        return Some("application/json");
    }
    if matches_extension(extension, &["toml", "yaml", "yml", "xml", "csv", "ini"]) {
        return Some("text/plain");
    }
    if matches_extension(extension, &["jpg", "jpeg"]) {
        return Some("image/jpeg");
    }
    if matches_extension(extension, &["png"]) {
        return Some("image/png");
    }
    if matches_extension(extension, &["gif"]) {
        return Some("image/gif");
    }
    if matches_extension(extension, &["bmp"]) {
        return Some("image/bmp");
    }
    if matches_extension(extension, &["webp"]) {
        return Some("image/webp");
    }
    if matches_extension(extension, &["pdf"]) {
        return Some("application/pdf");
    }
    if matches_extension(extension, &["mp3"]) {
        return Some("audio/mpeg");
    }
    if matches_extension(extension, &["wav"]) {
        return Some("audio/wav");
    }
    if matches_extension(extension, &["ogg"]) {
        return Some("audio/ogg");
    }
    if matches_extension(extension, &["flac"]) {
        return Some("audio/flac");
    }
    if matches_extension(extension, &["m4a"]) {
        return Some("audio/mp4");
    }
    if matches_extension(extension, &["aac"]) {
        return Some("audio/aac");
    }
    if matches_extension(extension, &["mp4", "m4v"]) {
        return Some("video/mp4");
    }
    if matches_extension(extension, &["webm"]) {
        return Some("video/webm");
    }
    if matches_extension(extension, &["mkv"]) {
        return Some("video/x-matroska");
    }
    if matches_extension(extension, &["mov"]) {
        return Some("video/quicktime");
    }
    if matches_extension(extension, &["avi"]) {
        return Some("video/x-msvideo");
    }

    None
}

/// Expand a desktop entry `Exec` field into argv values.
///
/// The initial implementation supports the file and URI field codes needed
/// by the desktop file opener: `%f`, `%F`, `%u`, `%U`, and `%%`.
pub fn expand_exec(exec: &str, files: &[String]) -> Result<Vec<String>, &'static str> {
    let words = split_exec_words(exec)?;
    let mut argv = Vec::new();

    for word in words {
        match word.as_str() {
            "%f" | "%u" => {
                if let Some(file) = files.first() {
                    argv.push(file.clone());
                }
            }
            "%F" | "%U" => argv.extend(files.iter().cloned()),
            "%%" => argv.push(String::from("%")),
            "%i" | "%c" | "%k" => {}
            _ if word.contains('%') => return Err("Unsupported desktop Exec field code"),
            _ => argv.push(word),
        }
    }

    Ok(argv)
}

fn matches_extension(extension: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

fn split_exec_words(exec: &str) -> Result<Vec<String>, &'static str> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;

    for character in exec.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character.is_whitespace() && !quoted {
            if !current.is_empty() {
                words.push(core::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }

    if escaped || quoted {
        return Err("Malformed desktop Exec field");
    }
    if !current.is_empty() {
        words.push(current);
    }

    Ok(words)
}

/// Load all .desktop files from a directory
pub fn load_desktop_files(dir_path: &str) -> Result<usize, &'static str> {
    let entries = fs::read_dir(dir_path).map_err(|_| "Failed to list directory")?;
    let mut count = 0;

    for entry in entries {
        let entry = entry.map_err(|_| "Failed to read directory entry")?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "." || name == ".." {
            continue;
        }

        let file_type = entry
            .file_type()
            .map_err(|_| "Failed to query directory entry")?;
        if file_type.is_file() && name.ends_with(".desktop") {
            let file_path = format!("{}/{}", dir_path, name);

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
                    if let Some(entry) = parser.parse(&name) {
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

#[cfg(test)]
mod tests {
    use super::{DesktopParser, expand_exec, mime_type_for_path};
    use std::string::String;
    use std::vec;

    #[test]
    fn new_instance_launch_requires_an_explicit_opt_in() {
        for (setting, expected) in [
            ("", false),
            ("X-Scarlet-NewInstance=false\n", false),
            ("X-Scarlet-NewInstance=0\n", false),
            ("X-Scarlet-NewInstance=invalid\n", false),
            ("X-Scarlet-NewInstance=true\n", true),
            ("X-Scarlet-NewInstance=1\n", true),
        ] {
            let entry = DesktopParser::new(std::format!(
                "[Desktop Entry]\nName=Example\nExec=/bin/example\n{setting}"
            ))
            .parse("example.desktop")
            .expect("desktop entry should parse");

            assert_eq!(entry.new_instance, expected, "setting: {setting:?}");
        }
    }

    #[test]
    fn terminal_desktop_entry_requests_a_new_instance() {
        let entry = DesktopParser::new(String::from(include_str!(
            "../../../../bundles/desktop/fs/system/scarlet/etc/stemd.d/apps/org.scarlet-os.desktop.terminal.desktop"
        )))
        .parse("org.scarlet-os.desktop.terminal.desktop")
        .expect("Terminal desktop entry should parse");

        assert!(entry.new_instance);
        assert_eq!(
            expand_exec(&entry.exec, &[]).expect("Terminal launch should expand"),
            vec!["/bin/terminal"]
        );
    }

    #[test]
    fn settings_desktop_entry_keeps_focus_existing_as_the_default() {
        let entry = DesktopParser::new(String::from(include_str!(
            "../../../../bundles/desktop/fs/system/scarlet/etc/stemd.d/apps/org.scarlet-os.desktop.settings.desktop"
        )))
        .parse("org.scarlet-os.desktop.settings.desktop")
        .expect("Settings desktop entry should parse");

        assert!(!entry.new_instance);
    }

    #[test]
    fn parses_mime_types() {
        let entry = DesktopParser::new(String::from(
            "[Desktop Entry]\nName=Viewer\nExec=/bin/viewer %F\nMimeType=image/png;image/jpeg;\n",
        ))
        .parse("viewer.desktop")
        .expect("desktop entry should parse");

        assert_eq!(entry.mime_types, vec!["image/png", "image/jpeg"]);
    }

    #[test]
    fn expands_file_arguments_without_shell() {
        let files = vec![String::from("/tmp/a file.txt"), String::from("/tmp/b.txt")];
        let argv = expand_exec("/bin/viewer --open %F", &files).expect("Exec should expand");

        assert_eq!(
            argv,
            vec!["/bin/viewer", "--open", "/tmp/a file.txt", "/tmp/b.txt"]
        );
    }

    #[test]
    fn notepad_desktop_entry_preserves_the_selected_path() {
        let entry = DesktopParser::new(String::from(include_str!(
            "../../../../bundles/desktop/fs/system/scarlet/etc/stemd.d/apps/org.scarlet-os.desktop.notepad.desktop"
        )))
        .parse("org.scarlet-os.desktop.notepad.desktop")
        .expect("Notepad desktop entry should parse");
        let files = vec![String::from("/tmp/メモ with spaces.txt")];

        assert_eq!(
            expand_exec(&entry.exec, &files).expect("Notepad file launch should expand"),
            vec!["/bin/notepad", "/tmp/メモ with spaces.txt"]
        );
        assert_eq!(
            expand_exec(&entry.exec, &[]).expect("Notepad launcher entry should expand"),
            vec!["/bin/notepad"]
        );
    }

    #[test]
    fn detects_common_mime_types() {
        assert_eq!(mime_type_for_path("movie.MP4"), Some("video/mp4"));
        assert_eq!(mime_type_for_path("clip.webm"), Some("video/webm"));
        assert_eq!(mime_type_for_path("sound.wav"), Some("audio/wav"));
        assert_eq!(mime_type_for_path("image.png"), Some("image/png"));
        assert_eq!(mime_type_for_path("unknown.bin"), None);
    }
}
