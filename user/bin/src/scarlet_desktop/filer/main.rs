//! Scarlet Filer (File Manager)
//!
//! Simple, clean file manager for Scarlet Desktop

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use scarlet_std::println;
use scarlet_ui::{Application, HStack, Spacer, Text, VStack, View, ViewExt, Window, WindowBuilder};

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[filer] Starting file manager");

    let mut app = match Application::new() {
        Ok(mut a) => {
            a.app_id("org.scarlet-os.desktop.filer");
            a
        }
        Err(e) => {
            println!("[filer] Failed to create application: {}", e);
            return 1;
        }
    };

    // Content area
    let ui_content = VStack::new()
        .spacing(16)
        // Title
        .child(Text::new("File Manager").font_size(18))
        // Sidebar area
        .child(
            HStack::new()
                .spacing(16)
                .child(
                    VStack::new()
                        .spacing(8)
                        .child(Text::new("Folders").font_size(12))
                        .child(Text::new("  Home").font_size(13))
                        .child(Text::new("  Desktop").font_size(13))
                        .child(Text::new("  Documents").font_size(13)),
                )
                .child(Spacer::new())
                .background(scarlet_ui::Color::rgb(30, 30, 30))
                .padding(20),
        );

    let window = Window::builder()
        .title("Filer")
        .size(800, 500)
        .min_size(600, 400)
        .build()
        .content(ui_content);

    if let Err(e) = app.add_window(window) {
        println!("[filer] Failed to add window: {}", e);
        return 1;
    }

    println!("[filer] Running file manager");
    app.run();
}
