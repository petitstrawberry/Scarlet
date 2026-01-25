//! UI Demo - ScarletUI Demo Application
//!
//! Simple demo to test Window rendering

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std;
extern crate scarlet_ui_macros;

use core::f32;

use scarlet_std::println;
use scarlet_ui::{hstack, prelude::*, vstack};
use scarlet_ui_macros::View;
use scarlet_std::format;

#[derive(View, Clone)]
struct DemoApp {
    toggle_state: State<bool>,
}

impl DemoApp {
    pub fn new() -> Self {
        Default::default()
    }
}

impl Application for DemoApp {
    fn body(&self) -> impl View {
        Window::new("ScarletUI Demo",
        vstack! {
                Text::new("Hello ScarletUI!")
                    .font_size(40.0),
                Text::new("ScarletUIの世界からこんにちは!")
                    .font_size(24.0),
                hstack! {
                    Text::new(format!("Toggle State: {}", if self.toggle_state.get() { "ON" } else { "OFF" }))
                        .font_size(20.0),
                    Spacer::new(),
                    Toggle::new(self.toggle_state.clone())
                }
                .frame_width(200.0),
                Rectangle::new()
                    .fill(Color::RED)
                    .frame(300.0, 100.0),
                Spacer::new(),
                Rectangle::new()
                    .fill(Color::BLUE)
                    .frame(300.0, 100.0)
        }
        .frame(f32::INFINITY, f32::INFINITY)
        // .frame(400.0, 300.0)
        )
        .app_id("oeg.scarlet-os.ui_demo")
        .size(Size::new(800.0, 600.0))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("[ui_demo] Starting ScarletUI Demo");

    let mut app = DemoApp::new();

    match app.run() {
        Ok(_) => {
            println!("[ui_demo] Application exited successfully");
        }
        Err(e) => {
            println!("[ui_demo] Application error: {}", e);
        }
    }
}
