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
use scarlet_youtube_net::{
    YoutubeSearchCursor, YoutubeVideoDetails, fetch_youtube_thumbnail_bytes, youtube_video_details,
};

use std::sync::Mutex;
use std::task::{execve, exit, fork};
use std::thread;
use std::{env, format, println};

const PAGE_SIZE: usize = 8;
const THUMB_WIDTH: u32 = 160;
const THUMB_HEIGHT: u32 = 90;
const DETAIL_TEXT_WIDTH: u32 = 272;
const DETAIL_DESCRIPTION_FONT_SIZE: f32 = 11.0;
const DETAIL_DESCRIPTION_MAX_CHARS: usize = 900;
const DETAIL_DESCRIPTION_MAX_LINES: usize = 24;

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
enum DetailState {
    Empty,
    Loading { video_id: String },
    Ready(GuiVideoDetails),
    Failed { video_id: String, message: String },
}

impl Default for DetailState {
    fn default() -> Self {
        Self::Empty
    }
}

#[derive(Clone)]
struct GuiVideoDetails {
    video_id: String,
    title: Option<String>,
    author: Option<String>,
    description: Option<String>,
    thumbnail_url: Option<String>,
}

impl GuiVideoDetails {
    fn from_youtube(details: YoutubeVideoDetails) -> Self {
        Self {
            video_id: details.video_id,
            title: details.title,
            author: details.author,
            description: details.description,
            thumbnail_url: details.thumbnail_url,
        }
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

enum GuiMessage {
    SearchFinished {
        generation: u64,
        result: SearchLoadOutcome,
    },
    SearchMoreFinished {
        generation: u64,
        target_page: Option<usize>,
        result: SearchLoadOutcome,
    },
    ThumbnailFinished {
        generation: u64,
        video_id: String,
        thumbnail: ThumbnailState,
    },
    ThumbnailBatchFinished {
        generation: u64,
    },
    DetailsFinished {
        generation: u64,
        video_id: String,
        result: core::result::Result<GuiVideoDetails, String>,
    },
}

struct SearchLoadResult {
    results: Vec<GuiSearchResult>,
    has_more: bool,
    cursor: YoutubeSearchCursor,
}

struct SearchLoadError {
    message: String,
    cursor: Option<YoutubeSearchCursor>,
}

type SearchLoadOutcome = core::result::Result<SearchLoadResult, SearchLoadError>;

static YT_GUI_GENERATION: Mutex<u64> = Mutex::new(0);
static YT_GUI_MESSAGES: Mutex<Vec<GuiMessage>> = Mutex::new(Vec::new());
static YT_GUI_SEARCH_CURSOR: Mutex<Option<YoutubeSearchCursor>> = Mutex::new(None);
static YT_GUI_THUMBNAIL_ACTIVE: Mutex<bool> = Mutex::new(false);
static YT_GUI_DETAILS_ACTIVE: Mutex<bool> = Mutex::new(false);

#[derive(View, Clone)]
struct YtGuiApp {
    query: State<String>,
    results: State<Vec<GuiSearchResult>>,
    selected: State<usize>,
    page: State<usize>,
    status: State<String>,
    search_focused: State<bool>,
    list_focused: State<bool>,
    has_more: State<bool>,
    loading_more: State<bool>,
    details: State<DetailState>,
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
            has_more: State::new(StateId::new(7), false),
            loading_more: State::new(StateId::new(8), false),
            details: State::new(StateId::new(9), DetailState::Empty),
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
            let details = self.details.clone();
            let search_focused = self.search_focused.clone();
            let list_focused = self.list_focused.clone();
            move || {
                if absolute_index < results.get().len() {
                    selected.set(absolute_index);
                    search_focused.set(false);
                    list_focused.set(true);
                    request_selected_details(
                        results.clone(),
                        selected.clone(),
                        details.clone(),
                        current_generation(),
                    );
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
        let has_more = self.has_more.get();
        let selected_result = self.selected_result();
        let (base_title, base_channel, detail_duration, detail_id, detail_thumb) =
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
        let (detail_title, detail_channel, detail_description) =
            detail_pane_text(self.details.get(), &detail_id, base_title, base_channel);

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
                            let has_more = self.has_more.clone();
                            let loading_more = self.loading_more.clone();
                            let details = self.details.clone();
                            move || perform_search(query.clone(), results.clone(), selected.clone(), page.clone(), status.clone(), has_more.clone(), loading_more.clone(), details.clone())
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
                            let has_more = self.has_more.clone();
                            let loading_more = self.loading_more.clone();
                            let details = self.details.clone();
                            move || perform_search(query.clone(), results.clone(), selected.clone(), page.clone(), status.clone(), has_more.clone(), loading_more.clone(), details.clone())
                        }),
                },
                hstack! {
                    vstack! {
                        hstack! {
                            Text::new(format!("{} results{}", results_len, if has_more { "+" } else { "" }))
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
                                    let details = self.details.clone();
                                    move || previous_page(results.clone(), page.clone(), selected.clone(), status.clone(), details.clone())
                                }),
                            Button::new("Next")
                                .font_size(11.0)
                                .padding(5.0)
                                .on_click({
                                    let results = self.results.clone();
                                    let page = self.page.clone();
                                    let selected = self.selected.clone();
                                    let status = self.status.clone();
                                    let has_more = self.has_more.clone();
                                    let loading_more = self.loading_more.clone();
                                    let details = self.details.clone();
                                    move || next_page(results.clone(), page.clone(), selected.clone(), status.clone(), has_more.clone(), loading_more.clone(), details.clone())
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
                        let has_more = self.has_more.clone();
                        let loading_more = self.loading_more.clone();
                        let details = self.details.clone();
                        move |event| handle_list_key(event, results.clone(), selected.clone(), page.clone(), status.clone(), list_focused.clone(), has_more.clone(), loading_more.clone(), details.clone())
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
                            Text::new(detail_description)
                                .font_size(DETAIL_DESCRIPTION_FONT_SIZE)
                                .color(Color::gray(0.28))
                                .frame_width(DETAIL_TEXT_WIDTH as f32),
                            Spacer::new(),
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

    fn on_idle(&mut self) {
        while let Some(message) = pop_gui_message() {
            self.handle_message(message);
        }
        request_selected_details(
            self.results.clone(),
            self.selected.clone(),
            self.details.clone(),
            current_generation(),
        );
    }
}

impl YtGuiApp {
    fn handle_message(&self, message: GuiMessage) {
        match message {
            GuiMessage::SearchFinished { generation, result } => {
                if generation != current_generation() {
                    return;
                }
                match result {
                    Ok(load) => {
                        let count = load.results.len();
                        *YT_GUI_SEARCH_CURSOR.lock() = Some(load.cursor);
                        self.results.set(load.results);
                        self.selected.set(0);
                        self.page.set(0);
                        self.has_more.set(load.has_more);
                        self.loading_more.set(false);
                        self.status.set(format!(
                            "Loaded {} search results{}.",
                            count,
                            if load.has_more { " so far" } else { "" }
                        ));
                        request_visible_thumbnails(
                            self.results.clone(),
                            0,
                            self.status.clone(),
                            generation,
                        );
                        request_selected_details(
                            self.results.clone(),
                            self.selected.clone(),
                            self.details.clone(),
                            generation,
                        );
                    }
                    Err(error) => {
                        *YT_GUI_SEARCH_CURSOR.lock() = None;
                        self.results.set(Vec::new());
                        self.selected.set(0);
                        self.page.set(0);
                        self.has_more.set(false);
                        self.loading_more.set(false);
                        self.details.set(DetailState::Empty);
                        self.status.set(format!("Search failed: {}", error.message));
                    }
                }
            }
            GuiMessage::SearchMoreFinished {
                generation,
                target_page,
                result,
            } => {
                if generation != current_generation() {
                    return;
                }
                self.loading_more.set(false);
                match result {
                    Ok(load) => {
                        let mut list = self.results.get();
                        let added = load.results.len();
                        list.extend(load.results);
                        let len = list.len();
                        *YT_GUI_SEARCH_CURSOR.lock() = Some(load.cursor);
                        self.results.set(list);
                        self.has_more.set(load.has_more);

                        let page_to_show = if let Some(target_page) = target_page {
                            target_page.min(page_count(len).saturating_sub(1))
                        } else {
                            self.page.get()
                        };
                        self.page.set(page_to_show);
                        if len > 0 {
                            self.selected
                                .set((page_to_show.saturating_mul(PAGE_SIZE)).min(len - 1));
                        }
                        self.status.set(format!(
                            "Loaded {} more results ({} total{}).",
                            added,
                            len,
                            if load.has_more { "+" } else { "" }
                        ));
                        request_visible_thumbnails(
                            self.results.clone(),
                            page_to_show,
                            self.status.clone(),
                            generation,
                        );
                        request_selected_details(
                            self.results.clone(),
                            self.selected.clone(),
                            self.details.clone(),
                            generation,
                        );
                    }
                    Err(error) => {
                        if let Some(cursor) = error.cursor {
                            *YT_GUI_SEARCH_CURSOR.lock() = Some(cursor);
                        }
                        self.status
                            .set(format!("Search continuation failed: {}", error.message));
                    }
                }
            }
            GuiMessage::ThumbnailFinished {
                generation,
                video_id,
                thumbnail,
            } => {
                if generation != current_generation() {
                    return;
                }
                let mut list = self.results.get();
                if let Some(result) = list.iter_mut().find(|result| result.video_id == video_id) {
                    result.thumbnail = thumbnail;
                    self.results.set(list);
                }
            }
            GuiMessage::ThumbnailBatchFinished { generation } => {
                *YT_GUI_THUMBNAIL_ACTIVE.lock() = false;
                if generation != current_generation() {
                    return;
                }
                request_visible_thumbnails(
                    self.results.clone(),
                    self.page.get(),
                    self.status.clone(),
                    generation,
                );
            }
            GuiMessage::DetailsFinished {
                generation,
                video_id,
                result,
            } => {
                *YT_GUI_DETAILS_ACTIVE.lock() = false;
                if generation != current_generation() {
                    return;
                }
                let current = self.details.get();
                if !matches!(current, DetailState::Loading { video_id: ref current_id } if current_id == &video_id)
                {
                    return;
                }
                match result {
                    Ok(details) => self.details.set(DetailState::Ready(details)),
                    Err(message) => self.details.set(DetailState::Failed { video_id, message }),
                }
            }
        }
    }
}

fn handle_list_key(
    event: KeyEvent,
    results: State<Vec<GuiSearchResult>>,
    selected: State<usize>,
    page: State<usize>,
    status: State<String>,
    list_focused: State<bool>,
    has_more: State<bool>,
    loading_more: State<bool>,
    details: State<DetailState>,
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
            move_selection(
                results,
                selected,
                page,
                status,
                has_more,
                loading_more,
                details,
                1,
            );
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Up,
        } => {
            move_selection(
                results,
                selected,
                page,
                status,
                has_more,
                loading_more,
                details,
                -1,
            );
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::PageDown,
        }
        | KeyEvent::Pressed {
            keycode: KeyCode::Right,
        } => {
            next_page(
                results,
                page,
                selected,
                status,
                has_more,
                loading_more,
                details,
            );
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::PageUp,
        }
        | KeyEvent::Pressed {
            keycode: KeyCode::Left,
        } => {
            previous_page(results, page, selected, status, details);
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
    has_more: State<bool>,
    loading_more: State<bool>,
    details: State<DetailState>,
) {
    let query_text = query.get().trim().to_string();
    if query_text.is_empty() {
        status.set(String::from("Enter a search query."));
        return;
    }

    let generation_id = next_generation();
    results.set(Vec::new());
    selected.set(0);
    page.set(0);
    has_more.set(false);
    loading_more.set(true);
    details.set(DetailState::Empty);
    *YT_GUI_SEARCH_CURSOR.lock() = None;
    status.set(format!("Searching: {}", query_text));
    println!("[yt-gui] searching: {}", query_text);
    thread::spawn(move || {
        let result = load_search_page(YoutubeSearchCursor::new(&query_text), PAGE_SIZE);
        push_gui_message(GuiMessage::SearchFinished {
            generation: generation_id,
            result,
        });
    });
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
    has_more: State<bool>,
    loading_more: State<bool>,
    details: State<DetailState>,
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
    if delta > 0 && next == current && has_more.get() {
        load_more_search_results(
            status,
            has_more,
            loading_more,
            Some(page.get().saturating_add(1)),
        );
        return;
    }
    let next_page = next / PAGE_SIZE;
    selected.set(next);
    page.set(next_page);
    let generation = current_generation();
    request_visible_thumbnails(results.clone(), next_page, status, generation);
    request_selected_details(results, selected, details, generation);
}

fn next_page(
    results: State<Vec<GuiSearchResult>>,
    page: State<usize>,
    selected: State<usize>,
    status: State<String>,
    has_more: State<bool>,
    loading_more: State<bool>,
    details: State<DetailState>,
) {
    let len = results.get().len();
    let next = page.get().saturating_add(1);
    if next.saturating_mul(PAGE_SIZE) >= len {
        if has_more.get() {
            load_more_search_results(status, has_more, loading_more, Some(next));
        }
        return;
    }
    page.set(next);
    if len > 0 {
        selected.set((next * PAGE_SIZE).min(len - 1));
    }
    let generation = current_generation();
    request_visible_thumbnails(results.clone(), next, status, generation);
    request_selected_details(results, selected, details, generation);
}

fn previous_page(
    results: State<Vec<GuiSearchResult>>,
    page: State<usize>,
    selected: State<usize>,
    status: State<String>,
    details: State<DetailState>,
) {
    let previous = page.get().saturating_sub(1);
    page.set(previous);
    selected.set(previous * PAGE_SIZE);
    let generation = current_generation();
    request_visible_thumbnails(results.clone(), previous, status, generation);
    request_selected_details(results, selected, details, generation);
}

fn page_count(len: usize) -> usize {
    len.div_ceil(PAGE_SIZE).max(1)
}

fn load_search_page(mut cursor: YoutubeSearchCursor, limit: usize) -> SearchLoadOutcome {
    match cursor.next_page(limit) {
        Ok(page) => Ok(SearchLoadResult {
            results: page.results.into_iter().map(gui_search_result).collect(),
            has_more: page.has_more,
            cursor,
        }),
        Err(message) => Err(SearchLoadError {
            message,
            cursor: Some(cursor),
        }),
    }
}

fn load_more_search_results(
    status: State<String>,
    has_more: State<bool>,
    loading_more: State<bool>,
    target_page: Option<usize>,
) {
    if loading_more.get() || !has_more.get() {
        return;
    }
    let generation = current_generation();
    let cursor = {
        let mut cursor = YT_GUI_SEARCH_CURSOR.lock();
        cursor.take()
    };
    let Some(cursor) = cursor else {
        status.set(String::from("Search results are still loading."));
        return;
    };
    loading_more.set(true);
    status.set(String::from("Loading more search results."));
    thread::spawn(move || {
        let result = load_search_page(cursor, PAGE_SIZE);
        push_gui_message(GuiMessage::SearchMoreFinished {
            generation,
            target_page,
            result,
        });
    });
}

fn gui_search_result(result: scarlet_youtube::YoutubeSearchResult) -> GuiSearchResult {
    GuiSearchResult {
        video_id: result.video_id,
        title: result.title,
        channel: result.channel,
        duration: result.duration,
        thumbnail: ThumbnailState::NotRequested,
        thumbnail_url: result.thumbnail_url,
    }
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
    generation: u64,
) {
    {
        let mut active = YT_GUI_THUMBNAIL_ACTIVE.lock();
        if *active {
            return;
        }
        *active = true;
    }

    let mut list = results.get();
    let start = page.saturating_mul(PAGE_SIZE);
    let end = start.saturating_add(PAGE_SIZE).min(list.len());
    let mut requests = Vec::new();

    for result in list.iter_mut().take(end).skip(start) {
        if result.thumbnail.is_not_requested() {
            if result.thumbnail_url.is_some() {
                result.thumbnail = ThumbnailState::Loading;
                requests.push(result.video_id.clone());
            } else {
                result.thumbnail = ThumbnailState::Failed;
            }
        }
    }

    if requests.is_empty() {
        *YT_GUI_THUMBNAIL_ACTIVE.lock() = false;
        return;
    }

    results.set(list);
    status.set(format!("Loading thumbnails for page {}.", page + 1));

    thread::spawn(move || {
        for video_id in requests {
            if generation != current_generation() {
                break;
            }
            let thumbnail = match download_thumbnail_image(&video_id) {
                Ok(image) => ThumbnailState::Ready(image),
                Err(error) => {
                    println!("[yt-gui] thumbnail {} failed: {}", video_id, error);
                    ThumbnailState::Failed
                }
            };
            push_gui_message(GuiMessage::ThumbnailFinished {
                generation,
                video_id,
                thumbnail,
            });
        }
        push_gui_message(GuiMessage::ThumbnailBatchFinished { generation });
    });
}

fn request_selected_details(
    results: State<Vec<GuiSearchResult>>,
    selected: State<usize>,
    details: State<DetailState>,
    generation: u64,
) {
    let list = results.get();
    let Some(row) = list.get(selected.get()) else {
        if !matches!(details.get(), DetailState::Empty) {
            details.set(DetailState::Empty);
        }
        return;
    };
    let video_id = row.video_id.clone();
    if detail_state_matches_video(&details.get(), &video_id) {
        return;
    }
    {
        let mut active = YT_GUI_DETAILS_ACTIVE.lock();
        if *active {
            return;
        }
        *active = true;
    }

    details.set(DetailState::Loading {
        video_id: video_id.clone(),
    });
    thread::spawn(move || {
        let result = if generation == current_generation() {
            youtube_video_details(&video_id).map(GuiVideoDetails::from_youtube)
        } else {
            Err(String::from("stale details request"))
        };
        push_gui_message(GuiMessage::DetailsFinished {
            generation,
            video_id,
            result,
        });
    });
}

fn detail_state_matches_video(state: &DetailState, video_id: &str) -> bool {
    match state {
        DetailState::Loading { video_id: current }
        | DetailState::Failed {
            video_id: current, ..
        } => current == video_id,
        DetailState::Ready(details) => details.video_id == video_id,
        DetailState::Empty => false,
    }
}

fn download_thumbnail_image(video_id: &str) -> core::result::Result<BitmapImage, String> {
    let bytes = fetch_youtube_thumbnail_bytes(video_id)?;
    BitmapImage::from_jpeg_bytes(&bytes).ok_or_else(|| String::from("thumbnail JPEG decode failed"))
}

fn next_generation() -> u64 {
    let mut generation = YT_GUI_GENERATION.lock();
    *generation = generation.saturating_add(1);
    *generation
}

fn current_generation() -> u64 {
    *YT_GUI_GENERATION.lock()
}

fn push_gui_message(message: GuiMessage) {
    YT_GUI_MESSAGES.lock().push(message);
}

fn pop_gui_message() -> Option<GuiMessage> {
    let mut messages = YT_GUI_MESSAGES.lock();
    if messages.is_empty() {
        None
    } else {
        Some(messages.remove(0))
    }
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

fn wrap_description_text(input: &str) -> String {
    let mut lines = Vec::new();
    let mut truncated = false;
    let mut consumed_chars = 0usize;

    for paragraph in input.replace('\r', "").split('\n') {
        if consumed_chars >= DETAIL_DESCRIPTION_MAX_CHARS {
            truncated = true;
            break;
        }
        if lines.len() >= DETAIL_DESCRIPTION_MAX_LINES {
            truncated = true;
            break;
        }

        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            continue;
        }

        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if consumed_chars >= DETAIL_DESCRIPTION_MAX_CHARS
                || lines.len() >= DETAIL_DESCRIPTION_MAX_LINES
            {
                truncated = true;
                break;
            }

            let remaining = DETAIL_DESCRIPTION_MAX_CHARS - consumed_chars;
            let word = compact_text(word, remaining);
            consumed_chars = consumed_chars.saturating_add(word.chars().count());

            let candidate = if current.is_empty() {
                word.clone()
            } else {
                format!("{} {}", current, word)
            };

            if text_fits_detail_width(&candidate) {
                current = candidate;
                continue;
            }

            if !current.is_empty() {
                lines.push(current);
                current = String::new();
            }
            push_wrapped_word(&mut lines, &mut current, &word);
        }

        if !current.is_empty() && lines.len() < DETAIL_DESCRIPTION_MAX_LINES {
            lines.push(current);
        }
    }

    if lines.len() > DETAIL_DESCRIPTION_MAX_LINES {
        lines.truncate(DETAIL_DESCRIPTION_MAX_LINES);
        truncated = true;
    }

    if truncated {
        if let Some(last) = lines.last_mut() {
            append_ellipsis(last);
        } else {
            lines.push(String::from("..."));
        }
    }

    join_lines(&lines)
}

fn push_wrapped_word(lines: &mut Vec<String>, current: &mut String, word: &str) {
    for ch in word.chars() {
        if lines.len() >= DETAIL_DESCRIPTION_MAX_LINES {
            return;
        }
        let mut candidate = current.clone();
        candidate.push(ch);
        if current.is_empty() || text_fits_detail_width(&candidate) {
            current.push(ch);
        } else {
            lines.push(core::mem::take(current));
            current.push(ch);
        }
    }
}

fn text_fits_detail_width(text: &str) -> bool {
    let (width, _) = scarlet_ui::measure_text_sized(text, DETAIL_DESCRIPTION_FONT_SIZE);
    width <= DETAIL_TEXT_WIDTH
}

fn append_ellipsis(text: &mut String) {
    while !text.is_empty() {
        let candidate = format!("{}...", text);
        if text_fits_detail_width(&candidate) {
            text.push_str("...");
            return;
        }
        text.pop();
    }
    text.push_str("...");
}

fn join_lines(lines: &[String]) -> String {
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

fn detail_pane_text(
    state: DetailState,
    selected_video_id: &str,
    base_title: String,
    base_channel: String,
) -> (String, String, String) {
    match state {
        DetailState::Ready(details) if details.video_id == selected_video_id => {
            let title = details
                .title
                .as_deref()
                .map(|title| compact_text(title, 90))
                .unwrap_or(base_title);
            let channel = details
                .author
                .as_deref()
                .map(|author| compact_text(author, 46))
                .unwrap_or(base_channel);
            let description = details
                .description
                .as_deref()
                .map(str::trim)
                .filter(|description| !description.is_empty())
                .map(|description| wrap_description_text(description))
                .unwrap_or_else(|| String::from("No description."));
            let _thumbnail_url = details.thumbnail_url.as_deref();
            (title, channel, description)
        }
        DetailState::Loading { video_id } if video_id == selected_video_id => {
            (base_title, base_channel, String::from("Loading details..."))
        }
        DetailState::Failed { video_id, message } if video_id == selected_video_id => (
            base_title,
            base_channel,
            format!("Details failed: {}", compact_text(&message, 180)),
        ),
        _ if selected_video_id == "-" => (
            base_title,
            base_channel,
            String::from("Search and select a result."),
        ),
        _ => (base_title, base_channel, String::from("Loading details...")),
    }
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
