#![no_std]
#![no_main]

extern crate scarlet_std;
extern crate scarlet_ui_macros;

use scarlet_std::{format, println};
use scarlet_ui::prelude::*;
use scarlet_ui_macros::View;

// Import the vstack! macro from scarlet_ui root
use scarlet_ui::vstack;

#[derive(View, Clone)]
struct CounterApp {
    count: State<i32>,
}

impl Application for CounterApp {
    fn body(&self) -> impl View {
        let count = self.count.clone();
        let count_value = self.count.get();
        let count_text = format!("Count: {}", count_value);

        Window::new("Counter",
            vstack! {
                Text::new("ScarletUI Counter Demo")
                    .font_size(24.0),
                Text::new(&count_text)
                    .font_size(48.0),
                Button::new("Increment")
                    .on_click(move || {
                        count.update(|c| *c += 1);
                    }),
            }
            .spacing(20.0)
            .alignment(Alignment::Center)
            .padding(20.0)
        )
        .decorated(true)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("[counter] Starting CounterApp");
    let mut app = CounterApp::default();

    match app.run() {
        Ok(_) => {
            println!("[counter] Application exited successfully");
        }
        Err(e) => {
            println!("[counter] Application error: {}", e);
        }
    }
}
