//! UI Demo - ScarletUI Demo Application
//!
//! Simple demo to test Window rendering

#![no_std]
#![no_main]

extern crate scarlet_std;
extern crate scarlet_ui_macros;

use scarlet_std::println;
use scarlet_ui::{prelude::*, vstack};
use scarlet_ui_macros::View;

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
                Toggle::new(self.toggle_state.clone()),
                Rectangle::new()
                    .fill(Color::RED)
                    .frame_height(300.0),
                Rectangle::new()
                    .fill(Color::GREEN),
                Rectangle::new()
                    .fill(Color::BLUE),
        
            })
            .app_id("com.scarlet.ui_demo")
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
