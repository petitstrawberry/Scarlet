//! Scarlet Desktop Background.
//!
//! Renders a full-screen desktop background as a regular SWS client.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::time::Duration;
use scarlet_ui::Color;
use scarlet_ui::graphics::Canvas;
use std::println;
use std::thread;
use sws_client::{Connection, Event};
use sws_protocol::window_types;

fn draw_background(conn: &mut Connection, surface_id: u32) {
    let Some(surface) = conn.surface_mut(surface_id) else {
        return;
    };

    let w = surface.width();
    let h = surface.height();
    surface.with_buffer(|buf, width, height| {
        let mut canvas = Canvas::new(buf, width, height);

        // Simple vertical gradient.
        let top = Color::rgb(18, 22, 30);
        let bottom = Color::rgb(30, 34, 44);

        if h == 0 {
            return;
        }

        for y in 0..h {
            let t = (y as u32).saturating_mul(255) / (h.saturating_sub(1).max(1));
            let r = (top.r as u32 * (255 - t) + bottom.r as u32 * t) / 255;
            let g = (top.g as u32 * (255 - t) + bottom.g as u32 * t) / 255;
            let b = (top.b as u32 * (255 - t) + bottom.b as u32 * t) / 255;
            canvas.fill_rect(0, y as i32, w, 1, Color::rgb(r as u8, g as u8, b as u8));
        }

        // Subtle diagonal accent lines.
        let accent = Color::rgb(54, 176, 168);
        let mut x = 0i32;
        while x < w as i32 + h as i32 {
            canvas.draw_line(x, 0, x - h as i32, h as i32 - 1, accent.with_alpha(28));
            x += 64;
        }
    });

    let _ = conn.commit(surface_id);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[scarlet_desktop_background] starting");

    let mut conn = match Connection::connect("/tmp/sws.sock") {
        Ok(c) => c,
        Err(_) => {
            println!("[scarlet_desktop_background] Failed to connect to SWS");
            return 1;
        }
    };

    // Start with a tiny surface; we'll be configured to screen size after maximize.
    let surface_id = match conn.create_surface("org.scarlet-os.desktop.background", 16, 16) {
        Ok(id) => id,
        Err(_) => {
            println!("[scarlet_desktop_background] Failed to create surface");
            return 1;
        }
    };

    let _ = conn.set_window_type(surface_id, window_types::DESKTOP);
    let _ = conn.move_window(surface_id, 0, 0);
    let _ = conn.maximize_window(surface_id);

    // Initial draw.
    draw_background(&mut conn, surface_id);

    loop {
        let _ = conn.dispatch();
        while let Some(ev) = conn.poll_event() {
            match ev {
                Event::SurfaceConfigure {
                    surface_id: sid,
                    width,
                    height,
                } if sid == surface_id => {
                    // Resize to requested screen dimensions.
                    if conn.resize_window(surface_id, width, height).is_ok() {
                        let _ = conn.move_window(surface_id, 0, 0);
                        draw_background(&mut conn, surface_id);
                    }
                }
                Event::SurfaceDestroyed { surface_id: sid } if sid == surface_id => {
                    println!("[scarlet_desktop_background] destroyed");
                    return 0;
                }
                _ => {}
            }
        }

        thread::sleep(Duration::from_millis(16));
    }
}
