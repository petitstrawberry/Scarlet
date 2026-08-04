//! UI Demo - ScarletUI Demo Application
//!
//! Simple demo to test Window rendering

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std;
extern crate scarlet_ui_macros;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::f32;

use scarlet_std::format;
use scarlet_std::println;
use scarlet_ui::{MenuBarModel, MenuEntry, MenuItemModel};
use scarlet_ui::{hstack, prelude::*, vstack};
use scarlet_ui_macros::View;

#[derive(View, Clone)]
struct DemoApp {
    toggle_state: State<bool>,
    counter: State<i32>,
    slider_value: State<f32>,
    input_text: State<String>,
}

impl DemoApp {
    pub fn new() -> Self {
        Default::default()
    }
}

impl Application for DemoApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new("main", Window::new("ScarletUI Demo",
        vstack! {
            Text::new("Hello ScarletUI!")
                .font_size(40.0),
            Text::new("ScarletUIの世界からこんにちは!")
                .font_size(24.0),
            vstack! {
                Text::new("TextField")
                    .font_size(18.0),
                TextField::new(self.input_text.clone())
                    .placeholder("IME input")
                    .frame_width(360.0),
                Text::new(format!("Input: {}", self.input_text.get()))
                    .font_size(16.0),
            }
            .frame_width(380.0),
            hstack! {
                Text::new(format!("Toggle State: {}", if self.toggle_state.get() { "ON" } else { "OFF" }))
                    .font_size(20.0),
                Spacer::new(),
                Toggle::new(self.toggle_state.clone()),
                Spacer::new(),
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
        .app_id("org.scarlet-os.scarletui.ui-demo")
        .menu_bar(MenuBarModel::new(vec![
            MenuItemModel::new("file", "File")
                .on_activate(Arc::new(|| {
                    println!("[ui_demo] Menu: File");
                }))
                .children(vec![
                    MenuEntry::Item(MenuItemModel::new("new", "New").on_activate(Arc::new(|| {
                        println!("[ui_demo] Menu: New");
                    }))),
                    MenuEntry::Item(MenuItemModel::new("open", "Open").on_activate(Arc::new(|| {
                        println!("[ui_demo] Menu: Open");
                    }))),
                    MenuEntry::Separator,
                    MenuEntry::Item(MenuItemModel::new("quit", "Quit").on_activate(Arc::new(|| {
                        println!("[ui_demo] Menu: Quit");
                    }))),
                ]),
            MenuItemModel::new("edit", "Edit")
                .on_activate(Arc::new(|| {
                    println!("[ui_demo] Menu: Edit");
                }))
                .children(vec![
                    MenuEntry::Item(MenuItemModel::new("undo", "Undo").on_activate(Arc::new(|| {
                        println!("[ui_demo] Menu: Undo");
                    }))),
                    MenuEntry::Item(MenuItemModel::new("redo", "Redo").on_activate(Arc::new(|| {
                        println!("[ui_demo] Menu: Redo");
                    }))),
                ]),
            MenuItemModel::new("view", "View").on_activate(Arc::new(|| {
                println!("[ui_demo] Menu: View");
            })),
            MenuItemModel::new("help", "Help").on_activate(Arc::new(|| {
                println!("[ui_demo] Menu: Help");
            })),
        ]))
        .size(Size::new(480.0, 480.0)))
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
