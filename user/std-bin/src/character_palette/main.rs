//! Searchable Unicode palette with real asynchronous desktop clipboard access.
mod catalog;
use catalog::{CATEGORIES, Entry, codepoints, search};
use scarlet_ui::clipboard::{ClipboardHandle, TextClipboard};
use scarlet_ui::prelude::*;
use scarlet_ui::views::Either;
use scarlet_ui::{
    Event, GridView, KeyCode, KeyEvent, PlatformWindow, Select, WindowContext, hstack, vstack,
};
use scarlet_ui_macros::View;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Runtime {
    clipboard: Option<TextClipboard>,
    pending: Option<(Entry, ClipboardHandle<()>)>,
    filter_key: Option<(String, usize)>,
}
#[derive(View, Clone)]
struct CharacterPalette {
    query: State<String>,
    category: State<usize>,
    entries: State<Vec<Entry>>,
    selected: State<Option<usize>>,
    status: State<String>,
    copying: State<bool>,
    columns: State<usize>,
    visible_rows: State<usize>,
    recent: State<Vec<Entry>>,
    recent_selection: State<Option<usize>>,
    detail_width: State<u32>,
    runtime: State<Arc<Mutex<Runtime>>>,
}
impl CharacterPalette {
    fn new() -> Self {
        let app = Self::default();
        app.columns.set(8);
        app.visible_rows.set(3);
        app.refilter();
        app
    }
    fn refilter(&self) {
        let key = (self.query.get(), self.category.get());
        let entries = search(&key.0, key.1);
        self.runtime.get().lock().unwrap().filter_key = Some(key);
        self.selected
            .set(if entries.is_empty() { None } else { Some(0) });
        self.entries.set(entries);
    }
    fn selection(&self) -> Option<Entry> {
        self.selected
            .get()
            .and_then(|index| self.entries.get().get(index).cloned())
    }
    fn copy(&self, entry: Entry) {
        let runtime = self.runtime.get();
        let mut runtime = runtime.lock().unwrap();
        if runtime.pending.is_some() {
            return;
        }
        let Some(clipboard) = runtime.clipboard.as_ref() else {
            self.status
                .set("Desktop clipboard is unavailable on this backend".into());
            return;
        };
        let task = clipboard.write_text(entry.text.clone());
        runtime.pending = Some((entry, task));
        self.copying.set(true);
        self.status.set("Copying…".into());
    }
    fn copy_selection(&self) {
        if let Some(entry) = self.selection() {
            self.copy(entry);
        }
    }
    fn grid_key(&self, event: &Event) -> bool {
        let Event::Keyboard(KeyEvent::Pressed { keycode, modifiers }) = event else {
            return false;
        };
        if modifiers.control || modifiers.alt || modifiers.super_key {
            return false;
        }
        let count = self.entries.get().len();
        if count == 0 {
            return false;
        }
        let current = self.selected.get().unwrap_or(0).min(count - 1);
        let columns = self.columns.get().max(1);
        let page = columns.saturating_mul(self.visible_rows.get().max(1));
        let next = match keycode {
            KeyCode::Left => current.saturating_sub(1),
            KeyCode::Right => current.saturating_add(1),
            KeyCode::Up => current.saturating_sub(columns),
            KeyCode::Down => current.saturating_add(columns),
            KeyCode::PageUp => current.saturating_sub(page),
            KeyCode::PageDown => current.saturating_add(page),
            KeyCode::Home => 0,
            KeyCode::End => count - 1,
            KeyCode::Enter => {
                self.copy_selection();
                return true;
            }
            _ => return false,
        };
        self.selected.set(Some(next.min(count - 1)));
        true
    }
    fn poll(&self) {
        let key = (self.query.get(), self.category.get());
        if self.runtime.get().lock().unwrap().filter_key.as_ref() != Some(&key) {
            self.refilter();
        }
        let runtime = self.runtime.get();
        let completed = {
            let mut runtime = runtime.lock().unwrap();
            let result = runtime
                .pending
                .as_ref()
                .and_then(|(_, task)| task.try_take());
            result.map(|result| (runtime.pending.take().unwrap().0, result))
        };
        if let Some((entry, result)) = completed {
            self.copying.set(false);
            match result {
                Ok(()) => {
                    self.status
                        .set(format!("Copied {} to the desktop clipboard", entry.name));
                    self.recent.update(|recent| {
                        recent.retain(|old| old.text != entry.text);
                        recent.insert(0, entry);
                        recent.truncate(8);
                    });
                }
                Err(error) => self.status.set(error.to_string()),
            }
        }
    }
}
impl Application for CharacterPalette {
    fn scenes(&self) -> impl Scene {
        let colors = ColorPalette::default();
        let cell_colors = colors.clone();
        let category = self.clone();
        let cells = self.clone();
        let copy = self.clone();
        let keys = self.clone();
        let grid_keys = self.clone();
        let geometry = self.clone();
        let selected = self.selection();
        let detail_width = self.detail_width.clone();
        let details = selected
            .as_ref()
            .map(|entry| {
                format!(
                    "{}\n{}",
                    wrap_name(&entry.name, self.detail_width.get().max(160)),
                    codepoints(&entry.text)
                )
            })
            .unwrap_or_else(|| "Choose a character to copy".into());
        let recent_app = self.clone();
        let reset_search = self.clone();
        let grid = if self.entries.get().is_empty() {
            Either::A(vstack! {
                Spacer::new(),
                Text::new("No characters found").font_size(19.0),
                Text::new("Try a name, a symbol or a code such as U+263A.").font_size(12.0).color(colors.text_secondary()),
                Button::new("Clear filters").on_click(move || {
                    reset_search.query.set(String::new()); reset_search.category.set(0); reset_search.refilter();
                }),
                Spacer::new(),
            }.spacing(12.0).frame(f32::INFINITY, f32::INFINITY))
        } else {
            Either::B(GridView::new(self.entries.clone(), self.selected.clone(), 8, 86.0, move |index, entry, selected| {
                let app = cells.clone();
                let caption = format!("U+{:04X}{}", entry.text.chars().next().unwrap_or(' ') as u32, if entry.text.chars().count() > 1 { "+" } else { "" });
                vstack! {
                    Button::new(entry.text).font_size(30.0).background_color(if selected == Some(index) { cell_colors.surface_variant() } else { cell_colors.surface() })
                        .on_click(move || app.selected.set(Some(index))).frame(f32::INFINITY, 58.0),
                    Text::new(caption).font_size(11.0).color(cell_colors.text_secondary()).frame_height(16.0),
                }.frame(f32::INFINITY, 76.0)
            }).spacing(8.0).minimum_cell_width(70.0).reveal_selection(true)
                .on_geometry_change(|geometry| (((geometry.size().width + 8.0) / 78.0).floor().clamp(1.0, 8.0) as usize, (geometry.size().height / 86.0).floor().max(1.0) as usize),
                    move |(columns, rows)| { geometry.columns.set(columns); geometry.visible_rows.set(rows); })
                .on_event_capture(move |event| grid_keys.grid_key(event)).frame(f32::INFINITY, f32::INFINITY))
        };
        let recent = if self.recent.get().is_empty() {
            Either::A(Spacer::new().frame_height(0.0))
        } else {
            Either::B(vstack! {
                Text::new("RECENT COPIES").font_size(10.0).color(colors.text_secondary()),
                GridView::new(self.recent.clone(), self.recent_selection.clone(), 8, 38.0, move |_, entry, _| {
                    let app = recent_app.clone();
                    Button::new(entry.text.clone()).font_size(22.0).on_click(move || app.copy(entry.clone())).frame(f32::INFINITY, 36.0)
                }).spacing(6.0).frame_height(38.0),
            }.spacing(6.0).alignment(Alignment::Leading))
        };
        WindowGroup::new("main", Window::new("Characters", vstack! {
            hstack! {
                Text::new("Characters").font_size(22.0),
                Spacer::new(),
                Text::new(format!("{} matches", self.entries.get().len())).font_size(12.0).color(colors.text_secondary()),
            }.frame_width(f32::INFINITY),
            TextField::new(self.query.clone()).placeholder("Search names, symbols or U+1F600").autofocus(true)
                .frame(f32::INFINITY, 38.0),
            hstack! {
                Select::new(CATEGORIES.iter().map(|name| (*name).into()).collect(), self.category.clone()).width(240.0).on_change(move |_| category.refilter()),
                Spacer::new(), Text::new("Select to preview").font_size(11.0).color(colors.text_secondary()),
            }.spacing(12.0),
            Divider::new(),
            grid,
            hstack! {
                Text::new(selected.as_ref().map(|entry| entry.text.clone()).unwrap_or_default()).font_size(36.0).frame(52.0, 64.0),
                Text::new(details).font_size(13.0).alignment(Alignment::Leading).frame(f32::INFINITY, 64.0)
                    .on_geometry_change(|layout| layout.size().width.max(0.0) as u32, move |width| detail_width.set(width)),
                Button::new(if self.copying.get() { "Copying…" } else { "Copy" }).font_size(13.0)
                    .background_color(if selected.is_some() && !self.copying.get() { colors.primary() } else { colors.button_background() })
                    .text_color(if selected.is_some() && !self.copying.get() { colors.text_inverse() } else { colors.text_secondary() })
                    .enabled(selected.is_some() && !self.copying.get()).on_click(move || copy.copy_selection()).frame(80.0, 36.0),
            }.spacing(12.0).padding(12.0).frame(f32::INFINITY, 88.0).background(colors.surface_variant()),
            recent,
            Text::new(if self.status.get().is_empty() { "Enter  copy selection    Ctrl+C  copy".into() } else { self.status.get() })
                .font_size(11.0).color(colors.text_secondary()).alignment(Alignment::Leading).frame(f32::INFINITY, 26.0),
        }.spacing(10.0).padding(18.0).frame(f32::INFINITY, f32::INFINITY)
        .on_event_capture(move |event| match event {
            Event::Keyboard(KeyEvent::Pressed { keycode: KeyCode::Char('c' | 'C'), modifiers }) if modifiers.control || modifiers.super_key => { keys.copy_selection(); true }
            _ => false,
        })).app_id("org.scarlet-os.desktop.characters").size(Size::new(700.0, 720.0)).min_size(Size::new(480.0, 580.0)).resizable(true))
    }
    fn on_window_created(&mut self, _: &WindowContext, window: &mut dyn PlatformWindow) {
        match window.text_clipboard() {
            Ok(clipboard) => self.runtime.get().lock().unwrap().clipboard = Some(clipboard),
            Err(error) => self.status.set(error.to_string()),
        }
    }
    fn on_idle(&mut self) {
        self.poll();
    }
    fn debug_logging(&self) -> bool {
        false
    }
}
fn wrap_name(name: &str, width: u32) -> String {
    let mut lines = String::new();
    let mut line = String::new();
    for word in name.split_whitespace() {
        let candidate = if line.is_empty() {
            word.into()
        } else {
            format!("{line} {word}")
        };
        if !line.is_empty() && scarlet_ui::graphics::measure_text_sized(&candidate, 13.0).0 > width
        {
            if !lines.is_empty() {
                lines.push('\n');
            }
            lines.push_str(&line);
            line = word.into();
        } else {
            line = candidate;
        }
    }
    if !lines.is_empty() {
        lines.push('\n');
    }
    lines.push_str(&line);
    lines
}
fn main() {
    if let Err(error) = CharacterPalette::new().run() {
        eprintln!("Characters: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scarlet_ui::clipboard::{ClipboardError, ClipboardProvider};
    use std::sync::Barrier;

    fn finish_copy(app: &CharacterPalette) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.copying.get() {
            app.poll();
            assert!(
                std::time::Instant::now() < deadline,
                "clipboard worker timed out"
            );
            std::thread::yield_now();
        }
    }

    struct DelayedClipboard {
        gate: Arc<Barrier>,
        writes: Arc<Mutex<Vec<String>>>,
    }
    impl ClipboardProvider for DelayedClipboard {
        fn read_text(&self) -> std::result::Result<Option<String>, ClipboardError> {
            Ok(None)
        }
        fn write_text(&self, text: &str) -> std::result::Result<(), ClipboardError> {
            self.gate.wait();
            self.writes.lock().unwrap().push(text.into());
            Ok(())
        }
    }

    #[test]
    fn pending_copy_preserves_requested_sequence_and_records_only_success() {
        let app = CharacterPalette::new();
        let gate = Arc::new(Barrier::new(2));
        let writes = Arc::new(Mutex::new(Vec::new()));
        app.runtime.get().lock().unwrap().clipboard = Some(TextClipboard::new(DelayedClipboard {
            gate: gate.clone(),
            writes: writes.clone(),
        }));
        let requested = search("woman technologist", 0).remove(0);
        app.copy(requested.clone());
        app.query.set("U+3042".into());
        app.poll();
        app.copy_selection();
        assert!(app.copying.get());
        assert!(app.recent.get().is_empty());
        gate.wait();
        finish_copy(&app);
        assert_eq!(*writes.lock().unwrap(), ["👩‍💻"]);
        assert_eq!(app.recent.get(), [requested]);
        assert_eq!(app.selection().unwrap().text, "あ");
        assert!(app.status.get().starts_with("Copied woman technologist"));
    }

    struct DeniedClipboard;
    impl ClipboardProvider for DeniedClipboard {
        fn read_text(&self) -> std::result::Result<Option<String>, ClipboardError> {
            Err(ClipboardError::AccessDenied)
        }
        fn write_text(&self, _: &str) -> std::result::Result<(), ClipboardError> {
            Err(ClipboardError::AccessDenied)
        }
    }

    #[test]
    fn denied_copy_preserves_recent_and_reports_error() {
        let app = CharacterPalette::new();
        let previous = search("U+3042", 0).remove(0);
        app.recent.set(vec![previous.clone()]);
        app.runtime.get().lock().unwrap().clipboard = Some(TextClipboard::new(DeniedClipboard));
        app.copy_selection();
        finish_copy(&app);
        assert_eq!(app.recent.get(), [previous]);
        assert_eq!(app.status.get(), ClipboardError::AccessDenied.to_string());
        assert!(app.runtime.get().lock().unwrap().pending.is_none());
    }

    #[test]
    fn keyboard_navigation_and_query_changes_keep_selection_valid() {
        let app = CharacterPalette::new();
        let event = |keycode| {
            Event::Keyboard(KeyEvent::Pressed {
                keycode,
                modifiers: Default::default(),
            })
        };
        app.columns.set(4);
        app.visible_rows.set(3);
        assert!(app.grid_key(&event(KeyCode::Down)));
        assert_eq!(app.selected.get(), Some(4));
        assert!(app.grid_key(&event(KeyCode::PageDown)));
        assert_eq!(app.selected.get(), Some(16));
        app.grid_key(&event(KeyCode::End));
        assert_eq!(app.selected.get(), Some(app.entries.get().len() - 1));
        app.grid_key(&event(KeyCode::Right));
        assert_eq!(app.selected.get(), Some(app.entries.get().len() - 1));
        app.query.set("U+3042".into());
        app.poll();
        assert_eq!(app.selection().unwrap().text, "あ");
        app.query.set("no such symbol qzqz".into());
        app.poll();
        assert!(app.selection().is_none());
        assert!(!app.grid_key(&event(KeyCode::Down)));
    }
}
