//! ScrollView Demo - Demonstrates the ScrollView container
//!
//! This example shows:
//! - Vertical scrolling with a scrollbar
//! - Mouse wheel scrolling
//! - Drag-to-scroll on the scrollbar
//! - Click-to-jump on the scrollbar track
//! - Horizontal scrolling
//! - Bidirectional scrolling

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_ui::{
    Application, Color, HStack, Label, Padding, RectView, ScrollView, Spacer, VStack, Window,
};

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[scrollview_demo] Starting ScrollView Demo");

    let mut app = match Application::new() {
        Ok(mut a) => {
            a.app_id("org.scarlet.scrollview_demo");
            a
        }
        Err(e) => {
            println!("[scrollview_demo] Failed to create application: {}", e);
            return 1;
        }
    };

    // Create a large scrollable content
    let mut scrollable_content = VStack::new().spacing(8);

    // Add many items to demonstrate scrolling
    for i in 0..50 {
        scrollable_content = scrollable_content.child(
            HStack::new()
                .spacing(8)
                .child(
                    RectView::new(Color::rgb(100 + (i * 3) % 156, 150, 200))
                        .width(40)
                        .height(40)
                        .corner_radius(8),
                )
                .child(
                    VStack::new()
                        .spacing(4)
                        .child(
                            Label::new(format!("Item {}", i + 1))
                                .color(Color::rgb(40, 40, 50))
                                .font_size(18),
                        )
                        .child(
                            Label::new(format!("This is item number {} in the scrollable list", i + 1))
                                .color(Color::GRAY)
                                .font_size(14),
                        ),
                ),
        );
    }

    // Main window with ScrollView
    let window = Window::new("ScrollView Demo", 400, 500)
        .min_size(400, 500)
        .background(Color::rgb(245, 245, 250))
        .content(
            Padding::new(
                VStack::new()
                    .spacing(16)
                    .child(
                        Label::new("ScrollView Demonstration")
                            .color(Color::rgb(40, 40, 50))
                            .font_size(24),
                    )
                    .child(
                        Label::new("Scroll down to see more items!")
                            .color(Color::GRAY)
                            .font_size(14),
                    )
                    .child(Spacer::new().min_length(8))
                    .child(
                        // Vertical scrolling example
                        ScrollView::new(scrollable_content)
                            .scrollbar_width(12)
                            .scrollbar_color(Color::rgb(140, 140, 140))
                            .scrollbar_track_color(Color::rgb(70, 70, 70))
                            .wheel_scroll_speed(40),
                    ),
            )
            .all(20),
        );

    if let Err(e) = app.add_window(window) {
        println!("[scrollview_demo] Failed to add window: {}", e);
        return 1;
    }

    println!("[scrollview_demo] Running application");
    app.run();
}

/// Example: Horizontal scrolling
#[allow(dead_code)]
fn horizontal_scroll_example() {
    let mut horizontal_content = HStack::new().spacing(16);

    // Add many items horizontally
    for i in 0..20 {
        horizontal_content = horizontal_content.child(
            VStack::new()
                .spacing(8)
                .child(
                    RectView::new(Color::rgb(100 + i * 7, 150, 200))
                        .width(80)
                        .height(80)
                        .corner_radius(12),
                )
                .child(Label::new(format!("Col {}", i + 1)).color(Color::TEXT)),
        );
    }

    let _ = ScrollView::new(horizontal_content)
        .shows_horizontal_scrollbar(true)
        .shows_vertical_scrollbar(false);
}

/// Example: Bidirectional scrolling
#[allow(dead_code)]
fn bidirectional_scroll_example() {
    let mut content = VStack::new().spacing(0);

    // Create a grid larger than the view
    for row in 0..20 {
        let mut row_content = HStack::new().spacing(0);
        for col in 0..20 {
            let color = if (row + col) % 2 == 0 {
                Color::rgb(220, 220, 220)
            } else {
                Color::rgb(240, 240, 240)
            };
            row_content = row_content.child(
                RectView::new(color)
                    .width(60)
                    .height(60)
                    .border(1, Color::rgb(200, 200, 200)),
            );
        }
        content = content.child(row_content);
    }

    let _ = ScrollView::new(content)
        .shows_vertical_scrollbar(true)
        .shows_horizontal_scrollbar(true)
        .wheel_scroll_speed(50);
}

/// Example: Custom scrollbar appearance
#[allow(dead_code)]
fn custom_scrollbar_example() {
    let content = VStack::new()
        .spacing(8)
        .child(Label::new("Custom scrollbar colors").font_size(20));

    let _ = ScrollView::new(content)
        .scrollbar_width(16)
        .scrollbar_color(Color::rgb(66, 133, 244))  // Google Blue
        .scrollbar_track_color(Color::rgb(30, 30, 30))
        .wheel_scroll_speed(30);
}
