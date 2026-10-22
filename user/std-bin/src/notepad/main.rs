//! Scarlet Desktop text editor with guarded document transitions and asynchronous I/O.
mod document;

use document::{Operation, PendingAction, Session, Ticket};
use scarlet_ui::clipboard::{ClipboardError, ClipboardHandle, TextClipboard};
use scarlet_ui::file_dialog::{FileDialog, FileDialogHandle, FileDialogMode, FileDialogOutcome};
use scarlet_ui::prelude::*;
use scarlet_ui::{
    Event, KeyCode, MenuBarModel, MenuEntry, MenuItemModel, PlatformWindow, WindowContext,
    WindowId, hstack, scenes, vstack,
};
use scarlet_ui_macros::View;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

const APP_ID: &str = "org.scarlet-os.desktop.notepad";
const HISTORY_LIMIT: usize = 128;
const HISTORY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Default)]
struct Snapshot {
    document: TextDocument,
    selection: TextSelection,
    version: u64,
}
#[derive(Default)]
struct Runtime {
    owner: Option<WindowId>,
    window_title: String,
    position_key: Option<(TextSelection, u64, bool)>,
    dialog: Option<(Ticket, Operation, FileDialogHandle)>,
    results: VecDeque<IoResult>,
    undo: VecDeque<Snapshot>,
    redo: VecDeque<Snapshot>,
    current: Snapshot,
    clipboard: Option<TextClipboard>,
    clipboard_error: Option<String>,
    clipboard_job: Option<ClipboardJob>,
}
enum ClipboardJob {
    Copy {
        handle: ClipboardHandle<()>,
        cut: bool,
        identity: (u64, u64),
        selection: TextSelection,
    },
    Paste {
        handle: ClipboardHandle<Option<String>>,
        identity: (u64, u64),
        selection: TextSelection,
    },
}
enum ClipboardCompletion {
    Copy(
        std::result::Result<(), ClipboardError>,
        bool,
        (u64, u64),
        TextSelection,
    ),
    Paste(
        std::result::Result<Option<String>, ClipboardError>,
        (u64, u64),
        TextSelection,
    ),
}
enum IoResult {
    Open(Ticket, PathBuf, std::result::Result<String, String>),
    Save(Ticket, PathBuf, std::result::Result<(), String>),
}

type IoTask = Box<dyn FnOnce() + Send>;

fn spawn_io_worker(name: &str, task: IoTask) -> std::io::Result<()> {
    thread::Builder::new()
        .name(name.into())
        .spawn(task)
        .map(|_| ())
}

#[derive(View, Clone)]
struct NotepadApp {
    content: State<TextDocument>,
    selection: State<TextSelection>,
    scroll: State<TextViewScroll>,
    session: State<Session>,
    runtime: State<Arc<Mutex<Runtime>>>,
    status: State<String>,
    position: State<String>,
    viewport_width: State<f32>,
    wrap: State<WrapMode>,
    line_numbers: State<bool>,
    font_size: State<f32>,
    query: State<String>,
    replacement: State<String>,
    search_status: State<String>,
}

impl NotepadApp {
    fn new(initial_path: Option<PathBuf>) -> Self {
        let app = Self::default();
        app.status.set("Ready".into());
        app.position.set("Ln 1, Col 1".into());
        app.viewport_width.set(900.0);
        app.wrap.set(WrapMode::Soft);
        app.line_numbers.set(true);
        app.font_size.set(15.0);
        app.search_status.set("Ready to search".into());
        if let Some(path) = initial_path {
            app.open_path(path);
        }
        app
    }
    fn window_title(&self) -> String {
        let session = self.session.get();
        let name = session
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        format!(
            "{}{} — Notepad",
            if session.dirty() { "* " } else { "" },
            name
        )
    }
    fn request(&self, action: PendingAction) {
        let mut session = self.session.get();
        if !session.request(action) {
            self.status
                .set("Finish or cancel the current file operation first".into());
            return;
        }
        let dirty = session.dirty();
        self.session.set(session);
        if dirty {
            open_window("confirm");
        } else {
            self.continue_pending();
        }
    }
    fn continue_pending(&self) {
        let mut session = self.session.get();
        let action = session.take_ready_action();
        self.session.set(session);
        if let Some(action) = action {
            self.perform(action);
        }
    }
    fn perform(&self, action: PendingAction) {
        dismiss_window("confirm");
        match action {
            PendingAction::New => {
                self.load_document(None, String::new());
                self.status.set("New document".into());
            }
            PendingAction::Open => self.show_picker(Operation::OpenPicker),
            PendingAction::Close => {
                dismiss_window("find");
                dismiss_window("main");
            }
        }
    }
    fn cancel_pending(&self) {
        // Never turn a cancelled Save-As into a later close/new/open.
        self.session.update(|s| s.cancel());
        dismiss_window("confirm");
    }
    fn discard_pending(&self) {
        let mut session = self.session.get();
        if session.operation.is_some() {
            self.status
                .set("Wait for the current operation to finish".into());
            return;
        }
        let action = session.pending.take();
        self.session.set(session);
        if let Some(action) = action {
            self.perform(action);
        }
    }
    fn load_document(&self, path: Option<PathBuf>, text: String) {
        let document = TextDocument::from_str(&text);
        self.content.set(document.clone());
        self.selection.set(TextSelection::collapsed(0));
        self.scroll.set(TextViewScroll::default());
        self.session.update(|s| s.loaded(path));
        let shared = self.runtime.get();
        let mut rt = shared.lock().unwrap();
        rt.undo.clear();
        rt.redo.clear();
        rt.current = Snapshot {
            document,
            ..Snapshot::default()
        };
        drop(rt);
        self.update_position();
    }
    fn begin(&self, operation: Operation) -> Option<Ticket> {
        let mut session = self.session.get();
        let ticket = session.begin(operation);
        self.session.set(session);
        if ticket.is_none() {
            self.status
                .set("A file operation is already in progress".into());
        }
        ticket
    }
    fn open_path(&self, path: PathBuf) {
        self.open_path_with(path, spawn_io_worker);
    }
    fn open_path_with(
        &self,
        path: PathBuf,
        spawn: impl FnOnce(&str, IoTask) -> std::io::Result<()>,
    ) {
        let Some(ticket) = self.begin(Operation::Read) else {
            return;
        };
        self.status.set(format!("Opening {}…", path.display()));
        let shared = self.runtime.get();
        let failed_path = path.clone();
        let output = shared.clone();
        if let Err(error) = spawn(
            "notepad-open",
            Box::new(move || {
                let result = document::read_document(&path).map_err(|e| e.to_string());
                output
                    .lock()
                    .unwrap()
                    .results
                    .push_back(IoResult::Open(ticket, path, result));
            }),
        ) {
            shared.lock().unwrap().results.push_back(IoResult::Open(
                ticket,
                failed_path,
                Err(format!("Could not start file worker: {error}")),
            ));
        }
    }
    fn save_to_path(&self, path: PathBuf) {
        self.save_to_path_with(path, spawn_io_worker);
    }
    fn save_to_path_with(
        &self,
        path: PathBuf,
        spawn: impl FnOnce(&str, IoTask) -> std::io::Result<()>,
    ) {
        let Some(ticket) = self.begin(Operation::Save) else {
            return;
        };
        let document = self.content.get();
        self.status.set(format!("Saving {}…", path.display()));
        let shared = self.runtime.get();
        let failed_path = path.clone();
        let output = shared.clone();
        if let Err(error) = spawn(
            "notepad-save",
            Box::new(move || {
                let result = document::save_atomic(&path, document.as_str().as_bytes())
                    .map_err(|e| e.to_string());
                output
                    .lock()
                    .unwrap()
                    .results
                    .push_back(IoResult::Save(ticket, path, result));
            }),
        ) {
            shared.lock().unwrap().results.push_back(IoResult::Save(
                ticket,
                failed_path,
                Err(format!("Could not start file worker: {error}")),
            ));
        }
    }
    fn save(&self) {
        if let Some(path) = self.session.get().path {
            self.save_to_path(path);
        } else {
            self.show_picker(Operation::SavePicker);
        }
    }
    fn show_picker(&self, operation: Operation) {
        let shared = self.runtime.get();
        let owner = shared.lock().unwrap().owner;
        let Some(owner) = owner else {
            self.status
                .set("Window is not ready for file selection".into());
            return;
        };
        let Some(ticket) = self.begin(operation) else {
            return;
        };
        let save = operation == Operation::SavePicker;
        let session = self.session.get();
        let mut dialog = FileDialog::new(if save {
            FileDialogMode::Save
        } else {
            FileDialogMode::Open
        });
        dialog.title = if save {
            "Save Document"
        } else {
            "Open Document"
        }
        .into();
        let folder = session
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(PathBuf::from)
            .unwrap_or_else(home_path);
        if folder.is_absolute() {
            dialog.initial_directory = Some(folder.into());
        }
        if save {
            dialog.default_name = Some(
                session
                    .path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Untitled.txt".into()),
            );
        }
        shared.lock().unwrap().dialog = Some((ticket, operation, dialog.show(owner)));
        self.status.set("Waiting for file selection".into());
    }
    fn poll_results(&self) {
        let shared = self.runtime.get();
        let dialog_result = {
            let mut rt = shared.lock().unwrap();
            let result = rt
                .dialog
                .as_ref()
                .and_then(|(t, op, h)| h.take_result().map(|r| (*t, *op, r)));
            if result.is_some() {
                rt.dialog = None;
            }
            result
        };
        if let Some((ticket, operation, result)) = dialog_result {
            let mut session = self.session.get();
            if session.finish(ticket) {
                let unchanged = session.unchanged(ticket);
                self.session.set(session);
                match result {
                    Ok(FileDialogOutcome::Selected(paths)) if !paths.is_empty() => {
                        let path = paths[0].as_path().to_path_buf();
                        if operation == Operation::OpenPicker && !unchanged {
                            self.status.set(
                                "Document changed during file selection; open it again when ready"
                                    .into(),
                            );
                        } else if operation == Operation::OpenPicker {
                            self.open_path(path);
                        } else {
                            self.save_to_path(path);
                        }
                    }
                    Ok(_) => {
                        self.cancel_pending();
                        self.status.set("File selection cancelled".into());
                    }
                    Err(scarlet_ui::file_dialog::FileDialogError::Unsupported(_)) => {
                        self.status
                            .set("File dialogs are unavailable on this platform".into());
                    }
                    Err(error) => {
                        self.status.set(format!("File selection failed: {error}"));
                    }
                }
            }
        }
        let results: Vec<_> = shared.lock().unwrap().results.drain(..).collect();
        for result in results {
            match result {
                IoResult::Open(ticket, path, result) => {
                    let mut session = self.session.get();
                    if !session.finish(ticket) {
                        continue;
                    }
                    let unchanged = session.unchanged(ticket);
                    self.session.set(session);
                    match result {
                        Ok(text) if unchanged => {
                            self.load_document(Some(path.clone()), text);
                            self.status.set(format!("Opened {}", path.display()));
                        }
                        Ok(_) => self
                            .status
                            .set("Document changed while loading; your edits were kept".into()),
                        Err(error) => self
                            .status
                            .set(format!("Cannot open {}: {error}", path.display())),
                    }
                }
                IoResult::Save(ticket, path, result) => {
                    let mut session = self.session.get();
                    if !session.finish(ticket) {
                        continue;
                    }
                    if result.is_ok() {
                        session.saved(ticket, path.clone());
                    }
                    let dirty = session.dirty();
                    let pending = session.pending.is_some();
                    self.session.set(session);
                    self.update_position();
                    match result {
                        Ok(()) => {
                            self.status.set(if dirty {
                                "Saved earlier version; newer edits remain unsaved".into()
                            } else {
                                format!("Saved {}", path.display())
                            });
                            if dirty && pending {
                                open_window("confirm");
                            } else {
                                self.continue_pending();
                            }
                        }
                        Err(error) => self
                            .status
                            .set(format!("Cannot save {}: {error}", path.display())),
                    }
                }
            }
        }
    }
    fn record_edit(&self) {
        let shared = self.runtime.get();
        let mut rt = shared.lock().unwrap();
        let previous = rt.current.clone();
        rt.undo.push_back(previous);
        trim_history(&mut rt.undo);
        rt.redo.clear();
        self.session.update(|s| s.edited());
        rt.current = Snapshot {
            document: self.content.get(),
            selection: self.selection.get(),
            version: self.session.get().content_version,
        };
        drop(rt);
        self.update_position();
    }
    fn restore_history(&self, redo: bool) {
        let shared = self.runtime.get();
        let mut rt = shared.lock().unwrap();
        let snapshot = if redo {
            rt.redo.pop_back()
        } else {
            rt.undo.pop_back()
        };
        let Some(snapshot) = snapshot else {
            return;
        };
        let current = Snapshot {
            selection: self.selection.get(),
            ..rt.current.clone()
        };
        if redo {
            rt.undo.push_back(current);
        } else {
            rt.redo.push_back(current);
        }
        rt.current = snapshot.clone();
        drop(rt);
        self.content.set(snapshot.document);
        self.selection.set(snapshot.selection);
        self.session.update(|s| s.restored(snapshot.version));
        self.update_position();
        self.status.set(if redo { "Redo" } else { "Undo" }.into());
    }
    fn replace_range(&self, range: std::ops::Range<usize>, text: &str) {
        self.runtime.get().lock().unwrap().current.selection = self.selection.get();
        let (document, delta) = self.content.get().replace(range, text);
        self.content.set(document);
        self.selection.set(TextSelection::collapsed(
            delta.replaced_range.start + text.len(),
        ));
        self.record_edit();
    }
    fn clipboard_service(&self) -> Option<TextClipboard> {
        let shared = self.runtime.get();
        let runtime = shared.lock().unwrap();
        if runtime.clipboard_job.is_some() {
            self.status.set("A clipboard operation is pending".into());
            return None;
        }
        match runtime.clipboard.clone() {
            Some(service) => Some(service),
            None => {
                self.status.set(
                    runtime
                        .clipboard_error
                        .clone()
                        .unwrap_or_else(|| "Desktop clipboard unavailable".into()),
                );
                None
            }
        }
    }
    fn copy(&self, cut: bool) {
        let selection = self.selection.get();
        let Some(range) = selection.normalized_range() else {
            return;
        };
        let document = self.content.get();
        let text = document.as_str();
        let Some(selected) = text.get(range) else {
            return;
        };
        let Some(service) = self.clipboard_service() else {
            return;
        };
        self.runtime.get().lock().unwrap().clipboard_job = Some(ClipboardJob::Copy {
            handle: service.write_text(selected.into()),
            cut,
            identity: self.session.get().identity(),
            selection,
        });
        self.status.set("Copying to desktop clipboard…".into());
    }
    fn paste(&self) {
        let Some(service) = self.clipboard_service() else {
            return;
        };
        self.runtime.get().lock().unwrap().clipboard_job = Some(ClipboardJob::Paste {
            handle: service.read_text(),
            identity: self.session.get().identity(),
            selection: self.selection.get(),
        });
        self.status.set("Reading desktop clipboard…".into());
    }
    fn clipboard_target_unchanged(&self, identity: (u64, u64), selection: TextSelection) -> bool {
        self.session.get().identity() == identity && self.selection.get() == selection
    }
    fn poll_clipboard(&self) {
        let completion = {
            let shared = self.runtime.get();
            let mut runtime = shared.lock().unwrap();
            let ready = match runtime.clipboard_job.as_ref() {
                Some(ClipboardJob::Copy {
                    handle,
                    cut,
                    identity,
                    selection,
                }) => handle
                    .try_take()
                    .map(|r| ClipboardCompletion::Copy(r, *cut, *identity, *selection)),
                Some(ClipboardJob::Paste {
                    handle,
                    identity,
                    selection,
                }) => handle
                    .try_take()
                    .map(|r| ClipboardCompletion::Paste(r, *identity, *selection)),
                None => None,
            };
            if ready.is_some() {
                runtime.clipboard_job = None;
            }
            ready
        };
        match completion {
            Some(ClipboardCompletion::Copy(Ok(()), cut, identity, selection)) => {
                if cut {
                    if !self.clipboard_target_unchanged(identity, selection) {
                        self.status.set(
                            "Copied; Cut cancelled because the document or selection changed"
                                .into(),
                        );
                        return;
                    }
                    self.replace_range(selection.start().byte..selection.end().byte, "");
                }
                self.status.set(
                    if cut {
                        "Cut to desktop clipboard"
                    } else {
                        "Copied to desktop clipboard"
                    }
                    .into(),
                );
            }
            Some(ClipboardCompletion::Paste(Ok(Some(text)), identity, selection)) => {
                if !self.clipboard_target_unchanged(identity, selection) {
                    self.status
                        .set("Paste cancelled because the document or selection changed".into());
                    return;
                }
                self.replace_range(selection.start().byte..selection.end().byte, &text);
                self.status.set("Pasted from desktop clipboard".into());
            }
            Some(ClipboardCompletion::Paste(Ok(None), ..)) => {
                self.status.set("Desktop clipboard is empty".into())
            }
            Some(
                ClipboardCompletion::Copy(Err(error), ..)
                | ClipboardCompletion::Paste(Err(error), ..),
            ) => self.status.set(error.to_string()),
            None => {}
        }
    }
    fn select_all(&self) {
        self.selection.set(TextSelection {
            anchor: TextPosition::new(0),
            caret: TextPosition::new(self.content.get().len()),
        });
    }
    fn show_find(&self) {
        let doc = self.content.get();
        if let Some(range) = self.selection.get().normalized_range() {
            if let Some(text) = doc.as_str().get(range) {
                if !text.contains('\n') {
                    self.query.set(text.into());
                }
            }
        }
        open_window("find");
    }
    fn find(&self, backwards: bool) {
        let doc = self.content.get();
        let selection = self.selection.get();
        let start = if backwards {
            selection.start().byte
        } else {
            selection.end().byte
        };
        match document::find(&doc.as_str(), &self.query.get(), start, backwards) {
            Some(range) => {
                self.selection.set(TextSelection {
                    anchor: TextPosition::new(range.start),
                    caret: TextPosition::new(range.end),
                });
                self.search_status.set("Match selected in document".into());
            }
            None => self.search_status.set(
                if self.query.get().is_empty() {
                    "Enter text to find"
                } else {
                    "No matches"
                }
                .into(),
            ),
        }
    }
    fn replace_match(&self) {
        let query = self.query.get();
        if !query.is_empty() {
            if let Some(range) = self.selection.get().normalized_range() {
                if self.content.get().as_str().get(range.clone()) == Some(query.as_str()) {
                    self.replace_range(range, &self.replacement.get());
                }
            }
        }
        self.find(false);
    }
    fn replace_all(&self) {
        let document = self.content.get();
        if let Some((text, count)) = document::replace_all(
            &document.as_str(),
            &self.query.get(),
            &self.replacement.get(),
        ) {
            self.replace_range(0..document.len(), &text);
            self.search_status
                .set(format!("Replaced {count} matches (Undo restores all)"));
        } else {
            self.search_status
                .set("No matches; document unchanged".into());
        }
    }
    fn update_position(&self) {
        let selection = self.selection.get();
        let session = self.session.get();
        let key = (selection, session.revision, session.dirty());
        {
            let shared = self.runtime.get();
            let mut runtime = shared.lock().unwrap();
            if runtime.position_key == Some(key) {
                return;
            }
            runtime.position_key = Some(key);
        }
        let document = self.content.get();
        let byte = selection.caret.byte.min(document.len());
        let line = document.line_index_at_byte(byte);
        let start = document.line_range(line).map_or(0, |r| r.start);
        let column = document.line_text(line).map_or(1, |text| {
            let end = (byte - start).min(text.len());
            text.get(..end).map_or(1, |s| s.chars().count() + 1)
        });
        let label = format!(
            "{}Ln {}, Col {}  ·  UTF-8",
            if self.session.get().dirty() {
                "Modified  ·  "
            } else {
                ""
            },
            line + 1,
            column
        );
        if self.position.get() != label {
            self.position.set(label);
        }
    }
    fn handle_key(&self, event: KeyEvent) -> bool {
        let KeyEvent::Pressed { keycode, modifiers } = event else {
            return false;
        };
        if keycode == KeyCode::F(3) {
            self.find(modifiers.shift);
            return true;
        }
        if !modifiers.primary() {
            return false;
        }
        match keycode {
            KeyCode::Char('n' | 'N') => self.request(PendingAction::New),
            KeyCode::Char('o' | 'O') => self.request(PendingAction::Open),
            KeyCode::Char('s' | 'S') if modifiers.shift => self.show_picker(Operation::SavePicker),
            KeyCode::Char('s' | 'S') => self.save(),
            KeyCode::Char('w' | 'W') => self.request(PendingAction::Close),
            KeyCode::Char('z' | 'Z') => self.restore_history(modifiers.shift),
            KeyCode::Char('y' | 'Y') => self.restore_history(true),
            KeyCode::Char('f' | 'F' | 'h' | 'H') => self.show_find(),
            KeyCode::Char('a' | 'A') => self.select_all(),
            KeyCode::Char('c' | 'C') => self.copy(false),
            KeyCode::Char('x' | 'X') => self.copy(true),
            KeyCode::Char('v' | 'V') => self.paste(),
            _ => return false,
        }
        true
    }
    fn content_view(&self) -> impl View + Clone + use<> {
        let edit = self.clone();
        let key = self.clone();
        let palette = ColorPalette::default();
        let position_width =
            scarlet_ui::graphics::measure_text_sized(&self.position.get(), 11.0).0 as f32;
        let status = fitted_status(
            &self.status.get(),
            (self.viewport_width.get() - position_width - 42.0).max(0.0),
        );
        vstack! {
            TextView::with_document(self.content.clone(), self.selection.clone())
                .autofocus(true)
                .scroll_state(self.scroll.clone()).wrap_mode_state(self.wrap.clone())
                .line_numbers(self.line_numbers.get()).current_line_highlight(true)
                .border_color(palette.divider()).focused_border_color(palette.divider())
                .reveal_selection_on_change(true)
                .font_size(self.font_size.get()).padding(12.0)
                .on_text_change(move |_| edit.record_edit())
                .frame(f32::INFINITY, f32::INFINITY),
            hstack! {
                Text::new(status).font_size(11.0),
                Spacer::new(),
                Text::from_state(self.position.clone()).font_size(11.0),
            }.spacing(12.0).padding(8.0).background(palette.surface_variant()),
        }
        .on_event_capture(move |event| {
            // Remember selection before native TextView edits for faithful Undo.
            key.runtime.get().lock().unwrap().current.selection = key.selection.get();
            if let Event::Keyboard(event) = event {
                key.handle_key(event.clone())
            } else {
                false
            }
        })
        .frame(f32::INFINITY, f32::INFINITY)
    }
    fn confirm_view(&self) -> impl View + Clone + use<> {
        let save = self.clone();
        let discard = self.clone();
        let cancel = self.clone();
        let palette = ColorPalette::default();
        vstack! {
            Text::new("Save changes?").font_size(20.0),
            Text::new("Your document has unsaved changes.").font_size(13.0).color(palette.text_secondary()),
            Text::from_state(self.status.clone()).font_size(11.0).color(palette.text_secondary()),
            Spacer::new(),
            hstack! {
                Button::new("Cancel").header_style().on_click(move || cancel.cancel_pending()),
                Spacer::new(),
                Button::new("Discard").header_style().on_click(move || discard.discard_pending()),
                Button::new("Save").header_style().background_color(palette.surface_variant()).on_click(move || save.save()),
            }.spacing(10.0),
        }
        .alignment(Alignment::Leading)
        .spacing(14.0)
        .padding(24.0)
        .frame(f32::INFINITY, f32::INFINITY)
    }
    fn find_view(&self) -> impl View + Clone + use<> {
        let submit = self.clone();
        let next = self.clone();
        let previous = self.clone();
        let replace = self.clone();
        let all = self.clone();
        let palette = ColorPalette::default();
        vstack! {
            Text::new("Find & replace").font_size(20.0).color(palette.text_primary()),
            Text::new("Case-sensitive · search wraps at the end").font_size(12.0).color(palette.text_secondary()),
            vstack! {
                Text::new("Find").font_size(12.0).color(palette.text_secondary()),
                TextField::new(self.query.clone()).autofocus(true).placeholder("Text to find")
                    .on_submit(move || submit.find(false)).frame_width(f32::INFINITY),
                hstack! {
                    Text::new("Enter finds the next match").font_size(11.0).color(palette.text_secondary()),
                    Spacer::new(),
                    Button::new("Previous").on_click(move || previous.find(true)),
                    Button::new("Next").on_click(move || next.find(false)),
                }.spacing(8.0),
            }.alignment(Alignment::Leading).spacing(8.0).frame_width(f32::INFINITY),
            vstack! {
                Text::new("Replace with").font_size(12.0).color(palette.text_secondary()),
                TextField::new(self.replacement.clone()).placeholder("Replacement text (can be empty)").frame_width(f32::INFINITY),
                hstack! {
                    Spacer::new(),
                    Button::new("Replace").on_click(move || replace.replace_match()),
                    Button::new("Replace All").on_click(move || all.replace_all()),
                }.spacing(8.0),
            }.alignment(Alignment::Leading).spacing(8.0).frame_width(f32::INFINITY),
            Spacer::new(),
            hstack! {
                Text::from_state(self.search_status.clone()).font_size(11.0).color(palette.text_secondary()),
                Spacer::new(),
                Button::new("Done").on_click(|| dismiss_window("find")),
            }.spacing(8.0),
        }
        .alignment(Alignment::Leading)
        .spacing(14.0)
        .padding(24.0)
        .frame(f32::INFINITY, f32::INFINITY)
        .on_event_capture(|event| {
            if matches!(event, Event::Keyboard(KeyEvent::Pressed { keycode: KeyCode::Escape, .. })) {
                dismiss_window("find");
                true
            } else {
                false
            }
        })
    }
    fn menu_item(
        &self,
        id: &str,
        title: &str,
        shortcut: &str,
        action: impl Fn(&Self) + Send + Sync + 'static,
    ) -> MenuEntry {
        let app = self.clone();
        MenuEntry::Item(
            MenuItemModel::new(id, title)
                .shortcut(shortcut)
                .on_activate(Arc::new(move || action(&app))),
        )
    }
    fn menu_bar(&self) -> MenuBarModel {
        MenuBarModel::new(vec![
            MenuItemModel::new("file", "File").children(vec![
                self.menu_item("new", "New", "Ctrl+N", |a| a.request(PendingAction::New)),
                self.menu_item("open", "Open…", "Ctrl+O", |a| {
                    a.request(PendingAction::Open)
                }),
                MenuEntry::Separator,
                self.menu_item("save", "Save", "Ctrl+S", |a| a.save()),
                self.menu_item("save-as", "Save As…", "Ctrl+Shift+S", |a| {
                    a.show_picker(Operation::SavePicker)
                }),
                MenuEntry::Separator,
                self.menu_item("close", "Close", "Ctrl+W", |a| {
                    a.request(PendingAction::Close)
                }),
            ]),
            MenuItemModel::new("edit", "Edit").children(vec![
                self.menu_item("undo", "Undo", "Ctrl+Z", |a| a.restore_history(false)),
                self.menu_item("redo", "Redo", "Ctrl+Shift+Z", |a| a.restore_history(true)),
                MenuEntry::Separator,
                self.menu_item("cut", "Cut", "Ctrl+X", |a| a.copy(true)),
                self.menu_item("copy", "Copy", "Ctrl+C", |a| a.copy(false)),
                self.menu_item("paste", "Paste", "Ctrl+V", |a| a.paste()),
                self.menu_item("all", "Select All", "Ctrl+A", |a| a.select_all()),
                MenuEntry::Separator,
                self.menu_item("find", "Find / Replace…", "Ctrl+F", |a| a.show_find()),
                self.menu_item("next", "Find Next", "F3", |a| a.find(false)),
                self.menu_item("previous", "Find Previous", "Shift+F3", |a| a.find(true)),
            ]),
            MenuItemModel::new("view", "View").children(vec![
                self.menu_item(
                    "wrap",
                    if self.wrap.get() == WrapMode::Soft {
                        "Word Wrap: On"
                    } else {
                        "Word Wrap: Off"
                    },
                    "",
                    |a| {
                        a.wrap.update(|w| {
                            *w = if *w == WrapMode::Soft {
                                WrapMode::None
                            } else {
                                WrapMode::Soft
                            }
                        })
                    },
                ),
                self.menu_item(
                    "lines",
                    if self.line_numbers.get() {
                        "Line Numbers: On"
                    } else {
                        "Line Numbers: Off"
                    },
                    "",
                    |a| a.line_numbers.update(|v| *v = !*v),
                ),
                MenuEntry::Separator,
                self.menu_item("larger", "Larger Text", "", |a| {
                    a.font_size.update(|v| *v = (*v + 1.0).min(32.0))
                }),
                self.menu_item("smaller", "Smaller Text", "", |a| {
                    a.font_size.update(|v| *v = (*v - 1.0).max(10.0))
                }),
                self.menu_item("reset-zoom", "Reset Text Size", "", |a| {
                    a.font_size.set(15.0)
                }),
            ]),
        ])
    }
}
impl Application for NotepadApp {
    fn scenes(&self) -> impl Scene {
        scenes! {
            Window::new(self.window_title(), self.content_view()).scene_key("main")
                .app_id(APP_ID).menu_bar(self.menu_bar()).size(Size::new(900.0, 680.0))
                .min_size(Size::new(560.0, 360.0)),
            Window::new("Unsaved Changes — Notepad", self.confirm_view()).scene_key("confirm")
                .open_at_launch(false).app_id(APP_ID).size(Size::new(540.0, 220.0)),
            Window::new("Find / Replace — Notepad", self.find_view()).scene_key("find")
                .open_at_launch(false).app_id(APP_ID).size(Size::new(560.0, 400.0))
                .min_size(Size::new(560.0, 400.0)),
        }
    }
    fn on_window_created(&mut self, ctx: &WindowContext, window: &mut dyn PlatformWindow) {
        if ctx.scene_key.as_str() == "main" {
            self.viewport_width.set(window.managed_size().width);
            let shared = self.runtime.get();
            let mut runtime = shared.lock().unwrap();
            runtime.owner = Some(ctx.window_id);
            match window.text_clipboard() {
                Ok(service) => runtime.clipboard = Some(service),
                Err(error) => runtime.clipboard_error = Some(error.to_string()),
            }
        }
    }
    fn on_window_sync(&mut self, ctx: &WindowContext, window: &mut dyn PlatformWindow) {
        if ctx.scene_key.as_str() != "main" {
            return;
        }
        let width = window.managed_size().width;
        if self.viewport_width.get() != width {
            self.viewport_width.set(width);
        }
        let title = self.window_title();
        let shared = self.runtime.get();
        let mut runtime = shared.lock().unwrap();
        if runtime.window_title != title {
            window.set_title(&title);
            runtime.window_title = title;
        }
    }
    fn on_window_close_requested(&mut self, ctx: &WindowContext) -> bool {
        match ctx.scene_key.as_str() {
            "main" => {
                self.request(PendingAction::Close);
                false
            }
            "confirm" => {
                self.cancel_pending();
                true
            }
            _ => true,
        }
    }
    fn on_idle(&mut self) {
        self.poll_clipboard();
        self.poll_results();
        self.update_position();
    }
}
fn trim_history(history: &mut VecDeque<Snapshot>) {
    let mut bytes: usize = history.iter().map(|s| s.document.len()).sum();
    while history.len() > HISTORY_LIMIT || (history.len() > 1 && bytes > HISTORY_BYTES) {
        if let Some(old) = history.pop_front() {
            bytes = bytes.saturating_sub(old.document.len());
        }
    }
}
fn home_path() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/root"))
}
fn fitted_status(message: &str, width: f32) -> String {
    let message = message.replace(['\n', '\r', '\t'], " ");
    let measure = |text: &str| scarlet_ui::graphics::measure_text_sized(text, 11.0).0 as f32;
    if measure(&message) <= width {
        return message;
    }
    if measure("…") > width {
        return String::new();
    }
    let boundaries: Vec<_> = message.char_indices().map(|(byte, _)| byte).collect();
    let (mut low, mut high) = (0, boundaries.len().saturating_sub(1));
    while low < high {
        let middle = (low + high + 1) / 2;
        if measure(&format!("{}…", &message[..boundaries[middle]])) <= width {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let mut end = boundaries.get(low).copied().unwrap_or(0);
    loop {
        let grapheme = TextPosition::new(end).clamp_to_grapheme(&message).byte;
        if grapheme <= end {
            end = grapheme;
            break;
        }
        end = message[..end]
            .char_indices()
            .last()
            .map_or(0, |(byte, _)| byte);
    }
    format!("{}…", &message[..end])
}

fn main() {
    let mut app = NotepadApp::new(std::env::args_os().nth(1).map(PathBuf::from));
    if let Err(error) = app.run() {
        eprintln!("[notepad] error: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reject_worker(_name: &str, _task: IoTask) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "worker capacity exhausted",
        ))
    }

    #[test]
    fn failed_open_worker_releases_busy_state_and_preserves_edits() {
        let app = NotepadApp::new(None);
        app.load_document(Some("original.txt".into()), "猫".into());
        app.replace_range(3..3, " edited");
        let selection = app.selection.get();
        app.open_path_with("never-opened.txt".into(), reject_worker);
        assert!(app.session.get().operation.is_some());
        app.poll_results();
        assert!(app.session.get().operation.is_none());
        assert_eq!(app.session.get().path, Some("original.txt".into()));
        assert!(app.session.get().dirty());
        assert_eq!(app.content.get().as_str(), "猫 edited");
        assert_eq!(app.selection.get(), selection);
        assert!(app.status.get().contains("worker capacity exhausted"));
        assert!(app.begin(Operation::Read).is_some());
    }

    #[test]
    fn failed_save_worker_does_not_clear_dirty_or_continue_close() {
        let app = NotepadApp::new(None);
        app.load_document(Some("original.txt".into()), "keep".into());
        app.replace_range(4..4, " this");
        app.session.update(|session| {
            session.request(PendingAction::Close);
        });
        app.save_to_path_with("never-created.txt".into(), reject_worker);
        app.poll_results();
        assert!(app.session.get().operation.is_none());
        assert!(app.session.get().dirty());
        assert_eq!(app.session.get().pending, Some(PendingAction::Close));
        assert_eq!(app.session.get().path, Some("original.txt".into()));
        assert_eq!(app.content.get().as_str(), "keep this");
        assert!(app.status.get().contains("worker capacity exhausted"));
        app.cancel_pending();
        assert_eq!(app.session.get().pending, None);
        assert!(app.begin(Operation::Save).is_some());
    }

    #[test]
    fn stale_worker_start_error_cannot_affect_new_document_operation() {
        let app = NotepadApp::new(None);
        app.open_path_with("never-opened.txt".into(), reject_worker);
        app.load_document(Some("new.txt".into()), "new".into());
        let current = app.begin(Operation::Save).unwrap();
        app.status.set("Current operation".into());
        app.poll_results();
        assert_eq!(
            app.session.get().operation,
            Some((current, Operation::Save))
        );
        assert_eq!(app.content.get().as_str(), "new");
        assert_eq!(app.status.get(), "Current operation");
    }

    #[test]
    fn status_fits_available_width_and_keeps_unicode_clusters() {
        let text = "Opened /a/very/long/path/日本語 cafe\u{301} 👩‍💻 document.txt";
        for width in [0.0, 20.0, 80.0, 160.0, 380.0] {
            let fitted = fitted_status(text, width);
            assert!(scarlet_ui::graphics::measure_text_sized(&fitted, 11.0).0 as f32 <= width);
            if let Some(prefix) = fitted.strip_suffix('…') {
                assert_eq!(
                    TextPosition::new(prefix.len()).clamp_to_grapheme(text).byte,
                    prefix.len()
                );
            }
        }
        assert_eq!(fitted_status("Ready", 100.0), "Ready");
    }

    #[test]
    fn cursor_position_updates_after_release_events() {
        let mut app = NotepadApp::new(None);
        app.load_document(None, "猫\nhello".into());
        let selection = TextSelection::collapsed(6);
        app.selection.set(selection);
        // Key-up can observe the new selection before the next idle callback.
        app.runtime.get().lock().unwrap().current.selection = selection;
        app.on_idle();
        assert!(app.position.get().contains("Ln 2, Col 3"));
    }

    #[test]
    fn unicode_edits_undo_redo_and_saved_state() {
        let app = NotepadApp::new(None);
        app.load_document(Some("notes.txt".into()), "猫🐈\nhello".into());
        app.selection.set(TextSelection {
            anchor: TextPosition::new(0),
            caret: TextPosition::new(3),
        });
        app.replace_range(0..3, "犬");
        assert_eq!(app.content.get().as_str(), "犬🐈\nhello");
        assert!(app.session.get().dirty());
        app.restore_history(false);
        assert_eq!(app.content.get().as_str(), "猫🐈\nhello");
        assert_eq!(app.selection.get().normalized_range(), Some(0..3));
        assert!(!app.session.get().dirty());
        app.restore_history(true);
        assert_eq!(app.content.get().as_str(), "犬🐈\nhello");
        assert!(app.session.get().dirty());
        app.restore_history(false);
        app.replace_range(0..3, "鳥");
        app.restore_history(true);
        assert_eq!(app.content.get().as_str(), "鳥🐈\nhello");
    }

    struct TestClipboard {
        fail: bool,
    }
    impl scarlet_ui::clipboard::ClipboardProvider for TestClipboard {
        fn read_text(&self) -> std::result::Result<Option<String>, ClipboardError> {
            Ok(Some("猫".into()))
        }
        fn write_text(&self, _: &str) -> std::result::Result<(), ClipboardError> {
            if self.fail {
                Err(ClipboardError::AccessDenied)
            } else {
                Ok(())
            }
        }
    }
    fn finish_clipboard(app: &NotepadApp) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            app.poll_clipboard();
            if app.runtime.get().lock().unwrap().clipboard_job.is_none() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "clipboard worker timed out"
            );
            std::thread::yield_now();
        }
    }
    #[test]
    fn failed_clipboard_write_preserves_cut_selection_and_document() {
        let app = NotepadApp::new(None);
        app.load_document(None, "猫 text".into());
        app.select_all();
        let selection = app.selection.get();
        app.runtime.get().lock().unwrap().clipboard =
            Some(TextClipboard::new(TestClipboard { fail: true }));
        app.copy(true);
        finish_clipboard(&app);
        assert_eq!(app.content.get().as_str(), "猫 text");
        assert_eq!(app.selection.get(), selection);
        assert!(!app.session.get().dirty());
        assert!(app.status.get().contains("Focus"));
    }
    #[test]
    fn clipboard_cut_is_undoable_and_stale_paste_is_rejected() {
        let app = NotepadApp::new(None);
        app.load_document(None, "猫 text".into());
        app.select_all();
        app.runtime.get().lock().unwrap().clipboard =
            Some(TextClipboard::new(TestClipboard { fail: false }));
        app.copy(true);
        finish_clipboard(&app);
        assert_eq!(app.content.get().as_str(), "");
        app.restore_history(false);
        assert_eq!(app.content.get().as_str(), "猫 text");
        app.paste();
        app.replace_range(0..3, "犬");
        finish_clipboard(&app);
        assert_eq!(app.content.get().as_str(), "犬 text");
        assert!(app.status.get().contains("Paste cancelled"));
    }
    #[test]
    fn clipboard_target_guard_rejects_new_documents_and_moved_selections() {
        let app = NotepadApp::new(None);
        app.load_document(None, "first".into());
        let identity = app.session.get().identity();
        let selection = app.selection.get();
        assert!(app.clipboard_target_unchanged(identity, selection));
        app.selection.set(TextSelection::collapsed(2));
        assert!(!app.clipboard_target_unchanged(identity, selection));
        app.selection.set(selection);
        app.load_document(None, "other".into());
        assert!(!app.clipboard_target_unchanged(identity, selection));
    }
    #[test]
    fn replace_all_is_one_undo_step_and_empty_query_is_safe() {
        let app = NotepadApp::new(None);
        app.load_document(None, "猫 猫 🐈".into());
        app.query.set("猫".into());
        app.replacement.set("犬".into());
        app.replace_all();
        assert_eq!(app.content.get().as_str(), "犬 犬 🐈");
        app.restore_history(false);
        assert_eq!(app.content.get().as_str(), "猫 猫 🐈");
        assert!(!app.session.get().dirty());
        app.query.set(String::new());
        app.replace_all();
        assert_eq!(app.content.get().as_str(), "猫 猫 🐈");
        assert!(!app.session.get().dirty());
    }

    #[test]
    fn queued_save_never_clears_newer_edits_or_runs_pending_close() {
        let app = NotepadApp::new(None);
        app.replace_range(0..0, "first");
        app.session.update(|s| {
            s.request(PendingAction::Close);
        });
        let ticket = app.begin(Operation::Save).unwrap();
        app.replace_range(5..5, " newer");
        app.runtime
            .get()
            .lock()
            .unwrap()
            .results
            .push_back(IoResult::Save(ticket, "note.txt".into(), Ok(())));
        app.poll_results();
        assert!(app.session.get().dirty());
        assert_eq!(app.session.get().pending, Some(PendingAction::Close));
        assert_eq!(app.content.get().as_str(), "first newer");
    }

    #[test]
    fn cancelled_pending_action_stays_cancelled_after_save() {
        let app = NotepadApp::new(None);
        app.replace_range(0..0, "keep");
        app.session.update(|s| {
            s.request(PendingAction::New);
        });
        let ticket = app.begin(Operation::Save).unwrap();
        app.cancel_pending();
        app.runtime
            .get()
            .lock()
            .unwrap()
            .results
            .push_back(IoResult::Save(ticket, "note.txt".into(), Ok(())));
        app.poll_results();
        assert_eq!(app.content.get().as_str(), "keep");
        assert_eq!(app.session.get().pending, None);
        assert!(!app.session.get().dirty());
    }

    #[test]
    fn open_result_after_edit_or_newer_request_is_ignored() {
        let app = NotepadApp::new(None);
        let old = app.begin(Operation::Read).unwrap();
        app.replace_range(0..0, "keep my edit");
        app.runtime
            .get()
            .lock()
            .unwrap()
            .results
            .push_back(IoResult::Open(
                old,
                "other.txt".into(),
                Ok("replacement".into()),
            ));
        app.poll_results();
        assert_eq!(app.content.get().as_str(), "keep my edit");
        assert!(app.session.get().dirty());
        assert_eq!(app.session.get().path, None);
        let active = app.begin(Operation::Read).unwrap();
        app.runtime
            .get()
            .lock()
            .unwrap()
            .results
            .push_back(IoResult::Open(
                old,
                "other.txt".into(),
                Ok("replacement".into()),
            ));
        app.poll_results();
        assert_eq!(app.session.get().operation.unwrap().0, active);
        assert_eq!(app.content.get().as_str(), "keep my edit");
    }

    #[test]
    fn save_failure_keeps_document_path_dirty_and_pending_action() {
        let app = NotepadApp::new(None);
        app.load_document(Some("original.txt".into()), "old".into());
        app.replace_range(0..3, "new");
        app.session.update(|s| {
            s.request(PendingAction::New);
        });
        let ticket = app.begin(Operation::Save).unwrap();
        app.runtime
            .get()
            .lock()
            .unwrap()
            .results
            .push_back(IoResult::Save(
                ticket,
                "different.txt".into(),
                Err("disk full".into()),
            ));
        app.poll_results();
        assert_eq!(app.content.get().as_str(), "new");
        assert!(app.session.get().dirty());
        assert_eq!(app.session.get().path, Some("original.txt".into()));
        assert_eq!(app.session.get().pending, Some(PendingAction::New));
        assert!(app.status.get().contains("disk full"));
    }

    #[test]
    fn new_requires_explicit_discard_and_clears_history() {
        let app = NotepadApp::new(None);
        app.replace_range(0..0, "keep");
        app.request(PendingAction::New);
        assert_eq!(app.content.get().as_str(), "keep");
        app.cancel_pending();
        assert_eq!(app.content.get().as_str(), "keep");
        app.request(PendingAction::New);
        app.discard_pending();
        assert!(app.content.get().is_empty());
        assert!(!app.session.get().dirty());
        app.restore_history(false);
        assert!(app.content.get().is_empty());
    }
}
