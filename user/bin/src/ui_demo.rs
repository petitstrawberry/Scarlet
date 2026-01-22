//! UI Demo - ScarletUI Demo Application
//!
//! Demonstrates:
//! - State management with State<T>
//! - View/RenderNode architecture
//! - Common UI components (Button, Text, Slider, Toggle, TextField)
//! - Layout containers with macros
//! - Window with titlebar

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::println;
use scarlet_ui::prelude::*;

/// Main application state
struct DemoApp {
    counter: State<i32>,
    toggle_value: State<bool>,
    slider_value: State<f32>,
    text_value: State<std::string::String>,
}

impl App for DemoApp {
    type ViewType = DemoView;

    fn build(&self) -> Self::ViewType {
        DemoView {
            counter: self.counter.clone(),
            toggle_value: self.toggle_value.clone(),
            slider_value: self.slider_value.clone(),
            text_value: self.text_value.clone(),
        }
    }
}

#[derive(View, Clone)]
struct DemoView {
    counter: State<i32>,
    toggle_value: State<bool>,
    slider_value: State<f32>,
    text_value: State<std::string::String>,
}

impl DemoView {
    fn body(&self) -> impl View {
        // Clone state for closures
        let counter_dec = self.counter.clone();
        let counter_inc = self.counter.clone();
        let counter_text = self.counter.clone();
        let toggle = self.toggle_value.clone();
        let slider = self.slider_value.clone();
        let text_field = self.text_value.clone();

        Window::new("ScarletUI Demo",
            vstack! {
                Text::new("ScarletUI Demo").size(24.0),
                Text::new("Interactive UI Components Demo"),
                Text::new(std::format!("Counter: {}", counter_text.get()).as_str()),
                hstack! {
                    Spacer::new(),
                    Button::new("-").on_click(move || {
                        counter_dec.update(|c| *c -= 1);
                        println!("[ui_demo] Counter: {}", counter_dec.get());
                    }),
                    Button::new("+").on_click(move || {
                        counter_inc.update(|c| *c += 1);
                        println!("[ui_demo] Counter: {}", counter_inc.get());
                    }),
                    Spacer::new(),
                }
                .spacing(10.0),
                Toggle::new(toggle),
                Slider::new(slider).range(0.0, 100.0),
                TextField::new(text_field).placeholder("Enter text..."),
            }
            .spacing(16.0)
            .alignment(Alignment::Center)
        )
        .decorated(true)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[ui_demo] Starting ScarletUI Demo");

    let app = DemoApp {
        counter: State::new(0),
        toggle_value: State::new(true),
        slider_value: State::new(50.0),
        text_value: State::new(std::string::String::from("Hello, ScarletUI!")),
    };

    match Application::new(app) {
        Ok(app) => {
            println!("[ui_demo] Running application...");
            let _ = app.run();
            0
        }
        Err(e) => {
            println!("[ui_demo] Failed to create application: {}", e);
            1
        }
    }
}
