//! Counter Example - Demonstrates VStack and macros
//!
//! Shows how to use vstack! macro and modifiers.

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use scarlet_ui::prelude::*;
use scarlet_ui::geometry::Size;
use scarlet_ui::color::Color;
use scarlet_ui::state::{State, StateId};

/// Counter Application
#[derive(View, Clone)]
struct CounterApp {
    count: State<i32>,
}

impl CounterApp {
    /// Create a new counter app
    fn new() -> Self {
        Self {
            count: State::new(StateId::new(1), 0),
        }
    }
}

impl Application for CounterApp {
    fn body(&self) -> impl View {
        let count_text = Text::new(format!("Count: {}", self.count.get()));

        vstack! {
            Text::new("ScarletUI Counter Demo")
                .font_size(24.0),
            Rectangle::new().fill(Color::rgb(240, 240, 240)),
            count_text
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

#[no_mangle]
pub extern "C" fn main() {
    let app = CounterApp::new();
    // Run the application
    // Note: Full integration would require additional setup
    let _ = app;
}

mod std {
    pub use core::fmt;
    pub use core::ops;
    pub use core::option;
    pub use core::result;
}

fn format(args: core::fmt::Arguments<'_>) -> alloc::string::String {
    let mut s = alloc::string::String::new();
    core::fmt::write(&mut s, args).unwrap();
    s
}
