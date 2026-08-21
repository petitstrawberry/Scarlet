//! Filesystem-backed cursor theme metadata.

use std::fs::File;
use std::io::Read;
use std::string::String;
use std::vec::Vec;
use sws_protocol::CursorIcon;

const THEME_MANIFEST_FILE: &str = "theme.toml";
const MAX_THEME_MANIFEST_BYTES: usize = 64 * 1024;

/// One cursor image declared by a theme manifest.
#[derive(Debug, Clone)]
pub(super) struct CursorThemeImage {
    /// Cursor state represented by this image.
    pub(super) icon: CursorIcon,
    /// Absolute path to the raster image.
    pub(super) image_path: String,
    /// Source-image x-coordinate of the pointer hotspot.
    pub(super) hotspot_x: u32,
    /// Source-image y-coordinate of the pointer hotspot.
    pub(super) hotspot_y: u32,
}

/// Parsed cursor theme and its source-image density.
#[derive(Debug, Clone)]
pub(super) struct CursorTheme {
    name: String,
    image_scale_milli: u32,
    images: Vec<CursorThemeImage>,
}

#[derive(Debug, Clone)]
struct PendingImage {
    icon: CursorIcon,
    image_name: Option<String>,
    alias: Option<CursorIcon>,
    hotspot_x: Option<u32>,
    hotspot_y: Option<u32>,
}

#[derive(Debug, Clone)]
enum CursorThemeDeclaration {
    Image(CursorThemeImage),
    Alias {
        icon: CursorIcon,
        target: CursorIcon,
    },
}

impl CursorThemeDeclaration {
    fn icon(&self) -> CursorIcon {
        match self {
            Self::Image(image) => image.icon,
            Self::Alias { icon, .. } => *icon,
        }
    }
}

impl PendingImage {
    fn new(icon: CursorIcon) -> Self {
        Self {
            icon,
            image_name: None,
            alias: None,
            hotspot_x: None,
            hotspot_y: None,
        }
    }
}

impl CursorTheme {
    /// Load `<theme_directory>/theme.toml`.
    ///
    /// # Arguments
    ///
    /// * `theme_directory` - Directory containing the manifest and PNG files.
    ///
    /// # Returns
    ///
    /// The validated theme, or an error when required metadata is missing.
    pub(super) fn load(theme_directory: &str) -> Result<Self, &'static str> {
        if theme_directory.is_empty() {
            return Err("Cursor theme directory is empty");
        }
        let manifest_path = joined_path(theme_directory, THEME_MANIFEST_FILE);
        let content = read_manifest(&manifest_path)?;
        parse_theme(theme_directory, &content)
    }

    /// Return the human-readable theme name.
    ///
    /// # Returns
    ///
    /// The name declared in `[theme]`, or the directory name when omitted.
    pub(super) fn name(&self) -> &str {
        &self.name
    }

    /// Return the source-image density in thousandths.
    ///
    /// # Returns
    ///
    /// `2000` for a 2x theme, for example.
    pub(super) const fn image_scale_milli(&self) -> u32 {
        self.image_scale_milli
    }

    /// Return all image declarations.
    ///
    /// # Returns
    ///
    /// Theme images in manifest order.
    pub(super) fn images(&self) -> &[CursorThemeImage] {
        &self.images
    }

    /// Find metadata for one cursor state.
    ///
    /// # Arguments
    ///
    /// * `icon` - Cursor state to look up.
    ///
    /// # Returns
    ///
    /// The matching image metadata when declared by the theme.
    pub(super) fn image(&self, icon: CursorIcon) -> Option<&CursorThemeImage> {
        self.images.iter().find(|image| image.icon == icon)
    }
}

fn read_manifest(path: &str) -> Result<String, &'static str> {
    let mut file = File::open(path).map_err(|_| "Failed to open cursor theme manifest")?;
    let mut content = String::new();
    let mut buffer = [0u8; 1024];

    loop {
        let bytes = file
            .read(&mut buffer)
            .map_err(|_| "Failed to read cursor theme manifest")?;
        if bytes == 0 {
            break;
        }
        if content.len().saturating_add(bytes) > MAX_THEME_MANIFEST_BYTES {
            return Err("Cursor theme manifest exceeds 64 KiB");
        }
        let chunk = core::str::from_utf8(&buffer[..bytes])
            .map_err(|_| "Cursor theme manifest is not valid UTF-8")?;
        content.push_str(chunk);
    }

    Ok(content)
}

fn parse_theme(theme_directory: &str, content: &str) -> Result<CursorTheme, &'static str> {
    let mut name = String::new();
    let mut image_scale_milli = None;
    let mut declarations = Vec::new();
    let mut in_theme_section = false;
    let mut pending_image: Option<PendingImage> = None;

    for raw_line in content.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(section) = section_name(line) {
            finish_pending_image(theme_directory, pending_image.take(), &mut declarations)?;
            in_theme_section = section == "theme";
            pending_image = cursor_icon_for_section(section).map(PendingImage::new);
            continue;
        }

        let Some(eq_pos) = line.find('=') else {
            continue;
        };
        let key = line[..eq_pos].trim();
        let value = line[eq_pos + 1..].trim();

        if in_theme_section {
            match key {
                "name" => name = String::from(trim_toml_string(value)),
                "image_scale" => image_scale_milli = parse_scale_value_milli(value),
                "image_scale_milli" => image_scale_milli = parse_u32_value(value),
                _ => {}
            }
        } else if let Some(image) = pending_image.as_mut() {
            match key {
                "image" => image.image_name = Some(String::from(trim_toml_string(value))),
                "alias" => {
                    image.alias = Some(
                        cursor_icon_for_section(trim_toml_string(value))
                            .ok_or("Cursor theme alias names an unknown cursor state")?,
                    )
                }
                "hotspot_x" => image.hotspot_x = parse_u32_value(value),
                "hotspot_y" => image.hotspot_y = parse_u32_value(value),
                _ => {}
            }
        }
    }
    finish_pending_image(theme_directory, pending_image.take(), &mut declarations)?;

    let image_scale_milli = image_scale_milli
        .filter(|scale| (250..=8000).contains(scale))
        .ok_or("Cursor theme image_scale must be between 0.25 and 8.0")?;
    if !declarations
        .iter()
        .any(|declaration| declaration.icon() == CursorIcon::Arrow)
    {
        return Err("Cursor theme does not declare [arrow]");
    }
    let images = resolve_declarations(&declarations)?;
    if name.is_empty() {
        name = String::from(theme_directory.trim_end_matches('/'));
    }

    Ok(CursorTheme {
        name,
        image_scale_milli,
        images,
    })
}

fn finish_pending_image(
    theme_directory: &str,
    pending: Option<PendingImage>,
    declarations: &mut Vec<CursorThemeDeclaration>,
) -> Result<(), &'static str> {
    let Some(pending) = pending else {
        return Ok(());
    };
    if declarations
        .iter()
        .any(|declaration| declaration.icon() == pending.icon)
    {
        return Err("Cursor theme declares a cursor state more than once");
    }
    if let Some(target) = pending.alias {
        if pending.image_name.is_some()
            || pending.hotspot_x.is_some()
            || pending.hotspot_y.is_some()
        {
            return Err("Cursor theme alias cannot also declare image or hotspot");
        }
        declarations.push(CursorThemeDeclaration::Alias {
            icon: pending.icon,
            target,
        });
        return Ok(());
    }
    let image_name = pending
        .image_name
        .filter(|name| !name.is_empty())
        .ok_or("Cursor theme image section is missing image")?;
    if !valid_relative_image_name(&image_name) {
        return Err("Cursor theme image must be a relative path without parent traversal");
    }
    let hotspot_x = pending
        .hotspot_x
        .ok_or("Cursor theme image section is missing hotspot_x")?;
    let hotspot_y = pending
        .hotspot_y
        .ok_or("Cursor theme image section is missing hotspot_y")?;
    declarations.push(CursorThemeDeclaration::Image(CursorThemeImage {
        icon: pending.icon,
        image_path: joined_path(theme_directory, &image_name),
        hotspot_x,
        hotspot_y,
    }));
    Ok(())
}

fn resolve_declarations(
    declarations: &[CursorThemeDeclaration],
) -> Result<Vec<CursorThemeImage>, &'static str> {
    let mut images = Vec::new();
    for declaration in declarations {
        let mut resolving = Vec::new();
        images.push(resolve_declaration(
            declaration.icon(),
            declarations,
            &mut resolving,
        )?);
    }
    Ok(images)
}

fn resolve_declaration(
    icon: CursorIcon,
    declarations: &[CursorThemeDeclaration],
    resolving: &mut Vec<CursorIcon>,
) -> Result<CursorThemeImage, &'static str> {
    if resolving.contains(&icon) {
        return Err("Cursor theme alias cycle detected");
    }
    resolving.push(icon);
    let declaration = declarations
        .iter()
        .find(|declaration| declaration.icon() == icon)
        .ok_or("Cursor theme alias target is not declared")?;
    let mut image = match declaration {
        CursorThemeDeclaration::Image(image) => image.clone(),
        CursorThemeDeclaration::Alias { target, .. } => {
            resolve_declaration(*target, declarations, resolving)?
        }
    };
    let _ = resolving.pop();
    image.icon = icon;
    Ok(image)
}

fn cursor_icon_for_section(section: &str) -> Option<CursorIcon> {
    match section {
        "arrow" => Some(CursorIcon::Arrow),
        "pointer" => Some(CursorIcon::Pointer),
        "text" => Some(CursorIcon::Text),
        "crosshair" => Some(CursorIcon::Crosshair),
        "move" => Some(CursorIcon::Move),
        "resize_ns" => Some(CursorIcon::ResizeNs),
        "resize_ew" => Some(CursorIcon::ResizeEw),
        "resize_nesw" => Some(CursorIcon::ResizeNesw),
        "resize_nwse" => Some(CursorIcon::ResizeNwse),
        "wait" => Some(CursorIcon::Wait),
        "not_allowed" => Some(CursorIcon::NotAllowed),
        _ => None,
    }
}

fn valid_relative_image_name(name: &str) -> bool {
    !name.starts_with('/')
        && !name
            .split('/')
            .any(|component| component.is_empty() || component == "..")
}

fn joined_path(directory: &str, name: &str) -> String {
    let mut path = String::from(directory.trim_end_matches('/'));
    path.push('/');
    path.push_str(name);
    path
}

fn section_name(line: &str) -> Option<&str> {
    if line.starts_with('[') && line.ends_with(']') && line.len() >= 2 {
        Some(line[1..line.len() - 1].trim())
    } else {
        None
    }
}

fn strip_toml_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or(line)
}

fn trim_toml_string(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn parse_u32_value(value: &str) -> Option<u32> {
    trim_toml_string(value).parse::<u32>().ok()
}

fn parse_scale_value_milli(value: &str) -> Option<u32> {
    let value = trim_toml_string(value);
    let (whole, fraction) = match value.split_once('.') {
        Some(parts) => parts,
        None => (value, ""),
    };
    let whole = whole.parse::<u32>().ok()?;
    let mut fraction_milli = 0u32;
    let mut digits = 0u32;
    for byte in fraction.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        if digits < 3 {
            fraction_milli = fraction_milli
                .saturating_mul(10)
                .saturating_add(u32::from(byte - b'0'));
            digits += 1;
        }
    }
    while digits < 3 {
        fraction_milli = fraction_milli.saturating_mul(10);
        digits += 1;
    }
    whole
        .checked_mul(1000)
        .and_then(|whole_milli| whole_milli.checked_add(fraction_milli))
}

#[cfg(test)]
mod tests {
    use super::{CursorTheme, parse_theme};
    use sws_protocol::CursorIcon;

    const VALID_THEME: &str = r#"
[theme]
name = "Test"
image_scale = 2.0

[arrow]
image = "arrow.png"
hotspot_x = 6
hotspot_y = 3

[text]
image = "text.png"
hotspot_x = 21
hotspot_y = 28
"#;

    #[test]
    fn parses_theme_images_and_hotspots() {
        let theme = parse_theme("/share/cursors/test", VALID_THEME).expect("valid theme");
        assert_eq!(theme.name(), "Test");
        assert_eq!(theme.image_scale_milli(), 2000);
        let text = theme.image(CursorIcon::Text).expect("text cursor");
        assert_eq!(text.image_path, "/share/cursors/test/text.png");
        assert_eq!((text.hotspot_x, text.hotspot_y), (21, 28));
    }

    #[test]
    fn requires_arrow_and_complete_hotspots() {
        assert!(parse_theme("/theme", "[theme]\nimage_scale = 2.0").is_err());
        assert!(
            parse_theme(
                "/theme",
                "[theme]\nimage_scale = 2.0\n[arrow]\nimage = \"arrow.png\"\n",
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_parent_traversal() {
        let invalid = VALID_THEME.replace("arrow.png", "../arrow.png");
        assert!(parse_theme("/theme", &invalid).is_err());
    }

    #[test]
    fn resolves_forward_alias_with_target_hotspot() {
        let theme = parse_theme(
            "/theme",
            r#"
[theme]
image_scale = 2.0
[arrow]
alias = "pointer"
[pointer]
image = "pointer.png"
hotspot_x = 13
hotspot_y = 4
"#,
        )
        .expect("valid alias theme");
        let arrow = theme.image(CursorIcon::Arrow).expect("arrow alias");
        assert_eq!(arrow.image_path, "/theme/pointer.png");
        assert_eq!((arrow.hotspot_x, arrow.hotspot_y), (13, 4));
    }

    #[test]
    fn rejects_alias_cycles() {
        assert!(
            parse_theme(
                "/theme",
                r#"
[theme]
image_scale = 2.0
[arrow]
alias = "pointer"
[pointer]
alias = "arrow"
"#,
            )
            .is_err()
        );
    }

    #[test]
    fn cursor_theme_type_remains_constructible_from_parser() {
        let theme: CursorTheme = parse_theme("/theme", VALID_THEME).expect("valid theme");
        assert_eq!(theme.images().len(), 2);
    }
}
