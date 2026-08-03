//! Runtime file icon loading for the desktop file manager.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use lyon_path::Path as LyonPath;
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillTessellator, FillVertex, StrokeOptions, StrokeTessellator,
    StrokeVertex, VertexBuffers,
};
use scarlet_ui::{Color, Image, VectorImageData, VectorTriangle};
use svgtypes::{PathParser, PathSegment};

const ICON_THEME_ROOT: &str = "/share/icons/default";
const ICON_THEME_ROOT_ENV: &str = "SCARLET_ICON_THEME_ROOT";
const DEFAULT_FILE_ICON: &str = "scalable/mimetypes/blank";

static ICON_CACHE: OnceLock<Mutex<HashMap<String, Arc<VectorImageData>>>> = OnceLock::new();

/// Semantic kind used by the file manager to choose an icon and preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileKind {
    /// A directory.
    Folder,
    /// An image file.
    Image,
    /// A document-like file.
    Document,
    /// An audio file.
    Audio,
    /// A video file.
    Video,
    /// An archive file.
    Archive,
    /// An unclassified file.
    Unknown,
}

impl FileKind {
    /// Infer a kind from a file path or display name.
    pub(crate) fn from_path(path: &str) -> Self {
        let Some((_, extension)) = path.rsplit_once('.') else {
            return Self::Unknown;
        };
        if ["jpg", "jpeg", "png", "gif", "bmp", "webp"]
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        {
            Self::Image
        } else if ["mp3", "wav", "ogg", "flac", "m4a"]
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        {
            Self::Audio
        } else if ["mp4", "mkv", "webm", "mov", "avi"]
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        {
            Self::Video
        } else if ["zip", "tar", "gz", "xz", "bz2", "7z", "rar"]
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        {
            Self::Archive
        } else if [
            "txt", "md", "rs", "c", "h", "cpp", "toml", "json", "xml", "html", "pdf",
        ]
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        {
            Self::Document
        } else {
            Self::Unknown
        }
    }
}

/// Load an icon for a file-system entry from the installed default theme.
pub(crate) fn icon_for_entry(name: &str, is_directory: bool) -> Image {
    let relative = if is_directory {
        String::from("scalable/places/folder")
    } else {
        let extension = name
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
            .filter(|extension| !extension.is_empty());
        match extension {
            Some(extension) => format!("scalable/mimetypes/{extension}"),
            None => String::from(DEFAULT_FILE_ICON),
        }
    };

    let data = load_asset(&relative)
        .or_else(|| load_asset(DEFAULT_FILE_ICON))
        .unwrap_or_else(fallback_data);
    Image::from_vector((*data).clone())
}

fn icon_cache() -> &'static Mutex<HashMap<String, Arc<VectorImageData>>> {
    ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn theme_root() -> PathBuf {
    if let Ok(path) = std::env::var(ICON_THEME_ROOT_ENV) {
        let path = PathBuf::from(path);
        if path.is_dir() {
            return path;
        }
    }

    let installed = Path::new(ICON_THEME_ROOT);
    if installed.is_dir() {
        return installed.to_owned();
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../bundles/desktop/fs/system/scarlet/share/icons/default")
}

fn load_asset(relative: &str) -> Option<Arc<VectorImageData>> {
    if let Ok(cache) = icon_cache().lock() {
        if let Some(data) = cache.get(relative) {
            return Some(data.clone());
        }
    }

    let path = theme_root().join(format!("{relative}.svg"));
    let svg = fs::read_to_string(&path).ok()?;
    let parsed = catch_unwind(AssertUnwindSafe(|| {
        let (width, height) = view_box(&svg, &path);
        let styles = style_colors(&svg);
        let triangles = tessellate_svg(&svg, &styles, &path);
        let triangles = triangles
            .into_iter()
            .map(|triangle| VectorTriangle {
                a: triangle.a,
                b: triangle.b,
                c: triangle.c,
                color: Color::rgba(
                    (triangle.color >> 24) as u8,
                    (triangle.color >> 16) as u8,
                    (triangle.color >> 8) as u8,
                    triangle.color as u8,
                ),
            })
            .collect();
        VectorImageData::new(width as f32, height as f32, triangles)
    }))
    .ok()?;
    let data = Arc::new(parsed);

    if let Ok(mut cache) = icon_cache().lock() {
        cache.insert(relative.to_owned(), data.clone());
    }
    Some(data)
}

fn fallback_data() -> Arc<VectorImageData> {
    Arc::new(VectorImageData::new(
        32.0,
        32.0,
        vec![
            VectorTriangle {
                a: [4.0, 4.0],
                b: [28.0, 4.0],
                c: [28.0, 28.0],
                color: Color::rgb(128u8, 128u8, 128u8),
            },
            VectorTriangle {
                a: [4.0, 4.0],
                b: [28.0, 28.0],
                c: [4.0, 28.0],
                color: Color::rgb(128u8, 128u8, 128u8),
            },
        ],
    ))
}

struct Triangle {
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    color: u32,
}

#[derive(Clone, Copy)]
enum Command {
    Move(f64, f64),
    Line(f64, f64),
    Quad(f64, f64, f64, f64),
    Cubic(f64, f64, f64, f64, f64, f64),
    Close,
}

fn view_box(svg: &str, path: &Path) -> (f64, f64) {
    let value = attribute(svg, "viewBox")
        .unwrap_or_else(|| panic!("missing viewBox in {}", path.display()));
    let values: Vec<f64> = value
        .split_whitespace()
        .map(|part| part.parse().expect("numeric viewBox value"))
        .collect();
    assert!(values.len() == 4, "invalid viewBox in {}", path.display());
    (values[2], values[3])
}

fn style_colors(svg: &str) -> BTreeMap<String, u32> {
    let mut colors = BTreeMap::new();
    let mut rest = svg;

    while let Some(style_start) = rest.find("<style") {
        let style_fragment = &rest[style_start..];
        let Some(content_start) = style_fragment.find('>') else {
            break;
        };
        let content = &style_fragment[content_start + 1..];
        let Some(content_end) = content.find("</style>") else {
            break;
        };
        let mut rules = &content[..content_end];

        while let Some(class_start) = rules.find('.') {
            rules = &rules[class_start + 1..];
            let Some(open) = rules.find('{') else {
                break;
            };
            let selectors = rules[..open].trim();
            let Some(close) = rules[open + 1..].find('}') else {
                break;
            };
            let declarations = &rules[open + 1..open + 1 + close];
            let fill = declarations
                .split(';')
                .find_map(|rule| rule.trim().strip_prefix("fill:"))
                .and_then(|value| parse_color(value.trim()));

            if let Some(fill) = fill {
                for selector in selectors.split(',') {
                    let selector = selector.trim();
                    let class = selector.strip_prefix('.').unwrap_or(selector);
                    if !class.is_empty() {
                        colors.insert(class.to_owned(), fill);
                    }
                }
            }
            rules = &rules[open + 1 + close + 1..];
        }

        rest = &content[content_end + "</style>".len()..];
    }

    colors
}

fn tessellate_svg(svg: &str, styles: &BTreeMap<String, u32>, path: &Path) -> Vec<Triangle> {
    let mut triangles = Vec::new();
    let mut rest = svg;
    while let Some(start) = rest.find("<path") {
        rest = &rest[start + 5..];
        let end = rest.find('>').expect("terminated path element");
        let element = &rest[..end];
        let data = attribute(element, "d")
            .unwrap_or_else(|| panic!("path without d in {}", path.display()));
        let class_color = attribute(element, "class").and_then(|class| styles.get(class).copied());
        let fill = match attribute(element, "fill") {
            Some("none") => 0x00000000,
            Some(value) => parse_color(value).or(class_color).unwrap_or(0xff000000),
            None => class_color.unwrap_or(0xff000000),
        };
        let commands = normalize(PathParser::from(data).map(|segment| {
            segment.unwrap_or_else(|error| panic!("parse path in {}: {error}", path.display()))
        }));
        let lyon_path = lyon_path(&commands);

        if fill != 0x00000000 {
            let mut geometry: VertexBuffers<lyon_tessellation::math::Point, u16> =
                VertexBuffers::new();
            FillTessellator::new()
                .tessellate_path(
                    &lyon_path,
                    &FillOptions::default(),
                    &mut BuffersBuilder::new(&mut geometry, |vertex: FillVertex| vertex.position()),
                )
                .unwrap_or_else(|error| {
                    panic!("fill tessellation failed in {}: {error}", path.display())
                });
            append_triangles(&mut triangles, &geometry, fill);
        }

        if let Some(stroke) = attribute(element, "stroke").and_then(parse_color) {
            let width = attribute(element, "stroke-width")
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(1.0);
            let mut geometry: VertexBuffers<lyon_tessellation::math::Point, u16> =
                VertexBuffers::new();
            StrokeTessellator::new()
                .tessellate_path(
                    &lyon_path,
                    &StrokeOptions::default().with_line_width(width),
                    &mut BuffersBuilder::new(&mut geometry, |vertex: StrokeVertex| {
                        vertex.position()
                    }),
                )
                .unwrap_or_else(|error| {
                    panic!("stroke tessellation failed in {}: {error}", path.display())
                });
            append_triangles(&mut triangles, &geometry, stroke);
        }
        rest = &rest[end + 1..];
    }
    assert!(
        !triangles.is_empty(),
        "SVG contains no drawable paths: {}",
        path.display()
    );
    triangles
}

fn append_triangles(
    destination: &mut Vec<Triangle>,
    geometry: &VertexBuffers<lyon_tessellation::math::Point, u16>,
    color: u32,
) {
    for indices in geometry.indices.chunks_exact(3) {
        let a = geometry.vertices[usize::from(indices[0])];
        let b = geometry.vertices[usize::from(indices[1])];
        let c = geometry.vertices[usize::from(indices[2])];
        destination.push(Triangle {
            a: [a.x, a.y],
            b: [b.x, b.y],
            c: [c.x, c.y],
            color,
        });
    }
}

fn lyon_path(commands: &[Command]) -> LyonPath {
    let mut builder = LyonPath::builder();
    let mut open = false;
    for command in commands {
        match *command {
            Command::Move(x, y) => {
                if open {
                    builder.end(false);
                }
                builder.begin(lyon_tessellation::math::point(x as f32, y as f32));
                open = true;
            }
            Command::Line(x, y) => {
                builder.line_to(lyon_tessellation::math::point(x as f32, y as f32));
            }
            Command::Quad(x1, y1, x, y) => {
                builder.quadratic_bezier_to(
                    lyon_tessellation::math::point(x1 as f32, y1 as f32),
                    lyon_tessellation::math::point(x as f32, y as f32),
                );
            }
            Command::Cubic(x1, y1, x2, y2, x, y) => {
                builder.cubic_bezier_to(
                    lyon_tessellation::math::point(x1 as f32, y1 as f32),
                    lyon_tessellation::math::point(x2 as f32, y2 as f32),
                    lyon_tessellation::math::point(x as f32, y as f32),
                );
            }
            Command::Close => {
                builder.close();
                open = false;
            }
        }
    }
    if open {
        builder.end(false);
    }
    builder.build()
}

fn attribute<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("{name}=\"");
    let mut offset = 0;
    while let Some(found) = source[offset..].find(&prefix) {
        let start = offset + found;
        if start == 0 || source.as_bytes()[start - 1].is_ascii_whitespace() {
            let value_start = start + prefix.len();
            let end = source[value_start..].find('\"')?;
            return Some(&source[value_start..value_start + end]);
        }
        offset = start + prefix.len();
    }
    None
}

fn parse_color(value: &str) -> Option<u32> {
    let value = value.trim();
    let hex = value.strip_prefix('#')?;
    let digits = match hex.len() {
        3 => {
            let mut expanded = String::with_capacity(6);
            for character in hex.chars() {
                expanded.push(character);
                expanded.push(character);
            }
            expanded
        }
        6 => hex.to_owned(),
        8 => hex[..6].to_owned(),
        _ => return None,
    };
    let rgb = u32::from_str_radix(&digits, 16).ok()?;
    Some((rgb << 8) | 0xff)
}

fn normalize(segments: impl Iterator<Item = PathSegment>) -> Vec<Command> {
    let mut commands = Vec::new();
    let mut current = (0.0, 0.0);
    let mut subpath_start = current;
    let mut cubic_control = None;
    let mut quadratic_control = None;
    for segment in segments {
        let endpoint = |abs: bool, x: f64, y: f64, current: (f64, f64)| {
            if abs {
                (x, y)
            } else {
                (current.0 + x, current.1 + y)
            }
        };
        match segment {
            PathSegment::MoveTo { abs, x, y } => {
                current = endpoint(abs, x, y, current);
                subpath_start = current;
                commands.push(Command::Move(current.0, current.1));
                cubic_control = None;
                quadratic_control = None;
            }
            PathSegment::LineTo { abs, x, y } => {
                current = endpoint(abs, x, y, current);
                commands.push(Command::Line(current.0, current.1));
                cubic_control = None;
                quadratic_control = None;
            }
            PathSegment::HorizontalLineTo { abs, x } => {
                current.0 = if abs { x } else { current.0 + x };
                commands.push(Command::Line(current.0, current.1));
                cubic_control = None;
                quadratic_control = None;
            }
            PathSegment::VerticalLineTo { abs, y } => {
                current.1 = if abs { y } else { current.1 + y };
                commands.push(Command::Line(current.0, current.1));
                cubic_control = None;
                quadratic_control = None;
            }
            PathSegment::CurveTo {
                abs,
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let first = endpoint(abs, x1, y1, current);
                let second = endpoint(abs, x2, y2, current);
                current = endpoint(abs, x, y, current);
                commands.push(Command::Cubic(
                    first.0, first.1, second.0, second.1, current.0, current.1,
                ));
                cubic_control = Some(second);
                quadratic_control = None;
            }
            PathSegment::SmoothCurveTo { abs, x2, y2, x, y } => {
                let first = cubic_control
                    .map(|control| (current.0 * 2.0 - control.0, current.1 * 2.0 - control.1))
                    .unwrap_or(current);
                let second = endpoint(abs, x2, y2, current);
                current = endpoint(abs, x, y, current);
                commands.push(Command::Cubic(
                    first.0, first.1, second.0, second.1, current.0, current.1,
                ));
                cubic_control = Some(second);
                quadratic_control = None;
            }
            PathSegment::Quadratic { abs, x1, y1, x, y } => {
                let control = endpoint(abs, x1, y1, current);
                current = endpoint(abs, x, y, current);
                commands.push(Command::Quad(control.0, control.1, current.0, current.1));
                quadratic_control = Some(control);
                cubic_control = None;
            }
            PathSegment::SmoothQuadratic { abs, x, y } => {
                let control = quadratic_control
                    .map(|previous| (current.0 * 2.0 - previous.0, current.1 * 2.0 - previous.1))
                    .unwrap_or(current);
                current = endpoint(abs, x, y, current);
                commands.push(Command::Quad(control.0, control.1, current.0, current.1));
                quadratic_control = Some(control);
                cubic_control = None;
            }
            PathSegment::EllipticalArc {
                abs,
                rx,
                ry,
                x_axis_rotation,
                large_arc,
                sweep,
                x,
                y,
            } => {
                let start = current;
                current = endpoint(abs, x, y, current);
                append_arc(
                    &mut commands,
                    start,
                    current,
                    rx,
                    ry,
                    x_axis_rotation,
                    large_arc,
                    sweep,
                );
                cubic_control = None;
                quadratic_control = None;
            }
            PathSegment::ClosePath { .. } => {
                current = subpath_start;
                commands.push(Command::Close);
                cubic_control = None;
                quadratic_control = None;
            }
        }
    }
    commands
}

fn append_arc(
    commands: &mut Vec<Command>,
    start: (f64, f64),
    end: (f64, f64),
    radius_x: f64,
    radius_y: f64,
    rotation: f64,
    large_arc: bool,
    sweep: bool,
) {
    let mut radius_x = radius_x.abs();
    let mut radius_y = radius_y.abs();
    if radius_x <= f64::EPSILON || radius_y <= f64::EPSILON || start == end {
        commands.push(Command::Line(end.0, end.1));
        return;
    }

    let phi = rotation.to_radians();
    let cosine = phi.cos();
    let sine = phi.sin();
    let half_x = (start.0 - end.0) * 0.5;
    let half_y = (start.1 - end.1) * 0.5;
    let transformed_x = cosine * half_x + sine * half_y;
    let transformed_y = -sine * half_x + cosine * half_y;
    let lambda = transformed_x * transformed_x / (radius_x * radius_x)
        + transformed_y * transformed_y / (radius_y * radius_y);
    if lambda > 1.0 {
        let scale = lambda.sqrt();
        radius_x *= scale;
        radius_y *= scale;
    }

    let numerator = (radius_x * radius_x * radius_y * radius_y
        - radius_x * radius_x * transformed_y * transformed_y
        - radius_y * radius_y * transformed_x * transformed_x)
        .max(0.0);
    let denominator = radius_x * radius_x * transformed_y * transformed_y
        + radius_y * radius_y * transformed_x * transformed_x;
    let sign = if large_arc == sweep { -1.0 } else { 1.0 };
    let factor = if denominator <= f64::EPSILON {
        0.0
    } else {
        sign * (numerator / denominator).sqrt()
    };
    let center_transformed_x = factor * radius_x * transformed_y / radius_y;
    let center_transformed_y = factor * -radius_y * transformed_x / radius_x;
    let center = (
        cosine * center_transformed_x - sine * center_transformed_y + (start.0 + end.0) * 0.5,
        sine * center_transformed_x + cosine * center_transformed_y + (start.1 + end.1) * 0.5,
    );
    let start_vector = (
        (transformed_x - center_transformed_x) / radius_x,
        (transformed_y - center_transformed_y) / radius_y,
    );
    let end_vector = (
        (-transformed_x - center_transformed_x) / radius_x,
        (-transformed_y - center_transformed_y) / radius_y,
    );
    let angle = |left: (f64, f64), right: (f64, f64)| {
        (left.0 * right.1 - left.1 * right.0).atan2(left.0 * right.0 + left.1 * right.1)
    };
    let start_angle = angle((1.0, 0.0), start_vector);
    let mut sweep_angle = angle(start_vector, end_vector);
    if !sweep && sweep_angle > 0.0 {
        sweep_angle -= std::f64::consts::TAU;
    } else if sweep && sweep_angle < 0.0 {
        sweep_angle += std::f64::consts::TAU;
    }
    let steps = (sweep_angle.abs() * radius_x.max(radius_y))
        .ceil()
        .clamp(8.0, 96.0) as usize;
    for step in 1..=steps {
        let current_angle = start_angle + sweep_angle * step as f64 / steps as f64;
        let arc_x = radius_x * current_angle.cos();
        let arc_y = radius_y * current_angle.sin();
        commands.push(Command::Line(
            center.0 + cosine * arc_x - sine * arc_y,
            center.1 + sine * arc_x + cosine * arc_y,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::load_asset;

    #[test]
    fn uses_process_root_for_installed_icon_theme() {
        assert_eq!(super::ICON_THEME_ROOT, "/share/icons/default");
    }

    #[test]
    fn loads_mp4_with_css_class_colors() {
        let data = load_asset("scalable/mimetypes/mp4").expect("bundled mp4 icon");
        assert_eq!(data.width(), 72.0);
        assert_eq!(data.height(), 96.0);
        assert!(data.triangles().iter().any(|triangle| {
            triangle.color.r > 0.9 && triangle.color.g > 0.2 && triangle.color.g < 0.6
        }));
    }

    #[test]
    fn loads_folder_from_the_default_places_context() {
        let data = load_asset("scalable/places/folder").expect("bundled folder icon");
        assert!(!data.triangles().is_empty());
    }

    #[test]
    fn loads_blank_as_the_default_file_icon() {
        let data = load_asset(super::DEFAULT_FILE_ICON).expect("bundled blank icon");
        assert_eq!(data.width(), 72.0);
        assert_eq!(data.height(), 96.0);
        assert!(!data.triangles().is_empty());
    }

    #[test]
    fn loads_every_default_mimetype_icon() {
        let directory = super::theme_root().join("scalable/mimetypes");
        let mut count = 0;
        for entry in std::fs::read_dir(directory).expect("default mimetype icon directory") {
            let path = entry.expect("mimetype icon entry").path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("svg") {
                continue;
            }
            let name = path.file_stem().and_then(|name| name.to_str()).unwrap();
            assert!(
                load_asset(&format!("scalable/mimetypes/{name}")).is_some(),
                "failed to load {name}.svg"
            );
            count += 1;
        }
        assert_eq!(count, 404);
    }
}
