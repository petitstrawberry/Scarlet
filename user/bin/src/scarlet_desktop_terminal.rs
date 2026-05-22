//! Scarlet Desktop Terminal application.
//!
//! This app provides a GUI-accessible terminal endpoint backed by `/dev/tty*`.

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std;
extern crate scarlet_ui_macros;

use alloc::rc::Rc;
use alloc::string::String;
use core::f32;
use core::result::Result as CoreResult;
use core::time::Duration;
use scarlet_std::format;
use scarlet_std::fs::{File, OpenOptions};
use scarlet_std::println;
use scarlet_std::thread;
use scarlet_ui::prelude::*;
use scarlet_ui::{CanvasView, Event, KeyCode, KeyEvent, Text, vstack};
use scarlet_ui_macros::View;

const TERMINAL_MAX_CHARS: usize = 8192;

fn open_rw_tty(path: &str) -> Option<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    options.open(path).ok()
}

fn trim_terminal_text(text: &mut String) {
    if text.len() > TERMINAL_MAX_CHARS {
        let overflow = text.len() - TERMINAL_MAX_CHARS;
        text.drain(..overflow);
    }
}

fn append_terminal_text(state: &State<String>, text: &str) {
    state.update(|buf| {
        buf.push_str(text);
        trim_terminal_text(buf);
    });
}

fn send_to_tty(path: &str, bytes: &[u8]) -> CoreResult<(), &'static str> {
    let mut options = OpenOptions::new();
    options.write(true);
    let mut file = options
        .open(path)
        .map_err(|_| "failed to open tty for write")?;
    file.write(bytes).map_err(|_| "failed to write tty bytes")?;
    Ok(())
}

#[derive(View, Clone)]
struct ScarletTerminalApp {
    tty_path: State<String>,
    status_text: State<String>,
    terminal_text: State<String>,
}

impl ScarletTerminalApp {
    fn new() -> Self {
        Default::default()
    }

    fn spawn_reader(&self, tty_path: &str, file: &File) {
        let tty_path = String::from(tty_path);
        let read_handle = match file.clone_handle() {
            Ok(h) => h,
            Err(_) => {
                self.status_text.set(format!(
                    "Connected to {}, but failed to clone read handle",
                    tty_path
                ));
                return;
            }
        };
        let mut reader = match File::from_handle(read_handle) {
            Ok(f) => f,
            Err(_) => {
                self.status_text.set(format!(
                    "Connected to {}, but failed to build reader",
                    tty_path
                ));
                return;
            }
        };
        let _ = reader.set_nonblocking(true);
        let terminal_text = self.terminal_text.clone();
        let status_text = self.status_text.clone();
        thread::spawn(move || {
            status_text.set(format!("Connected to {}", tty_path));
            let mut buf = [0u8; 256];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        thread::sleep(Duration::from_millis(16));
                    }
                    Ok(n) => {
                        if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                            append_terminal_text(&terminal_text, s);
                        } else {
                            for b in &buf[..n] {
                                append_terminal_text(&terminal_text, &format!("{:02x} ", b));
                            }
                        }
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(16));
                    }
                }
            }
        });
    }
}

impl Application for ScarletTerminalApp {
    fn init(&mut self) {
        for tty_path in ["/dev/tty1", "/dev/tty2", "/dev/tty3", "/dev/tty0"] {
            if let Some(file) = open_rw_tty(tty_path) {
                self.tty_path.set(tty_path.into());
                append_terminal_text(
                    &self.terminal_text,
                    format!("[scarlet-terminal] attached to {}\n", tty_path).as_str(),
                );
                self.spawn_reader(tty_path, &file);
                return;
            }
        }
        self.status_text
            .set(String::from("No usable /dev/tty* device found"));
    }

    fn body(&self) -> impl View {
        let tty_path = self.tty_path.clone();
        let status_text = self.status_text.clone();

        let keyboard_capture =
            CanvasView::new(1.0, 1.0, Rc::new(|_, _, _| {})).on_event(move |event| {
                let path = tty_path.get();
                if path.is_empty() {
                    return false;
                }
                match event {
                    Event::Keyboard(KeyEvent::Char { c }) => {
                        let mut buf = [0u8; 4];
                        let s = c.encode_utf8(&mut buf);
                        if send_to_tty(&path, s.as_bytes()).is_err() {
                            status_text.set(format!("Write failed: {}", path));
                        }
                        true
                    }
                    Event::Keyboard(KeyEvent::Pressed { keycode }) => {
                        let bytes: &[u8] = match keycode {
                            KeyCode::Enter => b"\n",
                            KeyCode::Backspace => b"\x08",
                            KeyCode::Tab => b"\t",
                            _ => return false,
                        };
                        if send_to_tty(&path, bytes).is_err() {
                            status_text.set(format!("Write failed: {}", path));
                        }
                        true
                    }
                    _ => false,
                }
            });

        Window::new(
            "Terminal",
            vstack! {
                Text::new(format!("TTY: {}", self.tty_path.get())).font_size(14.0),
                Text::new(self.status_text.get()).font_size(14.0),
                Text::new("Type directly in this window. Keyboard input is forwarded to the active TTY.").font_size(12.0),
                Divider::new(),
                Text::new(self.terminal_text.get())
                    .font_size(14.0)
                    .frame(f32::INFINITY, f32::INFINITY),
                keyboard_capture.frame(f32::INFINITY, 1.0),
            }
            .frame(f32::INFINITY, f32::INFINITY),
        )
        .app_id("org.scarlet-os.desktop.terminal")
        .size(Size::new(900.0, 640.0))
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("[scarlet_desktop_terminal] starting");
    let mut app = ScarletTerminalApp::new();
    match app.run() {
        Ok(_) => println!("[scarlet_desktop_terminal] done"),
        Err(e) => println!("[scarlet_desktop_terminal] error: {}", e),
    }
}
