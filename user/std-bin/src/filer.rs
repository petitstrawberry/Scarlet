//! Scarlet Desktop file manager and file picker.
//!
//! The application follows the same direct-view structure as ScarletUI's
//! Widget Factory. It runs on Rust `std`, presents files in a grid, and can
//! switch into picker mode through sbus without embedding picker UI in the
//! calling application.

mod file_icons;

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use sbus::{Argument, Message};
use sbus_client::Connection as SbusConnection;
use scarlet_desktop_config::{
    DESKTOP_FILE_MANAGER_BUS_NAME, DESKTOP_FILE_MANAGER_INTERFACE,
    DESKTOP_FILE_MANAGER_OBJECT_PATH, DESKTOP_FILE_MANAGER_OPEN_FILE_METHOD,
    DESKTOP_FILE_MANAGER_RESPONSE_SIGNAL, DESKTOP_STEMD_BUS_NAME, DESKTOP_STEMD_INTERFACE,
    DESKTOP_STEMD_OBJECT_PATH, DESKTOP_STEMD_OPEN_PATH_METHOD,
};
use scarlet_ui::prelude::*;
use scarlet_ui::{
    Alignment, Button, Color, ColorPalette, Divider, GridView, HeaderBar, Icon, IconSize, IconView,
    Image, ImageFit, NavigationLink, Spacer, dismiss_window, hstack, measure_text_sized,
    navigation, vstack,
};
use scarlet_ui_macros::View;

use file_icons::{FileKind, icon_for_entry};

const APP_ID: &str = "org.scarlet-os.desktop.filer";
const ROOT_PATH: &str = "/";
const GRID_COLUMNS: usize = 5;
const GRID_ROW_HEIGHT: f32 = 146.0;
const FILE_CELL_WIDTH: f32 = 146.0;
const FILE_PREVIEW_WIDTH: f32 = 96.0;
const FILE_PREVIEW_HEIGHT: f32 = 78.0;
const FILE_NAME_FONT_SIZE: f32 = 13.0;
const FILE_NAME_HEIGHT: f32 = 32.0;
const FILE_NAME_MAX_WIDTH: f32 = 132.0;
const SERVICE_RETRY_DELAY: Duration = Duration::from_millis(100);
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone)]
struct FileEntry {
    name: String,
    path: String,
    is_directory: bool,
    size: u64,
    kind: FileKind,
}

#[derive(Clone)]
struct PickerRequest {
    id: String,
    title: String,
    initial_folder: String,
    filter: String,
    allow_multiple: bool,
    select_directories: bool,
}

#[derive(View, Clone)]
struct FilerApp {
    current_path: State<String>,
    entries: State<Vec<FileEntry>>,
    selected: State<Option<usize>>,
    hovered: State<Option<usize>>,
    last_click: State<Option<(usize, Instant)>>,
    picker_request: State<Option<PickerRequest>>,
    status: State<String>,
    request_sequence: State<u32>,
}

impl FilerApp {
    fn new() -> Self {
        let current_path = initial_path();
        let entries = read_entries(&current_path, None).unwrap_or_default();
        Self {
            current_path: State::new(StateId::new(0), current_path),
            entries: State::new(StateId::new(1), entries),
            selected: State::new(StateId::new(2), None),
            hovered: State::new(StateId::new(3), None),
            last_click: State::new(StateId::new(4), None),
            picker_request: State::new(StateId::new(5), None),
            status: State::new(StateId::new(6), String::from("Ready")),
            request_sequence: State::new(StateId::new(7), 1),
        }
    }

    fn refresh(&self) {
        let path = self.current_path.get();
        let picker = self.picker_request.get();
        match read_entries(&path, picker.as_ref()) {
            Ok(entries) => {
                self.entries.set(entries);
                self.selected.set(None);
                self.hovered.set(None);
                self.last_click.set(None);
                if picker.is_none() {
                    self.status.set(String::from("Ready"));
                }
            }
            Err(error) => self.status.set(format!("Cannot open {path}: {error}")),
        }
    }

    fn navigate_to(&self, path: impl Into<String>) {
        let path = path.into();
        if !Path::new(&path).is_dir() {
            self.status.set(format!("Cannot open {path}"));
            return;
        }
        self.current_path.set(path);
        self.refresh();
    }

    fn navigate_up(&self) {
        let current = PathBuf::from(self.current_path.get());
        let parent = current
            .parent()
            .unwrap_or_else(|| Path::new(ROOT_PATH))
            .to_string_lossy()
            .into_owned();
        self.navigate_to(parent);
    }

    fn activate_entry(&self, index: usize) {
        let Some(entry) = self.entries.get().get(index).cloned() else {
            return;
        };

        let now = Instant::now();
        let double_click = self
            .last_click
            .get()
            .is_some_and(|(last_index, timestamp)| {
                last_index == index && now.duration_since(timestamp) <= DOUBLE_CLICK_INTERVAL
            });
        self.last_click.set(if double_click {
            None
        } else {
            Some((index, now))
        });

        let picker_selects_directories = self
            .picker_request
            .get()
            .as_ref()
            .is_some_and(|request| request.select_directories);

        self.selected.set(Some(index));
        self.status.set(entry.path.clone());

        if !double_click || picker_selects_directories {
            return;
        }

        if entry.is_directory {
            self.navigate_to(entry.path);
        } else {
            self.open_entry(entry.path);
        }
    }

    fn open_entry(&self, path: String) {
        let result = SbusConnection::connect().and_then(|mut connection| {
            connection.call_method(
                DESKTOP_STEMD_BUS_NAME,
                DESKTOP_STEMD_OBJECT_PATH,
                DESKTOP_STEMD_INTERFACE,
                DESKTOP_STEMD_OPEN_PATH_METHOD,
                vec![Argument::String(path.clone())],
            )
        });

        match result {
            Ok(_) => self.status.set(format!("Opening {path}")),
            Err(error) => self.status.set(format!("Cannot open {path}: {error:?}")),
        }
    }

    fn create_folder(&self) {
        let base = PathBuf::from(self.current_path.get());
        for suffix in 0..100 {
            let name = if suffix == 0 {
                String::from("New Folder")
            } else {
                format!("New Folder {suffix}")
            };
            let path = base.join(name);
            if path.exists() {
                continue;
            }
            match fs::create_dir(&path) {
                Ok(()) => {
                    self.status
                        .set(format!("Created {}", path.to_string_lossy()));
                    self.refresh();
                }
                Err(error) => self.status.set(format!("Cannot create folder: {error}")),
            }
            return;
        }
        self.status
            .set(String::from("Cannot choose a name for the new folder"));
    }

    fn next_request_id(&self) -> String {
        let sequence = self.request_sequence.get();
        self.request_sequence.set(sequence.wrapping_add(1).max(1));
        format!("request-{sequence}")
    }

    fn begin_picker(&self, mut request: PickerRequest) -> String {
        request.id = self.next_request_id();
        let initial_folder = if Path::new(&request.initial_folder).is_dir() {
            request.initial_folder.clone()
        } else {
            initial_path()
        };
        let request_id = request.id.clone();
        let title = request.title.clone();
        self.picker_request.set(Some(request));
        self.current_path.set(initial_folder);
        self.refresh();
        self.status.set(title);
        request_id
    }

    fn finish_picker(&self, accepted: bool) {
        let Some(request) = self.picker_request.get() else {
            return;
        };
        let selected_path = self.selected.get().and_then(|index| {
            self.entries
                .get()
                .get(index)
                .map(|entry| entry.path.clone())
        });
        let success = accepted && selected_path.is_some();
        let path = selected_path.unwrap_or_default();

        match SbusConnection::connect().and_then(|mut connection| {
            connection.emit_signal(
                DESKTOP_FILE_MANAGER_BUS_NAME,
                DESKTOP_FILE_MANAGER_OBJECT_PATH,
                DESKTOP_FILE_MANAGER_INTERFACE,
                DESKTOP_FILE_MANAGER_RESPONSE_SIGNAL,
                vec![
                    Argument::String(request.id),
                    Argument::Boolean(success),
                    Argument::String(path),
                ],
            )
        }) {
            Ok(()) => {}
            Err(error) => eprintln!("[filer] failed to send picker response: {error:?}"),
        }

        self.picker_request.set(None);
        dismiss_window("main");
    }

    fn file_cell(
        &self,
        index: usize,
        entry: FileEntry,
        selected_index: Option<usize>,
    ) -> impl View + Clone + use<> {
        let palette = ColorPalette::default();
        let background = if selected_index == Some(index) {
            palette.primary_light().with_opacity(0.16)
        } else if self.hovered.get() == Some(index) {
            palette.background_tertiary()
        } else {
            Color::CLEAR
        };
        let border = if selected_index == Some(index) {
            palette.primary()
        } else {
            Color::CLEAR
        };
        let preview =
            if entry.kind == FileKind::Image && !entry.is_directory && is_jpeg(&entry.name) {
                Either::A(
                    Image::from_path(entry.path.clone())
                        .fit_mode(ImageFit::Cover)
                        .frame(FILE_PREVIEW_WIDTH, FILE_PREVIEW_HEIGHT),
                )
            } else {
                Either::B(
                    icon_for_entry(&entry.name, entry.is_directory)
                        .frame(FILE_PREVIEW_WIDTH, FILE_PREVIEW_HEIGHT),
                )
            };
        let detail = if entry.is_directory {
            String::from("Folder")
        } else {
            format_size(entry.size)
        };
        let file_name = if entry.is_directory {
            Either::A(Text::new(entry.name).font_size(FILE_NAME_FONT_SIZE))
        } else {
            Either::B(
                Text::new(file_name_label(&entry.name))
                    .font_size(FILE_NAME_FONT_SIZE)
                    .alignment(Alignment::Center)
                    .frame(f32::INFINITY, FILE_NAME_HEIGHT),
            )
        };
        let app = self.clone();
        let hover_state = self.hovered.clone();
        let exit_state = self.hovered.clone();

        vstack! {
            preview,
            file_name,
            Text::new(detail)
                .font_size(10.0)
                .color(palette.text_secondary()),
        }
        .alignment(Alignment::Center)
        .frame(f32::INFINITY, GRID_ROW_HEIGHT)
        .padding(8.0)
        .background(background)
        .clip_radius(8.0)
        .border_rounded(border, 2.0, 8.0)
        .on_hover(move || hover_state.set(Some(index)))
        .on_exit(move || {
            if exit_state.get() == Some(index) {
                exit_state.set(None);
            }
        })
        .on_click(move || app.activate_entry(index))
    }

    fn files_page(&self) -> impl View + Clone + use<> {
        let grid_app = self.clone();
        let grid = GridView::new(
            self.entries.clone(),
            self.selected.clone(),
            GRID_COLUMNS,
            GRID_ROW_HEIGHT,
            move |index, entry, selected| grid_app.file_cell(index, entry, selected),
        )
        .spacing(10.0)
        .minimum_cell_width(FILE_CELL_WIDTH);

        let picker = self.picker_request.get();
        let title = picker
            .as_ref()
            .map(|request| request.title.clone())
            .unwrap_or_else(|| String::from("Files"));
        let footer = if picker.is_some() {
            let cancel_app = self.clone();
            let open_app = self.clone();
            Either::A(vstack! {
                Divider::new(),
                hstack! {
                    Text::from_state(self.status.clone()).font_size(11.0),
                    Spacer::new(),
                    Button::new("Cancel").on_click(move || cancel_app.finish_picker(false)),
                    Button::new("Open").on_click(move || open_app.finish_picker(true)),
                }
                .alignment(Alignment::Center)
                .padding(8.0),
            })
        } else {
            Either::B(vstack! {})
        };

        vstack! {
            Text::new(title)
                .font_size(12.0)
                .color(ColorPalette::default().text_secondary()),
            grid,
            footer,
        }
        .alignment(Alignment::Leading)
        .padding(10.0)
    }

    fn header(&self) -> impl View + Clone + use<> {
        let back_app = self.clone();
        let up_app = self.clone();
        let create_app = self.clone();
        let refresh_app = self.clone();
        HeaderBar::new(
            hstack! {
                Button::icon_only(Icon::ChevronLeft)
                    .header_style()
                    .on_click(move || back_app.navigate_up()),
                Button::icon_only(Icon::ArrowUp)
                    .header_style()
                    .on_click(move || up_app.navigate_up()),
                IconView::new(Icon::Folder)
                    .size(IconSize::Medium)
                    .filled(),
                Text::from_state(self.current_path.clone()).font_size(14.0),
                Spacer::new(),
                Button::icon_only(Icon::FolderPlus)
                    .header_style()
                    .on_click(move || create_app.create_folder()),
                Button::icon_only(Icon::Refresh)
                    .header_style()
                    .on_click(move || refresh_app.refresh()),
                Button::icon_only(Icon::LayoutGrid).header_style(),
                Button::icon_only(Icon::Menu2).header_style(),
            }
            .alignment(Alignment::Center)
            .padding(8.0),
        )
        .height(48.0)
    }
}

impl Application for FilerApp {
    fn scenes(&self) -> impl Scene {
        let home = self.clone();
        let computer = self.clone();
        let pictures = self.clone();
        let downloads = self.clone();
        let header = self.clone();
        let home_path = home_path();

        WindowGroup::new(
            "main",
            Window::new(
                "Files",
                navigation! {
                    NavigationLink::new("Home", move || home.files_page())
                        .icon(Icon::Home)
                        .on_select({
                            let app = self.clone();
                            let home_path = home_path.clone();
                            move || app.navigate_to(home_path.clone())
                        }),
                    NavigationLink::new("Computer", move || computer.files_page())
                        .icon(Icon::DeviceDesktop)
                        .on_select({
                            let app = self.clone();
                            move || app.navigate_to(ROOT_PATH)
                        }),
                    NavigationLink::new("Pictures", move || pictures.files_page())
                        .icon(Icon::Photo)
                        .on_select({
                            let app = self.clone();
                            move || app.navigate_to("/home/pictures")
                        }),
                    NavigationLink::new("Downloads", move || downloads.files_page())
                        .icon(Icon::Download)
                        .on_select({
                            let app = self.clone();
                            move || app.navigate_to("/home/downloads")
                        }),
                }
                .header(move || header.header())
                .shows_icons(true)
                .sidebar_width(170.0),
            )
            .app_id(APP_ID)
            .size(Size::new(960.0, 640.0)),
        )
    }
}

fn file_name_label(name: &str) -> String {
    let normalized = name.replace('\n', " ");
    if text_width(&normalized) <= FILE_NAME_MAX_WIDTH {
        return normalized;
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    for character in normalized.chars() {
        let mut candidate = current.clone();
        candidate.push(character);
        if !current.is_empty() && text_width(&candidate) > FILE_NAME_MAX_WIDTH {
            lines.push(current);
            current = character.to_string();
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }

    if lines.len() <= 2 {
        return lines.join("\n");
    }

    let first = lines.remove(0);
    let mut second = lines.remove(0);
    while !second.is_empty() && text_width(&format!("{second}…")) > FILE_NAME_MAX_WIDTH {
        second.pop();
    }
    if second.is_empty() {
        return format!("{first}\n…");
    }
    format!("{first}\n{second}…")
}

fn text_width(text: &str) -> f32 {
    measure_text_sized(text, FILE_NAME_FONT_SIZE).0 as f32
}

#[cfg(test)]
mod layout_tests {
    use super::{FILE_NAME_MAX_WIDTH, FileEntry, FileKind, FilerApp, file_name_label, text_width};
    use scarlet_ui::{
        Application, Color, Event, MouseButton, MouseEvent, NavigationLink, NavigationView,
        RenderingPipeline, Scene, SceneBuilder, Size, Text, View, Window,
    };

    #[test]
    fn long_file_names_use_two_lines() {
        let label = file_name_label("witch_hat_atelier_op_source.mp4");
        let lines: Vec<&str> = label.lines().collect();

        assert_eq!(lines.len(), 2);
        assert!(
            lines
                .iter()
                .all(|line| text_width(line) <= FILE_NAME_MAX_WIDTH)
        );
    }

    #[test]
    fn renders_all_folder_columns_on_initial_paint() {
        let app = FilerApp::new();
        app.entries.set(
            (0..20)
                .map(|index| FileEntry {
                    name: format!("Folder {index}"),
                    path: format!("/tmp/folder-{index}"),
                    is_directory: true,
                    size: 0,
                    kind: FileKind::Folder,
                })
                .collect(),
        );
        let page_app = app.clone();
        let root = Window::new(
            "Files",
            NavigationView::new((NavigationLink::new("Files", move || page_app.files_page()),))
                .sidebar_width(170.0),
        )
        .size(Size::new(960.0, 640.0));

        let mut pipeline = RenderingPipeline::new();
        pipeline.set_root(root.create_element());
        pipeline.layout_initial();
        let buffer = pipeline
            .render_with_damage()
            .map(|(buffer, _)| buffer)
            .expect("filer should render");

        let folder_orange = Color::rgb(255, 160, 0).to_bgra();
        for column in 0..5 {
            let left = 180 + column * 156;
            let right = (left + 146).min(buffer.width());
            let found = (left..right).any(|x| {
                (0..buffer.height()).any(|y| buffer.get_pixel(x, y) == Some(folder_orange))
            });
            assert!(found, "folder column {column} should be painted");
        }
    }

    #[test]
    fn renders_all_folder_columns_after_navigation_switch() {
        let app = FilerApp::new();
        app.entries.set(
            (0..20)
                .map(|index| FileEntry {
                    name: format!("Folder {index}"),
                    path: format!("/tmp/folder-{index}"),
                    is_directory: true,
                    size: 0,
                    kind: FileKind::Folder,
                })
                .collect(),
        );
        let page_app = app.clone();
        let root = Window::new(
            "Files",
            NavigationView::new((
                NavigationLink::new("First", || Text::new("First")),
                NavigationLink::new("Files", move || page_app.files_page()),
            ))
            .sidebar_width(170.0),
        )
        .size(Size::new(960.0, 640.0));

        let mut pipeline = RenderingPipeline::new();
        pipeline.set_root(root.create_element());
        pipeline.layout_initial();
        pipeline
            .render_with_damage()
            .expect("initial page should render");
        assert!(
            pipeline.handle_event(&Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                x: 10,
                y: 85,
                click_count: 1,
            }))
        );
        let buffer = pipeline
            .render_with_damage()
            .map(|(buffer, _)| buffer)
            .expect("filer page should render after navigation");

        let folder_orange = Color::rgb(255, 160, 0).to_bgra();
        for column in 0..5 {
            let left = 180 + column * 156;
            let right = (left + 146).min(buffer.width());
            let found = (left..right).any(|x| {
                (0..buffer.height()).any(|y| buffer.get_pixel(x, y) == Some(folder_orange))
            });
            assert!(
                found,
                "folder column {column} should be painted after navigation"
            );
        }
    }

    #[test]
    fn renders_all_folder_columns_through_application_scene() {
        let app = FilerApp::new();
        app.entries.set(
            (0..20)
                .map(|index| FileEntry {
                    name: format!("Folder {index}"),
                    path: format!("/tmp/folder-{index}"),
                    is_directory: true,
                    size: 0,
                    kind: FileKind::Folder,
                })
                .collect(),
        );

        let mut builder = SceneBuilder::new();
        app.scenes().build(&mut builder);
        let declaration = builder
            .into_declarations()
            .into_iter()
            .next()
            .expect("filer should declare its main window");

        let mut pipeline = RenderingPipeline::new();
        pipeline.set_root(declaration.view.create_element());
        let window_info = pipeline.layout_initial();
        assert_eq!(window_info.size, Size::new(960.0, 640.0));
        let buffer = pipeline
            .render_with_damage()
            .map(|(buffer, _)| buffer)
            .expect("filer scene should render");

        let folder_orange = Color::rgb(255, 160, 0).to_bgra();
        for column in 0..5 {
            let left = 180 + column * 156;
            let right = (left + 146).min(buffer.width());
            let found = (left..right).any(|x| {
                (0..buffer.height()).any(|y| buffer.get_pixel(x, y) == Some(folder_orange))
            });
            assert!(
                found,
                "folder column {column} should be painted in app scene"
            );
        }
    }

    #[test]
    fn renders_new_folder_rows_after_entries_change() {
        let app = FilerApp::new();
        app.entries.set(
            (0..5)
                .map(|index| FileEntry {
                    name: format!("Folder {index}"),
                    path: format!("/tmp/folder-{index}"),
                    is_directory: true,
                    size: 0,
                    kind: FileKind::Folder,
                })
                .collect(),
        );

        let mut builder = SceneBuilder::new();
        app.scenes().build(&mut builder);
        let declaration = builder
            .into_declarations()
            .into_iter()
            .next()
            .expect("filer should declare its main window");

        let mut pipeline = RenderingPipeline::new();
        pipeline.set_root(declaration.view.create_element());
        pipeline.layout_initial();
        pipeline
            .render_with_damage()
            .expect("initial filer scene should render");

        app.entries.set(
            (0..20)
                .map(|index| FileEntry {
                    name: format!("Folder {index}"),
                    path: format!("/tmp/folder-{index}"),
                    is_directory: true,
                    size: 0,
                    kind: FileKind::Folder,
                })
                .collect(),
        );
        let buffer = pipeline
            .render_with_damage()
            .map(|(buffer, _)| buffer)
            .expect("filer scene should repaint after entries change");

        let folder_orange = Color::rgb(255, 160, 0).to_bgra();
        assert!(
            (180..buffer.width()).any(|x| {
                (200..buffer.height()).any(|y| buffer.get_pixel(x, y) == Some(folder_orange))
            }),
            "a second folder row should be painted after entries change"
        );
    }

    #[test]
    fn keeps_folder_cells_spaced_when_window_resizes() {
        let app = FilerApp::new();
        app.entries.set(
            (0..10)
                .map(|index| FileEntry {
                    name: format!("Folder {index}"),
                    path: format!("/tmp/folder-{index}"),
                    is_directory: true,
                    size: 0,
                    kind: FileKind::Folder,
                })
                .collect(),
        );

        let mut builder = SceneBuilder::new();
        app.scenes().build(&mut builder);
        let declaration = builder
            .into_declarations()
            .into_iter()
            .next()
            .expect("filer should declare its main window");

        let mut pipeline = RenderingPipeline::new();
        pipeline.set_root(declaration.view.create_element());
        pipeline.layout_initial();
        pipeline
            .render_with_damage()
            .expect("initial filer scene should render");

        pipeline.resize(Size::new(500.0, 640.0));
        let buffer = pipeline
            .render_with_damage()
            .map(|(buffer, _)| buffer)
            .expect("resized filer scene should render");

        let folder_orange = Color::rgb(255, 160, 0).to_bgra();
        assert!(
            (180..buffer.width()).any(|x| {
                (180..buffer.height()).any(|y| buffer.get_pixel(x, y) == Some(folder_orange))
            }),
            "resized filer should wrap folders into another row instead of squeezing them"
        );
    }
}

fn initial_path() -> String {
    let home = home_path();
    [home.as_str(), "/tmp", ROOT_PATH]
        .into_iter()
        .find(|path| Path::new(path).is_dir())
        .unwrap_or(ROOT_PATH)
        .to_owned()
}

fn home_path() -> String {
    std::env::var_os("HOME")
        .map(|path| path.to_string_lossy().into_owned())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| String::from(ROOT_PATH))
}

fn read_entries(path: &str, picker: Option<&PickerRequest>) -> std::io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type()?;
        let is_directory = file_type.is_dir();
        if let Some(request) = picker
            && !is_directory
            && !matches_filter(&name, &request.filter)
        {
            continue;
        }
        let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        let path = entry.path().to_string_lossy().into_owned();
        entries.push(FileEntry {
            kind: if is_directory {
                FileKind::Folder
            } else {
                FileKind::from_path(&name)
            },
            name,
            path,
            is_directory,
            size,
        });
    }

    entries.sort_by(
        |left, right| match (left.is_directory, right.is_directory) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => left.name.to_lowercase().cmp(&right.name.to_lowercase()),
        },
    );
    Ok(entries)
}

fn matches_filter(name: &str, filter: &str) -> bool {
    match filter {
        "" | "*" | "*/*" => true,
        "image/*" => is_image(name),
        "image/jpeg" | "image/jpg" => name.rsplit_once('.').is_some_and(|(_, extension)| {
            extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg")
        }),
        _ => true,
    }
}

fn is_image(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        ["jpg", "jpeg", "png", "gif", "bmp", "webp"]
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    })
}

fn is_jpeg(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg")
    })
}

fn format_size(size: u64) -> String {
    if size >= 1024 * 1024 * 1024 {
        format!("{} GiB", size / (1024 * 1024 * 1024))
    } else if size >= 1024 * 1024 {
        format!("{} MiB", size / (1024 * 1024))
    } else if size >= 1024 {
        format!("{} KiB", size / 1024)
    } else {
        format!("{size} B")
    }
}

fn argument_string(arguments: &[Argument], index: usize) -> Option<String> {
    match arguments.get(index) {
        Some(Argument::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn argument_bool(arguments: &[Argument], index: usize) -> bool {
    matches!(arguments.get(index), Some(Argument::Boolean(true)))
}

fn run_picker_service(app: FilerApp) {
    loop {
        let Ok(mut connection) = SbusConnection::connect() else {
            thread::sleep(SERVICE_RETRY_DELAY);
            continue;
        };
        if connection
            .register_service(DESKTOP_FILE_MANAGER_BUS_NAME)
            .is_err()
        {
            thread::sleep(SERVICE_RETRY_DELAY);
            continue;
        }
        println!("[filer] registered as {DESKTOP_FILE_MANAGER_BUS_NAME}");

        loop {
            let message = match connection.receive_message() {
                Ok(message) => message,
                Err(error) => {
                    eprintln!("[filer] sbus connection lost: {error:?}");
                    thread::sleep(SERVICE_RETRY_DELAY);
                    break;
                }
            };
            let Message::CallMethod {
                path,
                interface,
                method,
                args,
                ..
            } = message
            else {
                continue;
            };
            if path != DESKTOP_FILE_MANAGER_OBJECT_PATH
                || interface != DESKTOP_FILE_MANAGER_INTERFACE
            {
                continue;
            }
            if method != DESKTOP_FILE_MANAGER_OPEN_FILE_METHOD {
                let _ = connection.send_method_error(
                    0,
                    "org.scarlet.desktop.FileManager.UnknownMethod",
                    "Unknown FileManager method",
                );
                continue;
            }

            let request = PickerRequest {
                id: String::new(),
                title: argument_string(&args, 0).unwrap_or_else(|| String::from("Open File")),
                initial_folder: argument_string(&args, 1).unwrap_or_else(initial_path),
                filter: argument_string(&args, 2).unwrap_or_default(),
                allow_multiple: argument_bool(&args, 3),
                select_directories: argument_bool(&args, 4),
            };
            if request.allow_multiple {
                app.status
                    .set(String::from("Multiple selection is not supported yet"));
            }
            let request_id = app.begin_picker(request);
            let _ = connection.send_method_return(0, vec![Argument::String(request_id)]);
        }
    }
}

fn main() {
    println!("[filer] starting");
    let mut app = FilerApp::new();
    let service_app = app.clone();
    thread::spawn(move || run_picker_service(service_app));
    if let Err(error) = app.run() {
        eprintln!("[filer] error: {error}");
    }
}
