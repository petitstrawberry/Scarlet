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
//! `Icon` can also name an absolute PNG/JPEG path. Console artwork is optional:
//! `X-Scarlet-Background=/path/to/cover.jpg` and
//! `X-Scarlet-BackgroundBlur=none|full|label`.

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
    pub background: Option<String>,
    pub background_blur: Option<String>,
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
        let mut background = None;
        let mut background_blur = None;
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
                    "X-Scarlet-Background" => background = Some(Self::unquote(value)),
                    "X-Scarlet-BackgroundBlur" => background_blur = Some(Self::unquote(value)),
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
            background,
            background_blur,
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

/// Scarlet app format semantics are opt-in and only apply inside a .app.
fn app_path(root: &str, relative: &str) -> Result<String, &'static str> {
    if relative.is_empty()
        || relative.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
    {
        return Err("Invalid relative app path");
    }
    let mut path = std::path::PathBuf::from(root);
    for part in relative.split('/') {
        path.push(part);
        let metadata = fs::symlink_metadata(&path).map_err(|_| "Missing app file")?;
        if metadata.file_type().is_symlink() {
            return Err("App symlinks are unsupported");
        }
    }
    if !path.is_file() {
        return Err("App path must name a file");
    }
    Ok(format!("{root}/{relative}"))
}

fn parse_app(root: &str, filename: &str, content: String) -> Result<DesktopEntry, &'static str> {
    let mut in_entry = false;
    let mut version = None;
    let mut target = None;
    let mut entry_type = None;
    let mut raw_exec = None;
    let mut keys = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('=').ok_or("Invalid app descriptor")?;
        if keys.contains(&key) {
            return Err("Duplicate app descriptor key");
        }
        keys.push(key);
        match key {
            "X-Scarlet-AppFormat" => version = Some(value),
            "X-Scarlet-Target" => target = Some(value),
            "Type" => entry_type = Some(value),
            "Exec" => raw_exec = Some(value),
            _ => {}
        }
    }
    if version != Some("1") || entry_type != Some("Application") {
        return Err("Unsupported Scarlet app format");
    }
    #[cfg(target_os = "scarlet")]
    let compatible = match target {
        Some("aarch64-unknown-scarlet") => cfg!(target_arch = "aarch64"),
        Some("riscv64gc-unknown-scarlet") => cfg!(target_arch = "riscv64"),
        _ => false,
    };
    #[cfg(not(target_os = "scarlet"))]
    let compatible = matches!(
        target,
        Some("aarch64-unknown-scarlet" | "riscv64gc-unknown-scarlet")
    );
    if !compatible {
        return Err("Incompatible native app target");
    }
    let exec = raw_exec.ok_or("Missing app executable")?.to_string();
    let target = target.unwrap().to_string();
    let mut entry = DesktopParser::new(content)
        .parse(filename)
        .ok_or("Invalid app descriptor")?;
    entry.exec = exec;
    if entry.app_id.is_empty()
        || !entry
            .app_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || matches!(entry.app_id.as_str(), "." | "..")
    {
        return Err("Invalid app ID");
    }
    let mut words = split_exec_words(&entry.exec)?;
    if words.is_empty() {
        return Err("Missing app executable");
    }
    words[0] = app_path(root, &words[0])?;
    let mut executable = File::open(&words[0]).map_err(|_| "Missing app executable")?;
    let mut header = [0u8; 20];
    executable
        .read_exact(&mut header)
        .map_err(|_| "Invalid native app executable")?;
    let machine = u16::from_le_bytes([header[18], header[19]]);
    if &header[..8] != b"\x7fELF\x02\x01\x01\x53"
        || !matches!(u16::from_le_bytes([header[16], header[17]]), 2 | 3)
        || machine
            != if target == "aarch64-unknown-scarlet" {
                183
            } else {
                243
            }
    {
        return Err("Incompatible native app executable");
    }
    // Keep the argv contract used by launch, MIME and shell activation.
    entry.exec = words
        .iter()
        .map(|word| format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(icon) = entry.icon.as_mut() {
        if icon.contains('/') {
            *icon = app_path(root, icon)?;
        }
    }
    if let Some(background) = entry.background.as_mut() {
        *background = app_path(root, background)?;
    }
    expand_exec(&entry.exec, &[])?;
    Ok(entry)
}

fn directory(path: &str) -> Result<Vec<fs::DirEntry>, &'static str> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("Failed to list application directory"),
    };
    let mut entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Failed to read application directory")?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn collect_catalog(legacy: &str, applications: &str) -> Result<Vec<DesktopEntry>, &'static str> {
    let mut catalog: Vec<DesktopEntry> = Vec::new();
    for file in directory(legacy)? {
        let filename = file.file_name().to_string_lossy().into_owned();
        if !filename.ends_with(".desktop") {
            continue;
        }
        if !file
            .file_type()
            .map_err(|_| "Failed to inspect legacy entry")?
            .is_file()
        {
            continue;
        }
        let content =
            fs::read_to_string(file.path()).map_err(|_| "Failed to read legacy descriptor")?;
        let entry = DesktopParser::new(content)
            .parse(&filename)
            .ok_or("Invalid legacy descriptor")?;
        if catalog.iter().any(|old| old.app_id == entry.app_id) {
            return Err("Conflicting legacy app IDs");
        }
        catalog.push(entry);
    }
    if let Ok(metadata) = fs::symlink_metadata(applications) {
        if metadata.file_type().is_symlink() {
            return Err("Application root must be a real directory");
        }
    }
    let mut app_ids = Vec::new();
    for app in directory(applications)? {
        let name = app.file_name().to_string_lossy().into_owned();
        let Some(slug) = name.strip_suffix(".app") else {
            continue;
        };
        if slug.is_empty()
            || slug
                .bytes()
                .any(|b| !b.is_ascii_lowercase() && !b.is_ascii_digit() && !b"._-".contains(&b))
            || matches!(slug, "." | "..")
        {
            return Err("Invalid app directory slug");
        }
        if !app
            .file_type()
            .map_err(|_| "Failed to inspect app directory")?
            .is_dir()
        {
            return Err("App must be a real directory");
        }
        let root = format!("{applications}/{name}");
        let descriptors: Vec<_> = directory(&root)?
            .into_iter()
            .filter(|file| file.file_name().to_string_lossy().ends_with(".desktop"))
            .collect();
        if descriptors.len() != 1 {
            return Err("App requires exactly one desktop descriptor");
        }
        let file = &descriptors[0];
        if !file
            .file_type()
            .map_err(|_| "Failed to inspect app descriptor")?
            .is_file()
        {
            return Err("App descriptor must be a regular file");
        }
        let filename = file.file_name().to_string_lossy().into_owned();
        let content =
            fs::read_to_string(file.path()).map_err(|_| "Failed to read app descriptor")?;
        let entry = parse_app(&root, &filename, content).map_err(|error| {
            println!("stemd: {}: {}", root, error);
            error
        })?;
        if app_ids.contains(&entry.app_id) {
            println!("stemd: Conflicting app ID {} at {}", entry.app_id, root);
            return Err("Conflicting app IDs in /applications");
        }
        app_ids.push(entry.app_id.clone());
        catalog.retain(|old| old.app_id != entry.app_id);
        catalog.push(entry);
    }
    catalog.sort_by(|left, right| left.app_id.cmp(&right.app_id));
    Ok(catalog)
}

/// Build the next catalog off-lock, then replace it atomically. Running processes
/// belong to a separate registry and are never killed by reconciliation.
pub fn reload_applications() -> Result<usize, &'static str> {
    reload_from("/etc/stemd.d/apps", "/applications")
}

fn reload_from(legacy: &str, applications: &str) -> Result<usize, &'static str> {
    static RELOAD_LOCK: Mutex<()> = Mutex::new(());
    let _reload = RELOAD_LOCK.lock().expect("stemd mutex poisoned");
    let catalog = collect_catalog(legacy, applications)?;
    let count = catalog.len();
    *APP_REGISTRY.lock().expect("stemd mutex poisoned") = catalog;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::{DesktopParser, expand_exec, mime_type_for_path};
    use std::string::String;
    use std::vec;

    #[test]
    fn artwork_fields_are_optional_and_preserve_absolute_paths_with_spaces() {
        let old = DesktopParser::new(String::from(
            "[Desktop Entry]\nName=Files\nExec=/bin/files\nIcon=folder\n",
        ))
        .parse("files.desktop")
        .unwrap();
        assert_eq!(old.icon.as_deref(), Some("folder"));
        assert!(old.background.is_none());
        assert!(old.background_blur.is_none());

        let entry = DesktopParser::new(String::from(
            "[Desktop Entry]\nName=Files\nExec=/bin/files\nIcon=\"/share/app art/files.png\"\nX-Scarlet-Background='/share/app art/files.jpg'\nX-Scarlet-BackgroundBlur=label\n[Desktop Action Open]\nX-Scarlet-Background=/ignored.png\n",
        )).parse("files.desktop").unwrap();
        assert_eq!(entry.icon.as_deref(), Some("/share/app art/files.png"));
        assert_eq!(
            entry.background.as_deref(),
            Some("/share/app art/files.jpg")
        );
        assert_eq!(entry.background_blur.as_deref(), Some("label"));
        assert_eq!(entry.exec, "/bin/files");
    }

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
            "../../../../bundles/desktop/fs/etc/stemd.d/apps/org.scarlet-os.desktop.terminal.desktop"
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
            "../../../../bundles/desktop/fs/etc/stemd.d/apps/org.scarlet-os.desktop.settings.desktop"
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
            "../../../../bundles/desktop/fs/etc/stemd.d/apps/org.scarlet-os.desktop.notepad.desktop"
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

#[cfg(test)]
mod app_tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "stemd-app-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("legacy")).unwrap();
            fs::create_dir_all(root.join("applications/renamed.app/bin")).unwrap();
            fs::write(
                root.join("applications/renamed.app/bin/player"),
                Self::elf(),
            )
            .unwrap();
            Self(root)
        }
        fn elf() -> [u8; 64] {
            let mut elf = [0; 64];
            elf[..8].copy_from_slice(b"\x7fELF\x02\x01\x01\x53");
            elf[16..18].copy_from_slice(&2u16.to_le_bytes());
            elf[18..20].copy_from_slice(&183u16.to_le_bytes());
            elf
        }
        fn app(&self, exec: &str) -> String {
            format!(
                "[Desktop Entry]\nName=Player\nType=Application\nExec={exec} %F\nX-Scarlet-AppFormat=1\nX-Scarlet-Target=aarch64-unknown-scarlet\nMimeType=audio/wav;\n"
            )
        }
        fn write_app(&self, exec: &str) {
            fs::write(
                self.0
                    .join("applications/renamed.app/org.test.player.desktop"),
                self.app(exec),
            )
            .unwrap();
        }
        fn scan(&self) -> Result<Vec<DesktopEntry>, &'static str> {
            collect_catalog(
                self.0.join("legacy").to_str().unwrap(),
                self.0.join("applications").to_str().unwrap(),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn app_overrides_legacy_and_uses_descriptor_id_after_directory_rename() {
        let f = Fixture::new();
        f.write_app("bin/player");
        fs::write(
            f.0.join("legacy/org.test.player.desktop"),
            "[Desktop Entry]\nName=Legacy\nExec=/bin/old\n",
        )
        .unwrap();
        let catalog = f.scan().unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].app_id, "org.test.player");
        assert_eq!(catalog[0].name, "Player");
        assert_eq!(
            expand_exec(&catalog[0].exec, &[String::from("/tmp/a file.wav")]).unwrap(),
            vec![
                format!("{}/applications/renamed.app/bin/player", f.0.display()),
                String::from("/tmp/a file.wav")
            ]
        );
    }
    #[test]
    fn invalid_and_ambiguous_apps_reject_the_next_catalog() {
        let f = Fixture::new();
        for exec in [
            "../player",
            "/bin/player",
            "bin/../../player",
            "missing",
            "bin/player %z",
        ] {
            f.write_app(exec);
            assert!(f.scan().is_err(), "{exec}");
        }
        f.write_app("bin/player");
        fs::write(
            f.0.join("applications/renamed.app/second.desktop"),
            f.app("bin/player"),
        )
        .unwrap();
        assert!(f.scan().is_err());
        fs::remove_file(f.0.join("applications/renamed.app/second.desktop")).unwrap();
        fs::create_dir_all(f.0.join("applications/other.app/bin")).unwrap();
        fs::write(
            f.0.join("applications/other.app/bin/player"),
            Fixture::elf(),
        )
        .unwrap();
        fs::write(
            f.0.join("applications/other.app/org.test.player.desktop"),
            f.app("bin/player"),
        )
        .unwrap();
        assert_eq!(
            f.scan().unwrap_err(),
            "Conflicting app IDs in /applications"
        );
    }
    #[test]
    fn reload_removes_entries_and_preserves_previous_catalog_on_conflict() {
        let f = Fixture::new();
        f.write_app("bin/player");
        let legacy = f.0.join("legacy");
        let apps = f.0.join("applications");
        assert_eq!(
            reload_from(legacy.to_str().unwrap(), apps.to_str().unwrap()).unwrap(),
            1
        );
        f.write_app("../escape");
        assert!(reload_from(legacy.to_str().unwrap(), apps.to_str().unwrap()).is_err());
        assert_eq!(list_apps().len(), 1);
        fs::remove_dir_all(apps.join("renamed.app")).unwrap();
        assert_eq!(
            reload_from(legacy.to_str().unwrap(), apps.to_str().unwrap()).unwrap(),
            0
        );
        assert!(list_apps().is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_and_resolves_artwork() {
        let f = Fixture::new();
        f.write_app("bin/player");
        let path = f.0.join("applications/renamed.app/org.test.player.desktop");
        fs::write(f.0.join("applications/renamed.app/cover.png"), "fixture").unwrap();
        fs::write(
            &path,
            f.app("bin/player") + "Icon=cover.png\nX-Scarlet-Background=cover.png\n",
        )
        .unwrap();
        assert!(
            f.scan().unwrap()[0]
                .background
                .as_ref()
                .unwrap()
                .ends_with("/cover.png")
        );
        let executable = f.0.join("applications/renamed.app/bin/player");
        fs::remove_file(&executable).unwrap();
        std::os::unix::fs::symlink("/bin/sh", &executable).unwrap();
        assert!(f.scan().is_err());
    }
}
