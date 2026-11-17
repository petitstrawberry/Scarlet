//! Wallpaper surface owned by the Scarlet workspace shell.
//!
//! Renders a full-screen desktop background as a regular SWS client.

use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use sbus::Message as SbusMessage;
use sbus_client::Connection as SbusConnection;
use scarlet_desktop_config::{
    BackgroundStyle, DESKTOP_BACKGROUND_BUS_NAME, DESKTOP_BACKGROUND_CHANGED_SIGNAL,
    DESKTOP_SETTINGS_INTERFACE, DESKTOP_SETTINGS_OBJECT_PATH, DESKTOP_SETTINGS_SIGNAL_SENDER,
};
use scarlet_ui::{BitmapImage, Color};
use std::println;
use std::string::String;
use std::thread;
use sws_client::{Connection, Event, SurfaceBuilder};
use sws_protocol::window_types;

const SWS_CONNECT_RETRIES: usize = 100;
const SWS_RETRY_DELAY_MS: u64 = 50;

static BACKGROUND_CHANGED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, PartialEq, Eq)]
struct BackgroundState {
    color: [u8; 3],
    style: BackgroundStyle,
    image: Option<String>,
}

fn load_background_state() -> BackgroundState {
    let config = scarlet_desktop_config::load_desktop_config();
    BackgroundState {
        color: config.theme.background.unwrap_or([40, 40, 50]),
        style: config
            .theme
            .background_style
            .unwrap_or(BackgroundStyle::GradientLines),
        image: config.theme.background_image,
    }
}

fn background_signal_listener() {
    loop {
        let mut connection = match SbusConnection::connect() {
            Ok(connection) => connection,
            Err(_) => {
                thread::sleep(Duration::from_millis(SWS_RETRY_DELAY_MS));
                continue;
            }
        };

        if connection
            .register_service(DESKTOP_BACKGROUND_BUS_NAME)
            .is_err()
        {
            thread::sleep(Duration::from_millis(SWS_RETRY_DELAY_MS));
            continue;
        }

        loop {
            match connection.receive_message() {
                Ok(SbusMessage::Signal {
                    sender,
                    path,
                    interface,
                    signal,
                    ..
                }) if sender == DESKTOP_SETTINGS_SIGNAL_SENDER
                    && path == DESKTOP_SETTINGS_OBJECT_PATH
                    && interface == DESKTOP_SETTINGS_INTERFACE
                    && signal == DESKTOP_BACKGROUND_CHANGED_SIGNAL =>
                {
                    BACKGROUND_CHANGED.store(true, Ordering::Release);
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }
}

fn connect_sws_with_retry() -> Result<Connection, ()> {
    for attempt in 0..SWS_CONNECT_RETRIES {
        if let Ok(conn) = Connection::connect("/tmp/sws.sock")
            && conn.get_screen_size().is_ok()
        {
            println!(
                "[Shell::Background] connected to SWS after {} attempt(s)",
                attempt + 1
            );
            return Ok(conn);
        }

        thread::sleep(Duration::from_millis(SWS_RETRY_DELAY_MS));
    }

    Err(())
}

fn draw_gradient_background(
    conn: &mut Connection,
    surface_id: u32,
    top: Color,
    bottom: Color,
    draw_lines: bool,
) {
    let Some(()) = conn.with_surface_mut(surface_id, |surface| {
        let w = surface.width();
        let h = surface.height();
        surface.with_buffer(|buf, width, _height| {
            if h == 0 {
                return;
            }

            // Draw gradient
            for y in 0..h {
                let t = y.saturating_mul(255) / (h.saturating_sub(1).max(1));
                let r = (top.r * 255.0 * (255.0 - t as f32) + bottom.r * 255.0 * t as f32) / 255.0;
                let g = (top.g * 255.0 * (255.0 - t as f32) + bottom.g * 255.0 * t as f32) / 255.0;
                let b_val =
                    (top.b * 255.0 * (255.0 - t as f32) + bottom.b * 255.0 * t as f32) / 255.0;
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
    }) else {
        return;
    };

    let _ = conn.commit(surface_id);
}

fn draw_solid_background(conn: &mut Connection, surface_id: u32, color: Color) {
    let Some(()) = conn.with_surface_mut(surface_id, |surface| {
        let w = surface.width();
        let h = surface.height();
        surface.with_buffer(|buf, width, _height| {
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
    }) else {
        return;
    };

    let _ = conn.commit(surface_id);
}

fn draw_image_background(conn: &mut Connection, surface_id: u32, path: &str) -> bool {
    let Some(image) = BitmapImage::from_path(path) else {
        println!("[Shell::Background] failed to decode image: {}", path);
        return false;
    };

    let committed = conn.with_surface_mut(surface_id, |surface| {
        let width = surface.width();
        let height = surface.height();
        let source_width = image.width();
        let source_height = image.height();
        if width == 0 || height == 0 || source_width == 0 || source_height == 0 {
            return;
        }

        // Cover the surface while preserving the image aspect ratio. Integer
        // arithmetic keeps the mapping deterministic on both target arches.
        let (draw_width, draw_height) =
            if width as u64 * source_height as u64 >= height as u64 * source_width as u64 {
                (
                    width,
                    ((width as u64 * source_height as u64) / source_width as u64) as u32,
                )
            } else {
                (
                    ((height as u64 * source_width as u64) / source_height as u64) as u32,
                    height,
                )
            };
        let offset_x = (draw_width.saturating_sub(width)) / 2;
        let offset_y = (draw_height.saturating_sub(height)) / 2;
        let pixels = image.pixels();

        surface.with_buffer(|buf, stride, _| {
            for y in 0..height {
                let source_y = ((y + offset_y) as u64 * source_height as u64
                    / draw_height.max(1) as u64) as u32;
                for x in 0..width {
                    let source_x = ((x + offset_x) as u64 * source_width as u64
                        / draw_width.max(1) as u64) as u32;
                    let source_index = (source_y.min(source_height - 1) * source_width
                        + source_x.min(source_width - 1))
                        as usize;
                    let Some(pixel) = pixels.get(source_index).copied() else {
                        continue;
                    };
                    let index = (y as usize * stride as usize + x as usize) * 4;
                    if index + 3 < buf.len() {
                        buf[index] = (pixel & 0xFF) as u8;
                        buf[index + 1] = ((pixel >> 8) & 0xFF) as u8;
                        buf[index + 2] = ((pixel >> 16) & 0xFF) as u8;
                        buf[index + 3] = ((pixel >> 24) & 0xFF) as u8;
                    }
                }
            }
        });
    });

    if committed.is_none() {
        return false;
    }
    conn.commit(surface_id).is_ok()
}

fn draw_background(conn: &mut Connection, surface_id: u32, state: &BackgroundState) {
    if let Some(image) = state.image.as_deref()
        && draw_image_background(conn, surface_id, image)
    {
        return;
    }

    let base = Color::rgb(state.color[0], state.color[1], state.color[2]);
    let top = if state.color == [40, 40, 50] {
        Color::rgb(0.157, 0.157, 0.196)
    } else {
        base
    };
    let bottom = if state.color == [40, 40, 50] {
        Color::rgb(0.078, 0.078, 0.118)
    } else {
        Color::rgb(
            state.color[0] as f32 / 255.0 * 0.7,
            state.color[1] as f32 / 255.0 * 0.7,
            state.color[2] as f32 / 255.0 * 0.7,
        )
    };

    match state.style {
        BackgroundStyle::GradientLines => {
            draw_gradient_background(conn, surface_id, top, bottom, true)
        }
        BackgroundStyle::Gradient => draw_gradient_background(conn, surface_id, top, bottom, false),
        BackgroundStyle::Solid => draw_solid_background(conn, surface_id, top),
    }
}

struct OutputBackground {
    output: u32,
    surface: u32,
    extent: (u32, u32),
}

fn reconcile_outputs(conn: &mut Connection, outputs: &sws_protocol::outputs::OutputSnapshot,
    surfaces: &mut Vec<OutputBackground>, background: &BackgroundState) {
    surfaces.retain(|entry| {
        if outputs.outputs.iter().any(|output| output.id == entry.output) { return true; }
        let _ = conn.destroy_surface(entry.surface);
        false
    });
    for output in &outputs.outputs {
        if conn.output_context(output.id).is_err() { continue; }
        let extent = (output.mode.pixel_width, output.mode.pixel_height);
        if let Some(entry) = surfaces.iter_mut().find(|entry| entry.output == output.id) {
            if entry.extent != extent && conn.resize_window(entry.surface, extent.0, extent.1).is_ok() {
                entry.extent = extent;
                draw_background(conn, entry.surface, background);
            }
        } else {
            let result = SurfaceBuilder::new()
                .app_id("org.scarlet-os.desktop.background").app_name("Background").menu_titles("")
                .size(extent.0, extent.1).window_type(window_types::DESKTOP)
                .resizable(false).focus_on_create(false).active_on_focus(false).build(conn);
            if let Ok(surface) = result {
                surfaces.push(OutputBackground { output: output.id, surface, extent });
                draw_background(conn, surface, background);
            }
        }
    }
}

pub fn run() {
    thread::spawn(background_signal_listener);
    loop {
        let Ok(mut conn) = connect_sws_with_retry() else { return; };
        let mut background = load_background_state();
        let mut surfaces = Vec::new();
        let Ok(mut outputs) = conn.outputs() else { return; };
        reconcile_outputs(&mut conn, &outputs, &mut surfaces, &background);
        loop {
            if conn.dispatch().is_err() { break; }
            while let Some(event) = conn.poll_event() {
                match event {
                    Event::Output(sws_protocol::outputs::OutputEvent::Snapshot(next)) if next.generation != outputs.generation => {
                        reconcile_outputs(&mut conn, &next, &mut surfaces, &background);
                        outputs = next;
                    }
                    Event::SurfaceConfigure { surface_id, width, height }
                    | Event::Output(sws_protocol::outputs::OutputEvent::Window { window: surface_id, width, height, .. }) => {
                        if let Some(entry) = surfaces.iter_mut().find(|entry| entry.surface == surface_id) {
                            let native = outputs.outputs.iter().find(|output| output.id == entry.output)
                                .map_or((width, height), |output| (output.mode.pixel_width, output.mode.pixel_height));
                            // Wallpaper always fills the native output; a scale
                            // change does not increase its backing allocation.
                            if conn.resize_window(surface_id, native.0, native.1).is_ok() {
                                entry.extent = native;
                                draw_background(&mut conn, surface_id, &background);
                            }
                        }
                    }
                    Event::SurfaceDestroyed { surface_id } => surfaces.retain(|entry| entry.surface != surface_id),
                    _ => {}
                }
            }
            if BACKGROUND_CHANGED.swap(false, Ordering::Acquire) {
                let next = load_background_state();
                if next != background {
                    for entry in &surfaces { draw_background(&mut conn, entry.surface, &next); }
                    background = next;
                }
            }
            let _ = conn.wait_for_window_events(Duration::from_millis(100));
        }
    }
}
