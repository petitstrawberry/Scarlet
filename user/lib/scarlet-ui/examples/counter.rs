#![no_std]
#![no_main]

extern crate scarlet_std;

use scarlet_std::format;
use scarlet_std::println;
use scarlet_ui::prelude::*;

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

        vstack! {
            Text::new(format!("Count: {}", self.count.get()).as_str()),
            Button::new("Increment")
                .on_click(move || {
                    count.update(|c| *c += 1);
                }),
        }
        .spacing(20.0)
        .alignment(Alignment::Center)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    let app = CounterApp {
        count: State::new(0),
    };

    match Application::new(app) {
        Ok(app) => {
            let _ = app.run();
        }
        Err(e) => {
            // In no_std, we can't easily print errors
            let _ = e;
        }
    }
}
