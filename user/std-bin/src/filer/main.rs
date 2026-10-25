//! Scarlet Desktop file manager and file picker.
//!
//! The application follows the same direct-view structure as ScarletUI's
//! Widget Factory. It runs on Rust `std`, presents files in a grid, and can
//! switch into picker mode through sbus without embedding picker UI in the
//! calling application.

mod archive;
mod details;
mod icons;
mod listing;
mod menus;
mod operations;
mod picker_filter;
mod usage;
mod workers;

use core::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::collections::VecDeque;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use sbus::{Argument, Message};
#[cfg(target_os = "scarlet")]
use sbus_client::Connection as SbusConnection;
use scarlet_desktop_config::{
    DESKTOP_FILE_MANAGER_BUS_NAME, DESKTOP_FILE_MANAGER_GET_PICKER_CAPABILITIES_METHOD,
    DESKTOP_FILE_MANAGER_INTERFACE, DESKTOP_FILE_MANAGER_OBJECT_PATH,
    DESKTOP_FILE_MANAGER_OPEN_FILE_METHOD, DESKTOP_FILE_MANAGER_RESPONSE_SIGNAL,
    DESKTOP_FILE_MANAGER_SAVE_FILE_METHOD, DESKTOP_FILE_MANAGER_SHOW_METHOD, DESKTOP_FILES_APP_ID,
    DESKTOP_STEMD_BUS_NAME, DESKTOP_STEMD_INTERFACE, DESKTOP_STEMD_LAUNCH_OR_FOCUS_METHOD,
    DESKTOP_STEMD_OBJECT_PATH, DESKTOP_STEMD_OPEN_PATH_METHOD,
};
use scarlet_ui::prelude::*;
use scarlet_ui::{
    Alignment, BitmapImage, Button, Color, ColorPalette, Divider, GridView, HeaderBar, Icon, Image,
    ImageFit, KeyCode, NavigationLink, Spacer, dismiss_window, hstack, measure_text_sized,
    navigation, open_window, vstack,
};
use scarlet_ui_macros::View;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use icons::{FileKind, icon_for_entry};
use operations::{History, Operation};

const APP_ID: &str = DESKTOP_FILES_APP_ID;
const PICKER_WINDOW_KEY: &str = "picker";
const ROOT_PATH: &str = "/";
const GRID_COLUMNS: usize = 5;
const GRID_ROW_HEIGHT: f32 = 146.0;
const FILE_CELL_WIDTH: f32 = 146.0;
const THUMBNAIL_WIDTH: usize = 96;
const THUMBNAIL_HEIGHT: usize = 78;
const FILE_PREVIEW_WIDTH: f32 = THUMBNAIL_WIDTH as f32;
const FILE_PREVIEW_HEIGHT: f32 = THUMBNAIL_HEIGHT as f32;
const FILE_NAME_FONT_SIZE: f32 = 13.0;
const FILE_NAME_HEIGHT: f32 = 32.0;
const FILE_NAME_MAX_WIDTH: f32 = 132.0;
const JPEG_THUMBNAIL_SOURCE_MAX_PIXELS: usize = 16 * 1024 * 1024;
const JPEG_THUMBNAIL_FILE_MAX_BYTES: u64 = 32 * 1024 * 1024;
const THUMBNAIL_WORKER_IDLE_DELAY: Duration = Duration::from_millis(25);
const SERVICE_RETRY_DELAY: Duration = Duration::from_millis(100);
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);
const FILTER_DELAY: Duration = Duration::from_millis(180);

/// Path to the files binary, used to re-exec in standalone picker mode.
const FILES_BINARY_PATH: &str = "/bin/files";
/// Set when the process was launched with `--picker`. In that mode the picker
/// window opens at launch, the response signal is sent synchronously on
/// completion, and the process exits when the picker window closes.
static STANDALONE_PICKER: AtomicBool = AtomicBool::new(false);
#[cfg(not(target_os = "scarlet"))]
static PREVIEW_ROOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn move_supported() -> bool {
    scarlet_fs::require_noreplace_move().is_ok()
}

#[derive(Clone)]
struct FileEntry {
    name: String,
    path: String,
    is_directory: bool,
    size: u64,
    kind: FileKind,
    thumbnail: Option<BitmapImage>,
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

#[derive(Clone)]
struct FileClipboard {
    source: PathBuf,
    cut: bool,
}

#[derive(Clone)]
enum FilePrompt {
    NewFolder(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
}

struct OperationResult {
    instance_id: u64,
    directory: String,
    cut_source: Option<PathBuf>,
    result: std::result::Result<String, String>,
}

enum DirectoryJob {
    Main {
        instance_id: u64,
        generation: u32,
        path: String,
        filters: (String, bool, usize),
    },
    Picker {
        generation: u32,
        path: String,
        request: PickerRequest,
    },
}

impl DirectoryJob {
    fn same_owner(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Main {
                    instance_id: left, ..
                },
                Self::Main {
                    instance_id: right, ..
                },
            ) => left == right,
            (Self::Picker { request: left, .. }, Self::Picker { request: right, .. }) => {
                left.id == right.id
            }
            _ => false,
        }
    }
}

#[derive(Clone)]
enum ThumbnailContext {
    Main {
        instance_id: u64,
        generation: u32,
        directory: String,
    },
    Picker {
        request_id: String,
        generation: u32,
        directory: String,
    },
}

struct ThumbnailJob {
    context: ThumbnailContext,
    path: String,
    file_size: u64,
}

struct ThumbnailResult {
    context: ThumbnailContext,
    path: String,
    thumbnail: Option<BitmapImage>,
}

struct PickerChild {
    request: PickerRequest,
    child: std::process::Child,
}

static NEXT_FILER_INSTANCE_ID: Mutex<u64> = Mutex::new(1);
static NEXT_PICKER_REQUEST_ID: Mutex<u32> = Mutex::new(1);
static PENDING_DIRECTORY_READS: Mutex<Vec<DirectoryReadResult>> = Mutex::new(Vec::new());
static PENDING_OPERATIONS: Mutex<Vec<OperationResult>> = Mutex::new(Vec::new());
static PENDING_DIRECTORY_JOBS: Mutex<VecDeque<DirectoryJob>> = Mutex::new(VecDeque::new());
static DIRECTORY_WORKER_STARTED: AtomicBool = AtomicBool::new(false);
static PENDING_PICKER_REQUESTS: Mutex<Vec<PickerRequest>> = Mutex::new(Vec::new());
static PENDING_PICKER_READS: Mutex<Vec<PickerReadResult>> = Mutex::new(Vec::new());
static PENDING_THUMBNAIL_JOBS: Mutex<VecDeque<ThumbnailJob>> = Mutex::new(VecDeque::new());
static PENDING_THUMBNAIL_RESULTS: Mutex<Vec<ThumbnailResult>> = Mutex::new(Vec::new());
static THUMBNAIL_WORKER_STARTED: AtomicBool = AtomicBool::new(false);
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

fn enqueue_directory_job(job: DirectoryJob) -> std::io::Result<()> {
    {
        let mut pending = mutex_lock(&PENDING_DIRECTORY_JOBS);
        pending.retain(|existing| !existing.same_owner(&job));
        pending.push_back(job);
    }
    if DIRECTORY_WORKER_STARTED
        .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
        .is_err()
    {
        return Ok(());
    }
    // One worker and one pending request per browser, even while typing or
    // navigating quickly. Generation checks reject the obsolete in-flight read.
    workers::spawn("files-directory", || {
        loop {
            let job = mutex_lock(&PENDING_DIRECTORY_JOBS).pop_front();
            match job {
                Some(DirectoryJob::Main {
                    instance_id,
                    generation,
                    path,
                    filters,
                }) => {
                    let result =
                        read_entries_with_options(&path, None, &filters.0, filters.1, filters.2)
                            .map_err(|error| error.to_string());
                    let mut pending = mutex_lock(&PENDING_DIRECTORY_READS);
                    pending.retain(|read| read.instance_id != instance_id);
                    pending.push(DirectoryReadResult {
                        instance_id,
                        generation,
                        path,
                        result,
                    });
                }
                Some(DirectoryJob::Picker {
                    generation,
                    path,
                    request,
                }) => {
                    let result =
                        read_entries(&path, Some(&request)).map_err(|error| error.to_string());
                    let mut pending = mutex_lock(&PENDING_PICKER_READS);
                    pending.retain(|read| read.request_id != request.id);
                    pending.push(PickerReadResult {
                        request_id: request.id,
                        generation,
                        path,
                        result,
                    });
                }
                None => thread::sleep(THUMBNAIL_WORKER_IDLE_DELAY),
            }
        }
    })
    .map(|_| ())
    .inspect_err(|_| DIRECTORY_WORKER_STARTED.store(false, AtomicOrdering::Release))
}

fn thumbnail_context_has_same_owner(left: &ThumbnailContext, right: &ThumbnailContext) -> bool {
    match (left, right) {
        (
            ThumbnailContext::Main {
                instance_id: left, ..
            },
            ThumbnailContext::Main {
                instance_id: right, ..
            },
        ) => left == right,
        (
            ThumbnailContext::Picker {
                request_id: left, ..
            },
            ThumbnailContext::Picker {
                request_id: right, ..
            },
        ) => left == right,
        _ => false,
    }
}

fn cancel_main_thumbnail_jobs(instance_id: u64) {
    mutex_lock(&PENDING_THUMBNAIL_JOBS).retain(|job| {
        !matches!(
            &job.context,
            ThumbnailContext::Main {
                instance_id: queued_instance,
                ..
            } if *queued_instance == instance_id
        )
    });
}

fn cancel_picker_thumbnail_jobs(request_id: &str) {
    mutex_lock(&PENDING_THUMBNAIL_JOBS).retain(|job| {
        !matches!(
            &job.context,
            ThumbnailContext::Picker {
                request_id: queued_request,
                ..
            } if queued_request == request_id
        )
    });
}

fn enqueue_thumbnail_jobs(context: ThumbnailContext, entries: &[FileEntry]) {
    let jobs: Vec<_> = entries
        .iter()
        .filter(|entry| {
            !entry.is_directory && entry.kind == FileKind::Image && is_jpeg(&entry.name)
        })
        .map(|entry| ThumbnailJob {
            context: context.clone(),
            path: entry.path.clone(),
            file_size: entry.size,
        })
        .collect();
    if jobs.is_empty() {
        return;
    }

    if !ensure_thumbnail_worker() {
        return;
    }
    let mut pending = mutex_lock(&PENDING_THUMBNAIL_JOBS);
    pending.retain(|job| !thumbnail_context_has_same_owner(&job.context, &context));
    pending.extend(jobs);
}

fn ensure_thumbnail_worker() -> bool {
    if THUMBNAIL_WORKER_STARTED
        .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
        .is_err()
    {
        return true;
    }

    match workers::spawn("files-thumbnails", || {
        loop {
            let job = mutex_lock(&PENDING_THUMBNAIL_JOBS).pop_front();
            let Some(job) = job else {
                thread::sleep(THUMBNAIL_WORKER_IDLE_DELAY);
                continue;
            };

            let thumbnail = generate_jpeg_thumbnail(&job.path, job.file_size);
            mutex_lock(&PENDING_THUMBNAIL_RESULTS).push(ThumbnailResult {
                context: job.context,
                path: job.path,
                thumbnail,
            });
        }
    }) {
        Ok(_) => true,
        Err(error) => {
            THUMBNAIL_WORKER_STARTED.store(false, AtomicOrdering::Release);
            eprintln!("Files: thumbnails unavailable: {error}");
            false
        }
    }
}

fn read_thumbnail_source(path: &str, file_size: u64) -> Option<Vec<u8>> {
    if file_size > JPEG_THUMBNAIL_FILE_MAX_BYTES {
        return None;
    }

    let capacity = usize::try_from(file_size)
        .ok()?
        .min(JPEG_THUMBNAIL_FILE_MAX_BYTES as usize);
    let mut file = fs::File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut chunk = [0u8; 4096];

    loop {
        let remaining = (JPEG_THUMBNAIL_FILE_MAX_BYTES as usize).checked_sub(bytes.len())?;
        if remaining == 0 {
            return (file.read(&mut chunk[..1]).ok()? == 0).then_some(bytes);
        }
        let read_limit = remaining.min(chunk.len());
        let read = file.read(&mut chunk[..read_limit]).ok()?;
        if read == 0 {
            return Some(bytes);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

fn generate_jpeg_thumbnail(path: &str, file_size: u64) -> Option<BitmapImage> {
    let bytes = read_thumbnail_source(path, file_size)?;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(&bytes), options);
    decoder.decode_headers().ok()?;
    let (source_width, source_height) = decoder.dimensions()?;
    if !thumbnail_source_dimensions_are_safe(source_width, source_height) {
        return None;
    }

    let rgb = decoder.decode().ok()?;
    let pixels = resize_rgb_to_cover_thumbnail(
        &rgb,
        source_width,
        source_height,
        THUMBNAIL_WIDTH,
        THUMBNAIL_HEIGHT,
    )?;
    Some(BitmapImage::from_bgra(
        pixels,
        THUMBNAIL_WIDTH as u32,
        THUMBNAIL_HEIGHT as u32,
    ))
}

fn thumbnail_source_dimensions_are_safe(width: usize, height: usize) -> bool {
    width > 0
        && height > 0
        && width
            .checked_mul(height)
            .is_some_and(|pixels| pixels <= JPEG_THUMBNAIL_SOURCE_MAX_PIXELS)
}

fn resize_rgb_to_cover_thumbnail(
    rgb: &[u8],
    source_width: usize,
    source_height: usize,
    target_width: usize,
    target_height: usize,
) -> Option<Vec<u32>> {
    if source_width == 0 || source_height == 0 || target_width == 0 || target_height == 0 {
        return None;
    }
    let source_pixels = source_width.checked_mul(source_height)?;
    if rgb.len() != source_pixels.checked_mul(3)? {
        return None;
    }

    let source_width_u64 = source_width as u64;
    let source_height_u64 = source_height as u64;
    let target_width_u64 = target_width as u64;
    let target_height_u64 = target_height as u64;
    let source_is_wider = source_width_u64.checked_mul(target_height_u64)?
        > source_height_u64.checked_mul(target_width_u64)?;
    let (crop_width, crop_height) = if source_is_wider {
        (
            source_height_u64
                .checked_mul(target_width_u64)?
                .checked_div(target_height_u64)?
                .clamp(1, source_width_u64),
            source_height_u64,
        )
    } else {
        (
            source_width_u64,
            source_width_u64
                .checked_mul(target_height_u64)?
                .checked_div(target_width_u64)?
                .clamp(1, source_height_u64),
        )
    };
    let crop_x = (source_width_u64 - crop_width) / 2;
    let crop_y = (source_height_u64 - crop_height) / 2;

    let mut pixels = Vec::with_capacity(target_width.checked_mul(target_height)?);
    for target_y in 0..target_height_u64 {
        let source_y_start = crop_y + target_y * crop_height / target_height_u64;
        let source_y_end = (crop_y + (target_y + 1) * crop_height / target_height_u64)
            .max(source_y_start + 1)
            .min(crop_y + crop_height);

        for target_x in 0..target_width_u64 {
            let source_x_start = crop_x + target_x * crop_width / target_width_u64;
            let source_x_end = (crop_x + (target_x + 1) * crop_width / target_width_u64)
                .max(source_x_start + 1)
                .min(crop_x + crop_width);
            let mut red = 0u64;
            let mut green = 0u64;
            let mut blue = 0u64;
            let mut samples = 0u64;

            for source_y in source_y_start..source_y_end {
                for source_x in source_x_start..source_x_end {
                    let pixel_index =
                        usize::try_from((source_y * source_width_u64 + source_x).checked_mul(3)?)
                            .ok()?;
                    red += u64::from(*rgb.get(pixel_index)?);
                    green += u64::from(*rgb.get(pixel_index + 1)?);
                    blue += u64::from(*rgb.get(pixel_index + 2)?);
                    samples += 1;
                }
            }

            pixels.push(
                0xff00_0000
                    | ((red / samples) as u32) << 16
                    | ((green / samples) as u32) << 8
                    | (blue / samples) as u32,
            );
        }
    }

    Some(pixels)
}

#[derive(View, Clone)]
struct FilerApp {
    current_path: State<String>,
    entries: State<Vec<FileEntry>>,
    selected: State<Option<usize>>,
    last_click: State<Option<(usize, Instant)>>,
    picker_current_path: State<String>,
    picker_entries: State<Vec<FileEntry>>,
    picker_selected: State<Option<usize>>,
    picker_last_click: State<Option<(usize, Instant)>>,
    picker_request: State<Option<PickerRequest>>,
    picker_file_name: State<String>,
    picker_status: State<String>,
    picker_read_generation: State<u32>,
    directory_read_instance: State<u64>,
    directory_read_generation: State<u32>,
    status: State<String>,
    location: State<String>,
    location_focus: State<u64>,
    picker_location: State<String>,
    picker_location_focus: State<u64>,
    history: State<History>,
    picker_history: State<History>,
    search: State<String>,
    search_focus: State<u64>,
    show_hidden: State<bool>,
    sort_index: State<usize>,
    applied_filters: State<(String, bool, usize)>,
    filter_changed_at: State<Option<Instant>>,
    clipboard: State<Option<FileClipboard>>,
    prompt: State<Option<FilePrompt>>,
    prompt_name: State<String>,
    prompt_error: State<String>,
    operation_busy: State<bool>,
    refresh_message: State<Option<String>>,
    grid_columns: State<usize>,
    grid_focused: State<bool>,
    picker_grid_focused: State<bool>,
    picker_overwrite: State<Option<String>>,
    details_archive: State<bool>,
    details_path: State<String>,
    details_folder: State<String>,
    details_destination: State<String>,
    details_name: State<String>,
    details_status: State<String>,
    details_usage: State<Option<usage::Usage>>,
    details_usage_rows: State<Vec<(String, u64)>>,
    details_width: State<f32>,
    browser_width: State<f32>,
    picker_width: State<f32>,
    details_index: State<Option<Arc<archive::Index>>>,
    details_entries: State<Vec<archive::Entry>>,
    details_selected: State<Option<usize>>,
    details_last_click: State<Option<(usize, Instant)>>,
    details_columns: State<usize>,
    details_focused: State<bool>,
    details_busy: State<bool>,
    details_cancel: State<Option<Arc<AtomicBool>>>,
    details_close_requested: State<bool>,
    details_generation: State<u64>,
}

impl FilerApp {
    fn new() -> Self {
        let current_path = initial_path();
        let app = Self::default();
        app.directory_read_instance.set(next_filer_instance_id());
        app.current_path.set(current_path.clone());
        app.location.set(current_path.clone());
        app.grid_columns.set(GRID_COLUMNS);
        app.entries.set(Vec::new());
        app.picker_location.set(current_path.clone());
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
        cancel_main_thumbnail_jobs(instance_id);
        self.directory_read_generation.set(generation);
        self.entries.set(Vec::new());
        self.selected.set(None);
        self.last_click.set(None);
        self.status.set(format!("Loading {path}"));
        let filters = (
            self.search.get(),
            self.show_hidden.get(),
            self.sort_index.get(),
        );
        self.applied_filters.set(filters.clone());
        self.filter_changed_at.set(None);

        if let Err(error) = enqueue_directory_job(DirectoryJob::Main {
            instance_id,
            generation,
            path,
            filters,
        }) {
            self.status
                .set(format!("Cannot start folder listing: {error}"));
        }
    }

    fn navigate_to(&self, path: impl Into<String>) {
        let path = path.into();
        let mut history = self.history.get();
        history.visit(&self.current_path.get(), &path);
        self.history.set(history);
        self.current_path.set(path.clone());
        self.location.set(path);
        self.refresh_message.set(None);
        self.refresh();
    }

    fn navigate_history(&self, forward: bool, picker: bool) {
        let (history_state, path_state) = if picker {
            (&self.picker_history, &self.picker_current_path)
        } else {
            (&self.history, &self.current_path)
        };
        let mut history = history_state.get();
        let next = if forward {
            history.forward(&path_state.get())
        } else {
            history.back(&path_state.get())
        };
        if let Some(path) = next {
            history_state.set(history);
            path_state.set(path.clone());
            if picker {
                self.picker_location.set(path);
                self.picker_overwrite.set(None);
                self.refresh_picker();
            } else {
                self.location.set(path);
                self.refresh_message.set(None);
                self.refresh();
            }
        }
    }

    fn submit_location(&self, picker: bool) {
        let (current, location, focus, status) = if picker {
            (
                &self.picker_current_path,
                &self.picker_location,
                &self.picker_location_focus,
                &self.picker_status,
            )
        } else {
            (
                &self.current_path,
                &self.location,
                &self.location_focus,
                &self.status,
            )
        };
        focus.set(0);
        match operations::location_path(
            Path::new(&current.get()),
            &location.get(),
            Path::new(&home_path()),
        ) {
            Ok(path) => {
                let resolved = std::fs::canonicalize(&path).and_then(|path| {
                    if scarlet_fs::metadata(&path)?.is_dir() {
                        Ok(path)
                    } else {
                        Err(std::io::Error::other("Enter a folder path"))
                    }
                });
                match resolved {
                    Ok(path) => {
                        let path = path.to_string_lossy().into_owned();
                        if picker {
                            self.navigate_picker_to(path);
                        } else {
                            self.navigate_to(path);
                        }
                    }
                    Err(error) => status.set(format!("Cannot open {}: {error}", path.display())),
                }
            }
            Err(error) => status.set(error),
        }
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
        cancel_picker_thumbnail_jobs(&request.id);
        let generation = self.picker_read_generation.get().wrapping_add(1);
        self.picker_read_generation.set(generation);
        self.picker_entries.set(Vec::new());
        self.picker_selected.set(None);
        self.picker_last_click.set(None);
        self.picker_status.set(format!("Loading {path}"));

        if let Err(error) = enqueue_directory_job(DirectoryJob::Picker {
            generation,
            path,
            request,
        }) {
            self.picker_status
                .set(format!("Cannot start folder listing: {error}"));
        }
    }

    fn navigate_picker_to(&self, path: impl Into<String>) {
        let path = path.into();
        let mut history = self.picker_history.get();
        history.visit(&self.picker_current_path.get(), &path);
        self.picker_history.set(history);
        self.picker_location.set(path.clone());
        self.picker_current_path.set(path);
        self.picker_overwrite.set(None);
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

        self.selected.set(Some(index));
        self.status.set(entry.path.clone());

        if !double_click {
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
            self.navigate_picker_to(entry.path);
        } else {
            self.finish_picker(true);
        }
    }

    fn open_entry(&self, path: String) {
        if archive::supported(Path::new(&path)) {
            self.show_details_path(path, true);
            return;
        }
        let opening_status = format!("Opening {path}");
        self.status.set(opening_status.clone());

        let status = self.status.clone();
        if let Err(error) = workers::spawn("files-open", move || {
            let result = open_desktop_path(&path);

            if let Err(error) = result
                && status.get() == opening_status
            {
                status.set(format!("Cannot open {path}: {error:?}"));
            }
        }) {
            self.status
                .set(format!("Cannot start opening the file: {error}"));
        }
    }

    fn create_folder(&self) {
        if self.operation_busy.get() {
            return;
        }
        self.prompt_name.set(String::from("New Folder"));
        self.prompt_error.set(String::new());
        self.prompt.set(Some(FilePrompt::NewFolder(PathBuf::from(
            self.current_path.get(),
        ))));
    }

    fn selected_entry(&self) -> Option<FileEntry> {
        self.selected
            .get()
            .and_then(|index| self.entries.get().get(index).cloned())
    }

    fn rename_selected(&self) {
        if let Err(error) = scarlet_fs::require_noreplace_move() {
            self.status.set(error.to_string());
            return;
        }
        if self.operation_busy.get() {
            return;
        }
        if let Some(entry) = self.selected_entry() {
            self.prompt_name.set(entry.name);
            self.prompt_error.set(String::new());
            self.prompt
                .set(Some(FilePrompt::Rename(PathBuf::from(entry.path))));
        } else {
            self.status.set(String::from("Select an item to rename"));
        }
    }

    fn delete_selected(&self) {
        if self.operation_busy.get() {
            return;
        }
        if let Some(entry) = self.selected_entry() {
            self.prompt_error.set(String::new());
            self.prompt
                .set(Some(FilePrompt::Delete(PathBuf::from(entry.path))));
        } else {
            self.status.set(String::from("Select an item to delete"));
        }
    }

    fn copy_selected(&self, cut: bool) {
        if cut {
            if let Err(error) = scarlet_fs::require_noreplace_move() {
                self.status.set(error.to_string());
                return;
            }
        }
        if let Some(entry) = self.selected_entry() {
            self.clipboard.set(Some(FileClipboard {
                source: PathBuf::from(&entry.path),
                cut,
            }));
            self.status.set(format!(
                "{} {} — choose a destination and Paste",
                if cut { "Cut" } else { "Copied" },
                entry.name
            ));
        } else {
            self.status.set(String::from("Select an item first"));
        }
    }

    fn paste(&self) {
        let Some(clipboard) = self.clipboard.get() else {
            self.status
                .set(String::from("Copy or cut an item in Files first"));
            return;
        };
        self.run_operation(Operation::Transfer {
            source: clipboard.source,
            directory: PathBuf::from(self.current_path.get()),
            cut: clipboard.cut,
        });
    }

    fn confirm_prompt(&self) {
        let Some(prompt) = self.prompt.get() else {
            return;
        };
        let name = self.prompt_name.get();
        if !matches!(prompt, FilePrompt::Delete(_)) {
            if let Err(error) = operations::validate_name(&name) {
                self.prompt_error.set(error);
                return;
            }
        }
        let operation = match prompt {
            FilePrompt::NewFolder(directory) => Operation::NewFolder { directory, name },
            FilePrompt::Rename(source) => Operation::Rename { source, name },
            FilePrompt::Delete(source) => Operation::Delete { source },
        };
        self.prompt.set(None);
        self.run_operation(operation);
    }

    fn run_operation(&self, operation: Operation) {
        if self.operation_busy.get() || self.details_busy.get() {
            self.status.set(String::from(
                "Wait for the current file operation to finish",
            ));
            return;
        }
        self.operation_busy.set(true);
        self.status.set(String::from("Working…"));
        let instance_id = self.directory_read_instance.get();
        let directory = self.current_path.get();
        if let Err(error) = workers::spawn("files-operation", move || {
            let cut_source = match &operation {
                Operation::Transfer {
                    source, cut: true, ..
                } => Some(source.clone()),
                _ => None,
            };
            let result = operations::execute(&operation);
            mutex_lock(&PENDING_OPERATIONS).push(OperationResult {
                instance_id,
                directory,
                cut_source,
                result,
            });
        }) {
            self.operation_busy.set(false);
            self.status
                .set(format!("Cannot start file operation: {error}"));
        }
    }

    fn open_selected(&self, picker: bool) {
        let entry = if picker {
            self.picker_selected
                .get()
                .and_then(|index| self.picker_entries.get().get(index).cloned())
        } else {
            self.selected_entry()
        };
        if let Some(entry) = entry {
            if entry.is_directory {
                if picker {
                    self.navigate_picker_to(entry.path);
                } else {
                    self.navigate_to(entry.path);
                }
            } else if picker {
                self.picker_file_name.set(entry.name);
                self.finish_picker(true);
            } else {
                self.open_entry(entry.path);
            }
        }
    }

    fn handle_key(&self, event: KeyEvent, picker: bool) -> bool {
        let KeyEvent::Pressed { keycode, modifiers } = event else {
            return false;
        };
        if keycode == KeyCode::Escape {
            if picker {
                self.picker_overwrite.set(None);
            } else if self.prompt.get().is_some() {
                self.prompt.set(None);
            } else {
                self.selected.set(None);
            }
            return true;
        }
        if modifiers.alt {
            match keycode {
                KeyCode::Left => self.navigate_history(false, picker),
                KeyCode::Right => self.navigate_history(true, picker),
                KeyCode::Up => {
                    if picker {
                        self.navigate_picker_up();
                    } else {
                        self.navigate_up();
                    }
                }
                _ => return false,
            }
            return true;
        }
        if modifiers.primary() && picker && matches!(keycode, KeyCode::Char('l' | 'L')) {
            self.picker_grid_focused.set(false);
            self.picker_location_focus
                .set(self.picker_location_focus.get().wrapping_add(1).max(1));
            return true;
        }
        if modifiers.primary() && !picker {
            match keycode {
                KeyCode::Char('i' | 'I') => self.show_details(),
                KeyCode::Char('l' | 'L') => self.focus_location(),
                KeyCode::Char('f' | 'F') => self.focus_search(),
                KeyCode::Char('c' | 'C')
                    if self.grid_focused.get() && self.prompt.get().is_none() =>
                {
                    self.copy_selected(false)
                }
                KeyCode::Char('x' | 'X')
                    if self.grid_focused.get() && self.prompt.get().is_none() =>
                {
                    self.copy_selected(true)
                }
                KeyCode::Char('v' | 'V')
                    if self.grid_focused.get() && self.prompt.get().is_none() =>
                {
                    self.paste()
                }
                KeyCode::Char('h' | 'H') => self.show_hidden.set(!self.show_hidden.get()),
                KeyCode::Char('n' | 'N') if modifiers.shift => self.create_folder(),
                KeyCode::Char('r' | 'R') => self.refresh(),
                _ => return false,
            }
            return true;
        }
        if self.prompt.get().is_some() {
            return false;
        }
        match keycode {
            KeyCode::Enter => self.open_selected(picker),
            KeyCode::F(2) if !picker => self.rename_selected(),
            KeyCode::F(5) => {
                if picker {
                    self.refresh_picker();
                } else {
                    self.refresh();
                }
            }
            KeyCode::Delete if !picker => self.delete_selected(),
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End => {
                let (entries, selected) = if picker {
                    (&self.picker_entries, &self.picker_selected)
                } else {
                    (&self.entries, &self.selected)
                };
                let count = entries.get().len();
                if count == 0 {
                    return true;
                }
                let index = selected.get().unwrap_or(0);
                let columns = self.grid_columns.get().max(1);
                let next = match keycode {
                    KeyCode::Left => index.saturating_sub(1),
                    KeyCode::Right => (index + 1).min(count - 1),
                    KeyCode::Up => index.saturating_sub(columns),
                    KeyCode::Down => (index + columns).min(count - 1),
                    KeyCode::Home => 0,
                    _ => count - 1,
                };
                selected.set(Some(next));
            }
            _ => return false,
        }
        true
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
        self.picker_history.set(History::default());
        self.picker_overwrite.set(None);
        self.picker_request.set(Some(request));
        self.picker_location.set(initial_folder.clone());
        self.picker_location_focus.set(0);
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
        self.process_details_results();
        let filters = (
            self.search.get(),
            self.show_hidden.get(),
            self.sort_index.get(),
        );
        if filters != self.applied_filters.get() {
            match self.filter_changed_at.get() {
                None => self.filter_changed_at.set(Some(Instant::now())),
                Some(at) if at.elapsed() >= FILTER_DELAY => self.refresh(),
                _ => {}
            }
        }
        let operations: Vec<_> = mutex_lock(&PENDING_OPERATIONS).drain(..).collect();
        for completed in operations {
            if completed.instance_id != self.directory_read_instance.get() {
                continue;
            }
            self.operation_busy.set(false);
            let message =
                match completed.result {
                    Ok(message) => {
                        if let Some(source) = completed.cut_source {
                            if self.clipboard.get().is_some_and(|clipboard| {
                                clipboard.cut && clipboard.source == source
                            }) {
                                self.clipboard.set(None);
                            }
                        }
                        message
                    }
                    Err(error) => error,
                };
            self.status.set(message.clone());
            if completed.directory == self.current_path.get() {
                self.refresh_message.set(Some(message));
                self.refresh();
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
                    enqueue_thumbnail_jobs(
                        ThumbnailContext::Main {
                            instance_id: read.instance_id,
                            generation: read.generation,
                            directory: read.path.clone(),
                        },
                        &entries,
                    );
                    let count = entries.len();
                    self.entries.set(entries);
                    let message = self.refresh_message.get().unwrap_or_else(|| {
                        format!("{count} item{}", if count == 1 { "" } else { "s" })
                    });
                    self.refresh_message.set(None);
                    self.status.set(message);
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
                    enqueue_thumbnail_jobs(
                        ThumbnailContext::Picker {
                            request_id: read.request_id.clone(),
                            generation: read.generation,
                            directory: read.path.clone(),
                        },
                        &entries,
                    );
                    self.picker_entries.set(entries);
                    self.picker_status.set(
                        if self
                            .picker_request
                            .get()
                            .is_some_and(|request| request.allow_multiple)
                        {
                            String::from("Only one item can be selected")
                        } else {
                            String::from("Ready")
                        },
                    );
                }
                Err(error) => self
                    .picker_status
                    .set(format!("Cannot open {}: {error}", read.path)),
            }
        }

        let thumbnails: Vec<_> = mutex_lock(&PENDING_THUMBNAIL_RESULTS).drain(..).collect();
        for result in thumbnails {
            let Some(thumbnail) = result.thumbnail else {
                continue;
            };

            match result.context {
                ThumbnailContext::Main {
                    instance_id,
                    generation,
                    directory,
                } => {
                    let is_current = self.directory_read_instance.get() == instance_id
                        && self.directory_read_generation.get() == generation
                        && self.current_path.get() == directory;
                    if !is_current {
                        continue;
                    }

                    let mut entries = self.entries.get();
                    let Some(entry) = entries.iter_mut().find(|entry| entry.path == result.path)
                    else {
                        continue;
                    };
                    entry.thumbnail = Some(thumbnail);
                    self.entries.set(entries);
                }
                ThumbnailContext::Picker {
                    request_id,
                    generation,
                    directory,
                } => {
                    let is_current = self
                        .picker_request
                        .get()
                        .as_ref()
                        .is_some_and(|request| request.id == request_id)
                        && self.picker_read_generation.get() == generation
                        && self.picker_current_path.get() == directory;
                    if !is_current {
                        continue;
                    }

                    let mut entries = self.picker_entries.get();
                    let Some(entry) = entries.iter_mut().find(|entry| entry.path == result.path)
                    else {
                        continue;
                    };
                    entry.thumbnail = Some(thumbnail);
                    self.picker_entries.set(entries);
                }
            }
        }
    }

    fn finish_picker(&self, accepted: bool) {
        self.complete_picker(accepted, true);
    }

    fn complete_picker(&self, accepted: bool, dismiss: bool) {
        self.complete_picker_selection(accepted, dismiss, false);
    }

    fn complete_picker_selection(&self, accepted: bool, dismiss: bool, replace_confirmed: bool) {
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
                .unwrap_or_else(|| {
                    if request.select_directories {
                        self.picker_current_path.get()
                    } else {
                        String::new()
                    }
                })
        };
        if accepted {
            let name = Path::new(&path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(if request.select_directories && path == ROOT_PATH {
                    "root"
                } else {
                    ""
                });
            if let Err(message) = picker_filter::validate_selection(
                name,
                Path::new(&path).is_dir(),
                request.select_directories,
                request.save_mode,
                &request.filter,
            ) {
                self.picker_status.set(String::from(message));
                return;
            }
            if request.save_mode && scarlet_fs::symlink_metadata(&path).is_ok() {
                if !replace_confirmed
                    || self.picker_overwrite.get().as_deref() != Some(path.as_str())
                {
                    self.picker_overwrite.set(Some(path.clone()));
                    self.picker_status
                        .set(format!("{} already exists. Replace this file?", path));
                    return;
                }
            }
        }
        let success = accepted && !path.is_empty();

        let request_id = request.id;
        cancel_picker_thumbnail_jobs(&request_id);
        self.picker_request.set(None);
        self.picker_file_name.set(String::new());
        self.picker_overwrite.set(None);
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
        let background = if selected_index == Some(index) {
            palette.primary_light().with_opacity(0.16)
        } else {
            Color::CLEAR
        };
        let border = if selected_index == Some(index) {
            palette.primary()
        } else {
            Color::CLEAR
        };
        let preview = if let Some(thumbnail) = entry.thumbnail.clone() {
            Either::A(
                Image::from_bitmap(thumbnail)
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
        let file_name = Text::new(file_name_label(&entry.name))
            .font_size(FILE_NAME_FONT_SIZE)
            .alignment(Alignment::Center)
            .frame(f32::INFINITY, FILE_NAME_HEIGHT);
        let app = self.clone();

        vstack! {
            preview,
            file_name,
            Text::new(detail)
                .font_size(11.0)
                .color(palette.text_secondary()),
        }
        .alignment(Alignment::Center)
        .frame(f32::INFINITY, GRID_ROW_HEIGHT)
        .padding(8.0)
        .background(background)
        .hover_color(if selected_index == Some(index) {
            background
        } else {
            palette.background_tertiary()
        })
        .clip_radius(8.0)
        .border_rounded(border, 2.0, 8.0)
        .on_click(move || {
            if picker {
                app.activate_picker_entry(index);
            } else {
                app.activate_entry(index);
            }
        })
    }

    fn browser_page(&self, picker: bool) -> impl View + Clone + use<> {
        let width_state = if picker {
            self.picker_width.clone()
        } else {
            self.browser_width.clone()
        };
        let text_width = (width_state.get() - 36.0).max(300.0);
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
        let columns = self.grid_columns.clone();
        let key_app = self.clone();
        let no_matches = !picker
            && entries.is_empty()
            && !self.search.get().is_empty()
            && self.status.get().starts_with("0 item");
        let grid = GridView::new(
            entries,
            selected,
            GRID_COLUMNS,
            GRID_ROW_HEIGHT,
            move |index, entry, selected| grid_app.file_cell(index, entry, selected, picker),
        )
        .spacing(10.0)
        .minimum_cell_width(FILE_CELL_WIDTH)
        .reveal_selection(true)
        .item_key(|entry| entry.path.clone().into())
        .on_geometry_change(
            |geometry| {
                (((geometry.size().width + 10.0) / (FILE_CELL_WIDTH + 10.0))
                    .floor()
                    .max(1.0) as usize)
                    .min(GRID_COLUMNS)
            },
            move |count| columns.set(count),
        )
        .focusable(if picker {
            self.picker_grid_focused.clone()
        } else {
            self.grid_focused.clone()
        })
        .on_key(move |event| key_app.handle_key(event, picker));
        let clear_filter = self.clone();
        let grid = if no_matches {
            Either::A(vstack! {
                Spacer::new(),
                Text::new("No matching items").font_size(18.0),
                Text::new("Try another filter or show all items in this folder.")
                    .font_size(12.0).color(ColorPalette::default().text_secondary()),
                Button::new("Clear Filter").on_click(move || clear_filter.search.set(String::new())),
                Spacer::new(),
            }.spacing(12.0).frame(f32::INFINITY, f32::INFINITY))
        } else {
            Either::B(grid)
        };

        let filter_hint = if picker {
            self.picker_request
                .get()
                .as_ref()
                .and_then(|request| {
                    request
                        .filter
                        .strip_prefix(picker_filter::EXTENSION_FILTER_PREFIX)
                })
                .map(|extensions| format!("File types: .{}", extensions.replace(',', ", .")))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let filter_hint = if filter_hint.is_empty() {
            Either::A(vstack! {})
        } else {
            Either::B(
                Text::new(filter_hint)
                    .font_size(12.0)
                    .color(ColorPalette::default().text_secondary()),
            )
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
                let submit_app = self.clone();
                Either::A(
                    TextField::new(self.picker_file_name.clone())
                        .placeholder("File name")
                        .on_submit(move || submit_app.finish_picker(true))
                        .frame_width(260.0),
                )
            } else {
                Either::B(Spacer::new().frame_width(0.0))
            };
            let action_title = if save_mode {
                "Save"
            } else if self
                .picker_request
                .get()
                .is_some_and(|request| request.select_directories)
            {
                "Select Folder"
            } else {
                "Open"
            };
            let replacement = if let Some(path) = self.picker_overwrite.get() {
                let replace_app = self.clone();
                let keep_app = self.clone();
                Either::A(hstack! {
                    Text::new(format!("Replace {}?", Path::new(&path).file_name().unwrap_or_default().to_string_lossy())).font_size(12.0),
                    Spacer::new(),
                    Button::new("Keep Existing").on_click(move || {
                        keep_app.picker_overwrite.set(None);
                        keep_app.picker_status.set(String::from("Choose a different name or folder"));
                    }),
                    Button::new("Replace").on_click(move || replace_app.complete_picker_selection(true, true, true)),
                }.spacing(8.0).padding(8.0))
            } else {
                Either::B(vstack! {})
            };
            Either::A(vstack! {
                Divider::new(),
                replacement,
                Text::new(wrap_display_text(&self.picker_status.get(), text_width, 12.0)).font_size(12.0),
                hstack! {
                    Spacer::new(),
                    file_name,
                    Button::new("Cancel").on_click(move || cancel_app.finish_picker(false)),
                    Button::new(action_title).on_click(move || open_app.finish_picker(true)),
                }
                .alignment(Alignment::Center)
                .padding(8.0),
            })
        } else {
            Either::B(vstack! {
                Divider::new(),
                Text::new(wrap_display_text(&self.status.get(), text_width, 12.0)).font_size(12.0),
                Text::new(if !move_supported() {
                    "Enter: Open · Ctrl+C/V: Copy/Paste · Delete: Delete… · Ctrl+I: Details"
                } else {
                    "Enter: Open · F2: Rename · Ctrl+C/X/V: Copy/Cut/Paste · Delete: Delete…"
                }).font_size(11.0).color(ColorPalette::default().text_secondary()),
                Text::new(if !move_supported() { "Move and rename are not available on this system yet." } else { "" })
                    .font_size(11.0).color(ColorPalette::default().text_secondary()),
            }.alignment(Alignment::Leading).spacing(6.0).padding(8.0))
        };

        let tools = if picker {
            Either::A(vstack! {})
        } else {
            Either::B(self.browser_tools())
        };
        let prompt = if picker {
            Either::A(vstack! {})
        } else {
            Either::B(self.prompt_view())
        };
        let key_app = self.clone();

        vstack! {
            filter_hint,
            tools,
            prompt,
            grid,
            footer,
        }
        .on_key(move |event| key_app.handle_key(event, picker))
        .alignment(Alignment::Leading)
        .padding(10.0)
        .frame(f32::INFINITY, f32::INFINITY)
        .on_geometry_change(
            |geometry| geometry.size().width,
            move |width| width_state.set(width),
        )
    }

    fn browser_tools(&self) -> impl View + Clone + use<> {
        let new_app = self.clone();
        let copy_app = self.clone();
        let paste_app = self.clone();
        let cancel_search_app = self.clone();
        let ready = self.browser_commands_ready();
        hstack! {
            TextField::new(self.search.clone()).placeholder("Filter this folder…")
                .autofocus(self.search_focus.get() > 0)
                .on_cancel(move || { cancel_search_app.search.set(String::new()); cancel_search_app.search_focus.set(0); })
                .key(format!("files-search-{}", self.search_focus.get()))
                .frame(f32::INFINITY, 34.0),
            Button::new("New Folder…").font_size(13.0).enabled(ready).on_click(move || new_app.create_folder()),
            Button::new("Copy").font_size(13.0).enabled(ready && self.selected_entry().is_some()).on_click(move || copy_app.copy_selected(false)),
            Button::new("Paste").font_size(13.0).enabled(ready && self.clipboard.get().is_some_and(|item| !item.cut || move_supported())).on_click(move || paste_app.paste()),
        }.spacing(8.0)
    }

    fn prompt_view(&self) -> impl View + Clone + use<> {
        if let Some(prompt) = self.prompt.get() {
            let text_width = (self.browser_width.get() - 48.0).max(300.0);
            let (title, target, action, needs_name) = match prompt {
                FilePrompt::NewFolder(path) => ("Create a folder in", path, "Create", true),
                FilePrompt::Rename(path) => ("Rename", path, "Rename", true),
                FilePrompt::Delete(path) => (
                    "Delete permanently? This cannot be undone.",
                    path,
                    "Delete Permanently",
                    false,
                ),
            };
            let confirm_app = self.clone();
            let cancel_app = self.clone();
            let submit_app = self.clone();
            let escape_app = self.clone();
            let name = if needs_name {
                Either::A(
                    TextField::new(self.prompt_name.clone())
                        .placeholder("Name")
                        .autofocus(true)
                        .on_submit(move || submit_app.confirm_prompt())
                        .on_cancel(move || escape_app.prompt.set(None))
                        .frame(f32::INFINITY, 34.0),
                )
            } else {
                Either::B(vstack! {})
            };
            let deletion_note = if !needs_name {
                if cfg!(target_os = "scarlet") {
                    "Files and links are removed. Folders must be empty on this system."
                } else {
                    "Folder contents are also removed. Symbolic links keep their targets."
                }
            } else {
                ""
            };
            Either::A(
                vstack! {
                    Text::new(wrap_display_text(title, text_width, 14.0)).font_size(14.0),
                    Text::new(wrap_display_text(&target.to_string_lossy(), text_width, 12.0)).font_size(12.0),
                    Text::new(wrap_display_text(deletion_note, text_width, 12.0)).font_size(12.0),
                    name,
                    Text::new(wrap_display_text(&self.prompt_error.get(), text_width, 12.0)).font_size(12.0),
                    hstack! {
                        Spacer::new(),
                        Button::new("Cancel").on_click(move || cancel_app.prompt.set(None)),
                        Button::new(action).on_click(move || confirm_app.confirm_prompt()),
                    }.spacing(8.0),
                }
                .alignment(Alignment::Leading)
                .spacing(4.0)
                .padding(10.0)
                .background(ColorPalette::default().surface_variant()),
            )
        } else {
            Either::B(vstack! {})
        }
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
        let forward_app = self.clone();
        let up_app = self.clone();
        let refresh_app = self.clone();
        let submit_app = self.clone();
        let (location, current, focus) = if picker {
            (
                self.picker_location.clone(),
                self.picker_current_path.clone(),
                self.picker_location_focus.clone(),
            )
        } else {
            (
                self.location.clone(),
                self.current_path.clone(),
                self.location_focus.clone(),
            )
        };
        let focus_generation = focus.get();
        let current_path = TextField::new(location.clone())
            .placeholder("Folder path (Enter to open)")
            .autofocus(focus_generation > 0)
            .blur_on_submit(true)
            .on_submit(move || submit_app.submit_location(picker))
            .on_cancel(move || {
                location.set(current.get());
                focus.set(0);
            })
            .key(format!(
                "{}-location-{}",
                if picker { "picker" } else { "files" },
                focus_generation,
            ))
            .frame(f32::INFINITY, 32.0);
        HeaderBar::new(
            hstack! {
                Button::icon_only(Icon::ChevronLeft)
                    .header_style()
                    .on_click(move || back_app.navigate_history(false, picker)),
                Button::icon_only(Icon::ChevronRight)
                    .header_style()
                    .on_click(move || forward_app.navigate_history(true, picker)),
                Button::icon_only(Icon::ArrowUp)
                    .header_style()
                    .on_click(move || {
                        if picker {
                            up_app.navigate_picker_up();
                        } else {
                            up_app.navigate_up();
                        }
                    }),
                current_path,
                Button::icon_only(Icon::Refresh)
                    .header_style()
                    .on_click(move || {
                        if picker {
                            refresh_app.refresh_picker();
                        } else {
                            refresh_app.refresh();
                        }
                    }),
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
        } else if context.scene_key.as_str() == details::WINDOW_KEY {
            if self.details_busy.get() {
                self.cancel_details(true);
                return false;
            }
        } else if self.operation_busy.get() || self.details_busy.get() {
            self.status.set(String::from(
                "A file operation is still running. Close Files when it has finished.",
            ));
            return false;
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
        .menu_bar(self.browser_menu_bar())
        .size(Size::new(960.0, 640.0))
        .min_size(Size::new(760.0, 520.0))
        .scene_key("main")
        .open_at_launch(!STANDALONE_PICKER.load(AtomicOrdering::Acquire));

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
            .min_size(Size::new(640.0, 440.0))
            .open_at_launch(picker_open_at_launch);

        let details_window = Window::new("File Details", self.details_page())
            .scene_key(details::WINDOW_KEY)
            .app_id(APP_ID)
            .menu_bar(self.details_menu_bar())
            .size(Size::new(820.0, 620.0))
            .min_size(Size::new(680.0, 500.0))
            .open_at_launch(false);
        (main_window, picker_window, details_window)
    }

    fn exit_when_all_windows_closed(&self) -> bool {
        true
    }
}

/// Text has explicit line breaks in this toolkit. Wrap status/path text to the
/// measured viewport width so full error messages remain readable after resize.
fn wrap_display_text(text: &str, width: f32, font_size: f32) -> String {
    let mut output = String::new();
    let mut line = String::new();
    for character in text.chars() {
        if character == '\n' {
            output.push_str(&line);
            output.push('\n');
            line.clear();
            continue;
        }
        let mut candidate = line.clone();
        candidate.push(character);
        if !line.is_empty() && measure_text_sized(&candidate, font_size).0 as f32 > width {
            if let Some(split) = line.rfind(char::is_whitespace).filter(|index| *index > 0) {
                output.push_str(line[..split].trim_end());
                output.push('\n');
                line = line[split..].trim_start().to_owned();
            } else {
                output.push_str(&line);
                output.push('\n');
                line.clear();
            }
        }
        line.push(character);
    }
    output.push_str(&line);
    output
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
        resize_rgb_to_cover_thumbnail, text_width, thumbnail_source_dimensions_are_safe,
    };
    use scarlet_ui::{
        Application, Color, Event, MouseButton, MouseEvent, NavigationLink, NavigationView,
        RenderingPipeline, Scene, SceneBuilder, Size, Text, View, Window,
    };

    #[test]
    fn jpeg_thumbnail_source_dimensions_are_bounded() {
        assert!(thumbnail_source_dimensions_are_safe(3840, 2160));
        assert!(!thumbnail_source_dimensions_are_safe(5000, 5000));
        assert!(!thumbnail_source_dimensions_are_safe(0, 480));
        assert!(!thumbnail_source_dimensions_are_safe(usize::MAX, 2));
    }

    #[test]
    fn thumbnail_center_crops_rgb_pixels() {
        #[rustfmt::skip]
        let rgb = [
            255, 0, 0,  0, 255, 0,  0, 0, 255,  255, 255, 255,
            255, 0, 0,  0, 255, 0,  0, 0, 255,  255, 255, 255,
        ];

        let thumbnail = resize_rgb_to_cover_thumbnail(&rgb, 4, 2, 2, 2).unwrap();

        assert_eq!(
            thumbnail,
            vec![0xff00_ff00, 0xff00_00ff, 0xff00_ff00, 0xff00_00ff]
        );
    }

    #[test]
    fn thumbnail_box_filters_each_source_region() {
        #[rustfmt::skip]
        let rgb = [
            200, 0, 0,  100, 0, 0,  0, 200, 0,  0, 100, 0,
            200, 0, 0,  100, 0, 0,  0, 200, 0,  0, 100, 0,
        ];

        let thumbnail = resize_rgb_to_cover_thumbnail(&rgb, 4, 2, 2, 1).unwrap();

        assert_eq!(thumbnail, vec![0xff96_0000, 0xff00_9600]);
    }

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
    fn main_window_opens_and_picker_stays_hidden_until_requested() {
        let app = FilerApp::new();
        let mut builder = SceneBuilder::new();
        app.scenes().build(&mut builder);
        let declarations = builder.into_declarations();

        assert_eq!(declarations.len(), 3);
        assert!(declarations[0].opens_at_launch);
        assert!(!declarations[1].opens_at_launch);
        assert!(!declarations[2].opens_at_launch);
    }

    #[test]
    fn picker_process_arguments_preserve_selection_modes_and_names() {
        let request = PickerRequest {
            id: "request-123".into(),
            title: "Choose a folder".into(),
            initial_folder: "/tmp/日本語 with spaces".into(),
            filter: "extensions:txt,md".into(),
            allow_multiple: true,
            select_directories: true,
            save_mode: true,
            suggested_name: "メモ with spaces.txt".into(),
        };
        let parsed = super::parse_picker_args(&super::build_picker_argv(&request));
        assert_eq!(parsed.id, request.id);
        assert!(parsed.same_invocation(&request));
    }

    #[test]
    fn delete_command_only_prepares_confirmation_and_freezes_the_target() {
        let app = FilerApp::default();
        app.entries.set(vec![FileEntry {
            name: "keep.txt".into(),
            path: "/tmp/keep.txt".into(),
            is_directory: false,
            size: 4,
            kind: FileKind::from_path("keep.txt"),
            thumbnail: None,
        }]);
        app.selected.set(Some(0));
        app.delete_selected();
        assert!(!app.operation_busy.get());
        app.selected.set(None);
        assert!(
            matches!(app.prompt.get(), Some(super::FilePrompt::Delete(path)) if path == std::path::Path::new("/tmp/keep.txt"))
        );
        app.prompt.set(None);
        assert!(!app.operation_busy.get());
    }

    #[test]
    fn keyboard_navigation_respects_actual_grid_columns() {
        let app = FilerApp::default();
        app.grid_columns.set(3);
        app.entries.set(
            (0..10)
                .map(|index| FileEntry {
                    name: index.to_string(),
                    path: format!("/tmp/{index}"),
                    is_directory: true,
                    size: 0,
                    kind: FileKind::Folder,
                    thumbnail: None,
                })
                .collect(),
        );
        app.selected.set(Some(1));
        assert!(app.handle_key(
            scarlet_ui::KeyEvent::Pressed {
                keycode: scarlet_ui::KeyCode::Down,
                modifiers: Default::default()
            },
            false
        ));
        assert_eq!(app.selected.get(), Some(4));
        assert!(app.handle_key(
            scarlet_ui::KeyEvent::Pressed {
                keycode: scarlet_ui::KeyCode::End,
                modifiers: Default::default()
            },
            false
        ));
        assert_eq!(app.selected.get(), Some(9));
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
                    thumbnail: None,
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
                    thumbnail: None,
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
                    thumbnail: None,
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
                    thumbnail: None,
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
                    thumbnail: None,
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
                    thumbnail: None,
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
    #[cfg(not(target_os = "scarlet"))]
    {
        return PREVIEW_ROOT
            .get()
            .cloned()
            .unwrap_or_else(|| String::from("/__scarlet_files_preview_requires_fixture__"));
    }
    #[cfg(target_os = "scarlet")]
    std::env::var_os("HOME")
        .map(|path| path.to_string_lossy().into_owned())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| String::from(ROOT_PATH))
}

fn read_entries(path: &str, picker: Option<&PickerRequest>) -> std::io::Result<Vec<FileEntry>> {
    read_entries_with_options(path, picker, "", false, 0)
}

fn read_entries_with_options(
    path: &str,
    picker: Option<&PickerRequest>,
    query: &str,
    show_hidden: bool,
    sort_index: usize,
) -> std::io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    for entry in listing::read_directory(path, query, show_hidden, sort_index)? {
        let listing::Entry {
            name,
            path,
            is_directory,
            size,
        } = entry;
        if let Some(request) = picker
            && !is_directory
            && !matches_filter(&name, &request.filter)
        {
            continue;
        }
        let kind = if is_directory {
            FileKind::Folder
        } else {
            FileKind::from_path(&name)
        };
        entries.push(FileEntry {
            kind,
            name,
            path,
            is_directory,
            size,
            thumbnail: None,
        });
    }

    Ok(entries)
}

fn matches_filter(name: &str, filter: &str) -> bool {
    if let Some(matches) = picker_filter::matches_extensions(name, filter) {
        return matches;
    }
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

#[cfg(target_os = "scarlet")]
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
            if method == DESKTOP_FILE_MANAGER_GET_PICKER_CAPABILITIES_METHOD {
                let _ = connection.send_method_return(
                    0,
                    vec![Argument::String(String::from(
                        picker_filter::EXTENSION_FILTER_CAPABILITY,
                    ))],
                );
                continue;
            }
            if method == DESKTOP_FILE_MANAGER_SHOW_METHOD {
                let _ = connection.send_method_return(0, Vec::new());
                // The resident service owns no UI thread. The main Files
                // window is a normal application, so a Show request asks
                // stemd to launch or focus it.
                thread::spawn(|| {
                    let _ = SbusConnection::connect().and_then(|mut connection| {
                        connection.call_method_timeout(
                            DESKTOP_STEMD_BUS_NAME,
                            DESKTOP_STEMD_OBJECT_PATH,
                            DESKTOP_STEMD_INTERFACE,
                            DESKTOP_STEMD_LAUNCH_OR_FOCUS_METHOD,
                            vec![Argument::String(String::from(DESKTOP_FILES_APP_ID))],
                            3_000,
                        )
                    });
                });
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

            if !picker_filter::valid_filter(&request.filter) {
                let _ = connection.send_method_error(
                    0,
                    "org.scarlet.desktop.FileManager.InvalidFilter",
                    "Expected comma-separated literal extensions after extensions:",
                );
                continue;
            }
            if request.allow_multiple {
                let _ = connection.send_method_error(
                    0,
                    "org.scarlet.desktop.FileManager.UnsupportedSelection",
                    "This file picker supports selecting one item at a time",
                );
                continue;
            }
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

#[cfg(target_os = "scarlet")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--picker") {
        println!("[filer] starting in standalone picker mode");
        run_standalone_picker(&args);
        return;
    }

    if args.iter().any(|a| a == "--service") {
        println!("[filer] starting in service mode");
        run_file_manager_service_forever();
        return;
    }

    // Default (no mode flag) and `--main` run the normal application with the
    // main window open at launch. The FileManager sbus service is a separate
    // process (`/bin/files --service`) so a wedged UI can never take the
    // service down with it, and vice versa.
    println!("[filer] starting in main window mode");
    run_main_window();
}

/// Host visual verification uses the production views and an explicitly supplied
/// temporary fixture. It does not provide Scarlet launch or picker IPC services.
#[cfg(not(target_os = "scarlet"))]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("--preview-fixture") {
        eprintln!(
            "Host Files requires --preview-fixture DIRECTORY; native services are unavailable."
        );
        return;
    }
    let Some(path) = args
        .get(2)
        .and_then(|path| fs::canonicalize(path).ok())
        .filter(|path| path.is_dir())
    else {
        eprintln!("Preview fixture must be an existing directory.");
        return;
    };
    let _ = PREVIEW_ROOT.set(path.to_string_lossy().into_owned());
    run_main_window();
}

#[cfg(target_os = "scarlet")]
fn open_desktop_path(path: &str) -> std::result::Result<(), String> {
    SbusConnection::connect()
        .and_then(|mut connection| {
            connection.call_method_timeout(
                DESKTOP_STEMD_BUS_NAME,
                DESKTOP_STEMD_OBJECT_PATH,
                DESKTOP_STEMD_INTERFACE,
                DESKTOP_STEMD_OPEN_PATH_METHOD,
                vec![Argument::String(path.into())],
                3_000,
            )
        })
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}
#[cfg(not(target_os = "scarlet"))]
fn open_desktop_path(_path: &str) -> std::result::Result<(), String> {
    Err("Opening external applications requires Scarlet; this is a host preview".into())
}
#[cfg(not(target_os = "scarlet"))]
fn send_picker_response(_request_id: String, _success: bool, _path: String) {
    eprintln!("File-picker IPC requires Scarlet; this is a host preview");
}

/// Resident FileManager sbus service. Serves OpenFile/SaveFile (spawning
/// standalone picker children) and Show (delegating to stemd). Runs no UI.
#[cfg(target_os = "scarlet")]
fn run_file_manager_service_forever() {
    thread::spawn(run_picker_service);
    thread::spawn(picker_reaper);
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

/// Normal application entry: open the main window and exit when it closes.
fn run_main_window() {
    let mut app = FilerApp::new();
    println!("[filer-dbg] main: entering app.run");
    if let Err(error) = app.run() {
        eprintln!("[filer] error: {error}");
    }
    println!("[filer-dbg] main: app.run returned");
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
#[cfg(target_os = "scarlet")]
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
            send_picker_response(request.id.clone(), false, String::new());
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
            "--allow-multiple" => request.allow_multiple = true,
            "--select-directories" => request.select_directories = true,
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
    if request.allow_multiple {
        argv.push(String::from("--allow-multiple"));
    }
    if request.select_directories {
        argv.push(String::from("--select-directories"));
    }
    if request.save_mode {
        argv.push(String::from("--save"));
        if !request.suggested_name.is_empty() {
            argv.push(String::from("--suggested-name"));
            argv.push(request.suggested_name.clone());
        }
    }
    argv
}
