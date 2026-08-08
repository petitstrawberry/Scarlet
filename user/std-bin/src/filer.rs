//! Scarlet Desktop file manager and file picker.
//!
//! The application follows the same direct-view structure as ScarletUI's
//! Widget Factory. It runs on Rust `std`, presents files in a grid, and can
//! switch into picker mode through sbus without embedding picker UI in the
//! calling application.

mod file_icons;

use core::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use sbus::{Argument, Message};
use sbus_client::Connection as SbusConnection;
use scarlet_desktop_config::{
    DESKTOP_FILE_MANAGER_BUS_NAME, DESKTOP_FILE_MANAGER_INTERFACE,
    DESKTOP_FILE_MANAGER_OBJECT_PATH, DESKTOP_FILE_MANAGER_OPEN_FILE_METHOD,
    DESKTOP_FILE_MANAGER_RESPONSE_SIGNAL, DESKTOP_FILE_MANAGER_SAVE_FILE_METHOD,
    DESKTOP_FILE_MANAGER_SHOW_METHOD, DESKTOP_FILES_APP_ID, DESKTOP_STEMD_BUS_NAME,
    DESKTOP_STEMD_INTERFACE, DESKTOP_STEMD_OBJECT_PATH, DESKTOP_STEMD_OPEN_PATH_METHOD,
};
use scarlet_ui::prelude::*;
use scarlet_ui::{
    Alignment, Button, Color, ColorPalette, Divider, GridView, HeaderBar, Icon, IconSize, IconView,
    Image, ImageFit, NavigationLink, Spacer, dismiss_window, hstack, measure_text_sized,
    navigation, open_window, vstack,
};
use scarlet_ui_macros::View;

use file_icons::{FileKind, icon_for_entry};

const APP_ID: &str = DESKTOP_FILES_APP_ID;
const PICKER_WINDOW_KEY: &str = "picker";
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

/// Path to the files binary, used to re-exec in standalone picker mode.
const FILES_BINARY_PATH: &str = "/bin/files";
/// Set when the process was launched with `--picker`. In that mode the picker
/// window opens at launch, the response signal is sent synchronously on
/// completion, and the process exits when the picker window closes.
static STANDALONE_PICKER: AtomicBool = AtomicBool::new(false);

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
    save_mode: bool,
    suggested_name: String,
}

impl PickerRequest {
    fn same_invocation(&self, other: &Self) -> bool {
        self.title == other.title
            && self.initial_folder == other.initial_folder
            && self.filter == other.filter
            && self.allow_multiple == other.allow_multiple
            && self.select_directories == other.select_directories
            && self.save_mode == other.save_mode
            && self.suggested_name == other.suggested_name
    }
}

enum FilerUiRequest {
    ShowMain,
}

struct PickerReadResult {
    request_id: String,
    generation: u32,
    path: String,
    result: std::result::Result<Vec<FileEntry>, String>,
}

struct DirectoryReadResult {
    instance_id: u64,
    generation: u32,
    path: String,
    result: std::result::Result<Vec<FileEntry>, String>,
}

struct PickerChild {
    request: PickerRequest,
    child: std::process::Child,
}

static NEXT_FILER_INSTANCE_ID: Mutex<u64> = Mutex::new(1);
static NEXT_PICKER_REQUEST_ID: Mutex<u32> = Mutex::new(1);
static PENDING_UI_REQUESTS: Mutex<Vec<FilerUiRequest>> = Mutex::new(Vec::new());
static PENDING_DIRECTORY_READS: Mutex<Vec<DirectoryReadResult>> = Mutex::new(Vec::new());
static PENDING_PICKER_REQUESTS: Mutex<Vec<PickerRequest>> = Mutex::new(Vec::new());
static PENDING_PICKER_READS: Mutex<Vec<PickerReadResult>> = Mutex::new(Vec::new());
static PICKER_WINDOW_CLOSING: AtomicBool = AtomicBool::new(false);

/// Picker child processes spawned via `launch_picker_process`. The background
/// reaper thread polls them with `try_wait` so the UI thread never blocks and
/// exited children are reaped without becoming zombies. Keeping the request
/// beside the child also makes retried sbus calls idempotent while that picker
/// remains alive.
static PICKER_CHILDREN: Mutex<Vec<PickerChild>> = Mutex::new(Vec::new());

fn mutex_lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(View, Clone)]
struct FilerApp {
    current_path: State<String>,
    entries: State<Vec<FileEntry>>,
    selected: State<Option<usize>>,
    hovered: State<Option<usize>>,
    last_click: State<Option<(usize, Instant)>>,
    picker_current_path: State<String>,
    picker_entries: State<Vec<FileEntry>>,
    picker_selected: State<Option<usize>>,
    picker_hovered: State<Option<usize>>,
    picker_last_click: State<Option<(usize, Instant)>>,
    picker_request: State<Option<PickerRequest>>,
    picker_file_name: State<String>,
    picker_status: State<String>,
    picker_read_generation: State<u32>,
    directory_read_instance: State<u64>,
    directory_read_generation: State<u32>,
    status: State<String>,
}

impl FilerApp {
    fn new() -> Self {
        let current_path = initial_path();
        let app = Self::default();
        app.directory_read_instance.set(next_filer_instance_id());
        app.current_path.set(current_path.clone());
        app.entries.set(Vec::new());
        app.picker_current_path.set(current_path);
        app.picker_entries.set(Vec::new());
        app.picker_status.set(String::from("Ready"));
        app.refresh();
        app
    }

    fn refresh(&self) {
        let path = self.current_path.get();
        let generation = self.directory_read_generation.get().wrapping_add(1);
        let instance_id = self.directory_read_instance.get();
        self.directory_read_generation.set(generation);
        self.entries.set(Vec::new());
        self.selected.set(None);
        self.hovered.set(None);
        self.last_click.set(None);
        self.status.set(format!("Loading {path}"));

        thread::spawn(move || {
            let result = read_entries(&path, None).map_err(|error| error.to_string());
            mutex_lock(&PENDING_DIRECTORY_READS).push(DirectoryReadResult {
                instance_id,
                generation,
                path,
                result,
            });
        });
    }

    fn navigate_to(&self, path: impl Into<String>) {
        let path = path.into();
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

    fn refresh_picker(&self) {
        let path = self.picker_current_path.get();
        let Some(request) = self.picker_request.get() else {
            return;
        };
        let generation = self.picker_read_generation.get().wrapping_add(1);
        self.picker_read_generation.set(generation);
        self.picker_entries.set(Vec::new());
        self.picker_selected.set(None);
        self.picker_hovered.set(None);
        self.picker_last_click.set(None);
        self.picker_status.set(format!("Loading {path}"));

        thread::spawn(move || {
            let result = read_entries(&path, Some(&request)).map_err(|error| error.to_string());
            mutex_lock(&PENDING_PICKER_READS).push(PickerReadResult {
                request_id: request.id,
                generation,
                path,
                result,
            });
        });
    }

    fn navigate_picker_to(&self, path: impl Into<String>) {
        let path = path.into();
        self.picker_current_path.set(path);
        self.refresh_picker();
    }

    fn navigate_picker_up(&self) {
        let current = PathBuf::from(self.picker_current_path.get());
        let parent = current
            .parent()
            .unwrap_or_else(|| Path::new(ROOT_PATH))
            .to_string_lossy()
            .into_owned();
        self.navigate_picker_to(parent);
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

    fn activate_picker_entry(&self, index: usize) {
        let Some(entry) = self.picker_entries.get().get(index).cloned() else {
            return;
        };

        let now = Instant::now();
        let double_click = self
            .picker_last_click
            .get()
            .is_some_and(|(last_index, timestamp)| {
                last_index == index && now.duration_since(timestamp) <= DOUBLE_CLICK_INTERVAL
            });
        self.picker_last_click.set(if double_click {
            None
        } else {
            Some((index, now))
        });

        let select_directories = self
            .picker_request
            .get()
            .as_ref()
            .is_some_and(|request| request.select_directories);
        let save_mode = self
            .picker_request
            .get()
            .as_ref()
            .is_some_and(|request| request.save_mode);
        self.picker_selected.set(Some(index));
        self.picker_status.set(entry.path.clone());

        if save_mode && !entry.is_directory {
            self.picker_file_name.set(entry.name.clone());
        }

        if !double_click {
            return;
        }
        if entry.is_directory {
            if !select_directories {
                self.navigate_picker_to(entry.path);
            }
        } else {
            self.finish_picker(true);
        }
    }

    fn open_entry(&self, path: String) {
        let opening_status = format!("Opening {path}");
        self.status.set(opening_status.clone());

        let status = self.status.clone();
        thread::spawn(move || {
            let result = SbusConnection::connect().and_then(|mut connection| {
                connection.call_method_timeout(
                    DESKTOP_STEMD_BUS_NAME,
                    DESKTOP_STEMD_OBJECT_PATH,
                    DESKTOP_STEMD_INTERFACE,
                    DESKTOP_STEMD_OPEN_PATH_METHOD,
                    vec![Argument::String(path.clone())],
                    3_000,
                )
            });

            if let Err(error) = result
                && status.get() == opening_status
            {
                status.set(format!("Cannot open {path}: {error:?}"));
            }
        });
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

    fn configure_picker(&self, request: PickerRequest) {
        let initial_folder = if request.initial_folder.is_empty() {
            home_path()
        } else {
            request.initial_folder.clone()
        };
        let status = if request.allow_multiple {
            String::from("Multiple selection is not supported yet")
        } else {
            request.title.clone()
        };
        self.picker_file_name.set(request.suggested_name.clone());
        self.picker_request.set(Some(request));
        self.picker_current_path.set(initial_folder);
        self.picker_entries.set(Vec::new());
        self.picker_status.set(status);
        self.refresh_picker();
    }

    fn begin_picker(&self, request: PickerRequest) {
        self.configure_picker(request);
        open_window(PICKER_WINDOW_KEY);
    }

    fn process_pending_ui_work(&self) {
        let requests: Vec<_> = mutex_lock(&PENDING_UI_REQUESTS).drain(..).collect();
        for request in requests {
            match request {
                FilerUiRequest::ShowMain => open_window("main"),
            }
        }

        let directory_reads: Vec<_> = mutex_lock(&PENDING_DIRECTORY_READS).drain(..).collect();
        for read in directory_reads {
            let is_current = self.directory_read_instance.get() == read.instance_id
                && self.directory_read_generation.get() == read.generation
                && self.current_path.get() == read.path;
            if !is_current {
                continue;
            }

            match read.result {
                Ok(entries) => {
                    self.entries.set(entries);
                    self.status.set(String::from("Ready"));
                }
                Err(error) => self
                    .status
                    .set(format!("Cannot open {}: {error}", read.path)),
            }
        }

        // The picker scene has one shared state object, so service requests
        // must be presented serially. Each caller already received its request
        // id and will get the matching response signal when its turn finishes.
        // Defer the next queued request for one idle pass after completing a
        // picker. This lets either a queued DismissWindow command or a close
        // request from the title bar finish removing the old window first.
        let picker_window_closing = PICKER_WINDOW_CLOSING.swap(false, AtomicOrdering::AcqRel);
        if !picker_window_closing && self.picker_request.get().is_none() {
            let next_request = {
                let mut pending = mutex_lock(&PENDING_PICKER_REQUESTS);
                if pending.is_empty() {
                    None
                } else {
                    Some(pending.remove(0))
                }
            };
            if let Some(request) = next_request {
                self.begin_picker(request);
            }
        }

        let reads: Vec<_> = mutex_lock(&PENDING_PICKER_READS).drain(..).collect();
        for read in reads {
            let is_current = self
                .picker_request
                .get()
                .as_ref()
                .is_some_and(|request| request.id == read.request_id)
                && self.picker_read_generation.get() == read.generation
                && self.picker_current_path.get() == read.path;
            if !is_current {
                continue;
            }

            match read.result {
                Ok(entries) => {
                    self.picker_entries.set(entries);
                    self.picker_status.set(String::from("Ready"));
                }
                Err(error) => self
                    .picker_status
                    .set(format!("Cannot open {}: {error}", read.path)),
            }
        }
    }

    fn finish_picker(&self, accepted: bool) {
        self.complete_picker(accepted, true);
    }

    fn complete_picker(&self, accepted: bool, dismiss: bool) {
        let Some(request) = self.picker_request.get() else {
            return;
        };
        let path = if request.save_mode {
            let file_name = self.picker_file_name.get();
            let file_name = file_name.trim();
            if file_name.is_empty() || file_name.contains('/') {
                String::new()
            } else {
                PathBuf::from(self.picker_current_path.get())
                    .join(file_name)
                    .to_string_lossy()
                    .into_owned()
            }
        } else {
            self.picker_selected
                .get()
                .and_then(|index| {
                    self.picker_entries
                        .get()
                        .get(index)
                        .map(|entry| entry.path.clone())
                })
                .unwrap_or_default()
        };
        let success = accepted && !path.is_empty();

        let request_id = request.id;
        self.picker_request.set(None);
        self.picker_file_name.set(String::new());
        PICKER_WINDOW_CLOSING.store(true, AtomicOrdering::Release);
        if dismiss {
            dismiss_window(PICKER_WINDOW_KEY);
        }

        if STANDALONE_PICKER.load(AtomicOrdering::Acquire) {
            // In standalone picker mode the process exits as soon as the
            // window closes. Send the response synchronously so the signal
            // is guaranteed to reach sbusd before exit.
            send_picker_response(request_id, success, path);
        } else {
            thread::spawn(move || {
                send_picker_response(request_id, success, path);
            });
        }
    }

    fn file_cell(
        &self,
        index: usize,
        entry: FileEntry,
        selected_index: Option<usize>,
        picker: bool,
    ) -> impl View + Clone + use<> {
        let palette = ColorPalette::default();
        let hovered_index = if picker {
            self.picker_hovered.get()
        } else {
            self.hovered.get()
        };
        let background = if selected_index == Some(index) {
            palette.primary_light().with_opacity(0.16)
        } else if hovered_index == Some(index) {
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
        let hover_state = if picker {
            self.picker_hovered.clone()
        } else {
            self.hovered.clone()
        };
        let exit_state = hover_state.clone();

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
        .on_click(move || {
            if picker {
                app.activate_picker_entry(index);
            } else {
                app.activate_entry(index);
            }
        })
    }

    fn browser_page(&self, picker: bool) -> impl View + Clone + use<> {
        let grid_app = self.clone();
        let entries = if picker {
            self.picker_entries.clone()
        } else {
            self.entries.clone()
        };
        let selected = if picker {
            self.picker_selected.clone()
        } else {
            self.selected.clone()
        };
        let grid = GridView::new(
            entries,
            selected,
            GRID_COLUMNS,
            GRID_ROW_HEIGHT,
            move |index, entry, selected| grid_app.file_cell(index, entry, selected, picker),
        )
        .spacing(10.0)
        .minimum_cell_width(FILE_CELL_WIDTH);

        let title = if picker {
            self.picker_request
                .get()
                .as_ref()
                .map(|request| request.title.clone())
                .unwrap_or_else(|| String::from("Open File"))
        } else {
            String::from("Files")
        };
        let footer = if picker {
            let cancel_app = self.clone();
            let open_app = self.clone();
            let save_mode = self
                .picker_request
                .get()
                .as_ref()
                .is_some_and(|request| request.save_mode);
            let file_name = if save_mode {
                Either::A(
                    TextField::new(self.picker_file_name.clone())
                        .placeholder("File name")
                        .frame_width(260.0),
                )
            } else {
                Either::B(Spacer::new().frame_width(0.0))
            };
            let action_title = if save_mode { "Save" } else { "Open" };
            Either::A(vstack! {
                Divider::new(),
                hstack! {
                    Text::from_state(self.picker_status.clone()).font_size(11.0),
                    Spacer::new(),
                    file_name,
                    Button::new("Cancel").on_click(move || cancel_app.finish_picker(false)),
                    Button::new(action_title).on_click(move || open_app.finish_picker(true)),
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

    fn files_page(&self) -> impl View + Clone + use<> {
        self.browser_page(false)
    }

    fn picker_page(&self) -> impl View + Clone + use<> {
        vstack! {
            self.header_for(true),
            self.browser_page(true),
        }
    }

    fn header_for(&self, picker: bool) -> impl View + Clone + use<> {
        let back_app = self.clone();
        let up_app = self.clone();
        let create_app = self.clone();
        let refresh_app = self.clone();
        let current_path = if picker {
            self.picker_current_path.clone()
        } else {
            self.current_path.clone()
        };
        HeaderBar::new(
            hstack! {
                Button::icon_only(Icon::ChevronLeft)
                    .header_style()
                    .on_click(move || {
                        if picker {
                            back_app.navigate_picker_up();
                        } else {
                            back_app.navigate_up();
                        }
                    }),
                Button::icon_only(Icon::ArrowUp)
                    .header_style()
                    .on_click(move || {
                        if picker {
                            up_app.navigate_picker_up();
                        } else {
                            up_app.navigate_up();
                        }
                    }),
                IconView::new(Icon::Folder)
                    .size(IconSize::Medium)
                    .filled(),
                Text::from_state(current_path).font_size(14.0),
                Spacer::new(),
                Button::icon_only(Icon::FolderPlus)
                    .header_style()
                    .on_click(move || {
                        if !picker {
                            create_app.create_folder();
                        }
                    }),
                Button::icon_only(Icon::Refresh)
                    .header_style()
                    .on_click(move || {
                        if picker {
                            refresh_app.refresh_picker();
                        } else {
                            refresh_app.refresh();
                        }
                    }),
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
    fn on_idle(&mut self) {
        self.process_pending_ui_work();
    }

    fn on_window_close_requested(&mut self, context: &WindowContext) -> bool {
        if context.scene_key.as_str() == PICKER_WINDOW_KEY {
            self.complete_picker(false, false);
        }
        true
    }

    fn scenes(&self) -> impl Scene {
        let home = self.clone();
        let computer = self.clone();
        let pictures = self.clone();
        let downloads = self.clone();
        let header = self.clone();
        let picker = self.clone();
        let home_path = home_path();
        let pictures_path = PathBuf::from(&home_path)
            .join("pictures")
            .to_string_lossy()
            .into_owned();
        let downloads_path = PathBuf::from(&home_path)
            .join("downloads")
            .to_string_lossy()
            .into_owned();

        let main_window = Window::new(
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
                        let pictures_path = pictures_path.clone();
                        move || app.navigate_to(pictures_path.clone())
                    }),
                NavigationLink::new("Downloads", move || downloads.files_page())
                    .icon(Icon::Download)
                    .on_select({
                        let app = self.clone();
                        let downloads_path = downloads_path.clone();
                        move || app.navigate_to(downloads_path.clone())
                    }),
            }
            .header(move || header.header_for(false))
            .shows_icons(true)
            .sidebar_width(170.0),
        )
        .app_id(APP_ID)
        .size(Size::new(960.0, 640.0))
        .scene_key("main")
        .open_at_launch(false);

        let picker_title = self
            .picker_request
            .get()
            .map(|request| request.title)
            .unwrap_or_else(|| String::from("Open File"));
        let picker_open_at_launch = STANDALONE_PICKER.load(AtomicOrdering::Acquire);
        let picker_window = Window::new(picker_title, picker.picker_page())
            .scene_key(PICKER_WINDOW_KEY)
            .app_id(APP_ID)
            .size(Size::new(960.0, 640.0))
            .open_at_launch(picker_open_at_launch);

        (main_window, picker_window)
    }

    fn exit_when_all_windows_closed(&self) -> bool {
        STANDALONE_PICKER.load(AtomicOrdering::Acquire)
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
    use super::{
        FILE_NAME_MAX_WIDTH, FileEntry, FileKind, FilerApp, PickerRequest, file_name_label,
        text_width,
    };
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
    fn picker_invocation_identity_ignores_generated_request_id() {
        let first = PickerRequest {
            id: String::from("request-1"),
            title: String::from("Open Video"),
            initial_folder: String::from("/home/user"),
            filter: String::from("video/*"),
            allow_multiple: false,
            select_directories: false,
            save_mode: false,
            suggested_name: String::new(),
        };
        let mut retry = first.clone();
        retry.id = String::from("request-2");
        assert!(first.same_invocation(&retry));

        retry.filter = String::from("image/*");
        assert!(!first.same_invocation(&retry));
    }

    #[test]
    fn file_manager_windows_start_hidden() {
        let app = FilerApp::new();
        let mut builder = SceneBuilder::new();
        app.scenes().build(&mut builder);
        let declarations = builder.into_declarations();

        assert_eq!(declarations.len(), 2);
        assert!(!declarations[0].opens_at_launch);
        assert!(!declarations[1].opens_at_launch);
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
    home_path()
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
        "video/*" => is_video(name),
        "video/mp4" => has_extension(name, &["mp4", "m4v", "m4a"]),
        "video/webm" => has_extension(name, &["webm"]),
        "text/*" | "text/plain" => is_text(name),
        _ => true,
    }
}

fn is_image(name: &str) -> bool {
    has_extension(name, &["jpg", "jpeg", "png", "gif", "bmp", "webp"])
}

fn is_video(name: &str) -> bool {
    has_extension(
        name,
        &["mp4", "m4v", "m4a", "webm", "h264", "264", "av1", "ivf"],
    )
}

fn is_text(name: &str) -> bool {
    has_extension(
        name,
        &[
            "txt", "md", "markdown", "rs", "toml", "json", "yaml", "yml", "ini", "conf", "csv",
            "xml", "html", "css", "js", "ts", "c", "h", "cpp", "hpp", "sh",
        ],
    )
}

fn has_extension(name: &str, extensions: &[&str]) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        extensions
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

fn next_picker_request_id() -> String {
    let mut sequence = mutex_lock(&NEXT_PICKER_REQUEST_ID);
    let request_id = *sequence;
    *sequence = (*sequence).wrapping_add(1).max(1);
    format!("request-{request_id}")
}

fn next_filer_instance_id() -> u64 {
    let mut sequence = mutex_lock(&NEXT_FILER_INSTANCE_ID);
    let instance_id = *sequence;
    *sequence = (*sequence).wrapping_add(1).max(1);
    instance_id
}

fn reap_picker_children_locked(children: &mut Vec<PickerChild>) {
    let mut index = 0;
    while index < children.len() {
        match children[index].child.try_wait() {
            Ok(Some(_)) | Err(_) => {
                children.swap_remove(index);
            }
            Ok(None) => index += 1,
        }
    }
}

fn active_picker_request_id(request: &PickerRequest) -> Option<String> {
    let mut children = mutex_lock(&PICKER_CHILDREN);
    reap_picker_children_locked(&mut children);
    children
        .iter()
        .find(|active| active.request.same_invocation(request))
        .map(|active| active.request.id.clone())
}

fn run_picker_service() {
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
            if method == DESKTOP_FILE_MANAGER_SHOW_METHOD {
                mutex_lock(&PENDING_UI_REQUESTS).push(FilerUiRequest::ShowMain);
                let _ = connection.send_method_return(0, Vec::new());
                continue;
            }
            let save_mode = if method == DESKTOP_FILE_MANAGER_OPEN_FILE_METHOD {
                false
            } else if method == DESKTOP_FILE_MANAGER_SAVE_FILE_METHOD {
                true
            } else {
                let _ = connection.send_method_error(
                    0,
                    "org.scarlet.desktop.FileManager.UnknownMethod",
                    "Unknown FileManager method",
                );
                continue;
            };

            let mut request = PickerRequest {
                id: String::new(),
                title: argument_string(&args, 0).unwrap_or_else(|| String::from("Open File")),
                initial_folder: argument_string(&args, 1).unwrap_or_else(home_path),
                filter: argument_string(&args, if save_mode { 3 } else { 2 }).unwrap_or_default(),
                allow_multiple: if save_mode {
                    false
                } else {
                    argument_bool(&args, 3)
                },
                select_directories: if save_mode {
                    false
                } else {
                    argument_bool(&args, 4)
                },
                save_mode,
                suggested_name: if save_mode {
                    argument_string(&args, 2).unwrap_or_default()
                } else {
                    String::new()
                },
            };

            if let Some(request_id) = active_picker_request_id(&request) {
                let _ = connection.send_method_return(0, vec![Argument::String(request_id)]);
                continue;
            }

            request.id = next_picker_request_id();
            if connection
                .send_method_return(0, vec![Argument::String(request.id.clone())])
                .is_ok()
            {
                launch_picker_process(&request);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--picker") {
        println!("[filer] starting in standalone picker mode");
        run_standalone_picker(&args);
        return;
    }

    println!("[filer] starting");
    thread::spawn(run_picker_service);
    thread::spawn(picker_reaper);
    let mut app = FilerApp::new();
    if let Err(error) = app.run() {
        eprintln!("[filer] error: {error}");
    }
}

/// Run as a standalone picker process. The request parameters were passed as
/// command-line arguments by the parent Files process. The picker window opens
/// immediately; when the user confirms or cancels, the response signal is sent
/// synchronously and the process exits.
fn run_standalone_picker(args: &[String]) {
    STANDALONE_PICKER.store(true, AtomicOrdering::Release);
    let request = parse_picker_args(args);
    let mut app = FilerApp::new();
    app.configure_picker(request);
    if let Err(error) = app.run() {
        eprintln!("[filer] picker error: {error}");
    }
}

/// Send the picker response signal via sbus. In standalone mode this is called
/// synchronously; in normal mode it runs on a spawned thread.
fn send_picker_response(request_id: String, success: bool, path: String) {
    match SbusConnection::connect().and_then(|mut connection| {
        connection.emit_signal(
            DESKTOP_FILE_MANAGER_BUS_NAME,
            DESKTOP_FILE_MANAGER_OBJECT_PATH,
            DESKTOP_FILE_MANAGER_INTERFACE,
            DESKTOP_FILE_MANAGER_RESPONSE_SIGNAL,
            vec![
                Argument::String(request_id),
                Argument::Boolean(success),
                Argument::String(path),
            ],
        )
    }) {
        Ok(()) => {}
        Err(error) => eprintln!("[filer] failed to send picker response: {error:?}"),
    }
}

/// Fork and exec a standalone picker child process with the request encoded as
/// command-line arguments. This isolates the picker UI from the Files main
/// process so a crash in either one cannot take down the other.
fn launch_picker_process(request: &PickerRequest) {
    // Build argv for the standalone picker child.
    let argv_strings = build_picker_argv(request);
    let mut command = std::process::Command::new(FILES_BINARY_PATH);
    command.args(&argv_strings[1..]);
    match command.spawn() {
        Ok(child) => {
            mutex_lock(&PICKER_CHILDREN).push(PickerChild {
                request: request.clone(),
                child,
            });
        }
        Err(error) => {
            eprintln!("[filer] failed to spawn picker process: {error}");
        }
    }
}

/// Background thread that reaps exited picker child processes without blocking
/// the UI thread. Uses `try_wait` (non-blocking) so it never stalls.
fn picker_reaper() {
    loop {
        reap_picker_children_locked(&mut mutex_lock(&PICKER_CHILDREN));
        thread::sleep(Duration::from_millis(500));
    }
}

/// Parse `--picker` command-line arguments into a PickerRequest.
fn parse_picker_args(args: &[String]) -> PickerRequest {
    let mut request = PickerRequest {
        id: String::new(),
        title: String::from("Open File"),
        initial_folder: String::new(),
        filter: String::new(),
        allow_multiple: false,
        select_directories: false,
        save_mode: false,
        suggested_name: String::new(),
    };

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--picker" => {}
            "--request-id" => {
                if let Some(val) = args.get(i + 1) {
                    request.id = val.clone();
                    i += 1;
                }
            }
            "--title" => {
                if let Some(val) = args.get(i + 1) {
                    request.title = val.clone();
                    i += 1;
                }
            }
            "--folder" => {
                if let Some(val) = args.get(i + 1) {
                    request.initial_folder = val.clone();
                    i += 1;
                }
            }
            "--filter" => {
                if let Some(val) = args.get(i + 1) {
                    request.filter = val.clone();
                    i += 1;
                }
            }
            "--save" => {
                request.save_mode = true;
            }
            "--suggested-name" => {
                if let Some(val) = args.get(i + 1) {
                    request.suggested_name = val.clone();
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    request
}

/// Build the argv vector for launching a standalone picker child process.
fn build_picker_argv(request: &PickerRequest) -> Vec<String> {
    let mut argv = vec![
        String::from("files"),
        String::from("--picker"),
        String::from("--request-id"),
        request.id.clone(),
        String::from("--title"),
        request.title.clone(),
        String::from("--folder"),
        request.initial_folder.clone(),
        String::from("--filter"),
        request.filter.clone(),
    ];
    if request.save_mode {
        argv.push(String::from("--save"));
        if !request.suggested_name.is_empty() {
            argv.push(String::from("--suggested-name"));
            argv.push(request.suggested_name.clone());
        }
    }
    argv
}
