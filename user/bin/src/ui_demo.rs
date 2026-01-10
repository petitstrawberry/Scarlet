//! UI Demo - Demonstrates scarlet-ui with the new View-based architecture
//!
//! Architecture:
//! - `sws-client`: Low-level SWS connection and protocol
//! - `scarlet-ui`: High-level UI toolkit with View hierarchy
//!
//! This demo shows how to use the new API where:
//! - Application owns the event loop (no manual poll_event)
//! - Window is a View with built-in decorations
//! - Views compose hierarchically (VStack, HStack, etc.)

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_ui::{
    Application, Button, Center, CheckBox, Color, HStack, Label, Padding, ProgressBar, RectView,
    Slider, Spacer, TextField, Toggle, VStack, ViewModifier, Window,
};
use std::println;

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[ui_demo] Starting UI demo application");

    // Create application (connects to SWS)
    let mut app = match Application::new() {
        Ok(a) => a,
        Err(e) => {
            println!("[ui_demo] Failed to create application: {}", e);
            return 1;
        }
    };
    println!("[ui_demo] Connected to SWS");

    // AppKit-like: opt into terminating after the last window is closed.
    app.set_terminate_after_last_window_closed(true);

    let handle = app.handle();

    // Debug: visualize view layout bounds
    // app.set_layout_debug(true);

    // Build the UI using View composition
    let popup_handle = handle.clone();
    let follow_handle = handle.clone();
    let resize_handle = handle.clone();
    let window = Window::new("ScarletUI Widget Demo", 600, 500)
        .min_size(600, 500)
        .max_size(1024, 768)
        .background(Color::rgb(245, 245, 250))
        .content(
            Padding::new(
                VStack::new()
                    .spacing(16)
                    // Title
                    .child(
                        Label::new("🎨 ScarletUI Widget Gallery")
                            .color(Color::rgb(40, 40, 50))
                            .font_size(32),
                    )
                    .child(
                        Label::new("Modern UI Components & Modifiers")
                            .color(Color::GRAY)
                            .font_size(14),
                    )
                    // Spacer
                    .child(Spacer::new().min_length(10))
                    // TextField demo
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(Label::new("TextField:").color(Color::TEXT).font_size(16))
                            .child(
                                TextField::new("Enter your name...")
                                    .text_color(Color::BLACK)
                                    .background(Color::WHITE),
                            ),
                    )
                    // CheckBox demo
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(Label::new("CheckBox:").color(Color::TEXT).font_size(16))
                            .child(
                                HStack::new()
                                    .spacing(16)
                                    .child(CheckBox::new("Enable feature A", true).on_toggle(
                                        |checked| {
                                            println!("[ui_demo] CheckBox A: {}", checked);
                                        },
                                    ))
                                    .child(CheckBox::new("Enable feature B", false).on_toggle(
                                        |checked| {
                                            println!("[ui_demo] CheckBox B: {}", checked);
                                        },
                                    )),
                            ),
                    )
                    // Slider demo
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(Label::new("Slider:").color(Color::TEXT).font_size(16))
                            .child(Slider::new(0.5, 0.0, 1.0).on_change(|value| {
                                println!("[ui_demo] Slider value: {:.2}", value);
                            })),
                    )
                    // ProgressBar demo
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(Label::new("ProgressBar:").color(Color::TEXT).font_size(16))
                            .child(
                                ProgressBar::new(0.7)
                                    .fill_color(Color::rgb(50, 200, 100))
                                    .height(20),
                            ),
                    )
                    // Toggle demo
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(Label::new("Toggle:").color(Color::TEXT).font_size(16))
                            .child(
                                HStack::new()
                                    .spacing(16)
                                    .child(Toggle::new(true).on_toggle(|enabled| {
                                        println!("[ui_demo] Toggle 1: {}", enabled);
                                    }))
                                    .child(Toggle::new(false).on_toggle(|enabled| {
                                        println!("[ui_demo] Toggle 2: {}", enabled);
                                    })),
                            ),
                    )
                    // Spacer
                    .child(Spacer::new())
                    // Colored boxes with modifiers
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("View Modifiers:")
                                    .color(Color::TEXT)
                                    .font_size(16),
                            )
                            .child(
                                HStack::new()
                                    .spacing(12)
                                    .child(Spacer::new())
                                    .child(
                                        RectView::new(Color::rgb(255, 100, 100))
                                            .width(60)
                                            .height(60)
                                            .border(2, Color::rgb(200, 50, 50)),
                                    )
                                    .child(
                                        RectView::new(Color::rgb(100, 255, 100))
                                            .width(60)
                                            .height(60)
                                            .border(2, Color::rgb(50, 200, 50)),
                                    )
                                    .child(
                                        RectView::new(Color::rgb(100, 100, 255))
                                            .width(60)
                                            .height(60)
                                            .border(2, Color::rgb(50, 50, 200)),
                                    )
                                    .child(Spacer::new()),
                            ),
                    )
                    // Spacer
                    .child(Spacer::new())
                    // Interactive buttons
                    .child(Center::new(
                        HStack::new()
                            .spacing(12)
                            .child(Button::new("Popup", move || {
                                println!("[ui_demo] Popup button clicked!");
                                popup_handle.request_popup();
                            }))
                            .child(Button::new("Toggle Follow", move || {
                                println!("[ui_demo] Toggle Follow clicked!");
                                follow_handle.toggle_popup_follow_parent_move();
                            }))
                            .child(Button::new("Resize", move || {
                                println!("[ui_demo] Resize clicked!");
                                resize_handle.toggle_main_resize();
                            }))
                            .child(Button::new("Info", || {
                                println!("[ui_demo] Info button clicked!");
                            }))
                            .child(Button::new("Exit", || {
                                println!("[ui_demo] Exit button clicked!");
                                // Note: Close request will be handled by window
                            })),
                    )),
            )
            .all(20), // 20px padding on all sides
        );

    // Add window to application
    if let Err(e) = app.add_window(window) {
        println!("[ui_demo] Failed to add window: {}", e);
        return 1;
    }
    println!("[ui_demo] Window created");

    // Run the application - this never returns
    // The framework handles all events, layout, and drawing
    println!("[ui_demo] Starting event loop (framework takes control)");
    app.run();
}
