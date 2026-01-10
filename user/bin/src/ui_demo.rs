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
    Application, Button, Center, Color, HStack, Label, Padding, RectView, Spacer, VStack, Window,
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

    let handle = app.handle();

    // Debug: visualize view layout bounds
    app.set_layout_debug(true);

    // Build the UI using View composition
    let popup_handle = handle.clone();
    let follow_handle = handle.clone();
    let resize_handle = handle.clone();
    let window = Window::new("UI Demo", 420, 300)
        .min_size(420, 300)
        .max_size(1024, 768)
        .background(Color::WHITE)
        .content(
            Padding::new(
                VStack::new()
                    // Title
                    .child(Label::new("Hello, Scarlet UI!").color(Color::TEXT))
                    .child(Label::new("View-based Architecture Demo").color(Color::GRAY))
                    // Spacing
                    .child(Spacer::new())
                    // Colored boxes in a row
                    .child(
                        HStack::new()
                            .child(Spacer::new())
                            .child(
                                RectView::new(Color::rgb(255, 100, 100))
                                    .width(80)
                                    .height(50),
                            )
                            .child(
                                RectView::new(Color::rgb(100, 255, 100))
                                    .width(80)
                                    .height(50),
                            )
                            .child(
                                RectView::new(Color::rgb(100, 100, 255))
                                    .width(80)
                                    .height(50),
                            )
                            .child(Spacer::new()),
                    )
                    // Spacing
                    .child(Spacer::new())
                    // Interactive buttons
                    .child(Center::new(
                        HStack::new()
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
                            .child(Button::new("Click Me", || {
                                println!("[ui_demo] Button 1 clicked!");
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
