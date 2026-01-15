//! UI Demo - ScarletUI Reactive State Demo
//!
//! This demo showcases:
//! - Reactive State<T> with automatic UI updates
//! - Direct State passing (no .binding() needed)
//! - Timer-based automatic updates
//! - ReactiveLabel for auto-updating text
//! - Rounded corners on all controls

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::time::Duration;
use scarlet_ui::{
    Application, Button, Center, CheckBox, Color, HStack, Label, Padding, ProgressBar, RectView,
    Slider, Spacer, State, Text, TextField, Timer, Toggle, VStack, Window, label,
};
use std::{format, println, string::String};

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[ui_demo] Starting ScarletUI Reactive Demo");

    let mut app = match Application::new() {
        Ok(a) => a.app_id("org.scarlet.ui_demo"),
        Err(e) => {
            println!("[ui_demo] Failed to create application: {}", e);
            return 1;
        }
    };

    app.set_terminate_after_last_window_closed(true);
    let handle = app.handle();

    // ========================================================================
    // Reactive State - UI auto-updates when values change
    // ========================================================================

    let counter = State::new(0i32);
    let progress = State::new(0.0f32);
    let text_input = State::new(String::new());
    let feature_a = State::new(true);
    let feature_b = State::new(false);
    let slider_value = State::new(0.5f32);
    let toggle1 = State::new(true);
    let toggle2 = State::new(false);

    // Timer - auto-increment progress (100ms intervals)
    let progress_t = progress.clone();
    let counter_t = counter.clone();
    Timer::periodic(Duration::from_millis(100), move || {
        let p = progress_t.get() + 0.01;
        if p >= 1.0 {
            progress_t.set(0.0);
            counter_t.update(|c| *c += 1);
        } else {
            progress_t.set(p);
        }
    });

    // Button callbacks
    let counter_reset = counter.clone();
    let counter_inc = counter.clone();
    let progress_reset = progress.clone();
    let popup_handle = handle.clone();

    // ========================================================================
    // Build UI - State passed directly to controls (no .binding())
    // ========================================================================

    let window = Window::new("ScarletUI Reactive Demo", 650, 720)
        .min_size(650, 720)
        .background(Color::rgb(245, 245, 250))
        .content(
            Padding::new(
                VStack::new()
                    .spacing(16)
                    // Header
                    .child(
                        Label::new("ScarletUI Reactive Gallery")
                            .color(Color::rgb(40, 40, 50))
                            .font_size(40),
                    )
                    .child(
                        Label::new("ScarletUIの世界からこんにちは！ This demo showcases reactive State<T> usage.")
                            .color(Color::GRAY)
                            .font_size(20),
                    )
                    .child(Spacer::new().min_length(8))
                    // Counter - ReactiveLabel auto-updates
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                HStack::new()
                                    .spacing(8)
                                    .child(
                                        VStack::new()
                                            .spacing(8)
                                            .child(
                                                Label::new("Reactive Counter:")
                                                    .color(Color::TEXT)
                                                    .font_size(14),
                                            )
                                            .child(
                                                label!("Count: {}", counter.clone())
                                                    .color(Color::rgb(50, 150, 255))
                                                    .font_size(24),
                                            )
                                    )
                                    .child(Button::new("Reset", move || {
                                        counter_reset.set(0);
                                    }))
                                    .child(Button::new("+1", move || {
                                        counter_inc.update(|c| *c += 1);
                                    })),
                            )
                    )
                    // Progress - auto-updates via timer
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("Auto Progress:")
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
                                        Text::new({
                                            let progress = progress.clone();
                                            move || format!("{:.0}%", progress.get() * 100.0)
                                        })
                                        .watch(progress.clone())
                                        .color(Color::GRAY)
                                        .font_size(12),
                                    )
                                    .child(Spacer::new())
                                    .child(Button::new("Reset", move || {
                                        progress_reset.set(0.0);
                                    })),
                            ),
                    )
                    .child(
                        // TextField - State passed directly
                        HStack::new()
                            .spacing(16)
                            .child(
                                VStack::new()
                                    .spacing(8)
                                .child(Label::new("TextField:").color(Color::TEXT).font_size(14)
                                )
                                .child(
                                    TextField::new("Type here...", text_input.clone()).corner_radius(6),
                                )
                            )
                            .child(
                            // CheckBox - State passed directly
                            VStack::new()
                                .spacing(8)
                                .child(Label::new("CheckBox:").color(Color::TEXT).font_size(14))
                                .child(
                                    HStack::new()
                                        .spacing(16)
                                        .child(CheckBox::new("Feature A", feature_a.clone()))
                                        .child(CheckBox::new("Feature B", feature_b.clone())),
                                ),
                            )
                            // Toggle - State passed directly
                            .child(
                                VStack::new()
                                    .spacing(8)
                                    .child(Label::new("Toggle:").color(Color::TEXT).font_size(14))
                                    .child(
                                        HStack::new()
                                            .spacing(16)
                                            .child(Toggle::new(toggle1.clone()))
                                            .child(Toggle::new(toggle2.clone())),
                                    ),
                            )
                    )
                    // Slider - State passed directly
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(Label::new("Slider:").color(Color::TEXT).font_size(14))
                            .child(Slider::new(0.0, 1.0, slider_value.clone()))
                            .child(
                                Text::new({
                                    let slider_value = slider_value.clone();
                                    move || format!("Value: {:.2}", slider_value.get())
                                })
                                .watch(slider_value.clone())
                                .color(Color::GRAY)
                                .font_size(12),
                            ),
                    )
                    // RectViews with corner_radius
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                Label::new("Rounded Corners:")
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
                                            .corner_radius(30),
                                    )
                                    .child(Spacer::new()),
                            ),
                    )
                    .child(Spacer::new())
                    // Buttons
                    .child(Center::new(
                        HStack::new()
                            .spacing(12)
                            .child({
                                let handle = popup_handle.clone();
                                Button::new("Popup", move || {
                                    handle.request_popup();
                                })
                            })
                            .child(Button::new(label!("Info (count={})", counter.clone()), {
                                let counter = counter.clone();
                                move || {
                                    println!(
                                        "[ui_demo] All controls use State<T> directly! count={}",
                                        counter.get()
                                    );
                                }
                            })),
                    )),
            )
            .all(20),
        );

    if let Err(e) = app.add_window(window) {
        println!("[ui_demo] Failed to add window: {}", e);
        return 1;
    }
    println!("[ui_demo] Window created");

    app.run();
}
