//! Counter Example - Demonstrates VStack and macros
//!
//! Shows how to use vstack! macro and modifiers.

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;
extern crate scarlet_ui_macros;

use scarlet_ui::prelude::*;
use scarlet_ui::color::Color;
use scarlet_ui::state::State;

/// Counter Application
#[derive(View, Clone)]
struct CounterApp {
    count: State<i32>,
}

impl Application for CounterApp {
    fn body(&self) -> impl View {
        let count_value = self.count.get();
        let count_text = alloc::format!("Count: {}", count_value);

        vstack! {
            Text::new("ScarletUI Counter Demo")
                .font_size(24.0),
            Rectangle::new().fill(Color::rgb(240, 240, 240)),
            Text::new(&count_text)
                .font_size(48.0),
            hstack! {
                Button::new("-"),
                Spacer::new(),
                Button::new("+"),
            }
            .padding(16.0),
        }
        .spacing(10.0)
        .padding(20.0)
        .background(Color::WHITE)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    let app = CounterApp::default();
    // Run the application
    // Note: Full integration would require additional setup
    let _ = app.run();
}
