//! Scarlet Desktop text editor.
//!
//! Notepad deliberately keeps document editing in ScarletUI's `TextView` and
//! uses the desktop File Manager service for file selection.  This keeps file
//! dialog policy out of the editor while still making the application useful
//! on a fresh Scarlet session.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::string::String;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use sbus::{Argument, Message};
use sbus_client::Connection as SbusConnection;
use scarlet_desktop_config::{
    DESKTOP_FILE_MANAGER_BUS_NAME, DESKTOP_FILE_MANAGER_INTERFACE,
    DESKTOP_FILE_MANAGER_OBJECT_PATH, DESKTOP_FILE_MANAGER_OPEN_FILE_METHOD,
    DESKTOP_FILE_MANAGER_RESPONSE_SIGNAL, DESKTOP_FILE_MANAGER_SAVE_FILE_METHOD,
    DESKTOP_FILES_APP_ID, DESKTOP_STEMD_BUS_NAME, DESKTOP_STEMD_INTERFACE,
    DESKTOP_STEMD_LAUNCH_OR_FOCUS_METHOD, DESKTOP_STEMD_OBJECT_PATH,
};
use scarlet_ui::prelude::*;
use scarlet_ui::{KeyCode, MenuBarModel, MenuEntry, MenuItemModel, hstack, vstack};
use scarlet_ui_macros::View;

const APP_ID: &str = "org.scarlet-os.desktop.notepad";
const FILES_APP_ID: &str = DESKTOP_FILES_APP_ID;
const SERVICE_RETRY_DELAY: Duration = Duration::from_millis(100);
const PICKER_RETRY_ATTEMPTS: usize = 20;

#[derive(Clone, Copy)]
enum PickerAction {
    Open,
    SaveAs,
}

#[derive(View, Clone)]
struct NotepadApp {
    content: State<String>,
    selection: State<TextSelection>,
    scroll: State<TextViewScroll>,
    path: State<Option<String>>,
    dirty: State<bool>,
    status: State<String>,
    picker_request_id: State<Option<String>>,
    picker_action: State<Option<PickerAction>>,
}

impl NotepadApp {
    fn new(initial_path: Option<String>) -> Self {
        let app = Self::default();
        app.status.set(String::from("Ready"));
        if let Some(path) = initial_path {
            app.open_path(path);
        }
        app
    }

    fn window_title(&self) -> String {
        let name = self
            .path
            .get()
            .as_deref()
            .and_then(|path| Path::new(path).file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| String::from("Untitled"));
        if self.dirty.get() {
            format!("{} — Notepad", name)
        } else {
            format!("{} - Notepad", name)
        }
    }

    fn new_document(&self) {
        self.content.set(String::new());
        self.selection.set(TextSelection::collapsed(0));
        self.scroll.set(TextViewScroll::default());
        self.path.set(None);
        self.dirty.set(false);
        self.status.set(String::from("New document"));
    }

    fn open_path(&self, path: String) {
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let length = content.len();
                self.content.set(content);
                self.selection.set(TextSelection::collapsed(0));
                self.scroll.set(TextViewScroll::default());
                self.path.set(Some(path.clone()));
                self.dirty.set(false);
                self.status
                    .set(format!("Opened {} ({} bytes)", path, length));
            }
            Err(error) => self.status.set(format!("Cannot open {path}: {error}")),
        }
    }

    fn save_to_path(&self, path: String) {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        let result = options
            .open(&path)
            .and_then(|mut file| file.write_all(self.content.get().as_bytes()));
        match result {
            Ok(()) => {
                self.path.set(Some(path.clone()));
                self.dirty.set(false);
                self.status.set(format!("Saved {path}"));
            }
            Err(error) => self.status.set(format!("Cannot save {path}: {error}")),
        }
    }

    fn save(&self) {
        if let Some(path) = self.path.get() {
            self.save_to_path(path);
        } else {
            self.save_as();
        }
    }

    fn save_as(&self) {
        let initial_folder = self
            .path
            .get()
            .and_then(|path| PathBuf::from(path).parent().map(Path::to_path_buf))
            .unwrap_or_else(home_path);
        let suggested_name = self
            .path
            .get()
            .and_then(|path| {
                Path::new(&path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| String::from("Untitled.txt"));
        self.request_picker(
            PickerAction::SaveAs,
            DESKTOP_FILE_MANAGER_SAVE_FILE_METHOD,
            vec![
                Argument::String(String::from("Save Document")),
                Argument::String(initial_folder.to_string_lossy().into_owned()),
                Argument::String(suggested_name),
                Argument::String(String::from("text/*")),
            ],
        );
    }

    fn open_picker(&self) {
        self.request_picker(
            PickerAction::Open,
            DESKTOP_FILE_MANAGER_OPEN_FILE_METHOD,
            vec![
                Argument::String(String::from("Open Document")),
                Argument::String(home_path().to_string_lossy().into_owned()),
                Argument::String(String::from("text/*")),
                Argument::Boolean(false),
                Argument::Boolean(false),
            ],
        );
    }

    fn request_picker(&self, action: PickerAction, method: &str, args: Vec<Argument>) {
        self.picker_action.set(Some(action));
        let mut last_error = None;
        for attempt in 0..PICKER_RETRY_ATTEMPTS {
            let result = SbusConnection::connect().and_then(|mut connection| {
                connection.call_method_timeout(
                    DESKTOP_FILE_MANAGER_BUS_NAME,
                    DESKTOP_FILE_MANAGER_OBJECT_PATH,
                    DESKTOP_FILE_MANAGER_INTERFACE,
                    method,
                    args.clone(),
                    2_000,
                )
            });

            match result {
                Ok(arguments) => match arguments.first() {
                    Some(Argument::String(request_id)) => {
                        self.picker_request_id.set(Some(request_id.clone()));
                        self.status.set(String::from("Waiting for file selection"));
                        return;
                    }
                    _ => {
                        self.picker_action.set(None);
                        self.status
                            .set(String::from("File Manager returned no request id"));
                        return;
                    }
                },
                Err(error) => {
                    last_error = Some(error);
                    if attempt == 0 {
                        let _ = ensure_files_service();
                    }
                    thread::sleep(SERVICE_RETRY_DELAY);
                }
            }
        }
        self.picker_request_id.set(None);
        self.picker_action.set(None);
        self.status
            .set(format!("Cannot open file picker: {last_error:?}"));
    }

    fn handle_picker_response(&self, accepted: bool, path: String) {
        let action = self.picker_action.get();
        self.picker_request_id.set(None);
        self.picker_action.set(None);
        if !accepted || path.is_empty() {
            self.status.set(String::from("File selection cancelled"));
            return;
        }

        match action {
            Some(PickerAction::Open) => self.open_path(path),
            Some(PickerAction::SaveAs) => self.save_to_path(path),
            None => {}
        }
    }

    fn handle_key(&self, event: KeyEvent) -> bool {
        match event {
            KeyEvent::Pressed {
                keycode: KeyCode::Char('n' | 'N'),
                modifiers,
            } if modifiers.primary() => {
                self.new_document();
                true
            }
            KeyEvent::Pressed {
                keycode: KeyCode::Char('o' | 'O'),
                modifiers,
            } if modifiers.primary() => {
                self.open_picker();
                true
            }
            KeyEvent::Pressed {
                keycode: KeyCode::Char('s' | 'S'),
                modifiers,
            } if modifiers.control && modifiers.shift => {
                self.save_as();
                true
            }
            KeyEvent::Pressed {
                keycode: KeyCode::Char('s' | 'S'),
                modifiers,
            } if modifiers.control => {
                self.save();
                true
            }
            _ => false,
        }
    }

    fn content_view(&self) -> impl View + Clone + use<> {
        let key_app = self.clone();

        vstack! {
            TextView::new(self.content.clone(), self.selection.clone())
                .scroll_state(self.scroll.clone())
                .wrap_mode(WrapMode::Soft)
                .line_numbers(true)
                .current_line_highlight(true)
                .font_size(15.0)
                .padding(12.0)
                .on_text_change({
                    let dirty = self.dirty.clone();
                    move |_| dirty.set(true)
                })
                .frame(f32::INFINITY, f32::INFINITY),
            hstack! {
                Text::from_state(self.status.clone()).font_size(11.0),
                Spacer::new(),
                Text::new("UTF-8").font_size(11.0),
            }
            .padding(8.0),
        }
        .on_key(move |event| key_app.handle_key(event))
        .frame(f32::INFINITY, f32::INFINITY)
    }

    fn menu_bar(&self) -> MenuBarModel {
        let new_app = self.clone();
        let open_app = self.clone();
        let save_app = self.clone();
        let save_as_app = self.clone();
        MenuBarModel::new(vec![
            MenuItemModel::new("file", "File")
                .on_activate(Arc::new(|| {}))
                .children(vec![
                    MenuEntry::Item(
                        MenuItemModel::new("new", "New")
                            .shortcut("Ctrl+N")
                            .on_activate(Arc::new(move || new_app.new_document())),
                    ),
                    MenuEntry::Item(
                        MenuItemModel::new("open", "Open…")
                            .shortcut("Ctrl+O")
                            .on_activate(Arc::new(move || open_app.open_picker())),
                    ),
                    MenuEntry::Separator,
                    MenuEntry::Item(
                        MenuItemModel::new("save", "Save")
                            .shortcut("Ctrl+S")
                            .on_activate(Arc::new(move || save_app.save())),
                    ),
                    MenuEntry::Item(
                        MenuItemModel::new("save-as", "Save As…")
                            .shortcut("Ctrl+Shift+S")
                            .on_activate(Arc::new(move || save_as_app.save_as())),
                    ),
                ]),
            MenuItemModel::new("edit", "Edit").on_activate(Arc::new(|| {})),
            MenuItemModel::new("view", "View").on_activate(Arc::new(|| {})),
        ])
    }
}

impl Application for NotepadApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new(
            "main",
            Window::new(self.window_title(), self.content_view())
                .app_id(APP_ID)
                .menu_bar(self.menu_bar())
                .size(Size::new(900.0, 680.0)),
        )
    }

    fn init(&mut self) {
        start_picker_listener(self);
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

fn start_picker_listener(app: &NotepadApp) {
    let app = app.clone();
    thread::spawn(move || {
        loop {
            let Ok(mut connection) = SbusConnection::connect() else {
                thread::sleep(SERVICE_RETRY_DELAY);
                continue;
            };

            loop {
                let message = match connection.receive_message() {
                    Ok(message) => message,
                    Err(_) => break,
                };
                let Message::Signal {
                    sender,
                    path,
                    interface,
                    signal,
                    args,
                } = message
                else {
                    continue;
                };
                if sender != DESKTOP_FILE_MANAGER_BUS_NAME
                    || path != DESKTOP_FILE_MANAGER_OBJECT_PATH
                    || interface != DESKTOP_FILE_MANAGER_INTERFACE
                    || signal != DESKTOP_FILE_MANAGER_RESPONSE_SIGNAL
                {
                    continue;
                }

                let Some(Argument::String(response_id)) = args.first() else {
                    continue;
                };
                if app.picker_request_id.get().as_deref() != Some(response_id.as_str()) {
                    continue;
                }
                let accepted = matches!(args.get(1), Some(Argument::Boolean(true)));
                let path = match args.get(2) {
                    Some(Argument::String(path)) => path.clone(),
                    _ => String::new(),
                };
                app.handle_picker_response(accepted, path);
            }
            thread::sleep(SERVICE_RETRY_DELAY);
        }
    });
}

fn ensure_files_service() -> core::result::Result<(), sbus_client::Error> {
    let mut connection = SbusConnection::connect()?;
    let _ = connection.call_method_timeout(
        DESKTOP_STEMD_BUS_NAME,
        DESKTOP_STEMD_OBJECT_PATH,
        DESKTOP_STEMD_INTERFACE,
        DESKTOP_STEMD_LAUNCH_OR_FOCUS_METHOD,
        vec![Argument::String(String::from(FILES_APP_ID))],
        3_000,
    );
    Ok(())
}

fn home_path() -> PathBuf {
    PathBuf::from(
        std::env::var_os("HOME")
            .map(|path| path.to_string_lossy().into_owned())
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| String::from("/root")),
    )
}

fn main() {
    println!("[notepad] starting");
    let initial_path = std::env::args().skip(1).next();
    let mut app = NotepadApp::new(initial_path);
    if let Err(error) = app.run() {
        eprintln!("[notepad] error: {error}");
    }
}
