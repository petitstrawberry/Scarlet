//! Scarlet Filer (File Manager)
//!
//! Simple, clean file manager for Scarlet Desktop

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_ui::{
    Application, Color, HStack, Label, Spacer, StackAlignment, State, VStack, ViewModifier, Window,
    WindowKind, design,
};
use std::{println, string::String, vec};

use design::Palette;

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[filer] Starting file manager");

    let mut app = match Application::new() {
        Ok(mut app) => {
            app.app_id("org.scarlet-os.desktop.filer");
            app
        }
        Err(e) => {
            println!("[filer] Failed to connect to SWS: {}", e);
            return 1;
        }
    };

    // File list data
    let files = State::new(vec![
        String::from("Documents"),
        String::from("Downloads"),
        String::from("Pictures"),
        String::from("Music"),
        String::from("Videos"),
    ]);

    let selected = State::new(0usize);

    // Content area
    let content = VStack::new()
        .spacing(16)
        .alignment(StackAlignment::Start)
        // Title
        .child(
            Label::new("File Manager")
                .color(Palette::current().text_main)
                .font_size(18),
        )
        // Sidebar area
        .child(
            HStack::new()
                .spacing(16)
                .alignment(StackAlignment::Start)
                .child(
                    VStack::new()
                        .spacing(8)
                        .alignment(StackAlignment::Start)
                        .child(
                            Label::new("Folders")
                                .color(Palette::current().text_sub)
                                .font_size(12),
                        )
                        .child(
                            Label::new("  Home")
                                .color(Palette::current().primary)
                                .font_size(13),
                        )
                        .child(
                            Label::new("  Desktop")
                                .color(Palette::current().text_main)
                                .font_size(13),
                        )
                        .child(
                            Label::new("  Documents")
                                .color(Palette::current().text_main)
                                .font_size(13),
                        ),
                )
                .child(Spacer::new()),
        )
        .background_color(Palette::current().bg)
        .padding(20);

    let window = Window::new("Filer", 800, 500)
        .window_type(WindowKind::Normal)
        .main_window()
        .min_size(600, 400)
        .content(content);

    if let Err(e) = app.add_window(window) {
        println!("[filer] Failed to add window: {}", e);
        return 1;
    }

    println!("[filer] Running file manager");
    app.run();
}
