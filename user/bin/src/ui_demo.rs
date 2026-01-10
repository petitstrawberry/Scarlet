//! UI Demo - Demonstrates scarlet-ui with reactive state and timer support
//!
//! Architecture:
//! - `sws-client`: Low-level SWS connection and protocol
//! - `scarlet-ui`: High-level UI toolkit with View hierarchy
//!
//! This demo shows:
//! - Application event loop (no manual poll_event)
//! - Window with built-in decorations
//! - Views compose hierarchically (VStack, HStack, etc.)
//! - Reactive state management with State<T>
//! - Timer-based updates
//! - Thread-safe UI updates

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::time::Duration;
use scarlet_ui::{
    Application, Button, Center, CheckBox, Color, HStack, Label, Padding, ProgressBar, RectView,
    Slider, Spacer, State, TextField, Timer, Toggle, VStack, ViewModifier, Window,
};
use std::{println, format};

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[ui_demo] Starting UI demo application with reactive state");

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

    // Reactive state
    let counter = State::new(0);
    let progress = State::new(0.0f32);

    // Setup timer to auto-increment counter and progress
    let counter_timer = counter.clone();
    let progress_timer = progress.clone();
    Timer::periodic(Duration::from_secs(1), move || {
        counter_timer.set(counter_timer.get() + 1);
        let new_progress = (progress_timer.get() + 0.1).min(1.0);
        progress_timer.set(if new_progress >= 1.0 {
            0.0
        } else {
            new_progress
        });
        println!(
            "[ui_demo] Timer tick: counter={}, progress={:.2}",
            counter_timer.get(),
            progress_timer.get()
        );
    });

    // Debug: visualize view layout bounds
    // app.set_layout_debug(true);

    // Build the UI using View composition
    let popup_handle = handle.clone();
    let follow_handle = handle.clone();
    let resize_handle = handle.clone();
    let counter_reset = counter.clone();
    let counter_inc = counter.clone();

    let window = Window::new("ScarletUI Reactive Demo", 650, 600)
        .min_size(650, 600)
        .max_size(1024, 768)
        .background(Color::rgb(245, 245, 250))
        .content(
            Padding::new(
                VStack::new()
                    .spacing(16)
                    // Title
                    .child(
                        Label::new("🎨 ScarletUI Reactive Gallery")
                            .color(Color::rgb(40, 40, 50))
                            .font_size(32),
                    )
                    .child(
                        Label::new("Reactive State & Timer Demo")
                            .color(Color::GRAY)
                            .font_size(14),
                    )
                    // Spacer
                    .child(Spacer::new().min_length(10))
                    // Counter with reactive state
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("Reactive Counter:")
                                    .color(Color::TEXT)
                                    .font_size(16),
                            )
                            .child(
                                Label::new(format!("Count: {}", counter.get()))
                                    .color(Color::rgb(50, 150, 255))
                                    .font_size(24),
                            )
                            .child(
                                HStack::new()
                                    .spacing(8)
                                    .child(Button::new("Reset", move || {
                                        counter_reset.set(0);
                                        println!("[ui_demo] Counter reset");
                                    }))
                                    .child(Button::new("+1", move || {
                                        counter_inc.set(counter_inc.get() + 1);
                                        println!("[ui_demo] Counter incremented");
                                    })),
                            ),
                    )
                    // Progress bar with reactive state
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("Auto Progress:")
                                    .color(Color::TEXT)
                                    .font_size(16),
                            )
                            .child(
                                ProgressBar::new(progress.get())
                                    .fill_color(Color::rgb(50, 200, 100))
                                    .height(20),
                            )
                            .child(
                                Label::new(format!("{:.0}%", progress.get() * 100.0))
                                    .color(Color::GRAY)
                                    .font_size(12),
                            ),
                    )
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
                                    .child(CheckBox::new("Feature A", true).on_toggle(|checked| {
                                        println!("[ui_demo] CheckBox A: {}", checked);
                                    }))
                                    .child(CheckBox::new("Feature B", false).on_toggle(
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
                            .child(Button::new("Follow", move || {
                                println!("[ui_demo] Toggle Follow clicked!");
                                follow_handle.toggle_popup_follow_parent_move();
                            }))
                            .child(Button::new("Resize", move || {
                                println!("[ui_demo] Resize clicked!");
                                resize_handle.toggle_main_resize();
                            }))
                            .child(Button::new("Info", || {
                                println!("[ui_demo] Info: Reactive state & timers active!");
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
    // The framework handles all events, layout, drawing, timers, and reactive updates
    println!("[ui_demo] Starting event loop (framework takes control)");
    app.run();
}
