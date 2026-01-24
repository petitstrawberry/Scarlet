//! UI Demo - ScarletUI Demo Application
//!
//! Comprehensive demo showcasing all ScarletUI features:
//! - State management with State<T>
//! - Button interactions
//! - Layout containers (VStack, HStack)
//! - View modifiers (padding, background, frame)
//! - Color system

#![no_std]
#![no_main]

extern crate scarlet_std;
extern crate scarlet_ui_macros;

use scarlet_std::{format, println};
use scarlet_ui::prelude::*;
use scarlet_ui_macros::View;
use scarlet_ui::{vstack, hstack};

#[derive(View, Clone)]
struct DemoApp {
    counter: State<i32>,
}

impl Application for DemoApp {
    fn body(&self) -> impl View {
        let counter_dec = self.counter.clone();
        let counter_inc = self.counter.clone();
        let counter_value = self.counter.get();
        let counter_text = format!("Count: {}", counter_value);

        vstack! {
            // === Header ===
            Text::new("ScarletUI Demo").font_size(32.0),

            // === Counter Demo ===
            Text::new(&counter_text).font_size(48.0),
            hstack! {
                Button::new("-").on_click(move || {
                    counter_dec.update(|c| *c -= 1);
                }),
                Button::new("+").on_click(move || {
                    counter_inc.update(|c| *c += 1);
                }),
            }
            .spacing(10.0)
            .padding(10.0)
            .background(Color::gray(0.9)),

            // === Color Palette Demo ===
            hstack! {
                Rectangle::new().fill(Color::RED),
                Rectangle::new().fill(Color::GREEN),
                Rectangle::new().fill(Color::BLUE),
            }
            .spacing(5.0),

            // === View Modifiers Demo ===
            Text::new("Padding & Background")
                .padding(10.0)
                .background(Color::YELLOW)
                .frame(200.0, 40.0),
        }
        .spacing(16.0)
        .alignment(Alignment::Center)
        .padding(20.0)
        .background(Color::WHITE)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("[ui_demo] Starting ScarletUI Demo");

    let mut app = DemoApp::default();

    match app.run() {
        Ok(_) => {
            println!("[ui_demo] Application exited successfully");
        }
        Err(e) => {
            println!("[ui_demo] Application error: {}", e);
        }
    }
}
