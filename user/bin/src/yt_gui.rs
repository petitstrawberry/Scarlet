#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;
extern crate scarlet_ui_macros;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::f32;

use scarlet_ui::views::ImageFit;
use scarlet_ui::{
    BitmapImage, KeyCode, KeyEvent, State, StateId, hstack, prelude::*, vstack, zstack,
};
use scarlet_ui_macros::View;

use std::fs::{File, remove_file};
use std::task::{execve, exit, fork, getpid, waitpid};
use std::{env, format, println};

const PAGE_SIZE: usize = 8;
const THUMB_WIDTH: u32 = 160;
const THUMB_HEIGHT: u32 = 90;

#[derive(Clone)]
enum ThumbnailState {
    NotRequested,
    Loading,
    Ready(BitmapImage),
    Failed,
}

impl ThumbnailState {
    fn is_not_requested(&self) -> bool {
        matches!(self, Self::NotRequested)
    }
}

#[derive(Clone)]
struct GuiSearchResult {
    video_id: String,
    title: String,
    channel: Option<String>,
    duration: Option<String>,
    thumbnail: ThumbnailState,
    thumbnail_url: Option<String>,
}

impl GuiSearchResult {
    fn watch_url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.video_id)
    }
}

#[derive(View, Clone)]
struct YtGuiApp {
    query: State<String>,
    results: State<Vec<GuiSearchResult>>,
    selected: State<usize>,
    page: State<usize>,
    status: State<String>,
    search_focused: State<bool>,
    list_focused: State<bool>,
}

impl YtGuiApp {
    fn new(query: String) -> Self {
        Self {
            query: State::new(StateId::new(0), query),
            results: State::new(StateId::new(1), Vec::new()),
            selected: State::new(StateId::new(2), 0),
            page: State::new(StateId::new(3), 0),
            status: State::new(
                StateId::new(4),
                String::from("Type a query and press Enter."),
            ),
            search_focused: State::new(StateId::new(5), false),
            list_focused: State::new(StateId::new(6), false),
        }
    }

    fn selected_result(&self) -> Option<GuiSearchResult> {
        self.results.get().get(self.selected.get()).cloned()
    }

    fn result_row(&self, slot: usize) -> impl View + Clone {
        let results = self.results.get();
        let page = self.page.get();
        let absolute_index = page.saturating_mul(PAGE_SIZE).saturating_add(slot);
        let selected = self.selected.get();
        let row = results.get(absolute_index).cloned();
        let is_selected = absolute_index == selected && row.is_some();
        let background = if is_selected {
            Color::rgb(230u8, 238u8, 248u8)
        } else {
            Color::rgb(250u8, 251u8, 252u8)
        };

        let border = if is_selected {
            Color::rgb(35u8, 95u8, 160u8)
        } else {
            Color::rgb(222u8, 226u8, 232u8)
        };

        let (index, title, channel, duration, thumb) = if let Some(row) = row {
            (
                format!("{}", absolute_index + 1),
                compact_text(&row.title, 58),
                row.channel
                    .unwrap_or_else(|| String::from("unknown channel")),
                row.duration.unwrap_or_else(|| String::from("--:--")),
                row.thumbnail,
            )
        } else {
            (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                ThumbnailState::NotRequested,
            )
        };

        zstack! {
            scarlet_ui::Rectangle::new()
                .fill(background)
                .corner_radius(5.0)
                .border(1.0, border),
            hstack! {
                Text::new(index)
                    .font_size(11.0)
                    .color(Color::gray(0.42))
                    .frame_width(24.0),
                thumbnail_image(thumb)
                    .fit_mode(ImageFit::Fill)
                    .frame(96.0, 54.0)
                    .clip_radius(4.0),
                vstack! {
                    Text::new(title)
                        .font_size(13.0)
                        .color(Color::rgb(22u8, 26u8, 32u8))
                        .frame_width(380.0),
                    Text::new(compact_text(&channel, 42))
                        .font_size(11.0)
                        .color(Color::gray(0.42))
                        .frame_width(380.0),
                }
                .frame_width(390.0),
                Spacer::new(),
                Text::new(duration)
                    .font_size(11.0)
                    .color(Color::gray(0.34))
                    .frame_width(42.0),
            }
            .padding(7.0)
        }
        .frame(620.0, 70.0)
        .on_click({
            let results = self.results.clone();
            let selected = self.selected.clone();
            let search_focused = self.search_focused.clone();
            let list_focused = self.list_focused.clone();
            move || {
                if absolute_index < results.get().len() {
                    selected.set(absolute_index);
                    search_focused.set(false);
                    list_focused.set(true);
                }
            }
        })
    }
}

impl Application for YtGuiApp {
    fn body(&self) -> impl View {
        let results_len = self.results.get().len();
        let page = self.page.get();
        let page_count = page_count(results_len);
        let selected = self.selected.get();
        let selected_result = self.selected_result();
        let (detail_title, detail_channel, detail_duration, detail_id, detail_thumb) =
            if let Some(row) = selected_result {
                (
                    compact_text(&row.title, 90),
                    row.channel
                        .map(|channel| compact_text(&channel, 46))
                        .unwrap_or_else(|| String::from("unknown channel")),
                    row.duration.unwrap_or_else(|| String::from("--:--")),
                    row.video_id,
                    row.thumbnail,
                )
            } else {
                (
                    String::from("No video selected"),
                    String::from("Search and select a result."),
                    String::from("--:--"),
                    String::from("-"),
                    ThumbnailState::NotRequested,
                )
            };

        Window::new(
            "YouTube",
            vstack! {
                hstack! {
                    Text::new("YouTube")
                        .font_size(20.0)
                        .color(Color::rgb(24u8, 28u8, 34u8))
                        .frame_width(92.0),
                    TextField::new(self.query.clone(), self.search_focused.clone())
                        .placeholder("Search YouTube")
                        .font_size(14.0)
                        .padding(8.0)
                        .blur_on_submit(true)
                        .on_submit({
                            let query = self.query.clone();
                            let results = self.results.clone();
                            let selected = self.selected.clone();
                            let page = self.page.clone();
                            let status = self.status.clone();
                            move || perform_search(query.clone(), results.clone(), selected.clone(), page.clone(), status.clone())
                        })
                    .frame(660.0, 34.0),
                    Button::new("Search")
                        .font_size(12.0)
                        .padding(7.0)
                        .on_click({
                            let query = self.query.clone();
                            let results = self.results.clone();
                            let selected = self.selected.clone();
                            let page = self.page.clone();
                            let status = self.status.clone();
                            move || perform_search(query.clone(), results.clone(), selected.clone(), page.clone(), status.clone())
                        }),
                },
                hstack! {
                    vstack! {
                        hstack! {
                            Text::new(format!("{} results", results_len))
                                .font_size(12.0)
                                .color(Color::gray(0.35))
                                .frame_width(110.0),
                            Text::new(format!("page {}/{}", page + 1, page_count))
                                .font_size(12.0)
                                .color(Color::gray(0.35))
                                .frame_width(90.0),
                            Text::new(format!("selected {}", if results_len == 0 { 0 } else { selected.saturating_add(1) }))
                                .font_size(12.0)
                                .color(Color::gray(0.35))
                                .frame_width(110.0),
                            Spacer::new(),
                            Button::new("Prev")
                                .font_size(11.0)
                                .padding(5.0)
                                .on_click({
                                    let results = self.results.clone();
                                    let page = self.page.clone();
                                    let selected = self.selected.clone();
                                    let status = self.status.clone();
                                    move || previous_page(results.clone(), page.clone(), selected.clone(), status.clone())
                                }),
                            Button::new("Next")
                                .font_size(11.0)
                                .padding(5.0)
                                .on_click({
                                    let results = self.results.clone();
                                    let page = self.page.clone();
                                    let selected = self.selected.clone();
                                    let status = self.status.clone();
                                    move || next_page(results.clone(), page.clone(), selected.clone(), status.clone())
                                }),
                        }
                        .frame_width(620.0),
                        vstack! {
                            self.result_row(0),
                            self.result_row(1),
                            self.result_row(2),
                            self.result_row(3),
                            self.result_row(4),
                            self.result_row(5),
                            self.result_row(6),
                            self.result_row(7),
                        },
                    }
                    .focusable(self.list_focused.clone())
                    .on_key({
                        let results = self.results.clone();
                        let selected = self.selected.clone();
                        let page = self.page.clone();
                        let status = self.status.clone();
                        let list_focused = self.list_focused.clone();
                        move |event| handle_list_key(event, results.clone(), selected.clone(), page.clone(), status.clone(), list_focused.clone())
                    })
                    .frame_width(630.0),
                    zstack! {
                        scarlet_ui::Rectangle::new()
                            .fill(Color::rgb(247u8, 248u8, 250u8))
                            .corner_radius(6.0)
                            .border(1.0, Color::rgb(218u8, 222u8, 228u8)),
                        vstack! {
                            thumbnail_image(detail_thumb)
                                .fit_mode(ImageFit::Fill)
                                .frame(272.0, 153.0)
                                .clip_radius(5.0),
                            Text::new(detail_title)
                                .font_size(15.0)
                                .color(Color::rgb(20u8, 24u8, 30u8))
                                .frame_width(272.0),
                            Text::new(detail_channel)
                                .font_size(12.0)
                                .color(Color::gray(0.38))
                                .frame_width(272.0),
                            hstack! {
                                Text::new(detail_duration)
                                    .font_size(12.0)
                                    .color(Color::gray(0.34))
                                    .frame_width(74.0),
                                Text::new(detail_id)
                                    .font_size(11.0)
                                    .color(Color::gray(0.48))
                                    .frame_width(176.0),
                            },
                        }
                        .padding(12.0)
                    }
                    .frame(302.0, 620.0),
                }
                .frame_width(948.0),
                Text::new(self.status.get())
                    .font_size(12.0)
                    .color(Color::gray(0.36))
                    .frame_width(948.0),
            }
            .padding(12.0)
            .background(Color::rgb(240u8, 242u8, 245u8))
            .frame(f32::INFINITY, f32::INFINITY),
        )
        .app_id("org.scarlet-os.yt-gui")
        .size(Size::new(990.0, 760.0))
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

fn handle_list_key(
    event: KeyEvent,
    results: State<Vec<GuiSearchResult>>,
    selected: State<usize>,
    page: State<usize>,
    status: State<String>,
    list_focused: State<bool>,
) -> bool {
    match event {
        KeyEvent::Pressed {
            keycode: KeyCode::Tab,
        } => {
            list_focused.set(false);
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Enter,
        } => {
            play_selected(results, selected, status);
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Down,
        } => {
            move_selection(results, selected, page, status, 1);
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Up,
        } => {
            move_selection(results, selected, page, status, -1);
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::PageDown,
        }
        | KeyEvent::Pressed {
            keycode: KeyCode::Right,
        } => {
            next_page(results, page, selected, status);
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::PageUp,
        }
        | KeyEvent::Pressed {
            keycode: KeyCode::Left,
        } => {
            previous_page(results, page, selected, status);
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Escape,
        } => exit(0),
        _ => false,
    }
}

fn perform_search(
    query: State<String>,
    results: State<Vec<GuiSearchResult>>,
    selected: State<usize>,
    page: State<usize>,
    status: State<String>,
) {
    let query_text = query.get().trim().to_string();
    if query_text.is_empty() {
        status.set(String::from("Enter a search query."));
        return;
    }

    status.set(format!("Searching: {}", query_text));
    println!("[yt-gui] searching: {}", query_text);
    match run_yt_search(&query_text) {
        Ok(gui_results) => {
            let count = gui_results.len();
            results.set(gui_results);
            selected.set(0);
            page.set(0);
            status.set(format!("Loaded {} search results.", count));
            request_visible_thumbnails(results, 0, status);
        }
        Err(error) => {
            results.set(Vec::new());
            selected.set(0);
            page.set(0);
            status.set(format!("Search failed: {}", error));
        }
    }
}

fn play_selected(
    results: State<Vec<GuiSearchResult>>,
    selected: State<usize>,
    status: State<String>,
) {
    let list = results.get();
    let Some(row) = list.get(selected.get()) else {
        status.set(String::from("No selected result."));
        return;
    };
    let url = row.watch_url();
    let title = row.title.clone();
    status.set(format!("Starting playback: {}", title));
    println!("[yt-gui] spawn: yt --title <title> {}", url);
    let child = fork();
    if child < 0 {
        status.set(String::from("failed to fork /bin/yt"));
        return;
    }
    if child == 0 {
        let argv = ["yt", "--title", title.as_str(), url.as_str()];
        let rc = execve("/bin/yt", &argv, &[]);
        println!("[yt-gui] failed to exec /bin/yt: {}", rc);
        exit(1);
    }
    status.set(format!("Playback started: {}", title));
}

fn move_selection(
    results: State<Vec<GuiSearchResult>>,
    selected: State<usize>,
    page: State<usize>,
    status: State<String>,
    delta: isize,
) {
    let len = results.get().len();
    if len == 0 {
        selected.set(0);
        page.set(0);
        return;
    }
    let current = selected.get().min(len - 1);
    let next = if delta < 0 {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        current.saturating_add(delta as usize).min(len - 1)
    };
    let next_page = next / PAGE_SIZE;
    selected.set(next);
    page.set(next_page);
    request_visible_thumbnails(results, next_page, status);
}

fn next_page(
    results: State<Vec<GuiSearchResult>>,
    page: State<usize>,
    selected: State<usize>,
    status: State<String>,
) {
    let len = results.get().len();
    let pages = page_count(len);
    let next = page.get().saturating_add(1).min(pages - 1);
    page.set(next);
    if len > 0 {
        selected.set((next * PAGE_SIZE).min(len - 1));
    }
    request_visible_thumbnails(results, next, status);
}

fn previous_page(
    results: State<Vec<GuiSearchResult>>,
    page: State<usize>,
    selected: State<usize>,
    status: State<String>,
) {
    let previous = page.get().saturating_sub(1);
    page.set(previous);
    selected.set(previous * PAGE_SIZE);
    request_visible_thumbnails(results, previous, status);
}

fn page_count(len: usize) -> usize {
    len.div_ceil(PAGE_SIZE).max(1)
}

fn run_yt_search(query: &str) -> core::result::Result<Vec<GuiSearchResult>, String> {
    let path = format!("/tmp/yt-gui-search-{}.tsv", getpid());
    let _ = remove_file(&path);
    let argv = ["yt", "--search-results", path.as_str(), query];
    run_child("/bin/yt", &argv)?;

    let text = read_file_to_string(&path)?;
    let _ = remove_file(&path);
    Ok(scarlet_youtube::parse_search_results_tsv(&text)
        .into_iter()
        .map(|result| GuiSearchResult {
            video_id: result.video_id,
            title: result.title,
            channel: result.channel,
            duration: result.duration,
            thumbnail: ThumbnailState::NotRequested,
            thumbnail_url: result.thumbnail_url,
        })
        .collect())
}

fn thumbnail_image(thumbnail: ThumbnailState) -> scarlet_ui::Image {
    match thumbnail {
        ThumbnailState::Ready(image) => scarlet_ui::Image::from_bitmap(image),
        _ => scarlet_ui::Image::placeholder(THUMB_WIDTH, THUMB_HEIGHT),
    }
}

fn request_visible_thumbnails(
    results: State<Vec<GuiSearchResult>>,
    page: usize,
    status: State<String>,
) {
    let mut list = results.get();
    let start = page.saturating_mul(PAGE_SIZE);
    let end = start.saturating_add(PAGE_SIZE).min(list.len());
    let mut requests = Vec::new();

    for result in list.iter_mut().take(end).skip(start) {
        if result.thumbnail.is_not_requested() {
            if let Some(url) = result.thumbnail_url.clone() {
                result.thumbnail = ThumbnailState::Loading;
                requests.push((result.video_id.clone(), url));
            } else {
                result.thumbnail = ThumbnailState::Failed;
            }
        }
    }

    if requests.is_empty() {
        return;
    }

    results.set(list);
    status.set(format!("Loading thumbnails for page {}.", page + 1));

    let mut list = results.get();
    for (video_id, url) in requests {
        let thumbnail = match download_thumbnail_image(&video_id, &url) {
            Some(image) => ThumbnailState::Ready(image),
            None => ThumbnailState::Failed,
        };
        if let Some(result) = list.iter_mut().find(|result| result.video_id == video_id) {
            result.thumbnail = thumbnail;
        }
    }
    results.set(list);
    status.set(format!("Loaded thumbnails for page {}.", page + 1));
}

fn download_thumbnail_image(video_id: &str, url: &str) -> Option<BitmapImage> {
    let path = format!("/tmp/yt-gui-thumb-{}-{}.jpg", getpid(), video_id);
    let _ = remove_file(&path);
    let argv = ["yt", "--no-play", "-o", path.as_str(), url];
    run_child("/bin/yt", &argv).ok()?;
    let bytes = read_file_to_bytes(&path).ok()?;
    let _ = remove_file(&path);
    BitmapImage::from_jpeg_bytes(&bytes)
}

fn run_child(path: &str, argv: &[&str]) -> core::result::Result<(), String> {
    let child = fork();
    if child < 0 {
        return Err(format!("failed to fork {}", path));
    }
    if child == 0 {
        let rc = execve(path, argv, &[]);
        println!("[yt-gui] failed to exec {}: {}", path, rc);
        exit(1);
    }
    let (_pid, status) = waitpid(child, 0);
    if status == 0 {
        Ok(())
    } else {
        Err(format!("{} exited with status {}", path, status))
    }
}

fn read_file_to_string(path: &str) -> core::result::Result<String, String> {
    let bytes = read_file_to_bytes(path)?;
    String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path))
}

fn read_file_to_bytes(path: &str) -> core::result::Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|_| format!("failed to open {}", path))?;
    let mut out = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|_| format!("failed to read {}", path))?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buffer[..n]);
    }
    Ok(out)
}

fn compact_text(input: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, ch) in input.chars().enumerate() {
        if index >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn initial_query() -> String {
    let args = env::args_vec();
    let mut query = String::new();
    for arg in args.iter().skip(1) {
        if !query.is_empty() {
            query.push(' ');
        }
        query.push_str(arg);
    }
    query
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("[yt-gui] Starting YouTube GUI");

    let mut app = YtGuiApp::new(initial_query());
    match app.run() {
        Ok(()) => println!("[yt-gui] exited"),
        Err(error) => println!("[yt-gui] error: {}", error),
    }
}
