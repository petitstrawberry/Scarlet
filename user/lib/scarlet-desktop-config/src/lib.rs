#![no_std]

extern crate scarlet_std as std;

use std::string::{String, ToString};
use std::vec::Vec;

#[derive(Debug, Clone, Default)]
pub struct ThemeColors {
    pub background: Option<[u8; 3]>,
    pub background_style: Option<BackgroundStyle>,
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

#[derive(Debug, Clone, Default)]
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

#[derive(Debug, Clone, Default)]
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
                } else if key == "background_style" {
                    let value = Self::unquote(value);
                    if let Some(style) = BackgroundStyle::from_str(&value) {
                        theme.background_style = Some(style);
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
    use std::fs::File;
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

pub fn read_config_dir(dir_path: &str) -> Result<String, &'static str> {
    use std::fs;

    let mut combined_content = String::new();

    match fs::list_directory(dir_path) {
        Ok(entries) => {
            let mut toml_files = Vec::new();
            for entry in entries {
                if entry.name == "." || entry.name == ".." {
                    continue;
                }
                if entry.is_file() && entry.name.ends_with(".toml") {
                    toml_files.push(entry.name);
                }
            }

            toml_files.sort();

            for filename in toml_files {
                let file_path = std::format!("{}/{}", dir_path, filename);
                match read_config(&file_path) {
                    Ok(content) => {
                        combined_content.push_str(&content);
                        combined_content.push('\n');
                    }
                    Err(_e) => {
                        return Err("Failed to read config file");
                    }
                }
            }

            Ok(combined_content)
        }
        Err(_) => Err("Failed to read config directory"),
    }
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
