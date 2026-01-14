//! Scarlet Desktop Taskbar.
//!
//! Provides a dock/taskbar surface as a regular SWS client.
//!
//! - Window type: TASKBAR
//! - Reads configuration from /etc/scarlet-desktop.d/
//! - Resizes to configured height
//! - Positions itself based on configured position (top/bottom)
//! - Sends workarea notification to SWS
//! - Clicking the left button launches `scarlet_desktop_overview`
//!
#![no_std]
#![no_main]

extern crate scarlet_desktop_config;
extern crate scarlet_std as std;

use core::time::Duration;
use scarlet_desktop_config::{TaskbarConfig, TaskbarPosition};
use scarlet_ui::Color;
use scarlet_ui::graphics::{Canvas, measure_text_sized};
use std::task::{EXECVE_FORCE_ABI_REBUILD, execve_with_flags, exit, fork};
use std::thread;
use std::{format, println};
use sws_client::{Connection, Event, InputEvent, WindowSizeLimits};
use sws_protocol::window_types;

fn load_config() -> TaskbarConfig {
    scarlet_desktop_config::load_desktop_config().taskbar
}

fn launch_overview() {
    match fork() {
        0 => {
            let candidates = [
                "/bin",
                "/scarlet/system/scarlet/bin",
                "/old_root/system/scarlet/bin",
            ];

            for base in &candidates {
                let mut path = std::string::String::new();
                path.push_str(base);
                path.push('/');
                path.push_str("scarlet_desktop_overview");

                let argv0 = path.as_str();
                let argv = [argv0];

                let rc = execve_with_flags(argv0, &argv, &[], EXECVE_FORCE_ABI_REBUILD);
                if rc == 0 {
                    break;
                }
            }

            println!("[scarlet_desktop_taskbar] Failed to exec overview");
            exit(127);
        }
        -1 => {
            println!("[scarlet_desktop_taskbar] fork failed for overview");
        }
        _pid => {}
    }
}

fn handle_input(
    ev: InputEvent,
    cursor_x: &mut i32,
    cursor_y: &mut i32,
    left_down: &mut bool,
    pressed_in_start: &mut bool,
    start_rect: (i32, i32, u32, u32),
) -> bool {
    // Return true if UI state changed and a redraw is needed.
    match ev.type_ {
        0x03 => {
            // EV_ABS
            match ev.code {
                0x00 => *cursor_x = ev.value, // ABS_X
                0x01 => *cursor_y = ev.value, // ABS_Y
                _ => {}
            }
            false
        }
        0x01 => {
            // EV_KEY
            // BTN_LEFT = 0x110
            if ev.code == 0x110 {
                if ev.value != 0 {
                    *left_down = true;
                    let (x, y, w, h) = start_rect;
                    let inside = *cursor_x >= x
                        && *cursor_x < x + w as i32
                        && *cursor_y >= y
                        && *cursor_y < y + h as i32;
                    *pressed_in_start = inside;
                    true
                } else {
                    *left_down = false;
                    let (x, y, w, h) = start_rect;
                    let inside = *cursor_x >= x
                        && *cursor_x < x + w as i32
                        && *cursor_y >= y
                        && *cursor_y < y + h as i32;
                    if *pressed_in_start && inside {
                        launch_overview();
                    }
                    *pressed_in_start = false;
                    true
                }
            } else {
                false
            }
        }
        _ => false,
    }
}

fn draw_taskbar(
    conn: &mut Connection,
    surface_id: u32,
    seconds: u32,
    left_down: bool,
    pressed_in_start: bool,
    position: TaskbarPosition,
) {
    let Some(surface) = conn.surface_mut(surface_id) else {
        return;
    };

    let w = surface.width();
    let h = surface.height();

    let start_label = "Overview";
    let font_size = 18.0;
    let (tw, th) = measure_text_sized(start_label, font_size);

    let pad_x = 14i32;
    let pad_y = 10i32;
    let button_w = tw.saturating_add(24);
    let button_h = th.saturating_add(12).min(h);
    let button_x = 8i32;
    let button_y = ((h as i32).saturating_sub(button_h as i32) / 2).max(0);

    let clock_text = format!("uptime {}s", seconds);
    let (cw, ch) = measure_text_sized(&clock_text, 16.0);
    let clock_x = (w as i32).saturating_sub(cw as i32).saturating_sub(12);
    let clock_y = ((h as i32).saturating_sub(ch as i32) / 2).max(0);

    // Design differences for top vs bottom
    let (bg_color, border_color, border_pos) = match position {
        TaskbarPosition::Top => (
            Color::rgb(22, 26, 34),
            Color::rgb(60, 68, 88),
            h.saturating_sub(1) as i32,
        ),
        TaskbarPosition::Bottom => (
            Color::rgb(22, 26, 34),
            Color::rgb(60, 68, 88),
            0,
        ),
    };

    surface.with_buffer(|buf, width, height| {
        let mut canvas = Canvas::new(buf, width, height);

        // Bar background.
        canvas.fill_rect(0, 0, w, h, bg_color);
        canvas.draw_hline(0, border_pos, w, border_color);

        // Start button.
        let base = Color::rgb(46, 52, 66);
        let active = Color::rgb(240, 96, 72);
        let btn_color = if pressed_in_start && left_down {
            active
        } else {
            base
        };

        canvas.fill_rounded_rect(button_x, button_y, button_w, button_h, 10, btn_color);
        canvas.draw_text_sized(
            button_x + pad_x,
            button_y + pad_y,
            start_label,
            Color::rgb(238, 242, 249),
            font_size,
        );

        // Clock (right-aligned).
        canvas.draw_text_sized(
            clock_x,
            clock_y + 12,
            &clock_text,
            Color::rgb(175, 186, 208),
            16.0,
        );
    });

    let _ = conn.commit(surface_id);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[scarlet_desktop_taskbar] starting");

    let config = load_config();
    let bar_height: u32 = config.height.unwrap_or(40).max(1);
    let position: TaskbarPosition = config.position.unwrap_or(TaskbarPosition::Top);

    let mut conn = match Connection::connect("/tmp/sws.sock") {
        Ok(c) => c,
        Err(_) => {
            println!("[scarlet_desktop_taskbar] Failed to connect to SWS");
            return 1;
        }
    };

    let surface_id = match conn.create_surface(320, bar_height) {
        Ok(id) => id,
        Err(_) => {
            println!("[scarlet_desktop_taskbar] Failed to create surface");
            return 1;
        }
    };

    let _ = conn.set_window_type(surface_id, window_types::TASKBAR);

    // Disable resizing by setting fixed size limits
    // Note: Initial limits will be updated after SurfaceConfigure
    let _ = conn.set_window_size_limits(
        surface_id,
        WindowSizeLimits {
            min_width: 320,
            min_height: bar_height,
            max_width: 320,
            max_height: bar_height,
        },
    );

    // Ask the server for the screen size; we'll convert the configure size into
    // a docked bar size.
    let _ = conn.maximize_window(surface_id);

    let mut screen_w: u32 = 320;
    let mut screen_h: u32 = 240;
    let mut actual_screen_w: u32 = 320;
    
    // Initialize actual_screen_w
    actual_screen_w = 320;

    let mut cursor_x: i32 = 0;
    let mut cursor_y: i32 = 0;
    let mut left_down: bool = false;
    let mut pressed_in_start: bool = false;

    let mut seconds: u32 = 0;
    let mut tick_ms: u32 = 0;

    // Initial draw.
    draw_taskbar(&mut conn, surface_id, seconds, left_down, pressed_in_start, position);

    loop {
        let _ = conn.dispatch();
        let mut needs_redraw = false;

        // Start button rect depends on current surface size.
        // Compute it from the same sizing logic used in draw_taskbar.
        let start_label = "Overview";
        let (tw, th) = measure_text_sized(start_label, 18.0);
        let button_w = tw.saturating_add(24);
        let button_h = th.saturating_add(12).min(bar_height);
        let button_x = 8i32;
        let button_y = ((bar_height as i32).saturating_sub(button_h as i32) / 2).max(0);
        let start_rect = (button_x, button_y, button_w, button_h);

        while let Some(ev) = conn.poll_event() {
            match ev {
                Event::Input(input) if input.surface_id == surface_id => {
                    if handle_input(
                        input,
                        &mut cursor_x,
                        &mut cursor_y,
                        &mut left_down,
                        &mut pressed_in_start,
                        start_rect,
                    ) {
                        needs_redraw = true;
                    }
                }
                Event::SurfaceConfigure {
                    surface_id: sid,
                    width,
                    height,
                } if sid == surface_id => {
                    // Treat configure width/height as screen dimensions.
                    screen_h = height;
                    actual_screen_w = width;

                    if conn.resize_window(surface_id, actual_screen_w, bar_height).is_ok() {
                        let y = match position {
                            TaskbarPosition::Top => 0,
                            TaskbarPosition::Bottom => screen_h.saturating_sub(bar_height) as i32,
                        };
                        let _ = conn.move_window(surface_id, 0, y);

                        // Send workarea notification
                        let workarea_y = match position {
                            TaskbarPosition::Top => bar_height as i32,
                            TaskbarPosition::Bottom => 0,
                        };
                        let workarea_width = actual_screen_w;
                        let workarea_height = match position {
                            TaskbarPosition::Top => screen_h.saturating_sub(bar_height),
                            TaskbarPosition::Bottom => screen_h.saturating_sub(bar_height),
                        };
                        let _ = conn.set_workarea(0, workarea_y, workarea_width, workarea_height);
                        println!(
                            "[scarlet_desktop_taskbar] Workarea: x=0, y={}, width={}, height={}",
                            workarea_y, workarea_width, workarea_height
                        );

                        needs_redraw = true;
                    }
                }
                Event::SurfaceDestroyed { surface_id: sid } if sid == surface_id => {
                    println!("[scarlet_desktop_taskbar] destroyed");
                    return 0;
                }
                _ => {}
            }
        }

        // Simple uptime clock.
        tick_ms = tick_ms.saturating_add(16);
        if tick_ms >= 1000 {
            tick_ms = 0;
            seconds = seconds.saturating_add(1);
            needs_redraw = true;
        }

        if needs_redraw {
            draw_taskbar(&mut conn, surface_id, seconds, left_down, pressed_in_start, position);
        }

        thread::sleep(Duration::from_millis(16));
    }
}
