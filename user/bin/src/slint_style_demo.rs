//! Slint-Style UI Demo
//!
//! This demo showcases a UI built with ScarletUI that demonstrates the concepts
//! that would be used in a Slint integration. It shows:
//!
//! - Window with ScarletUI decorations
//! - Complex widget layout similar to Slint applications
//! - Reactive state management
//! - Event handling and interaction
//!
//! This serves as a proof-of-concept for the architecture described in
//! docs/slint_backend_architecture.md

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_ui::{
    Application, Button, Center, CheckBox, Color, HStack, Label, Padding, ProgressBar,
    RectView, Slider, Spacer, State, TextField, Toggle, VStack, Window,
};
use std::{format, println, string::String};

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[slint_style_demo] Starting Slint-style UI demonstration");

    let mut app = match Application::new() {
        Ok(a) => a,
        Err(e) => {
            println!("[slint_style_demo] Failed to create application: {}", e);
            return 1;
        }
    };

    app.set_terminate_after_last_window_closed(true);

    // Create reactive state similar to how Slint would manage state
    let counter = State::new(0i32);
    let slider_value = State::new(0.5f32);
    let text_input = State::new(String::from("Hello, Scarlet!"));
    let feature_enabled = State::new(true);
    let switch_state = State::new(false);

    // Clone state for use in callbacks
    let counter_inc = counter.clone();
    let counter_dec = counter.clone();
    let counter_reset = counter.clone();

    // Build a complex UI similar to what you'd create in Slint
    let window = Window::new("Slint-Style Demo", 700, 550)
        .min_size(600, 400)
        .background(Color::rgb(245, 245, 250))
        .content(
            Padding::new(
                VStack::new()
                    .spacing(20)
                    // Header section
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("Slint Backend Concept Demo")
                                    .color(Color::rgb(40, 40, 60))
                                    .font_size(28)
                            )
                            .child(
                                Label::new("Demonstrating UI patterns for Slint integration")
                                    .color(Color::rgb(120, 120, 130))
                                    .font_size(14)
                            )
                    )
                    // Divider
                    .child(
                        RectView::new(Color::rgb(200, 200, 210))
                            .width(660)
                            .height(2)
                    )
                    // Counter section (demonstrating reactive state)
                    .child(
                        VStack::new()
                            .spacing(12)
                            .child(
                                Label::new("Reactive Counter")
                                    .color(Color::rgb(60, 60, 80))
                                    .font_size(18)
                            )
                            .child(
                                Center::new(
                                    Label::new(format!("Count: {}", counter.get()))
                                        .color(Color::rgb(40, 100, 200))
                                        .font_size(36)
                                )
                            )
                            .child(
                                HStack::new()
                                    .spacing(12)
                                    .child(
                                        Button::new("Decrement", move || {
                                            counter_dec.set(counter_dec.get() - 1);
                                        })
                                        .background(Color::rgb(220, 220, 230))
                                        .text_color(Color::rgb(40, 40, 60))
                                    )
                                    .child(
                                        Button::new("Reset", move || {
                                            counter_reset.set(0);
                                        })
                                        .background(Color::rgb(100, 150, 220))
                                        .text_color(Color::WHITE)
                                    )
                                    .child(
                                        Button::new("Increment", move || {
                                            counter_inc.set(counter_inc.get() + 1);
                                        })
                                        .background(Color::rgb(220, 220, 230))
                                        .text_color(Color::rgb(40, 40, 60))
                                    )
                            )
                    )
                    // Controls section
                    .child(
                        VStack::new()
                            .spacing(12)
                            .child(
                                Label::new("Interactive Controls")
                                    .color(Color::rgb(60, 60, 80))
                                    .font_size(18)
                            )
                            .child(
                                HStack::new()
                                    .spacing(16)
                                    .child(
                                        VStack::new()
                                            .spacing(8)
                                            .child(
                                                Label::new("Slider Value")
                                                    .color(Color::rgb(80, 80, 100))
                                                    .font_size(14)
                                            )
                                            .child(
                                                Slider::new(slider_value.clone(), 0.0, 1.0)
                                                    .on_change(|_value| {
                                                        // Slider value is automatically updated via State
                                                    })
                                            )
                                            .child(
                                                Label::new(format!("{:.2}", slider_value.get()))
                                                    .color(Color::rgb(100, 100, 120))
                                                    .font_size(12)
                                            )
                                    )
                                    .child(Spacer::new())
                                    .child(
                                        VStack::new()
                                            .spacing(8)
                                            .child(
                                                CheckBox::new("Enable Feature", feature_enabled.clone())
                                                    .on_toggle(|_checked| {
                                                        // State is automatically updated
                                                    })
                                            )
                                            .child(
                                                Toggle::new(switch_state.clone())
                                                    .on_toggle(|_enabled| {
                                                        // State is automatically updated
                                                    })
                                            )
                                    )
                            )
                    )
                    // Progress bar
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("Progress Indicator")
                                    .color(Color::rgb(60, 60, 80))
                                    .font_size(18)
                            )
                            .child(
                                ProgressBar::new(slider_value.clone())
                                    .fill_color(Color::rgb(80, 150, 220))
                                    .height(24)
                            )
                    )
                    // Text input section
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("Text Input")
                                    .color(Color::rgb(60, 60, 80))
                                    .font_size(18)
                            )
                            .child(
                                TextField::new(text_input.clone(), "Enter text here...")
                                    .text_color(Color::rgb(40, 40, 60))
                                    .background(Color::WHITE)
                            )
                            .child(
                                Label::new(format!("You typed: {}", text_input.get()))
                                    .color(Color::rgb(100, 100, 120))
                                    .font_size(12)
                            )
                    )
                    // Info section
                    .child(
                        VStack::new()
                            .spacing(4)
                            .child(
                                Label::new("This demo shows how a Slint backend would work:")
                                    .color(Color::rgb(100, 100, 120))
                                    .font_size(12)
                            )
                            .child(
                                Label::new("• Window decorations by ScarletUI")
                                    .color(Color::rgb(100, 100, 120))
                                    .font_size(11)
                            )
                            .child(
                                Label::new("• Content area with reactive widgets")
                                    .color(Color::rgb(100, 100, 120))
                                    .font_size(11)
                            )
                            .child(
                                Label::new("• Event handling and state management")
                                    .color(Color::rgb(100, 100, 120))
                                    .font_size(11)
                            )
                    )
            ).all(20)
        );

    app.add_window(window).unwrap();

    println!("[slint_style_demo] Window created, starting event loop");

    app.run(); // Never returns

    0
}
