//! Read-only desktop viewer for the current, volatile logd journal.

use std::process::ExitCode;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use scarlet_ui::prelude::*;
use scarlet_ui::state::Listenable;
use scarlet_ui::views::Either;
use scarlet_ui::{Event, KeyCode, ListView, ScrollView, hstack, vstack};
use scarlet_ui_macros::View;

#[path = "log_viewer/backend.rs"]
mod backend;
#[path = "log_viewer/model.rs"]
mod model;
use model::{Entry, Journal, MAX_RECORDS};

fn summary_label(entry: &Entry) -> String {
    // Keep the row readable at the minimum window width. Full original lines
    // remain available in the horizontally scrollable detail pane.
    let mut label = entry.summary();
    const MAX_WIDTH: u32 = 592;
    if scarlet_ui::graphics::measure_text_sized(&label, 12.0).0 <= MAX_WIDTH {
        return label;
    }
    while !label.is_empty() {
        label.pop();
        let shortened = format!("{label}…");
        if scarlet_ui::graphics::measure_text_sized(&shortened, 12.0).0 <= MAX_WIDTH {
            return shortened;
        }
    }
    "…".into()
}

#[derive(View, Clone)]
struct LogViewer {
    journal: State<Journal>,
    visible: State<Vec<Entry>>,
    filter: State<String>,
    priority: State<usize>,
    follow: State<bool>,
    refresh: State<u64>,
    selected: State<Option<Entry>>,
    selected_row: State<Option<usize>>,
    focused: State<bool>,
    preview: State<bool>,
}

impl LogViewer {
    fn rebuild_rows(&self) {
        let rows = self
            .journal
            .get()
            .visible(&self.filter.get(), self.priority.get());
        let selected = self.selected.get();
        let index = selected.as_ref().and_then(|selected| {
            rows.iter().position(|entry| {
                entry.sequence == selected.sequence && entry.boot_id == selected.boot_id
            })
        });
        self.visible.set(rows);
        self.selected_row.set(index);
        if index.is_none() {
            self.selected.set(None);
        }
    }

    fn clear_display(&self) {
        self.journal.update(Journal::clear_display);
        self.selected.set(None);
        self.selected_row.set(None);
    }

    fn refresh_now(&self) {
        if !self.preview.get() {
            self.refresh
                .update(|request| *request = request.wrapping_add(1));
        }
    }

    fn handle_key(&self, event: KeyEvent) -> bool {
        let KeyEvent::Pressed { keycode, modifiers } = event else {
            return false;
        };
        if !modifiers.control || modifiers.alt || modifiers.super_key {
            return false;
        }
        match keycode {
            KeyCode::Space if !self.preview.get() => {
                self.follow.update(|follow| *follow = !*follow)
            }
            KeyCode::Char('r' | 'R') => self.refresh_now(),
            KeyCode::Char('l' | 'L') => self.clear_display(),
            _ => return false,
        }
        true
    }

    fn content(&self) -> impl View + Clone + use<> {
        let palette = ColorPalette::default();
        let journal = self.journal.get();
        let count = self.visible.get().len();
        let preview = self.preview.get();
        let refreshing = self.clone();
        let clearing = self.clone();
        let rows_app = self.clone();
        let keys = self.clone();
        let status = if preview {
            "Static sample data — not system logs".to_owned()
        } else if let Some(error) = &journal.error {
            error.clone()
        } else if journal.loading {
            "Connecting to logd…".to_owned()
        } else if journal.restarted {
            "Journal restarted; loading its recent records…".to_owned()
        } else if self.follow.get() {
            "Following the current boot journal".to_owned()
        } else {
            "Paused — Refresh fetches once; Follow resumes updates".to_owned()
        };
        let list = if count == 0 {
            Either::A(
                Text::new(if journal.loading {
                    "Loading logs…"
                } else {
                    "No matching records in this window."
                })
                .color(palette.text_secondary())
                .alignment(Alignment::Center)
                .frame(f32::INFINITY, f32::INFINITY),
            )
        } else {
            Either::B(
                ListView::new(
                    self.visible.clone(),
                    self.selected_row.clone(),
                    36.0,
                    move |index, entry: Entry, selected| {
                        let selection = rows_app.selected.clone();
                        let selected_row = rows_app.selected_row.clone();
                        let palette = ColorPalette::default();
                        Button::new(summary_label(&entry))
                            .font_size(12.0)
                            .background_color(if selected == Some(index) {
                                palette.primary()
                            } else {
                                palette.background()
                            })
                            .text_color(if selected == Some(index) {
                                palette.text_inverse()
                            } else if entry.priority <= 3 {
                                palette.error()
                            } else {
                                palette.text()
                            })
                            .on_click(move || {
                                selection.set(Some(entry.clone()));
                                selected_row.set(Some(index));
                            })
                            .frame(f32::INFINITY, 32.0)
                    },
                )
                .item_key(|entry| format!("{}:{}", entry.boot_id, entry.sequence).into())
                .frame(f32::INFINITY, f32::INFINITY),
            )
        };
        let detail = self.selected.get().map_or_else(
            || "Select a record to inspect its message.".into(),
            |entry| entry.details(),
        );
        vstack! {
            hstack! {
                Text::new("System Logs").font_size(22.0),
                Spacer::new(),
                Text::new("Follow").font_size(13.0),
                Toggle::new(self.follow.clone()).enabled(!preview),
                Button::new("Refresh").enabled(!preview).on_click(move || refreshing.refresh_now()),
                Button::new("Clear display").on_click(move || clearing.clear_display()),
            }.spacing(10.0),
            hstack! {
                TextField::new(self.filter.clone()).placeholder("Filter unit, PID or message").frame_width(f32::INFINITY),
                Select::new(vec!["All priorities".into(), "Warnings + errors".into(), "Errors".into()], self.priority.clone()).width(180.0),
            }.spacing(12.0),
            Text::new(status).font_size(12.0).color(if journal.error.is_some() { palette.error() } else { palette.text_secondary() }),
            list,
            Divider::new(),
            ScrollView::new(Text::new(detail).font_size(13.0).padding(8.0)).both_axes()
                .frame(f32::INFINITY, 145.0),
            Text::new(format!("{} matching / {} retained · latest {} records · newest first", count, journal.entries.len(), MAX_RECORDS))
                .font_size(11.0).color(palette.text_secondary()),
            Text::new("Ctrl+Space: follow/pause   Ctrl+R: refresh   Ctrl+L: clear this window")
                .font_size(11.0).color(palette.text_secondary()),
        }.spacing(10.0).padding(16.0).frame(f32::INFINITY, f32::INFINITY)
            .focusable(self.focused.clone())
            .on_event_capture(move |event| match event { Event::Keyboard(key) => keys.handle_key(*key), _ => false })
    }
}

impl Application for LogViewer {
    fn scenes(&self) -> impl Scene {
        Window::new(
            if self.preview.get() {
                "System Logs — Sample Preview"
            } else {
                "System Logs"
            },
            self.content(),
        )
        .app_id("org.scarlet-os.desktop.log-viewer")
        .background_color(ColorPalette::default().background())
        .size(Size::new(820.0, 620.0))
        .min_size(Size::new(640.0, 520.0))
    }
    fn debug_logging(&self) -> bool {
        false
    }
}

fn start_updates(app: &LogViewer) {
    for state in [&app.journal as &dyn Listenable, &app.filter, &app.priority] {
        let app = app.clone();
        state.subscribe_any(Arc::new(move || app.rebuild_rows()));
    }
    app.rebuild_rows();
    if app.preview.get() {
        return;
    }
    let (wake, changes) = mpsc::sync_channel(1);
    for state in [&app.follow as &dyn Listenable, &app.refresh] {
        let wake = wake.clone();
        state.subscribe_any(Arc::new(move || {
            let _ = wake.try_send(());
        }));
    }
    let journal = app.journal.clone();
    let follow = app.follow.clone();
    let refresh = app.refresh.clone();
    thread::spawn(move || {
        let mut first = true;
        let mut last_request = 0;
        loop {
            let generation = refresh.get();
            let following = follow.get();
            if !first && !following && generation == last_request {
                if changes.recv().is_err() {
                    break;
                }
                continue;
            }
            first = false;
            last_request = generation;
            let cancelled = || follow.get() != following || refresh.get() != generation;
            let result = backend::query(journal.get().after_sequence, &cancelled);
            if cancelled() {
                continue;
            }
            let failed = result.is_err();
            match result {
                Ok(snapshot) => {
                    let current = journal.get();
                    if !snapshot.entries.is_empty()
                        || current.boot_id != Some(snapshot.end.boot_id)
                        || current.after_sequence != snapshot.end.last_sequence
                        || current.error.is_some()
                        || current.loading
                        || current.restarted
                    {
                        journal.update(|journal| journal.accept(snapshot.entries, snapshot.end));
                    }
                }
                Err(error) => {
                    if journal.get().error.as_ref() != Some(&error) {
                        journal.update(|journal| {
                            journal.error = Some(error);
                            journal.loading = false;
                        });
                    }
                }
            }
            if following {
                let _ = changes.recv_timeout(Duration::from_secs(if failed { 3 } else { 1 }));
            }
        }
    });
}

#[cfg(not(target_os = "scarlet"))]
fn sample_journal() -> Journal {
    use log_protocol::{LogPriority, LogRecord, LogStream, QueryEnd};
    let messages = [
        (
            "stemd",
            LogPriority::Info,
            "Sample: desktop services are ready.",
        ),
        (
            "sws",
            LogPriority::Info,
            "Sample: display connected at 1920 x 1080.",
        ),
        (
            "example.service",
            LogPriority::Warning,
            "Sample: retrying an unavailable resource.",
        ),
        (
            "example.service",
            LogPriority::Error,
            "Sample: operation failed.\nThis is a fixture for inspecting the details pane, not a real system error.",
        ),
        ("clock", LogPriority::Info, "Sample: utility window opened."),
    ];
    let entries = messages
        .iter()
        .enumerate()
        .map(|(index, (unit, priority, message))| {
            Entry::from_record(LogRecord {
                sequence: index as u64 + 1,
                boot_id: 1,
                monotonic_ns: (index as u64 + 10) * 1_000_000_000,
                realtime_ns: u64::MAX,
                unit: (*unit).into(),
                pid: index as i32 + 100,
                stream: LogStream::Internal,
                priority: *priority,
                message: message.as_bytes().to_vec(),
            })
        })
        .collect();
    let mut journal = Journal::default();
    journal.accept(
        entries,
        QueryEnd {
            last_sequence: messages.len() as u64,
            boot_id: 1,
        },
    );
    journal
}

fn main() -> ExitCode {
    #[cfg(not(target_os = "scarlet"))]
    let preview = std::env::args()
        .skip(1)
        .any(|argument| argument == "--preview");
    #[cfg(target_os = "scarlet")]
    let preview = false;
    let journal = Journal {
        loading: true,
        ..Journal::default()
    };
    #[cfg(not(target_os = "scarlet"))]
    let journal = if preview { sample_journal() } else { journal };
    let mut app = LogViewer {
        journal: State::new(StateId::new(0), journal),
        visible: State::new(StateId::new(1), Vec::new()),
        filter: State::new(StateId::new(2), String::new()),
        priority: State::new(StateId::new(3), 0),
        follow: State::new(StateId::new(4), !preview),
        refresh: State::new(StateId::new(5), 0),
        selected: State::new(StateId::new(6), None),
        selected_row: State::new(StateId::new(7), None),
        focused: State::new(StateId::new(8), true),
        preview: State::new(StateId::new(9), preview),
    };
    start_updates(&app);
    match app.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[log-viewer] {error}");
            ExitCode::FAILURE
        }
    }
}
