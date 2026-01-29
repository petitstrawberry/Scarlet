//! Scarlet Desktop Background.
//!
//! Renders a full-screen desktop background as a regular SWS client.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::time::Duration;
use scarlet_desktop_config::BackgroundStyle;
use scarlet_ui::Color;
use std::println;
use std::thread;
use sws_client::{Connection, Event, SurfaceBuilder};
use sws_protocol::window_types;

fn draw_gradient_background(
    conn: &mut Connection,
    surface_id: u32,
    top: Color,
    bottom: Color,
    draw_lines: bool,
) {
    let Some(surface) = conn.surface_mut(surface_id) else {
        return;
    };

    let w = surface.width();
    let h = surface.height();
    surface.with_buffer(|buf, width, height| {
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

        if draw_lines {
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
        }
    });

    let _ = conn.commit(surface_id);
}

fn draw_solid_background(conn: &mut Connection, surface_id: u32, color: Color) {
    let Some(surface) = conn.surface_mut(surface_id) else {
        return;
    };

    let w = surface.width();
    let h = surface.height();
    surface.with_buffer(|buf, width, height| {
        let bgra = color.to_bgra();

        for y in 0..h {
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
    });

    let _ = conn.commit(surface_id);
}

fn draw_background(conn: &mut Connection, surface_id: u32) {
    // Load config to get background color
    let config = scarlet_desktop_config::load_desktop_config();
    let style = config
        .theme
        .background_style
        .unwrap_or(BackgroundStyle::GradientLines);

    let (top, bottom, base) = if let Some(bg_color) = config.theme.background {
        let base = Color::rgb(
            bg_color[0] as f32 / 255.0,
            bg_color[1] as f32 / 255.0,
            bg_color[2] as f32 / 255.0,
        );
        let darker = Color::rgb(
            (bg_color[0] as f32 / 255.0 * 0.7).max(0.0),
            (bg_color[1] as f32 / 255.0 * 0.7).max(0.0),
            (bg_color[2] as f32 / 255.0 * 0.7).max(0.0),
        );
        (base, darker, base)
    } else {
        let top = Color::rgb(0.157, 0.157, 0.196);
        let bottom = Color::rgb(0.078, 0.078, 0.118);
        (top, bottom, top)
    };

    match style {
        BackgroundStyle::GradientLines => {
            draw_gradient_background(conn, surface_id, top, bottom, true)
        }
        BackgroundStyle::Gradient => draw_gradient_background(conn, surface_id, top, bottom, false),
        BackgroundStyle::Solid => draw_solid_background(conn, surface_id, base),
    }
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

    let (screen_width, screen_height) = match conn.get_screen_size() {
        Ok((width, height)) => (width, height),
        Err(_) => (1024, 768),
    };

    let surface_id = match SurfaceBuilder::new()
        .app_id("org.scarlet-os.desktop.background")
        .app_name("Background")
        .menu_titles("")
        .size(screen_width, screen_height)
        .window_type(window_types::DESKTOP)
        .resizable(false)
        .focus_on_create(false)
        .active_on_focus(false)
        .position(0, 0)
        .build(&mut conn)
    {
        Ok(id) => id,
        Err(_) => {
            println!("[scarlet_desktop_background] Failed to create surface");
            return 1;
        }
    };

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
