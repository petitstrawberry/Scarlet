#![no_std]
#![no_main]

extern crate scarlet_std;

use core::result;

use scarlet_std::{format, println};
use scarlet_ui::prelude::*;

#[derive(View, Clone)]
struct CounterApp {
    count: State<i32>,
}

impl App for CounterApp {
    type ViewType = CounterView;

    fn build(&self) -> Self::ViewType {
        CounterView {
            count: self.count.clone(),
        }
    }
}

#[derive(View, Clone)]
struct CounterView {
    count: State<i32>,
}

impl CounterView {
    fn body(&self) -> impl View {
        let count = self.count.clone();

        Window::new("Counter",
            vstack! {
                Text::new(format!("Count: {}", self.count.get()).as_str()).size(36.0),
                Button::new("Increment")
                    .on_click(move || {
                        count.update(|c| *c += 1);
                    }),
            }
            .spacing(20.0)
            .alignment(Alignment::Center)
        )
        .decorated(true)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("[counter] Starting CounterApp");
    let app = CounterApp::default();

    match Application::new(app) {
        Ok(app) => {
            let result = app.run();
            println!("[counter] Application exited with result: {:?}", result);
        }
        Err(e) => {
            // In no_std, we can't easily print errors
            println!("[counter] Application error: {}", e);
        }
    }
}
