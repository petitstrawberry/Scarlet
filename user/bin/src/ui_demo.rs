//! UI Demo - Demonstrates scarlet-ui with the new architecture
//!
//! Architecture:
//! - `sws-client`: Low-level SWS connection and protocol
//! - `scarlet-ui`: High-level UI toolkit (this demo uses this)

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_ui::{Application, Color, Event, MouseButton, Rect};
use std::println;

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[ui_demo] Starting UI demo application");

    // Create application (connects to SWS)
    let mut app = match Application::new() {
        Ok(a) => a,
        Err(e) => {
            println!("[ui_demo] Failed to create application: {}", e);
            return 1;
        }
    };
    println!("[ui_demo] Connected to SWS");

    // Create window with decorations
    let mut window = match app.create_window("UI Demo", 400, 300) {
        Ok(w) => w,
        Err(e) => {
            println!("[ui_demo] Failed to create window: {}", e);
            return 1;
        }
    };
    let surface_id = window.surface_id();
    println!("[ui_demo] Window created (surface_id={})", surface_id);

    // Draw initial content
    {
        let mut canvas = window.canvas();
        // Fill with white background
        canvas.fill_rect(Rect::new(0, 0, 400, 300), Color::WHITE);
        canvas.draw_text(50, 50, "Hello, Scarlet UI!", Color::TEXT);
        canvas.draw_text(50, 80, "Move mouse to see coordinates", Color::TEXT);
        canvas.fill_rect(Rect::new(50, 120, 100, 60), Color::rgb(255, 100, 100));
        canvas.fill_rect(Rect::new(170, 120, 100, 60), Color::rgb(100, 255, 100));
    }

    // Commit initial draw
    if let Err(e) = app.commit(surface_id) {
        println!("[ui_demo] Failed to commit: {}", e);
    }

    println!("[ui_demo] Entering main loop");

    let mut mouse_x: i32 = 0;
    let mut mouse_y: i32 = 0;

    // Main event loop
    loop {
        // Poll for events
        while let Some((win_id, event)) = app.poll_event() {
            if win_id != surface_id {
                continue;
            }

            match event {
                Event::MouseMove { x, y } => {
                    // Update partial coordinates
                    if x >= 0 {
                        mouse_x = x;
                    }
                    if y >= 0 {
                        mouse_y = y;
                    }
                    window.update_mouse(mouse_x, mouse_y);
                }
                Event::MouseDown(MouseButton::Left) => {
                    println!("[ui_demo] Left click at ({}, {})", mouse_x, mouse_y);
                    if window.is_close_clicked(mouse_x, mouse_y) {
                        println!("[ui_demo] Close button clicked, exiting");
                        return 0;
                    }
                }
                Event::WindowClose => {
                    println!("[ui_demo] Window close event");
                    return 0;
                }
                _ => {}
            }
        }

        // Commit any changes
        let _ = app.commit(surface_id);
    }
}
