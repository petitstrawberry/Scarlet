//! Wallpaper surface owned by the Scarlet workspace shell.
//!
//! Renders a full-screen desktop background as a regular SWS client.

use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use sbus::Message as SbusMessage;
use sbus_client::Connection as SbusConnection;
use scarlet_desktop_config::{
    DESKTOP_BACKGROUND_BUS_NAME, DESKTOP_BACKGROUND_CHANGED_SIGNAL,
    DESKTOP_SETTINGS_INTERFACE, DESKTOP_SETTINGS_OBJECT_PATH, DESKTOP_SETTINGS_SIGNAL_SENDER,
};
use std::println;
use std::thread;
use sws_client::{Connection, Event, SurfaceBuilder};
use sws_protocol::window_types;

const SWS_CONNECT_RETRIES: usize = 100;
const SWS_RETRY_DELAY_MS: u64 = 50;

static BACKGROUND_CHANGED: AtomicBool = AtomicBool::new(false);

#[path = "wallpaper.rs"]
mod wallpaper;
use wallpaper::{load_background_state,BackgroundState};

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

fn draw_background(conn: &mut Connection, surface_id: u32, state: &BackgroundState) {
    let drawn = conn.with_surface_mut(surface_id, |surface| {
        let (width, height) = (surface.width(), surface.height());
        surface.with_buffer(|buf, stride, _| {
            state.draw(width, height, |y, row| {
                let start = y * stride as usize * 4;
                if let Some(dst) = buf.get_mut(start..start + row.len() * 4) {
                    for (pixel, bytes) in row.iter().zip(dst.chunks_exact_mut(4)) {
                        bytes.copy_from_slice(&pixel.to_le_bytes());
                    }
                }
            });
        });
    });
    if drawn.is_some() {
        let _ = conn.commit(surface_id);
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
