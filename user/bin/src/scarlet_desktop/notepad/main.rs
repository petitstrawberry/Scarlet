//! Scarlet Notepad
//!
//! Text editor application for Scarlet Desktop
//!
//! Features:
//! - New, Open, Save file operations
//! - Line and column tracking
//! - Character and line count
//! - Clean, modern UI
//! - Status bar with feedback

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_ui::{
    Application, Button, Color, HStack, Label, Padding, RectView, Spacer,
    State, StackAlignment, TextField, Text, VStack, Window, WindowKind,
};
use std::{format, fs, println, string::String};

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

/// Create a separator line with modern color
fn separator() -> RectView {
    RectView::new(Color::rgb(50, 50, 55)).height(1)
}

/// Create a menu button with consistent styling
fn menu_button(
    label: &str,
    on_click: impl FnMut() + 'static,
) -> Button<impl FnMut() + 'static> {
    Button::new(label, on_click)
        .background(Color::rgb(70, 70, 75))
        .text_color(Color::rgb(230, 230, 235))
        .corner_radius(4)
        .padding(6)
}

/// Create a styled menu label
fn menu_label(text: &str) -> Label {
    Label::new(text)
        .color(Color::rgb(160, 160, 165))
        .font_size(13)
}

/// Read file content helper
fn read_file_content(path: &str) -> String {
    match fs::File::open(path) {
        Ok(mut file) => {
            let mut content = String::new();
            let mut buffer = [0u8; 4096];

            loop {
                match file.read(&mut buffer) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        // Convert bytes to string (assuming UTF-8)
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
        Ok(mut file) => {
            match file.write_all(content.as_bytes()) {
                Ok(()) => true,
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[notepad] Starting Scarlet Notepad");

    // Application state
    let text_content = State::new(String::new());
    let current_file = State::new(String::from("Untitled"));
    let status_message = State::new(String::from("Ready - New document"));

    // File operations state clones
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

    // Create notepad window
    let window = Window::new("Scarlet Notepad", 900, 650)
        .min_size(500, 400)
        .background(Color::rgb(28, 28, 32))
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
                            .spacing(6)
                            .alignment(StackAlignment::Center)
                            .child(menu_label("File"))
                            .child(menu_button("New", move || {
                                // New: Clear content and reset filename
                                text_content_new.set(String::new());
                                current_file_new.set(String::from("Untitled"));
                                status_new.set(String::from("New document created"));
                                println!("[notepad] New document");
                            }))
                            .child(menu_button("Open", move || {
                                // Open: Read from default path
                                let path = "/home/user/document.txt";
                                let content = read_file_content(path);
                                if !content.is_empty() || {
                                    // Check if file exists and is empty
                                    fs::File::open(path).is_ok()
                                } {
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
                                // Save: Write to default path
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
                                Label::new("Shortcuts: New | Open | Save")
                                    .color(Color::rgb(100, 100, 105))
                                    .font_size(11),
                            )
                            .child(Spacer::new().min_length(16)),
                    )
                    .all(8),
                )
                // Separator
                .child(separator())
                // File info bar
                .child({
                    let current_file_clone = current_file.clone();
                    let current_file_clonea = current_file_clone.clone();
                    Padding::new(
                        HStack::new()
                            .spacing(12)
                            .alignment(StackAlignment::Center)
                            .child(
                                Text::new(move || {
                                    format!("File: {}", current_file_clone.get())
                                })
                                .color(Color::rgb(100, 170, 255))
                                .font_size(12)
                                .watch(current_file_clonea),
                            )
                            .child(Spacer::new()),
                    )
                    .horizontal(12)
                    .vertical(6)
                })
                // Text editing area
                .child(
                    Padding::new(
                        TextField::new("", text_content.clone())
                            .action(move |_text| {
                                // Text action - could auto-save or show modified status
                            })
                            .background(Color::rgb(38, 38, 42))
                            .text_color(Color::rgb(230, 230, 235))
                            .border_color(Color::rgb(70, 70, 75))
                            .focused_border_color(Color::rgb(100, 150, 255))
                            .padding(12)
                            .corner_radius(6),
                    )
                    .all(12),
                )
                .child(Spacer::new())
                // Status bar separator
                .child(separator())
                // Status bar
                .child({
                    let text_content_clone1 = text_content.clone();
                    let text_content_clone1a = text_content_clone1.clone();
                    let text_content_clone2 = text_content.clone();
                    let text_content_clone2a = text_content_clone2.clone();
                    let status_msg_clone = status_message.clone();
                    let status_msg_clonea = status_msg_clone.clone();

                    Padding::new(
                        HStack::new()
                            .spacing(16)
                            .alignment(StackAlignment::Center)
                            .child(
                                Text::new(move || {
                                    let text = text_content_clone1.get();
                                    let cursor_pos = text.len();
                                    let (line, column) = calculate_line_column(&text, cursor_pos);
                                    format!("Ln {}, Col {}", line, column)
                                })
                                .color(Color::rgb(140, 140, 145))
                                .font_size(12)
                                .watch(text_content_clone1a),
                            )
                            .child(
                                Text::new(move || {
                                    let text = text_content_clone2.get();
                                    let lines = text.lines().count();
                                    let chars = text.len();
                                    format!("{} lines, {} chars", lines, chars)
                                })
                                .color(Color::rgb(140, 140, 145))
                                .font_size(12)
                                .watch(text_content_clone2a),
                            )
                            .child(Spacer::new())
                            .child(
                                Text::new(move || format!("{}", status_msg_clone.get()))
                                    .color(Color::rgb(100, 180, 100))
                                    .font_size(12)
                                    .watch(status_msg_clonea),
                            )
                            .child(Spacer::new().min_length(16))
                            .child(
                                Label::new("UTF-8")
                                    .color(Color::rgb(120, 120, 125))
                                    .font_size(12),
                            ),
                    )
                    .horizontal(12)
                    .vertical(6)
                }),
        );

    if let Err(e) = app.add_window(window) {
        println!("[notepad] Failed to add window: {}", e);
        return 1;
    }

    println!("[notepad] Running notepad app");
    println!("[notepad] File operations: New, Open (/home/user/document.txt), Save");
    app.run();
}
