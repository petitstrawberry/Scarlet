//! Storage: a bounded, read-only browser of logical file sizes.
mod browser;
mod scan;
mod worker;

use browser::Browser;
use scan::{Kind, Snapshot, size};
use scarlet_ui::file_dialog::{FileDialog, FileDialogHandle, FileDialogMode, FileDialogOutcome};
use scarlet_ui::prelude::*;
use scarlet_ui::views::Either;
use scarlet_ui::{
    Event, KeyCode, ListView, MenuBarModel, MenuEntry, MenuItemModel, PlatformWindow,
    WindowContext, WindowId, hstack, vstack,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
enum Command {
    Choose,
    Refresh,
    Cancel,
    Back,
    OpenSelected,
    Enter(Arc<Snapshot>, usize),
}
#[derive(Default)]
struct Runtime {
    browser: Browser,
    owner: Option<WindowId>,
    dialog: Option<FileDialogHandle>,
    commands: VecDeque<Command>,
}
#[derive(Clone)]
struct Storage {
    runtime: Arc<Mutex<Runtime>>,
    worker: Arc<worker::Worker>,
    chrome: State<u64>,
    rows: State<Vec<usize>>,
    selected: State<Option<usize>>,
    focused: State<bool>,
    width: State<u32>,
}

fn shortcut(key: &str) -> String {
    format!(
        "{}+{key}",
        if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Ctrl"
        }
    )
}
fn fit(text: &str, width: u32) -> String {
    // Bound work even for filenames containing thousands of codepoints.
    let truncated = text.chars().nth(256).is_some();
    let mut text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(256)
        .collect();
    if !truncated && scarlet_ui::graphics::measure_text_sized(&text, 13.0).0 <= width {
        return text;
    }
    while !text.is_empty() {
        if scarlet_ui::graphics::measure_text_sized(&format!("{text}…"), 13.0).0 <= width {
            break;
        }
        text.pop();
    }
    format!("{text}…")
}

impl Storage {
    fn new() -> std::io::Result<Self> {
        Ok(Self {
            runtime: Arc::new(Mutex::new(Runtime::default())),
            worker: Arc::new(worker::Worker::new()?),
            chrome: State::new(StateId::new(0), 0),
            rows: State::new(StateId::new(1), Vec::new()),
            selected: State::new(StateId::new(2), None),
            focused: State::new(StateId::new(3), true),
            width: State::new(StateId::new(4), 760),
        })
    }
    fn changed(&self) {
        self.chrome.update(|n| *n = n.wrapping_add(1));
    }
    fn enqueue(&self, command: Command) {
        let mut rt = self.runtime.lock().unwrap();
        if rt.commands.len() < 32 {
            rt.commands.push_back(command);
        }
    }
    fn update_rows(&self) {
        let rows = {
            let rt = self.runtime.lock().unwrap();
            rt.browser
                .snapshot
                .as_ref()
                .map(|s| s.nodes[rt.browser.current].children.clone())
                .unwrap_or_default()
        };
        self.selected.set(None);
        self.rows.set(rows);
        self.changed();
    }
    fn start(&self, path: PathBuf) {
        let generation = self.worker.start(path.clone());
        self.runtime.lock().unwrap().browser.begin(path, generation);
        self.update_rows();
    }
    fn choose(&self) {
        let (owner, initial) = {
            let rt = self.runtime.lock().unwrap();
            if rt.dialog.is_some() {
                return;
            }
            let Some(owner) = rt.owner else {
                return;
            };
            (owner, rt.browser.root.clone())
        };
        let mut dialog = FileDialog::new(FileDialogMode::OpenFolder);
        dialog.title = "Choose Folder to Measure".into();
        dialog.initial_directory = initial.map(Into::into);
        self.runtime.lock().unwrap().dialog = Some(dialog.show(owner));
        self.changed();
    }
    fn dispatch(&self, command: Command) {
        match command {
            Command::Choose => self.choose(),
            Command::Refresh => {
                let root = self.runtime.lock().unwrap().browser.root.clone();
                if let Some(root) = root {
                    self.start(root);
                }
            }
            Command::Cancel => {
                self.worker.cancel();
                let mut rt = self.runtime.lock().unwrap();
                if rt.browser.busy {
                    rt.browser.cancelling = true;
                    drop(rt);
                    self.changed();
                }
            }
            Command::Back => {
                let changed = self.runtime.lock().unwrap().browser.back();
                if changed {
                    self.update_rows();
                }
            }
            Command::OpenSelected => {
                if let Some(id) = self
                    .selected
                    .get()
                    .and_then(|row| self.rows.get().get(row).copied())
                {
                    let changed = self.runtime.lock().unwrap().browser.enter(id);
                    if changed {
                        self.update_rows();
                    }
                }
            }
            Command::Enter(snapshot, id) => {
                let changed = {
                    let mut rt = self.runtime.lock().unwrap();
                    rt.browser
                        .snapshot
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, &snapshot))
                        && rt.browser.enter(id)
                };
                if changed {
                    self.update_rows();
                }
            }
        }
    }
    fn menu_item(
        &self,
        id: &str,
        name: &str,
        shortcut: &str,
        enabled: bool,
        command: Command,
    ) -> MenuEntry {
        let app = self.clone();
        MenuEntry::Item(
            MenuItemModel::new(id, name)
                .shortcut(shortcut)
                .enabled(enabled)
                .on_activate(Arc::new(move || app.enqueue(command.clone()))),
        )
    }
    fn menu(&self) -> MenuBarModel {
        let rt = self.runtime.lock().unwrap();
        let b = &rt.browser;
        let ready = rt.dialog.is_none();
        let folder_selected = b
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                self.selected
                    .get()
                    .and_then(|row| snapshot.nodes[b.current].children.get(row))
                    .map(|&id| snapshot.nodes[id].kind == Kind::Directory)
            })
            .unwrap_or(false);
        MenuBarModel::new(vec![
            MenuItemModel::new("file", "File").children(vec![self.menu_item(
                "choose",
                "Choose Folder…",
                &shortcut("O"),
                ready,
                Command::Choose,
            )]),
            MenuItemModel::new("view", "View").children(vec![
                self.menu_item(
                    "refresh",
                    "Refresh Scan",
                    &shortcut("R"),
                    ready && b.root.is_some(),
                    Command::Refresh,
                ),
                self.menu_item(
                    "cancel",
                    "Cancel Scan",
                    "Esc",
                    ready && b.busy && !b.cancelling,
                    Command::Cancel,
                ),
            ]),
            MenuItemModel::new("go", "Go").children(vec![
                self.menu_item(
                    "back",
                    "Enclosing Folder",
                    "Alt+Left",
                    ready && b.current != 0,
                    Command::Back,
                ),
                self.menu_item(
                    "open",
                    "Open Selected Folder",
                    "Enter",
                    ready && folder_selected,
                    Command::OpenSelected,
                ),
            ]),
        ])
    }
    fn handle_key(&self, event: KeyEvent) -> bool {
        let KeyEvent::Pressed { keycode, modifiers } = event else {
            return false;
        };
        if self.runtime.lock().unwrap().dialog.is_some() {
            return false;
        }
        let primary = if cfg!(target_os = "macos") {
            modifiers.super_key && !modifiers.control
        } else {
            modifiers.control && !modifiers.super_key
        };
        let command = match keycode {
            KeyCode::Char('o' | 'O') if primary && !modifiers.alt => Command::Choose,
            KeyCode::Char('r' | 'R') if primary && !modifiers.alt => Command::Refresh,
            KeyCode::Escape => Command::Cancel,
            KeyCode::Left if modifiers.alt => Command::Back,
            KeyCode::Backspace if !primary => Command::Back,
            KeyCode::Enter if !primary => Command::OpenSelected,
            KeyCode::Up | KeyCode::Down if !primary && !modifiers.alt => {
                let count = self.rows.get().len();
                if count != 0 {
                    let index = self
                        .selected
                        .get()
                        .map(|n| {
                            if keycode == KeyCode::Up {
                                n.saturating_sub(1)
                            } else {
                                (n + 1).min(count - 1)
                            }
                        })
                        .unwrap_or(0);
                    self.selected.set(Some(index));
                }
                return true;
            }
            _ => return false,
        };
        self.enqueue(command);
        true
    }
    fn content(&self) -> impl View + Clone + use<> {
        let palette = ColorPalette::default();
        let (snapshot, current, busy, cancelling, error, root) = {
            let rt = self.runtime.lock().unwrap();
            let b = &rt.browser;
            (
                b.snapshot.clone(),
                b.current,
                b.busy,
                b.cancelling,
                b.error.clone(),
                b.root.clone(),
            )
        };
        let width = self.width.get().max(480);
        let path = snapshot
            .as_ref()
            .map(|s| &s.nodes[current].path)
            .or(root.as_ref())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Choose a folder to see what uses space".into());
        let status = if cancelling {
            "Cancelling… Measured entries will remain available.".into()
        } else if busy {
            if snapshot.is_some() {
                "Scanning… Showing the previous snapshot until the scan finishes.".into()
            } else {
                "Scanning… You can cancel from the View menu or press Esc.".into()
            }
        } else if let Some(error) = &error {
            format!(
                "{error}{}",
                if snapshot.is_some() {
                    " · Showing the previous snapshot"
                } else {
                    ""
                }
            )
        } else if let Some(s) = &snapshot {
            let n = &s.nodes[current];
            let progress = if !s.nodes[0].complete {
                format!(
                    "Partial scan: {} · ",
                    s.stopped.clone().unwrap_or_else(|| {
                        if s.errors > 0 {
                            format!("{} read/depth errors", s.errors)
                        } else {
                            "some contents could not be measured".into()
                        }
                    })
                )
            } else {
                String::new()
            };
            format!(
                "{}{} logical · {} files · {} links / {} special files excluded · {:.1}s",
                progress,
                size(n.bytes),
                n.files,
                n.links,
                n.excluded,
                s.elapsed.as_secs_f64()
            )
        } else {
            "File → Choose Folder… to start a read-only scan".into()
        };
        let app = self.clone();
        let list = if let Some(snapshot) = snapshot.clone() {
            let parent_bytes = snapshot.nodes[current].bytes;
            let row_snapshot = snapshot.clone();
            Either::A(ListView::new(self.rows.clone(), self.selected.clone(), 38.0, move |index, id, selected| {
                let palette = ColorPalette::default();
                let node = &row_snapshot.nodes[id];
                let ratio = if parent_bytes == 0 { 0.0 } else { node.bytes as f64 / parent_bytes as f64 };
                let name = format!("{}{}", if !node.complete { "Partial · " } else if node.kind == Kind::Link { "Link · " } else if node.kind == Kind::Special { "Excluded · " } else { "" }, node.name());
                let opening = app.clone();
                let cache = row_snapshot.clone();
                let is_folder = node.kind == Kind::Directory;
                hstack! {
                    Text::new(if is_folder { "›" } else { " " }).font_size(13.0).color(palette.text_secondary()).frame_width(12.0),
                    Text::new(fit(&name, width.saturating_sub(290))).font_size(13.0).alignment(Alignment::Leading).frame_width(f32::INFINITY),
                    Text::new(if node.kind == Kind::Link || node.kind == Kind::Special || node.kind == Kind::Unknown { "—".into() } else { size(node.bytes) }).font_size(13.0).alignment(Alignment::Trailing).frame_width(92.0),
                    ProgressView::new(ratio as f32).frame(86.0, 4.0),
                    Text::new(format!("{:.1}%", ratio * 100.0)).font_size(12.0).color(palette.text_secondary()).alignment(Alignment::Trailing).frame_width(48.0),
                }.spacing(10.0).padding(8.0).frame(f32::INFINITY, 38.0)
                    .background(if selected == Some(index) { palette.surface_variant() } else { palette.background() })
                    .on_click(move || {
                        opening.selected.set(Some(index));
                        if is_folder { opening.enqueue(Command::Enter(cache.clone(), id)); }
                    })
            }).item_key(|id| format!("storage-{id}").into()).key(format!("storage-{current}"))
                .frame(f32::INFINITY, f32::INFINITY))
        } else {
            let choose = self.clone();
            Either::B(vstack! {
                Spacer::new(),
                Text::new(if busy { "Measuring folder contents…" } else { "Storage" }).font_size(17.0),
                Text::new("Find large folders and files without changing them.").font_size(13.0).color(palette.text_secondary()),
                Button::new("Choose Folder…").font_size(13.0).on_click(move || choose.enqueue(Command::Choose)),
                Spacer::new(),
            }.spacing(12.0).frame(f32::INFINITY, f32::INFINITY))
        };
        let detail = snapshot.as_ref().map(|s| {
            let id = self.selected.get().and_then(|row| s.nodes[current].children.get(row).copied()).unwrap_or(current);
            let n = &s.nodes[id];
            n.issue.clone().unwrap_or_else(|| if n.kind == Kind::Link { "Symbolic link — target was not followed".into() }
                else if n.kind == Kind::Special { "Special file — not included in logical bytes".into() }
                else if !n.complete { "Partial contents. Open folders marked partial to inspect errors; refresh to retry.".into() }
                else if n.kind == Kind::File { format!("{} bytes · {}", n.bytes, n.path.display()) }
                else if n.children.is_empty() { "This folder is empty.".into() }
                else { "Open a folder to inspect its cached contents. Go → Enclosing Folder returns.".into() })
        }).unwrap_or_default();
        let keys = self.clone();
        let geometry = self.width.clone();
        vstack! {
            Text::new(fit(&path, width.saturating_sub(24))).font_size(13.0).alignment(Alignment::Leading).padding(12.0).frame(f32::INFINITY, 40.0),
            Divider::new(),
            list,
            Divider::new(),
            vstack! {
                Text::new(fit(&status, width.saturating_sub(24))).font_size(13.0).color(if error.is_some() { palette.error() } else { palette.text() }),
                Text::new(fit(&detail, width.saturating_sub(24))).font_size(13.0).color(palette.text_secondary()),
                Text::new("Logical bytes, counted per file path. Physical free space is not measured.").font_size(12.0).color(palette.text_secondary()),
            }.alignment(Alignment::Leading).spacing(5.0).padding(12.0).frame_width(f32::INFINITY),
        }.spacing(0.0).frame(f32::INFINITY, f32::INFINITY).focusable(self.focused.clone())
            .on_geometry_change(|layout| layout.size().width.max(0.0) as u32, move |width| { if geometry.get() != width { geometry.set(width); } })
            .on_event_capture(move |event| match event { Event::Keyboard(key) => keys.handle_key(*key), _ => false })
    }
    fn poll(&self) {
        let commands = std::mem::take(&mut self.runtime.lock().unwrap().commands);
        for command in commands {
            self.dispatch(command);
        }
        let dialog = {
            let mut rt = self.runtime.lock().unwrap();
            let result = rt.dialog.as_ref().and_then(FileDialogHandle::take_result);
            if result.is_some() {
                rt.dialog = None;
            }
            result
        };
        if let Some(result) = dialog {
            match result {
                Ok(FileDialogOutcome::Selected(paths)) if !paths.is_empty() => {
                    self.start(paths[0].as_path().to_path_buf())
                }
                Err(error) => {
                    self.runtime.lock().unwrap().browser.error = Some(error.to_string());
                }
                _ => {}
            }
            self.changed();
        }
        if let Some(response) = self.worker.take_result() {
            let changed = self
                .runtime
                .lock()
                .unwrap()
                .browser
                .accept(response.generation, response.result);
            if changed {
                self.update_rows();
            }
        }
    }
}

impl View for Storage {
    fn create_element(&self) -> Box<dyn Element> {
        self.content().create_element()
    }
    fn listenables(&self) -> Vec<&dyn Listenable> {
        vec![&self.chrome, &self.selected, &self.width]
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
impl Application for Storage {
    fn scenes(&self) -> impl Scene {
        Window::new("Storage", self.clone())
            .scene_key("storage")
            .app_id("org.scarlet-os.desktop.storage")
            .menu_bar(self.menu())
            .size(Size::new(780.0, 560.0))
            .min_size(Size::new(560.0, 360.0))
    }
    fn on_window_created(&mut self, context: &WindowContext, _: &mut dyn PlatformWindow) {
        self.runtime.lock().unwrap().owner = Some(context.window_id);
    }
    fn on_idle(&mut self) {
        self.poll();
    }
    fn on_window_close_requested(&mut self, _: &WindowContext) -> bool {
        if let Some(dialog) = self.runtime.lock().unwrap().dialog.take() {
            dialog.cancel();
        }
        self.worker.shutdown();
        true
    }
    fn debug_logging(&self) -> bool {
        false
    }
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut app = Storage::new()?;
    if let Some(path) = std::env::args_os().nth(1) {
        let path = PathBuf::from(path);
        app.start(if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(path)
        });
    }
    app.run()
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    Ok(())
}
