//! UI Demo - ScarletUI Demo Application
//!
//! Simple demo to test Window rendering

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std;
extern crate scarlet_ui_macros;

use core::f32;

use scarlet_std::format;
use scarlet_std::println;
use scarlet_ui::{hstack, prelude::*, vstack};
use scarlet_ui_macros::View;

#[derive(View, Clone)]
struct DemoApp {
    toggle_state: State<bool>,
    counter: State<i32>,
    slider_value: State<f32>,
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
                Toggle::new(self.toggle_state.clone()),
                Button::new("Click Me")
                    .on_click(|| {
                        println!("[ui_demo] Button clicked!");
                    }),
            }
            .frame_width(300.0),
            hstack! {
                vstack! {
                    Text::new("Left VStack")
                        .font_size(18.0),
                    Rectangle::new()
                        .fill(Color::BLUE)
                }.frame_width(100.0),
                Rectangle::new()
                    .fill(Color::RED),
                Rectangle::new()
                    .fill(Color::GREEN),
                Rectangle::new()
                    .fill(Color::BLUE)
                    .frame_width(50.0),
                Rectangle::new()
                    .fill(Color::YELLOW)
                    .frame_width(50.0),
            }
            .frame(f32::INFINITY, 100.0),
            Text::new(format!("Counter: {}", self.counter.get()))
                .font_size(30.0),
            Button::new("Increment Counter")
                .on_click({
                    let counter = self.counter.clone();
                    move || {
                        counter.set(counter.get() + 1);
                        println!("[ui_demo] Counter incremented to {}", counter.get());
                    }
                }),
            Text::new(format!("Slider Value: {:.2}", self.slider_value.get()))
                .font_size(20.0),
            Slider::new(self.slider_value.clone())
                .min(0.0)
                .max(100.0)
        }
        .frame(f32::INFINITY, f32::INFINITY)
        // .frame(400.0, 300.0)
        )
        .app_id("oeg.scarlet-os.ui_demo")
        .size(Size::new(800.0, 600.0))
    }

    fn debug_logging(&self) -> bool {
        false
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
