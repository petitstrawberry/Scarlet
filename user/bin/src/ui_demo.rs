//! UI Demo - Demonstrates scarlet-ui reactive state management
//!
//! This demo showcases:
//! - Reactive State<T> with automatic UI updates
//! - Two-way Binding<T> for controls
//! - Timer-based automatic updates
//! - ReactiveLabel for auto-updating text
//! - All controls with proper rounded corners
//! - View modifiers and styling

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::time::Duration;
use scarlet_ui::{
    Application, Button, Center, CheckBox, Color, HStack, Label, Padding, ProgressBar,
    ReactiveLabel, RectView, Slider, Spacer, State, TextField, Timer, Toggle, VStack, Window,
};
use std::{format, println, string::String};

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[ui_demo] Starting ScarletUI Reactive Demo");

    // Create application
    let mut app = match Application::new() {
        Ok(a) => a,
        Err(e) => {
            println!("[ui_demo] Failed to create application: {}", e);
            return 1;
        }
    };
    println!("[ui_demo] Connected to SWS");

    app.set_terminate_after_last_window_closed(true);
    let handle = app.handle();

    // ========================================================================
    // Reactive State - all UI automatically updates when these change
    // ========================================================================

    // Counter with ReactiveLabel
    let counter = State::new(0i32);

    // Progress bar state (auto-increments via timer)
    let progress = State::new(0.0f32);

    // Text input state (two-way binding)
    let text_input = State::new(String::from(""));

    // CheckBox states
    let feature_a = State::new(true);
    let feature_b = State::new(false);

    // Slider state
    let slider_value = State::new(0.5f32);

    // Toggle states
    let toggle1 = State::new(true);
    let toggle2 = State::new(false);

    // ========================================================================
    // Timer - automatically updates progress and logs state
    // ========================================================================

    let progress_timer = progress.clone();
    let counter_timer = counter.clone();
    Timer::periodic(Duration::from_millis(100), move || {
        // Auto-increment progress
        let new_progress = (progress_timer.get() + 0.01).min(1.0);
        if new_progress >= 1.0 {
            progress_timer.set(0.0);
            // Also increment counter when progress resets
            counter_timer.set(counter_timer.get() + 1);
        } else {
            progress_timer.set(new_progress);
        }
    });

    // ========================================================================
    // Clone states for button callbacks
    // ========================================================================

    let counter_reset = counter.clone();
    let counter_inc = counter.clone();
    let progress_reset = progress.clone();

    // ========================================================================
    // Build UI with reactive bindings
    // ========================================================================

    let popup_handle = handle.clone();

    let window = Window::new("ScarletUI Reactive Demo", 650, 700)
        .min_size(650, 700)
        .background(Color::rgb(245, 245, 250))
        .content(
            Padding::new(
                VStack::new()
                    .spacing(16)
                    // Title
                    .child(
                        Label::new("🎨 ScarletUI Reactive Gallery")
                            .color(Color::rgb(40, 40, 50))
                            .font_size(28),
                    )
                    .child(
                        Label::new("State<T> & Binding<T> Demo")
                            .color(Color::GRAY)
                            .font_size(14),
                    )
                    .child(Spacer::new().min_length(8))
                    // --------------------------------------------------------
                    // Reactive Counter - ReactiveLabel auto-updates
                    // --------------------------------------------------------
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("Reactive Counter:")
                                    .color(Color::TEXT)
                                    .font_size(14),
                            )
                            .child(
                                ReactiveLabel::new(counter.clone(), |count| {
                                    format!("Count: {}", count)
                                })
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
                                        println!("[ui_demo] Counter: {}", counter_inc.get());
                                    })),
                            ),
                    )
                    // --------------------------------------------------------
                    // Progress Bar - reacts to State<f32>
                    // --------------------------------------------------------
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("Auto Progress (resets at 100%):")
                                    .color(Color::TEXT)
                                    .font_size(14),
                            )
                            .child(
                                ProgressBar::new(progress.clone())
                                    .fill_color(Color::rgb(50, 200, 100))
                                    .corner_radius(8)
                                    .height(20),
                            )
                            .child(
                                HStack::new()
                                    .spacing(8)
                                    .child(
                                        ReactiveLabel::new(progress.clone(), |p| {
                                            format!("{:.0}%", p * 100.0)
                                        })
                                        .color(Color::GRAY)
                                        .font_size(12),
                                    )
                                    .child(Spacer::new())
                                    .child(Button::new("Reset", move || {
                                        progress_reset.set(0.0);
                                    })),
                            ),
                    )
                    // --------------------------------------------------------
                    // TextField with Binding
                    // --------------------------------------------------------
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("TextField (with Binding):")
                                    .color(Color::TEXT)
                                    .font_size(14),
                            )
                            .child(
                                TextField::new("Enter your name...", text_input.binding())
                                    .corner_radius(6),
                            ),
                    )
                    // --------------------------------------------------------
                    // CheckBoxes with Binding
                    // --------------------------------------------------------
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("CheckBox (with Binding):")
                                    .color(Color::TEXT)
                                    .font_size(14),
                            )
                            .child(
                                HStack::new()
                                    .spacing(16)
                                    .child(CheckBox::new("Feature A", feature_a.binding()))
                                    .child(CheckBox::new("Feature B", feature_b.binding())),
                            ),
                    )
                    // --------------------------------------------------------
                    // Slider with Binding + ReactiveLabel
                    // --------------------------------------------------------
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("Slider (with Binding):")
                                    .color(Color::TEXT)
                                    .font_size(14),
                            )
                            .child(Slider::new(0.0, 1.0, slider_value.binding()))
                            .child(
                                ReactiveLabel::new(slider_value.clone(), |v| {
                                    format!("Value: {:.2}", v)
                                })
                                .color(Color::GRAY)
                                .font_size(12),
                            ),
                    )
                    // --------------------------------------------------------
                    // Toggle with Binding
                    // --------------------------------------------------------
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("Toggle (with Binding):")
                                    .color(Color::TEXT)
                                    .font_size(14),
                            )
                            .child(
                                HStack::new()
                                    .spacing(16)
                                    .child(Toggle::new(toggle1.binding()))
                                    .child(Toggle::new(toggle2.binding())),
                            ),
                    )
                    // --------------------------------------------------------
                    // RectViews with corner radius
                    // --------------------------------------------------------
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("RectView with corner_radius:")
                                    .color(Color::TEXT)
                                    .font_size(14),
                            )
                            .child(
                                HStack::new()
                                    .spacing(12)
                                    .child(Spacer::new())
                                    .child(
                                        RectView::new(Color::rgb(255, 100, 100))
                                            .width(60)
                                            .height(60)
                                            .corner_radius(12)
                                            .border(2, Color::rgb(200, 50, 50)),
                                    )
                                    .child(
                                        RectView::new(Color::rgb(100, 255, 100))
                                            .width(60)
                                            .height(60)
                                            .corner_radius(12)
                                            .border(2, Color::rgb(50, 200, 50)),
                                    )
                                    .child(
                                        RectView::new(Color::rgb(100, 100, 255))
                                            .width(60)
                                            .height(60)
                                            .corner_radius(12)
                                            .border(2, Color::rgb(50, 50, 200)),
                                    )
                                    .child(
                                        RectView::new(Color::rgb(255, 200, 50))
                                            .width(60)
                                            .height(60)
                                            .corner_radius(30), // Circle!
                                    )
                                    .child(Spacer::new()),
                            ),
                    )
                    .child(Spacer::new())
                    // --------------------------------------------------------
                    // Action buttons
                    // --------------------------------------------------------
                    .child(Center::new(
                        HStack::new()
                            .spacing(12)
                            .child(Button::new("Popup", move || {
                                println!("[ui_demo] Popup clicked");
                                popup_handle.request_popup();
                            }))
                            .child(Button::new("Info", || {
                                println!("[ui_demo] All controls use State<T>/Binding<T>!");
                            })),
                    )),
            )
            .all(20),
        );

    // Add window
    if let Err(e) = app.add_window(window) {
        println!("[ui_demo] Failed to add window: {}", e);
        return 1;
    }
    println!("[ui_demo] Window created - reactive state is live!");

    // Run event loop
    app.run();
}
