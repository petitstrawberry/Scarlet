//! Counter Example - A simple counter application
//!
//! Demonstrates:
//! - State management
//! - Button interactions
//! - Text display
//! - View composition

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::string::String;
use alloc::boxed::Box;
use scarlet_ui::prelude::*;
use scarlet_ui::{View, Application, Window, Text, Button, Spacer, Rectangle};
use scarlet_ui::geometry::{Size, Point};
use scarlet_ui::color::Color;
use scarlet_ui::state::{State, StateId};
use scarlet_ui::event::Event;

/// Counter Application
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

    /// Increment the counter
    fn increment(&self) {
        let current = self.count.get();
        self.count.set(current + 1);
    }

    /// Decrement the counter
    fn decrement(&self) {
        let current = self.count.get();
        self.count.set(current - 1);
    }
}

impl View for CounterApp {
    fn create_element(&self) -> Box<dyn scarlet_ui::element::Element> {
        // For now, we create a simple window with a title
        // The full UI composition will be added when ViewTuple/macro support is ready
        Window::new("Counter Demo", Text::new(format!("Count: {}", self.count.get())))
            .create_element()
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn scarlet_ui::state::Listenable> {
        alloc::vec::Vec::new()
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

impl Application for CounterApp {
    fn body(&self) -> impl View {
        let count_text = format!("Count: {}", self.count.get());

        // Simple window with counter display
        // Note: Full View composition will be enabled when VStack/HStack are implemented
        Window::new("Counter Demo", Text::new(count_text))
            .size(Size::new(400.0, 300.0))
    }
}

#[no_mangle]
pub extern "C" fn main() {
    let mut app = CounterApp::new();

    // Run the application
    // Note: This will block in the main loop
    // In a real implementation, you'd handle the Result
    let _ = app.run();
}

// Define format! for no_std
mod std {
    pub use core::fmt;
    pub use core::ops;
    pub use core::option;
    pub use core::result;
}

/// Simple format implementation
fn format(args: core::fmt::Arguments<'_>) -> String {
    let mut s = String::new();
    core::fmt::write(&mut s, args).unwrap();
    s
}
