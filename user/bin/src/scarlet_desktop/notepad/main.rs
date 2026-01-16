//! Scarlet Notepad
//!
//! Text editor application for Scarlet Desktop

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_ui::{
    Application, Button, HStack, Label, Padding, RectView, Spacer, StackAlignment, State, Text,
    TextField, VStack, Window, WindowKind, design,
};
use std::{format, fs, println, string::String};

use design::Palette;

/// Calculate line and column from text and cursor position
fn calculate_line_column(text: &str, cursor_pos: usize) -> (usize, usize) {
    let mut line = 1;
    let mut column = 1;

    for (i, c) in text.chars().enumerate() {
        if i >= cursor_pos {
            break;
        }
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }

    (line, column)
}

/// Create a separator line
fn separator() -> RectView {
    RectView::new(Palette::current().separator).height(1)
}

/// Create a menu button with consistent styling
fn menu_button(label: &str, on_click: impl FnMut() + 'static) -> Button<impl FnMut() + 'static> {
    Button::new(label, on_click)
        .background(Palette::current().sidebar_bg)
        .text_color(Palette::current().text_main)
        .corner_radius(6)
        .padding(6)
}

/// Read file content helper
fn read_file_content(path: &str) -> String {
    match fs::File::open(path) {
        Ok(mut file) => {
            let mut content = String::new();
            let mut buffer = [0u8; 4096];

            loop {
                match file.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        for i in 0..n {
                            content.push(buffer[i] as char);
                        }
                    }
                    Err(_) => break,
                }
            }
            content
        }
        Err(_) => String::new(),
    }
}

/// Write file content helper
fn write_file_content(path: &str, content: &str) -> bool {
    match fs::File::create(path) {
        Ok(mut file) => match file.write_all(content.as_bytes()) {
            Ok(()) => true,
            Err(_) => false,
        },
        Err(_) => false,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[notepad] Starting Scarlet Notepad");

    let text_content = State::new(String::new());
    let current_file = State::new(String::from("Untitled"));
    let status_message = State::new(String::from("Ready - New document"));

    let text_content_new = text_content.clone();
    let current_file_new = current_file.clone();
    let status_new = status_message.clone();

    let text_content_open = text_content.clone();
    let current_file_open = current_file.clone();
    let status_open = status_message.clone();

    let text_content_save = text_content.clone();
    let current_file_save = current_file.clone();
    let status_save = status_message.clone();

    let mut app = match Application::new() {
        Ok(mut app) => {
            app.app_id("org.scarlet-os.desktop.notepad");
            app
        }
        Err(e) => {
            println!("[notepad] Failed to connect to SWS: {}", e);
            return 1;
        }
    };

    let window = Window::new("Scarlet Notepad", 950, 680)
        .min_size(550, 450)
        .background(Palette::current().bg)
        .window_type(WindowKind::Normal)
        .main_window()
        .content(
            VStack::new()
                .spacing(0)
                .alignment(StackAlignment::Start)
                // Menu bar
                .child(
                    Padding::new(
                        HStack::new()
                            .spacing(8)
                            .alignment(StackAlignment::Center)
                            .child(
                                Label::new("File")
                                    .color(Palette::current().text_sub)
                                    .font_size(13),
                            )
                            .child(menu_button("New", move || {
                                text_content_new.set(String::new());
                                current_file_new.set(String::from("Untitled"));
                                status_new.set(String::from("New document created"));
                                println!("[notepad] New document");
                            }))
                            .child(menu_button("Open", move || {
                                let path = "/home/user/document.txt";
                                let content = read_file_content(path);
                                if !content.is_empty() || { fs::File::open(path).is_ok() } {
                                    text_content_open.set(content);
                                    current_file_open.set(String::from(path));
                                    status_open.set(format!("Opened: {}", path));
                                    println!("[notepad] Opened: {}", path);
                                } else {
                                    status_open.set(format!("Failed to open: {}", path));
                                    println!("[notepad] Failed to open: {}", path);
                                }
                            }))
                            .child(menu_button("Save", move || {
                                let current = current_file_save.get();
                                let path = if current == "Untitled" {
                                    "/home/user/document.txt"
                                } else {
                                    &current
                                };
                                let content = text_content_save.get();
                                if write_file_content(path, &content) {
                                    current_file_save.set(String::from(path));
                                    status_save.set(format!("Saved: {}", path));
                                    println!("[notepad] Saved: {}", path);
                                } else {
                                    status_save.set(format!("Failed to save: {}", path));
                                    println!("[notepad] Failed to save: {}", path);
                                }
                            }))
                            .child(Spacer::new())
                            .child(
                                Label::new("Ctrl+N: New | Ctrl+O: Open | Ctrl+S: Save")
                                    .color(Palette::current().text_mute)
                                    .font_size(11),
                            )
                            .child(Spacer::new().min_length(16)),
                    )
                    .all(12),
                )
                // Separator
                .child(separator())
                // File info bar
                .child({
                    let current_file_clone = current_file.clone();
                    let current_file_display = current_file_clone.clone();
                    Padding::new(
                        HStack::new()
                            .spacing(12)
                            .alignment(StackAlignment::Center)
                            .child(
                                Label::new("File:")
                                    .color(Palette::current().text_sub)
                                    .font_size(12),
                            )
                            .child(
                                Text::new(move || format!("{}", current_file_clone.get()))
                                    .color(Palette::current().primary)
                                    .font_size(12)
                                    .watch(current_file_display),
                            )
                            .child(Spacer::new()),
                    )
                    .horizontal(16)
                    .vertical(8)
                })
                // Text editing area
                .child(
                    Padding::new(
                        TextField::new("", text_content.clone())
                            .action(move |_text| {
                                // Text action
                            })
                            .text_color(Palette::current().text_main)
                            .background(Palette::current().surface)
                            .border_color(Palette::current().border)
                            .focused_border_color(Palette::current().primary)
                            .padding(16)
                            .corner_radius(8),
                    )
                    .all(16),
                )
                .child(Spacer::new())
                // Status bar separator
                .child(separator())
                // Status bar
                .child({
                    let text_content_clone1 = text_content.clone();
                    let text_content_watch1 = text_content_clone1.clone();
                    let text_content_clone2 = text_content.clone();
                    let text_content_watch2 = text_content_clone2.clone();
                    let status_msg_clone = status_message.clone();
                    let status_msg_watch = status_msg_clone.clone();

                    Padding::new(
                        HStack::new()
                            .spacing(16)
                            .alignment(StackAlignment::Center)
                            .child(
                                Label::new("Line:")
                                    .color(Palette::current().text_mute)
                                    .font_size(11),
                            )
                            .child(
                                Text::new(move || {
                                    let text = text_content_clone1.get();
                                    let cursor_pos = text.len();
                                    let (line, _) = calculate_line_column(&text, cursor_pos);
                                    format!("{}", line)
                                })
                                .color(Palette::current().text_sub)
                                .font_size(11)
                                .watch(text_content_watch1),
                            )
                            .child(
                                Label::new("Column:")
                                    .color(Palette::current().text_mute)
                                    .font_size(11),
                            )
                            .child(
                                Text::new(move || {
                                    let text = text_content_clone2.get();
                                    let cursor_pos = text.len();
                                    let (_, column) = calculate_line_column(&text, cursor_pos);
                                    format!("{}", column)
                                })
                                .color(Palette::current().text_sub)
                                .font_size(11)
                                .watch(text_content_watch2),
                            )
                            .child(Spacer::new())
                            .child(
                                Text::new(move || format!("{}", status_msg_clone.get()))
                                    .color(Palette::current().primary)
                                    .font_size(11)
                                    .watch(status_msg_watch),
                            )
                            .child(Spacer::new().min_length(16))
                            .child(
                                Label::new("UTF-8")
                                    .color(Palette::current().text_mute)
                                    .font_size(11),
                            ),
                    )
                    .all(12)
                }),
        );

    if let Err(e) = app.add_window(window) {
        println!("[notepad] Failed to add window: {}", e);
        return 1;
    }

    println!("[notepad] Running notepad app");
    app.run();
}
