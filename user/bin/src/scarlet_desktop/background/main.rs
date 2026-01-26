//! Scarlet Desktop Background.
//!
//! Renders a full-screen desktop background as a regular SWS client.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::time::Duration;
use scarlet_ui::Color;
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
        // Simple vertical gradient
        let top = Color::rgb(0.157, 0.157, 0.196); // Dark blue-gray (40/255)
        let bottom = Color::rgb(0.078, 0.078, 0.118); // Darker blue-gray (20/255)

        if h == 0 {
            return;
        }

        // Draw gradient
        for y in 0..h {
            let t = (y as u32).saturating_mul(255) / (h.saturating_sub(1).max(1));
            let r = (top.r * 255.0 * (255.0 - t as f32) + bottom.r * 255.0 * t as f32) / 255.0;
            let g = (top.g * 255.0 * (255.0 - t as f32) + bottom.g * 255.0 * t as f32) / 255.0;
            let b_val = (top.b * 255.0 * (255.0 - t as f32) + bottom.b * 255.0 * t as f32) / 255.0;
            let color = Color::rgb(r / 255.0, g / 255.0, b_val / 255.0);
            let bgra = color.to_bgra();

            for x in 0..w {
                let idx = ((y as usize) * width as usize + (x as usize)) * 4;
                if idx + 3 < buf.len() {
                    buf[idx] = (bgra & 0xFF) as u8;
                    buf[idx + 1] = ((bgra >> 8) & 0xFF) as u8;
                    buf[idx + 2] = ((bgra >> 16) & 0xFF) as u8;
                    buf[idx + 3] = ((bgra >> 24) & 0xFF) as u8;
                }
            }
        }

        // Subtle diagonal accent lines
        let accent = Color::rgba(0.392, 0.471, 0.549, 0.110); // RGB(100, 120, 140) with alpha 28/255
        let bgra = accent.to_bgra();
        let mut x = 0i32;
        while x < w as i32 + h as i32 {
            // Draw diagonal line from (x, 0) to (x-h, h-1)
            for i in 0..h as i32 {
                let px = x - i;
                let py = i;
                if px >= 0 && px < w as i32 && py >= 0 && py < h as i32 {
                    let idx = (py as usize * width as usize + px as usize) * 4;
                    if idx + 3 < buf.len() {
                        buf[idx] = (bgra & 0xFF) as u8;
                        buf[idx + 1] = ((bgra >> 8) & 0xFF) as u8;
                        buf[idx + 2] = ((bgra >> 16) & 0xFF) as u8;
                        buf[idx + 3] = ((bgra >> 24) & 0xFF) as u8;
                    }
                }
            }
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
    let surface_id = match conn.create_surface_with_type(
        "org.scarlet-os.desktop.background",
        "Background",
        "",
        16,
        16,
        window_types::DESKTOP,
    ) {
        Ok(id) => id,
        Err(_) => {
            println!("[scarlet_desktop_background] Failed to create surface");
            return 1;
        }
    };

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
