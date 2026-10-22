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

fn fit_label(mut label: String, width: u32, size: f32) -> String {
    if scarlet_ui::graphics::measure_text_sized(&label, size).0 <= width {
        return label;
    }
    while !label.is_empty() {
        label.pop();
        let shortened = format!("{label}…");
        if scarlet_ui::graphics::measure_text_sized(&shortened, size).0 <= width {
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
    row_width: State<u32>,
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
        let geometry = self.row_width.clone();
        let row_width = self.row_width.get().max(480);
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
            Either::A(vstack! {
                Spacer::new(),
                Text::new(if journal.loading { "Loading logs…" } else if journal.error.is_some() { "Journal unavailable" }
                    else if journal.entries.is_empty() { "No records to show" } else { "No matching records" }).font_size(19.0),
                Text::new(if journal.error.is_some() { "Live system logs require a running Scarlet journal." }
                    else if preview && journal.entries.is_empty() { "Sample records cleared. Reopen the preview to reload." }
                    else if journal.entries.is_empty() { "New records appear here while Follow is enabled." }
                    else { "Try a different search or choose All priorities." }).font_size(12.0).color(palette.text_secondary()),
                Spacer::new(),
            }.spacing(10.0).frame(f32::INFINITY, f32::INFINITY))
        } else {
            Either::B(
                ListView::new(
                    self.visible.clone(),
                    self.selected_row.clone(),
                    64.0,
                    move |index, entry: Entry, selected| {
                        let selection = rows_app.selected.clone();
                        let selected_row = rows_app.selected_row.clone();
                        let palette = ColorPalette::default();
                        let is_selected = selected == Some(index);
                        let row = entry.clone();
                        let choose = move || {
                            selection.set(Some(entry.clone()));
                            selected_row.set(Some(index));
                        };
                        let message = fit_label(row.message.split_whitespace().collect::<Vec<_>>().join(" "), row_width.saturating_sub(104), 12.0);
                        hstack! {
                            vstack! {
                                hstack! {
                                    Text::new(fit_label(format!("{} · PID {}", row.unit, row.pid), row_width.saturating_sub(255), 13.0)).font_size(13.0),
                                    Spacer::new(),
                                    Text::new(row.priority_name).font_size(11.0).color(if row.priority <= 3 { palette.error() } else { palette.text_secondary() }),
                                    Text::new(format!("{}.{:03}s", row.monotonic_ns / 1_000_000_000, row.monotonic_ns / 1_000_000 % 1000)).font_size(11.0).color(palette.text_secondary()),
                                }.spacing(12.0).frame_width(f32::INFINITY),
                                Text::new(message).font_size(12.0).color(palette.text_secondary()),
                            }.spacing(5.0).alignment(Alignment::Leading).frame_width(f32::INFINITY),
                            Button::new(if is_selected { "Open" } else { "View" }).font_size(12.0)
                                .background_color(if is_selected { palette.primary() } else { palette.surface_variant() })
                                .text_color(if is_selected { palette.text_inverse() } else { palette.text() })
                                .on_click(choose.clone()).frame(54.0, 30.0),
                        }.spacing(12.0).padding(10.0).frame(f32::INFINITY, 60.0)
                            .background(if is_selected { palette.surface_variant() } else { palette.background() })
                            .on_click(choose)
                    },
                )
                .item_key(|entry| format!("{}:{}", entry.boot_id, entry.sequence).into())
                .frame(f32::INFINITY, f32::INFINITY)
                .on_geometry_change(|layout| layout.size().width.max(0.0) as u32, move |width| geometry.set(width)),
            )
        };
        let detail = if let Some(entry) = self.selected.get() {
            Either::A(vstack! {
                Text::new("RECORD DETAILS").font_size(10.0).color(palette.text_secondary()),
                ScrollView::new(Text::new(entry.details()).font_size(13.0)).both_axes().frame(f32::INFINITY, 124.0),
            }.spacing(8.0).alignment(Alignment::Leading).padding(12.0).frame_width(f32::INFINITY).background(palette.surface_variant()))
        } else {
            Either::B(
                Text::new("Choose a record to read its full message.")
                    .font_size(12.0)
                    .color(palette.text_secondary())
                    .alignment(Alignment::Leading)
                    .padding(12.0)
                    .frame(f32::INFINITY, 42.0)
                    .background(palette.surface_variant()),
            )
        };
        vstack! {
            hstack! {
                vstack! {
                    Text::new("System Logs").font_size(22.0),
                    Text::new(if preview { "SAMPLE PREVIEW" } else { "CURRENT BOOT" }).font_size(10.0).color(palette.text_secondary()),
                }.spacing(2.0).alignment(Alignment::Leading),
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
            Text::new(status).font_size(12.0).color(if journal.error.is_some() { palette.error() } else { palette.text_secondary() })
                .alignment(Alignment::Leading).frame(f32::INFINITY, 20.0),
            Divider::new(),
            list,
            detail,
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
        row_width: State::new(StateId::new(10), 600),
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
