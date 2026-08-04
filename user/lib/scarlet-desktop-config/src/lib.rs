#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Sender bus name used by Settings for desktop configuration signals.
pub const DESKTOP_SETTINGS_SIGNAL_SENDER: &str = "org.scarlet-os.desktop.settings";

/// Bus name owned by the headless desktop settings service.
pub const DESKTOP_SETTINGS_BUS_NAME: &str = "org.scarlet-os.desktop.settings";

/// Object path used by the desktop settings service.
pub const DESKTOP_SETTINGS_SERVICE_OBJECT_PATH: &str = "/org/scarlet/os/desktop";

/// Interface implemented by the desktop settings service.
pub const DESKTOP_SETTINGS_SERVICE_INTERFACE: &str = "org.scarlet.desktop.Settings";

/// Method used to persist the complete background configuration.
pub const DESKTOP_SETTINGS_SET_BACKGROUND_METHOD: &str = "SetBackground";

/// Method used to restore the generated default background.
pub const DESKTOP_SETTINGS_RESET_BACKGROUND_METHOD: &str = "ResetBackground";

/// User/system desktop configuration directory used by the current Scarlet
/// desktop profile.
pub const DESKTOP_CONFIG_DIR: &str = "/etc/scarlet-desktop.d";

/// Persistent desktop background configuration path.
pub const DESKTOP_BACKGROUND_CONFIG_PATH: &str = "/etc/scarlet-desktop.d/background.toml";

/// Bus name registered by the desktop background listener.
pub const DESKTOP_BACKGROUND_BUS_NAME: &str = "org.scarlet-os.desktop.background";

/// Object path used for desktop configuration signals.
pub const DESKTOP_SETTINGS_OBJECT_PATH: &str = "/org/scarlet/os/desktop";

/// Interface used for desktop configuration signals.
pub const DESKTOP_SETTINGS_INTERFACE: &str = "org.scarlet.desktop.Settings";

/// Signal emitted after the desktop background configuration was saved.
pub const DESKTOP_BACKGROUND_CHANGED_SIGNAL: &str = "BackgroundChanged";

/// Bus name owned by the File Manager and its picker mode.
pub const DESKTOP_FILE_MANAGER_BUS_NAME: &str = "org.scarlet-os.desktop.filemanager";

/// Object path used by the File Manager service.
pub const DESKTOP_FILE_MANAGER_OBJECT_PATH: &str = "/org/scarlet/os/filemanager";

/// Interface implemented by the File Manager service.
pub const DESKTOP_FILE_MANAGER_INTERFACE: &str = "org.scarlet.desktop.FileManager";

/// Method used to open the File Manager in picker mode.
pub const DESKTOP_FILE_MANAGER_OPEN_FILE_METHOD: &str = "OpenFile";

/// Method used to show the normal File Manager window.
pub const DESKTOP_FILE_MANAGER_SHOW_METHOD: &str = "Show";

/// Signal emitted after a picker request is accepted or cancelled.
pub const DESKTOP_FILE_MANAGER_RESPONSE_SIGNAL: &str = "Response";

/// Bus name owned by the desktop service manager.
pub const DESKTOP_STEMD_BUS_NAME: &str = "org.scarlet-os.stemd";

/// Object path used by the desktop service manager.
pub const DESKTOP_STEMD_OBJECT_PATH: &str = "/org/scarlet/os/stemd";

/// Interface implemented by the desktop service manager.
pub const DESKTOP_STEMD_INTERFACE: &str = "org.scarlet-os.stemd";

/// Method used to open a local filesystem path with its default application.
pub const DESKTOP_STEMD_OPEN_PATH_METHOD: &str = "OpenPath";

/// Method used to list applications registered from desktop entries.
pub const DESKTOP_STEMD_LIST_APPLICATIONS_METHOD: &str = "ListApplications";

/// Method used to launch an application or focus its existing window.
pub const DESKTOP_STEMD_LAUNCH_OR_FOCUS_METHOD: &str = "LaunchOrFocus";

/// Bus name owned by the resident desktop application launcher.
pub const DESKTOP_LAUNCHER_BUS_NAME: &str = "org.scarlet-os.desktop.launcher";

/// Object path used by the resident application launcher.
pub const DESKTOP_LAUNCHER_OBJECT_PATH: &str = "/org/scarlet/os/launcher";

/// Interface implemented by the resident application launcher.
pub const DESKTOP_LAUNCHER_INTERFACE: &str = "org.scarlet.desktop.Launcher";

/// Method used to show the resident application launcher window.
pub const DESKTOP_LAUNCHER_SHOW_METHOD: &str = "Show";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThemeColors {
    /// Generated background color.
    pub background: Option<[u8; 3]>,
    /// Generated background style.
    pub background_style: Option<BackgroundStyle>,
    /// Optional local image path used as the desktop background.
    pub background_image: Option<String>,
    pub taskbar: Option<[u8; 3]>,
    pub text: Option<[u8; 3]>,
    pub highlight: Option<[u8; 3]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackgroundStyle {
    #[default]
    GradientLines,
    Gradient,
    Solid,
}

impl BackgroundStyle {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "gradient_lines" | "gradient-lines" | "lines" => Some(BackgroundStyle::GradientLines),
            "gradient" => Some(BackgroundStyle::Gradient),
            "solid" => Some(BackgroundStyle::Solid),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            BackgroundStyle::GradientLines => "gradient_lines",
            BackgroundStyle::Gradient => "gradient",
            BackgroundStyle::Solid => "solid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskbarConfig {
    pub height: Option<u32>,
    pub position: Option<TaskbarPosition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarPosition {
    Top,
    Bottom,
}

impl TaskbarPosition {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "top" => Some(TaskbarPosition::Top),
            "bottom" => Some(TaskbarPosition::Bottom),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DesktopConfig {
    pub theme: ThemeColors,
    pub taskbar: TaskbarConfig,
}

pub struct ConfigParser {
    content: String,
}

impl ConfigParser {
    pub fn new(content: String) -> Self {
        Self { content }
    }

    pub fn parse(&self) -> DesktopConfig {
        let mut config = DesktopConfig::default();
        let lines: Vec<&str> = self.content.lines().collect();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i].trim();

            if line.starts_with("[theme]") {
                self.parse_theme(&lines, &mut i, &mut config.theme);
            } else if line.starts_with("[taskbar]") {
                self.parse_taskbar(&lines, &mut i, &mut config.taskbar);
            }

            i += 1;
        }

        config
    }

    fn parse_theme(&self, lines: &[&str], i: &mut usize, theme: &mut ThemeColors) {
        *i += 1;
        while *i < lines.len() {
            let line = lines[*i].trim();

            if line.is_empty() || line.starts_with('[') {
                break;
            }

            if line.starts_with('#') {
                *i += 1;
                continue;
            }

            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim();
                let value = line[eq_pos + 1..].trim();

                if let Some(color) = Self::parse_color(value) {
                    match key {
                        "background" => theme.background = Some(color),
                        "taskbar" => theme.taskbar = Some(color),
                        "text" => theme.text = Some(color),
                        "highlight" => theme.highlight = Some(color),
                        _ => {}
                    }
                } else {
                    let value = Self::unquote(value);
                    if key == "background_style" {
                        if let Some(style) = BackgroundStyle::from_str(&value) {
                            theme.background_style = Some(style);
                        }
                    } else if key == "background_image" && !value.is_empty() {
                        theme.background_image = Some(value);
                    }
                }
            }

            *i += 1;
        }
    }

    fn parse_taskbar(&self, lines: &[&str], i: &mut usize, taskbar: &mut TaskbarConfig) {
        *i += 1;
        while *i < lines.len() {
            let line = lines[*i].trim();

            if line.is_empty() || line.starts_with('[') {
                break;
            }

            if line.starts_with('#') {
                *i += 1;
                continue;
            }

            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim();
                let value = line[eq_pos + 1..].trim();
                let value = Self::unquote(value);

                match key {
                    "height" => {
                        if let Some(h) = Self::parse_u32(&value) {
                            taskbar.height = Some(h);
                        }
                    }
                    "position" => {
                        if let Some(pos) = TaskbarPosition::from_str(&value) {
                            taskbar.position = Some(pos);
                        }
                    }
                    _ => {}
                }
            }

            *i += 1;
        }
    }

    fn unquote(s: &str) -> String {
        let s = s.trim();
        if ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
            && s.len() >= 2
        {
            return s[1..s.len() - 1].to_string();
        }
        s.to_string()
    }

    fn parse_color(s: &str) -> Option<[u8; 3]> {
        let unquoted = Self::unquote(s);
        let s = unquoted.trim();

        if s.starts_with('#') && s.len() == 7 {
            let r = u8::from_str_radix(&s[1..3], 16).ok()?;
            let g = u8::from_str_radix(&s[3..5], 16).ok()?;
            let b = u8::from_str_radix(&s[5..7], 16).ok()?;
            return Some([r, g, b]);
        }

        if s.starts_with('#') && s.len() == 4 {
            let r = u8::from_str_radix(&s[1..2], 16).ok()? * 17;
            let g = u8::from_str_radix(&s[2..3], 16).ok()? * 17;
            let b = u8::from_str_radix(&s[3..4], 16).ok()? * 17;
            return Some([r, g, b]);
        }

        None
    }

    fn parse_u32(s: &str) -> Option<u32> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }

        let mut acc: u32 = 0;
        for ch in s.bytes() {
            if !ch.is_ascii_digit() {
                return None;
            }
            let digit = (ch - b'0') as u32;
            acc = acc.saturating_mul(10).saturating_add(digit);
        }

        Some(acc)
    }
}

pub fn read_config(path: &str) -> Result<String, &'static str> {
    #[cfg(feature = "std")]
    {
        return std::fs::read_to_string(path).map_err(|_| "Failed to read config file");
    }

    #[cfg(not(feature = "std"))]
    use scarlet_std::fs::File;
    #[cfg(not(feature = "std"))]
    {
        let mut file = File::open(path).map_err(|_| "Failed to open config file")?;

        let mut content = String::new();
        let mut buf = [0u8; 4096];

        loop {
            let n = file
                .read(&mut buf)
                .map_err(|_| "Failed to read config file")?;
            if n == 0 {
                break;
            }
            let chunk =
                core::str::from_utf8(&buf[..n]).map_err(|_| "Config file is not valid UTF-8")?;
            content.push_str(chunk);
        }

        Ok(content)
    }
}

#[cfg(feature = "std")]
fn config_filenames(dir_path: &str) -> Result<Vec<String>, &'static str> {
    let entries = std::fs::read_dir(dir_path).map_err(|_| "Failed to read config directory")?;
    let mut filenames = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| "Failed to read config directory entry")?;
        let file_type = entry
            .file_type()
            .map_err(|_| "Failed to read config file type")?;
        let filename = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_file() && filename.ends_with(".toml") {
            filenames.push(filename);
        }
    }
    Ok(filenames)
}

#[cfg(not(feature = "std"))]
fn config_filenames(dir_path: &str) -> Result<Vec<String>, &'static str> {
    use scarlet_std::fs;

    let entries = fs::list_directory(dir_path).map_err(|_| "Failed to read config directory")?;
    let mut filenames = Vec::new();
    for entry in entries {
        if entry.name == "." || entry.name == ".." {
            continue;
        }
        if entry.is_file() && entry.name.ends_with(".toml") {
            filenames.push(entry.name);
        }
    }
    Ok(filenames)
}

pub fn read_config_dir(dir_path: &str) -> Result<String, &'static str> {
    let mut combined_content = String::new();
    let mut toml_files = config_filenames(dir_path)?;
    toml_files.sort();

    for filename in toml_files {
        let file_path = format!("{}/{}", dir_path, filename);
        let content = read_config(&file_path).map_err(|_| "Failed to read config file")?;
        combined_content.push_str(&content);
        combined_content.push('\n');
    }

    Ok(combined_content)
}

pub fn load_desktop_config() -> DesktopConfig {
    let config_dirs = [
        "/etc/scarlet-desktop.d",
        "/system/scarlet/etc/scarlet-desktop.d",
    ];

    for dir in &config_dirs {
        if let Ok(content) = read_config_dir(dir) {
            let parser = ConfigParser::new(content);
            return parser.parse();
        }
    }

    DesktopConfig::default()
}
