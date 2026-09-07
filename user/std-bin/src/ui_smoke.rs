use std::sync::Arc;

use scarlet_ui::prelude::*;
use scarlet_ui::{MenuBarModel, MenuEntry, MenuItemModel};
use scarlet_ui::{hstack, vstack};
use scarlet_ui_macros::View;

#[derive(View, Clone)]
struct UiSmokeApp {
    toggle_state: State<bool>,
    counter: State<i32>,
    input_text: State<String>,
}

impl UiSmokeApp {
    fn new() -> Self {
        Default::default()
    }
}

impl Application for UiSmokeApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new("main", Window::new(
            "ScarletUI std smoke",
            vstack! {
                Text::new("ScarletUI std smoke")
                    .font_size(32.0),
                TextField::new(self.input_text.clone())
                    .placeholder("IME input")
                    .frame_width(360.0),
                Text::new(format!("Input: {}", self.input_text.get()))
                    .font_size(16.0),
                hstack! {
                    Text::new(format!("Toggle: {}", if self.toggle_state.get() { "ON" } else { "OFF" }))
                        .font_size(20.0),
                    Spacer::new(),
                    Toggle::new(self.toggle_state.clone()),
                }
                .frame_width(300.0),
                Text::new(format!("Counter: {}", self.counter.get()))
                    .font_size(24.0),
                Button::new("Increment")
                    .on_click({
                        let counter = self.counter.clone();
                        move || {
                            counter.set(counter.get() + 1);
                            println!("[ui-smoke] counter={}", counter.get());
                        }
                    }),
            }
            .frame(f32::INFINITY, f32::INFINITY),
        )
        .app_id("org.scarlet-os.scarletui.ui-smoke")
        .menu_bar(MenuBarModel::new(vec![
            MenuItemModel::new("file", "File").children(vec![
                MenuEntry::Item(MenuItemModel::new("quit", "Quit").on_activate(Arc::new(|| {
                    println!("[ui-smoke] menu quit");
                }))),
            ]),
            MenuItemModel::new("help", "Help").on_activate(Arc::new(|| {
                println!("[ui-smoke] menu help");
            })),
        ]))
        .size(Size::new(640.0, 420.0)))
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

fn main() {
    println!("[ui-smoke] starting");

    let mut app = UiSmokeApp::new();
    match app.run() {
        Ok(()) => println!("[ui-smoke] exited"),
        Err(error) => println!("[ui-smoke] error: {}", error),
    }
}
