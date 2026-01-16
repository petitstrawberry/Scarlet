//! Scarlet Desktop Clock
//!
//! A beautiful analog clock application with smooth animations
//!
//! Features:
//! - Analog clock face with hour, minute, and second hands
//! - Smooth second hand animation
//! - Clock markings (hours and minutes)
//! - Digital time display option
//! - Resizable window
//! - Toggle between analog/digital display
//!
#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use core::f32::consts::PI;
use core::time::Duration;
use scarlet_ui::graphics::{Canvas, measure_text_sized};
use scarlet_ui::{Color, design::Palette};
use std::thread;
use std::vec::Vec;
use std::{format, println};
use sws_client::{Connection, Event, WindowSizeLimits};
use sws_protocol::window_types;

// Simple math functions for clock drawing
trait MathFloat {
    fn sin(self) -> Self;
    fn cos(self) -> Self;
    fn sqrt(self) -> Self;
    fn floor(self) -> Self;
    fn ceil(self) -> Self;
}

impl MathFloat for f32 {
    fn sin(self) -> Self {
        // Taylor series approximation for small angles
        // Normalize to [-π, π]
        let mut x = self;
        while x > PI {
            x -= 2.0 * PI;
        }
        while x < -PI {
            x += 2.0 * PI;
        }

        // Taylor series: sin(x) ≈ x - x³/3! + x⁵/5! - x⁷/7!
        let x2 = x * x;
        let x3 = x2 * x;
        let x5 = x3 * x2;
        let x7 = x5 * x2;

        x - x3 / 6.0 + x5 / 120.0 - x7 / 5040.0
    }

    fn cos(self) -> Self {
        // Taylor series: cos(x) ≈ 1 - x²/2! + x⁴/4! - x⁶/6!
        let mut x = self;
        while x > PI {
            x -= 2.0 * PI;
        }
        while x < -PI {
            x += 2.0 * PI;
        }

        let x2 = x * x;
        let x4 = x2 * x2;
        let x6 = x4 * x2;

        1.0 - x2 / 2.0 + x4 / 24.0 - x6 / 720.0
    }

    fn sqrt(self) -> Self {
        // Newton-Raphson method
        if self < 0.0 {
            return 0.0; // Undefined for negative numbers
        }
        if self == 0.0 {
            return 0.0;
        }

        let mut x = self;
        let mut y = self;
        if self < 1.0 {
            y = 1.0;
        }

        for _ in 0..10 {
            let next = 0.5 * (y + x / y);
            if (next - y).abs() < 0.00001 {
                return next;
            }
            y = next;
        }
        y
    }

    fn floor(self) -> Self {
        self as i32 as f32
    }

    fn ceil(self) -> Self {
        let i = self as i32;
        if i as f32 == self {
            i as f32
        } else if self >= 0.0 {
            (i + 1) as f32
        } else {
            i as f32
        }
    }
}

/// Clock display mode
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DisplayMode {
    Analog,
    Digital,
}

/// Clock application state
struct ClockApp {
    surface_id: u32,
    width: u32,
    height: u32,
    center_x: i32,
    center_y: i32,
    radius: u32,
    display_mode: DisplayMode,
    elapsed_seconds: u32,
    elapsed_ms: u32,
}

impl ClockApp {
    fn new(surface_id: u32, width: u32, height: u32) -> Self {
        let center_x = (width / 2) as i32;
        let center_y = (height / 2) as i32;
        let radius = ((width.min(height) / 2) as u32).saturating_sub(20);

        Self {
            surface_id,
            width,
            height,
            center_x,
            center_y,
            radius,
            display_mode: DisplayMode::Analog,
            elapsed_seconds: 0,
            elapsed_ms: 0,
        }
    }

    fn toggle_mode(&mut self) {
        self.display_mode = match self.display_mode {
            DisplayMode::Analog => DisplayMode::Digital,
            DisplayMode::Digital => DisplayMode::Analog,
        };
    }

    fn update_time(&mut self, delta_ms: u32) {
        self.elapsed_ms += delta_ms;
        while self.elapsed_ms >= 1000 {
            self.elapsed_ms -= 1000;
            self.elapsed_seconds = self.elapsed_seconds.saturating_add(1);
        }
    }
}

/// Draw the analog clock face
fn draw_clock_face(canvas: &mut Canvas, center_x: i32, center_y: i32, radius: u32) {
    let palette = Palette::current();
    let bg_color = palette.bg;
    let face_color = palette.surface;
    let border_color = palette.border;
    let tick_color = palette.text_main;
    let tick_dim = palette.text_sub;

    // Draw outer circle background
    canvas.fill_circle(center_x, center_y, radius + 8, bg_color);

    // Draw clock face
    canvas.fill_circle(center_x, center_y, radius, face_color);

    // Draw outer border
    canvas.draw_circle(center_x, center_y, radius, border_color);
    canvas.draw_circle(center_x, center_y, radius + 1, border_color);

    // Draw hour markings (12 major ticks)
    for i in 0..12 {
        let angle = (i as f32 * 30.0 - 90.0) * PI / 180.0;
        let cos_a = angle.cos();
        let sin_a = angle.sin();

        // Outer point (on the edge)
        let x1 = center_x as f32 + (radius as f32 - 8.0) * cos_a;
        let y1 = center_y as f32 + (radius as f32 - 8.0) * sin_a;

        // Inner point (shorter line for hour marks)
        let x2 = center_x as f32 + (radius as f32 - 25.0) * cos_a;
        let y2 = center_y as f32 + (radius as f32 - 25.0) * sin_a;

        canvas.draw_line(x1 as i32, y1 as i32, x2 as i32, y2 as i32, tick_color);
    }

    // Draw minute markings (48 minor ticks)
    for i in 0..60 {
        if i % 5 == 0 {
            continue; // Skip hour positions
        }

        let angle = (i as f32 * 6.0 - 90.0) * PI / 180.0;
        let cos_a = angle.cos();
        let sin_a = angle.sin();

        // Outer point
        let x1 = center_x as f32 + (radius as f32 - 8.0) * cos_a;
        let y1 = center_y as f32 + (radius as f32 - 8.0) * sin_a;

        // Inner point (shorter for minute marks)
        let x2 = center_x as f32 + (radius as f32 - 15.0) * cos_a;
        let y2 = center_y as f32 + (radius as f32 - 15.0) * sin_a;

        canvas.draw_line(x1 as i32, y1 as i32, x2 as i32, y2 as i32, tick_dim);
    }
}

/// Draw clock hands (hour, minute, second)
fn draw_hands(
    canvas: &mut Canvas,
    center_x: i32,
    center_y: i32,
    radius: u32,
    hours: u32,
    minutes: u32,
    seconds: u32,
    milliseconds: u32,
) {
    // Hand colors
    let palette = Palette::current();
    let hour_color = palette.text_main;
    let minute_color = palette.text_sub;
    let second_color = palette.error;

    // Calculate angles (in radians, -90 to start at 12 o'clock)
    // Hour hand: moves based on hours + minutes + seconds
    let hour_angle = ((hours % 12) as f32 * 30.0 + minutes as f32 * 0.5 - 90.0) * PI / 180.0;

    // Minute hand: moves based on minutes + seconds
    let minute_angle = (minutes as f32 * 6.0 + seconds as f32 * 0.1 - 90.0) * PI / 180.0;

    // Second hand: smooth animation with milliseconds
    let second_angle = (seconds as f32 * 6.0 + milliseconds as f32 * 0.006 - 90.0) * PI / 180.0;

    // Draw hour hand (thicker, shorter)
    let hour_length = radius as f32 * 0.5;
    let hour_x = center_x as f32 + hour_length * hour_angle.cos();
    let hour_y = center_y as f32 + hour_length * hour_angle.sin();
    draw_thick_line(
        canvas,
        center_x,
        center_y,
        hour_x as i32,
        hour_y as i32,
        6,
        hour_color,
    );

    // Draw minute hand (thinner, longer)
    let minute_length = radius as f32 * 0.75;
    let minute_x = center_x as f32 + minute_length * minute_angle.cos();
    let minute_y = center_y as f32 + minute_length * minute_angle.sin();
    draw_thick_line(
        canvas,
        center_x,
        center_y,
        minute_x as i32,
        minute_y as i32,
        4,
        minute_color,
    );

    // Draw second hand (thinnest, longest, red accent color)
    let second_length = radius as f32 * 0.85;
    let second_x = center_x as f32 + second_length * second_angle.cos();
    let second_y = center_y as f32 + second_length * second_angle.sin();
    draw_thick_line(
        canvas,
        center_x,
        center_y,
        second_x as i32,
        second_y as i32,
        2,
        second_color,
    );

    // Draw center cap (covers the hand origins)
    canvas.fill_circle(center_x, center_y, 8, palette.elevated);
    canvas.fill_circle(center_x, center_y, 6, second_color);
}

/// Draw a thick line (hand with width)
fn draw_thick_line(
    canvas: &mut Canvas,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    thickness: u32,
    color: Color,
) {
    if thickness <= 1 {
        canvas.draw_line(x0, y0, x1, y1, color);
        return;
    }

    let dx = (x1 - x0) as f32;
    let dy = (y1 - y0) as f32;
    let length = (dx * dx + dy * dy).sqrt();

    if length < 0.1 {
        // Point, draw a circle
        canvas.fill_circle(x0, y0, thickness / 2, color);
        return;
    }

    // Perpendicular vector
    let px = -dy / length;
    let py = dx / length;

    let half_thick = thickness as f32 / 2.0;

    // Four corners of the thick line
    let corners = [
        (x0 as f32 + px * half_thick, y0 as f32 + py * half_thick),
        (x0 as f32 - px * half_thick, y0 as f32 - py * half_thick),
        (x1 as f32 - px * half_thick, y1 as f32 - py * half_thick),
        (x1 as f32 + px * half_thick, y1 as f32 + py * half_thick),
    ];

    // Fill the polygon using scanlines
    let min_y = corners
        .iter()
        .map(|p| p.1.floor() as i32)
        .min()
        .unwrap_or(0);
    let max_y = corners.iter().map(|p| p.1.ceil() as i32).max().unwrap_or(0);

    for y in min_y..=max_y {
        let mut intersections = Vec::new();

        for i in 0..4 {
            let p1 = corners[i];
            let p2 = corners[(i + 1) % 4];

            if (p1.1 <= y as f32 && p2.1 > y as f32) || (p2.1 <= y as f32 && p1.1 > y as f32) {
                let t = (y as f32 - p1.1) / (p2.1 - p1.1);
                let x = p1.0 + t * (p2.0 - p1.0);
                intersections.push(x);
            }
        }

        intersections.sort_by(|a, b| a.partial_cmp(b).unwrap());

        for i in (0..intersections.len()).step_by(2) {
            if i + 1 < intersections.len() {
                let x1 = intersections[i].floor() as i32;
                let x2 = intersections[i + 1].ceil() as i32;
                canvas.draw_hline(x1, y, (x2 - x1 + 1).max(0) as u32, color);
            }
        }
    }
}

/// Draw digital clock display
fn draw_digital_clock(
    canvas: &mut Canvas,
    width: u32,
    height: u32,
    hours: u32,
    minutes: u32,
    seconds: u32,
) {
    let palette = Palette::current();
    let bg_color = palette.bg;
    let text_color = palette.text_main;
    let accent_color = palette.error;

    // Fill background
    canvas.fill_rect(0, 0, width, height, bg_color);

    // Format time string
    let time_str = format!("{:02}:{:02}:{:02}", hours, minutes, seconds);

    // Measure text
    let (text_w, text_h) = measure_text_sized(&time_str, 72.0);

    // Center the text
    let text_x = ((width as i32 - text_w as i32) / 2).max(0);
    let text_y = ((height as i32 - text_h as i32) / 2).max(0);

    // Draw time
    canvas.draw_text_sized(text_x, text_y, &time_str, text_color, 72.0);

    // Draw "Digital Mode" label at bottom
    let label = "Digital Clock";
    let (label_w, _) = measure_text_sized(label, 14.0);
    let label_x = ((width as i32 - label_w as i32) / 2).max(0);
    let label_y = (height as i32 - 30).max(0);
    canvas.draw_text_sized(label_x, label_y, label, accent_color, 14.0);
}

/// Draw the clock (based on current mode)
fn draw_clock(app: &ClockApp, conn: &mut Connection) {
    let Some(surface) = conn.surface_mut(app.surface_id) else {
        return;
    };

    let w = surface.width();
    let h = surface.height();

    surface.with_buffer(|buf, width, height| {
        let mut canvas = Canvas::new(buf, width, height);

        match app.display_mode {
            DisplayMode::Analog => {
                // Draw analog clock
                draw_clock_face(&mut canvas, app.center_x, app.center_y, app.radius);

                // Calculate time components
                let total_seconds = app.elapsed_seconds;
                let hours = (total_seconds / 3600) % 12;
                let minutes = (total_seconds / 60) % 60;
                let seconds = total_seconds % 60;

                // Draw hands with smooth animation
                draw_hands(
                    &mut canvas,
                    app.center_x,
                    app.center_y,
                    app.radius,
                    hours,
                    minutes,
                    seconds,
                    app.elapsed_ms,
                );

                // Draw mode indicator at bottom
                let mode_text = "Analog - Click to toggle";
                let (text_w, _) = measure_text_sized(mode_text, 12.0);
                let text_x = ((w as i32 - text_w as i32) / 2).max(0);
                let text_y = (h as i32 - 20).max(0);
                let palette = Palette::current();
                canvas.draw_text_sized(text_x, text_y, mode_text, palette.text_mute, 12.0);
            }
            DisplayMode::Digital => {
                // Calculate time components (12-hour format)
                let total_seconds = app.elapsed_seconds;
                let hours = (total_seconds / 3600) % 12;
                let minutes = (total_seconds / 60) % 60;
                let seconds = total_seconds % 60;

                // Use 12-hour display (add 12 if it's 0 for midnight)
                let display_hours = if hours == 0 { 12 } else { hours };

                draw_digital_clock(&mut canvas, w, h, display_hours, minutes, seconds);
            }
        }
    });

    let _ = conn.commit(app.surface_id);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[clock] Starting Scarlet Desktop Clock");

    // Connect to SWS
    let mut conn = match Connection::connect("/tmp/sws.sock") {
        Ok(c) => c,
        Err(_) => {
            println!("[clock] Failed to connect to SWS");
            return 1;
        }
    };

    // Initial window size
    let width = 400u32;
    let height = 450u32;

    // Create surface
    let surface_id =
        match conn.create_surface("org.scarlet-os.desktop.clock", "Clock", "", width, height) {
            Ok(id) => id,
            Err(_) => {
                println!("[clock] Failed to create surface");
                return 1;
            }
        };

    // Set window type to NORMAL (resizable application window)
    let _ = conn.set_window_type(surface_id, window_types::NORMAL);

    // Enable resizing
    let _ = conn.set_window_resizable(surface_id, true);

    // Set size limits
    let _ = conn.set_window_size_limits(
        surface_id,
        WindowSizeLimits {
            min_width: 200,
            min_height: 250,
            max_width: 800,
            max_height: 900,
        },
    );

    // Center window on screen
    let _ = conn.move_window(surface_id, 760, 315);

    // Initialize clock app
    let mut app = ClockApp::new(surface_id, width, height);

    // Initial draw
    draw_clock(&app, &mut conn);

    let mut cursor_x: i32 = 0;
    let mut cursor_y: i32 = 0;
    let mut tick_ms: u32 = 0;

    loop {
        let _ = conn.dispatch();
        let mut needs_redraw = false;

        // Process all events
        while let Some(ev) = conn.poll_event() {
            match ev {
                Event::Input(input) if input.surface_id == surface_id => {
                    match input.type_ {
                        0x03 => {
                            // EV_ABS - absolute cursor position
                            match input.code {
                                0x00 => cursor_x = input.value, // ABS_X
                                0x01 => cursor_y = input.value, // ABS_Y
                                _ => {}
                            }
                        }
                        0x01 => {
                            // EV_KEY - mouse button
                            if input.code == 0x110 && input.value == 0 {
                                // BTN_LEFT released
                                // Check if click is within window
                                if cursor_x >= 0
                                    && cursor_x < app.width as i32
                                    && cursor_y >= 0
                                    && cursor_y < app.height as i32
                                {
                                    app.toggle_mode();
                                    needs_redraw = true;
                                    println!("[clock] Toggled mode to {:?}", app.display_mode);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Event::SurfaceConfigure {
                    surface_id: sid,
                    width,
                    height,
                } if sid == surface_id => {
                    println!("[clock] SurfaceConfigure: {}x{}", width, height);
                    app.width = width;
                    app.height = height;
                    app.center_x = (width / 2) as i32;
                    app.center_y = (height / 2) as i32;
                    app.radius = ((width.min(height) / 2) as u32).saturating_sub(20);
                    needs_redraw = true;
                }
                Event::SurfaceDestroyed { surface_id: sid } if sid == surface_id => {
                    println!("[clock] Clock window destroyed");
                    return 0;
                }
                _ => {}
            }
        }

        // Update time (smooth 60 FPS animation)
        tick_ms = tick_ms.saturating_add(16);
        if tick_ms >= 16 {
            tick_ms = 0;
            app.update_time(16);
            needs_redraw = true;
        }

        // Redraw if needed
        if needs_redraw {
            draw_clock(&app, &mut conn);
        }

        // Sleep for ~60 FPS
        thread::sleep(Duration::from_millis(16));
    }
}
