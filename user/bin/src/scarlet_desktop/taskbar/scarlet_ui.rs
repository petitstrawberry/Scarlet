//! Scarlet Desktop TaskBar (ScarletUI version)
//!
//! macOS-style menu bar implemented with ScarletUI

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_desktop_config;
extern crate scarlet_ui;
extern crate scarlet_ui_macros;
extern crate scarlet_std as std;

use core::time::Duration;
use scarlet_ui::prelude::*;
use scarlet_ui::color::Color;
use scarlet_ui::geometry::Size;
use scarlet_ui::{hstack, StateId};
use scarlet_ui_macros::View;
use std::{format, println};
use sws_client as sws;

/// TaskBar Application
#[derive(View, Clone)]
struct TaskBarApp {
    cpu_usage: State<u8>,
    memory_usage: State<u8>,
    uptime: State<u32>,
    screen_width: State<f32>,
}

impl TaskBarApp {
    fn new() -> Self {
        Self {
            cpu_usage: State::new(StateId::new(0), 15),
            memory_usage: State::new(StateId::new(1), 42),
            uptime: State::new(StateId::new(2), 0),
            screen_width: State::new(StateId::new(3), 1920.0),
        }
    }
}

impl Application for TaskBarApp {
    fn body(&self) -> impl View {
        let cpu = self.cpu_usage.get();
        let mem = self.memory_usage.get();
        let uptime = self.uptime.get();
        let screen_width = self.screen_width.get();

        let mins = (uptime / 60) % 60;
        let secs = uptime % 60;

        Window::new("TaskBar",
            hstack! {
                Text::new("Scarlet")
                    .font_size(14.0)
                    .color(Color::rgb(0.110, 0.110, 0.125)),
                Spacer::new(),
                Text::new(&format!("Mem {}%", mem))
                    .font_size(12.0)
                    .color(Color::rgb(0.280, 0.280, 0.310)),
                Text::new("•")
                    .font_size(12.0)
                    .color(Color::rgb(0.600, 0.600, 0.630)),
                Text::new(&format!("CPU {}%", cpu))
                    .font_size(12.0)
                    .color(Color::rgb(0.280, 0.280, 0.310)),
                Text::new("•")
                    .font_size(12.0)
                    .color(Color::rgb(0.600, 0.600, 0.630)),
                Text::new(&format!("Up {:02}:{:02}", mins, secs))
                    .font_size(12.0)
                    .color(Color::rgb(0.280, 0.280, 0.310)),
            }
            .spacing(10.0)
            .alignment(Alignment::Center)
            .padding(8.0)
        )
        .app_id("org.scarlet-os.desktop.taskbar")
        .decorated(false)
        .background_color(Some(Color::rgb(0.940, 0.940, 0.960)))
        .window_type(scarlet_ui::views::window_type::TASKBAR)
        .resizable(false)
        .movable(false)
        .size(Size::new(screen_width, 30.0))
    }

    fn init(&mut self) {
        println!("[TaskBar] Initializing ScarletUI TaskBar");
        // Screen size will be obtained by sws_client in main()
        self.start_background_tasks();
    }
}

impl TaskBarApp {
    fn start_background_tasks(&mut self) {
        // CPU/Memory simulation
        let cpu = self.cpu_usage.clone();
        let mem = self.memory_usage.clone();

        std::thread::spawn(move || {
            loop {
                cpu.update(|c| *c = (*c + 7) % 85 + 10);
                mem.update(|m| *m = (*m + 3) % 70 + 25);
                std::thread::sleep(Duration::from_secs(1));
            }
        });

        // Uptime counter
        let uptime = self.uptime.clone();

        std::thread::spawn(move || {
            loop {
                uptime.update(|u| *u += 1);
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }
}


#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("[TaskBar] Starting ScarletUI TaskBar");

    let bar_height: u32 = 30;

    // Get screen size from SWS before creating the app
    let screen_width = match sws::Connection::connect("/tmp/sws.sock") {
        Ok(mut conn) => {
            let (width, height) = match conn.get_screen_size() {
                Ok((width, height)) => {
                    println!("[TaskBar] Screen size: {}x{}", width, height);
                    (width, height)
                }
                Err(e) => {
                    println!("[TaskBar] Failed to get screen size: {:?}, using default 1920x1080", e);
                    (1920, 1080)
                }
            };

            let workarea_y = bar_height as i32;
            let workarea_height = height.saturating_sub(bar_height);
            let _ = conn.set_workarea(0, workarea_y, width, workarea_height);
            println!(
                "[TaskBar] Workarea: x=0, y={}, width={}, height={}",
                workarea_y, width, workarea_height
            );

            width as f32
        }
        Err(e) => {
            println!("[TaskBar] Failed to connect to SWS: {:?}, using default screen width 1920", e);
            1920.0
        }
    };

    let mut app = TaskBarApp::new();

    // Update screen_width state with actual screen size
    app.screen_width.update(|w| *w = screen_width);

    match app.run() {
        Ok(_) => {
            println!("[TaskBar] Application exited successfully");
        }
        Err(e) => {
            println!("[TaskBar] Application error: {}", e);
        }
    }
}
