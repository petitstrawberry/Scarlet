//! Built-in box-drawing and block-element rendering for text grids.

use crate::color::Color;
use crate::graphics;

#[derive(Clone, Copy)]
enum Arm {
    Right,
    Down,
    Up,
    Left,
}

fn rect(canvas: &mut graphics::Canvas, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w > 0 && h > 0 {
        canvas.fill_rect(x, y, w as u32, h as u32, color);
    }
}

fn arm(
    canvas: &mut graphics::Canvas,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    dir: Arm,
    stroke: u32,
    color: Color,
) {
    let cx = x + w as i32 / 2;
    let cy = y + h as i32 / 2;
    let end_x = x.saturating_add(w as i32);
    let end_y = y.saturating_add(h as i32);
    let stroke = stroke as i32;
    let half = stroke / 2;

    match dir {
        Arm::Right => rect(
            canvas,
            cx - half,
            cy - half,
            end_x - cx + half,
            stroke,
            color,
        ),
        Arm::Left => rect(canvas, x, cy - half, cx - x + half, stroke, color),
        Arm::Down => rect(
            canvas,
            cx - half,
            cy - half,
            stroke,
            end_y - cy + half,
            color,
        ),
        Arm::Up => rect(canvas, cx - half, y, stroke, cy - y + half, color),
    }
}

fn weighted_arm(
    canvas: &mut graphics::Canvas,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    dir: Arm,
    heavy_arm: bool,
    light: u32,
    color: Color,
) {
    arm(
        canvas,
        x,
        y,
        w,
        h,
        dir,
        if heavy_arm { light * 2 } else { light },
        color,
    );
}

fn dash(
    canvas: &mut graphics::Canvas,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    horizontal: bool,
    count: u32,
    stroke: u32,
    color: Color,
) {
    let cx = x + w as i32 / 2;
    let cy = y + h as i32 / 2;
    let stroke_i = stroke as i32;
    let half = stroke_i / 2;
    let total = if horizontal { w } else { h };
    let spacing = total / count;
    let seg_len = (spacing * 2) / 3;

    for i in 0..count {
        let start = i * spacing;
        if horizontal {
            rect(canvas, x + start as i32, cy - half, seg_len as i32, stroke_i, color);
        } else {
            rect(canvas, cx - half, y + start as i32, stroke_i, seg_len as i32, color);
        }
    }
}

fn double_arm(
    canvas: &mut graphics::Canvas,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    dir: Arm,
    light: u32,
    color: Color,
) {
    let cx = x + w as i32 / 2;
    let cy = y + h as i32 / 2;
    let end_x = x.saturating_add(w as i32);
    let end_y = y.saturating_add(h as i32);
    let sub = (light / 2).max(1) as i32;
    let offset = (light as i32 + sub) / 2;

    match dir {
        Arm::Right => {
            rect(canvas, cx, cy - offset, end_x - cx, sub, color);
            rect(canvas, cx, cy + offset, end_x - cx, sub, color);
        }
        Arm::Left => {
            rect(canvas, x, cy - offset, cx - x, sub, color);
            rect(canvas, x, cy + offset, cx - x, sub, color);
        }
        Arm::Down => {
            rect(canvas, cx - offset, cy, sub, end_y - cy, color);
            rect(canvas, cx + offset, cy, sub, end_y - cy, color);
        }
        Arm::Up => {
            rect(canvas, cx - offset, y, sub, cy - y, color);
            rect(canvas, cx + offset, y, sub, cy - y, color);
        }
    }
}

fn line(
    canvas: &mut graphics::Canvas,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    dirs: &[Arm],
    weights: u8,
    light: u32,
    color: Color,
) {
    for (index, dir) in dirs.iter().copied().enumerate() {
        weighted_arm(
            canvas,
            x,
            y,
            w,
            h,
            dir,
            weights & (1 << index) != 0,
            light,
            color,
        );
    }
}

fn double_line(
    canvas: &mut graphics::Canvas,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    dirs: &[Arm],
    mask: u8,
    light: u32,
    color: Color,
) {
    for (index, dir) in dirs.iter().copied().enumerate() {
        if mask & (1 << index) != 0 {
            double_arm(canvas, x, y, w, h, dir, light, color);
        } else {
            arm(canvas, x, y, w, h, dir, light, color);
        }
    }
}

fn lower_block(
    canvas: &mut graphics::Canvas,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    eighths: u32,
    color: Color,
) {
    let fill = h.saturating_mul(eighths) / 8;
    canvas.fill_rect(x, y + h.saturating_sub(fill) as i32, w, fill, color);
}

fn quad(
    canvas: &mut graphics::Canvas,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    ul: bool,
    ur: bool,
    ll: bool,
    lr: bool,
    color: Color,
) {
    let lw = w / 2;
    let rw = w - lw;
    let th = h / 2;
    let bh = h - th;

    if ul {
        canvas.fill_rect(x, y, lw, th, color);
    }
    if ur {
        canvas.fill_rect(x + lw as i32, y, rw, th, color);
    }
    if ll {
        canvas.fill_rect(x, y + th as i32, lw, bh, color);
    }
    if lr {
        canvas.fill_rect(x + lw as i32, y + th as i32, rw, bh, color);
    }
}

fn dim(color: Color, factor: f32) -> Color {
    Color::rgba_f32(color.r * factor, color.g * factor, color.b * factor, color.a)
}

fn shade(canvas: &mut graphics::Canvas, x: i32, y: i32, w: u32, h: u32, ch: char, color: Color) {
    let factor = match ch {
        '░' => 0.25,
        '▒' => 0.50,
        '▓' => 0.75,
        _ => return,
    };
    canvas.fill_rect(x, y, w, h, dim(color, factor));
}

fn draw_block(
    canvas: &mut graphics::Canvas,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    ch: char,
    color: Color,
) -> bool {
    match ch {
        '▁' => lower_block(canvas, x, y, w, h, 1, color),
        '▂' => lower_block(canvas, x, y, w, h, 2, color),
        '▃' => lower_block(canvas, x, y, w, h, 3, color),
        '▄' => lower_block(canvas, x, y, w, h, 4, color),
        '▅' => lower_block(canvas, x, y, w, h, 5, color),
        '▆' => lower_block(canvas, x, y, w, h, 6, color),
        '▇' => lower_block(canvas, x, y, w, h, 7, color),
        '▀' => canvas.fill_rect(x, y, w, h / 2, color),
        '█' => canvas.fill_rect(x, y, w, h, color),
        '▉' => canvas.fill_rect(x, y, w.saturating_mul(7) / 8, h, color),
        '▊' => canvas.fill_rect(x, y, w.saturating_mul(6) / 8, h, color),
        '▋' => canvas.fill_rect(x, y, w.saturating_mul(5) / 8, h, color),
        '▌' => canvas.fill_rect(x, y, w / 2, h, color),
        '▍' => canvas.fill_rect(x, y, w.saturating_mul(3) / 8, h, color),
        '▎' => canvas.fill_rect(x, y, w / 4, h, color),
        '▏' => canvas.fill_rect(x, y, w / 8, h, color),
        '▐' => canvas.fill_rect(x + (w / 2) as i32, y, w - w / 2, h, color),
        '░' | '▒' | '▓' => shade(canvas, x, y, w, h, ch, color),
        '▔' => canvas.fill_rect(x, y, w, (h / 8).max(1), color),
        '▕' => canvas.fill_rect(
            x + w.saturating_sub((w / 8).max(1)) as i32,
            y,
            (w / 8).max(1),
            h,
            color,
        ),
        '▖' => quad(canvas, x, y, w, h, false, false, true, false, color),
        '▗' => quad(canvas, x, y, w, h, false, false, false, true, color),
        '▘' => quad(canvas, x, y, w, h, true, false, false, false, color),
        '▙' => quad(canvas, x, y, w, h, true, false, true, true, color),
        '▚' => quad(canvas, x, y, w, h, true, false, false, true, color),
        '▛' => quad(canvas, x, y, w, h, true, true, true, false, color),
        '▜' => quad(canvas, x, y, w, h, true, true, false, true, color),
        '▝' => quad(canvas, x, y, w, h, false, true, false, false, color),
        '▞' => quad(canvas, x, y, w, h, false, true, true, false, color),
        '▟' => quad(canvas, x, y, w, h, false, true, true, true, color),
        _ => return false,
    }
    true
}

/// Draw a box-drawing or block-element character using built-in primitives.
///
/// # Arguments
///
/// * `canvas` - Canvas to draw into.
/// * `x` - Left edge of the text cell.
/// * `y` - Top edge of the text cell.
/// * `w` - Width of the text cell.
/// * `h` - Height of the text cell.
/// * `ch` - Box-drawing or block-element character to draw.
/// * `color` - Foreground color.
pub fn draw_box_drawing_char(
    canvas: &mut graphics::Canvas,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    ch: char,
    color: Color,
) {
    let light = libm::roundf(w as f32 / 8.0).max(1.0) as u32;
    let heavy = light * 2;
    let code = ch as u32;

    if draw_block(canvas, x, y, w, h, ch, color) {
        return;
    }

    match ch {
        '─' => line(
            canvas,
            x,
            y,
            w,
            h,
            &[Arm::Left, Arm::Right],
            0,
            light,
            color,
        ),
        '━' => line(
            canvas,
            x,
            y,
            w,
            h,
            &[Arm::Left, Arm::Right],
            0b11,
            light,
            color,
        ),
        '│' => line(canvas, x, y, w, h, &[Arm::Up, Arm::Down], 0, light, color),
        '┃' => line(
            canvas,
            x,
            y,
            w,
            h,
            &[Arm::Up, Arm::Down],
            0b11,
            light,
            color,
        ),
        '╴' => arm(canvas, x, y, w, h, Arm::Left, light, color),
        '╵' => arm(canvas, x, y, w, h, Arm::Up, light, color),
        '╶' => arm(canvas, x, y, w, h, Arm::Right, light, color),
        '╷' => arm(canvas, x, y, w, h, Arm::Down, light, color),
        '╸' => arm(canvas, x, y, w, h, Arm::Left, heavy, color),
        '╹' => arm(canvas, x, y, w, h, Arm::Up, heavy, color),
        '╺' => arm(canvas, x, y, w, h, Arm::Right, heavy, color),
        '╻' => arm(canvas, x, y, w, h, Arm::Down, heavy, color),
        '╼' => line(canvas, x, y, w, h, &[Arm::Left, Arm::Down], 0, light, color),
        '╽' => line(
            canvas,
            x,
            y,
            w,
            h,
            &[Arm::Left, Arm::Down],
            0b11,
            light,
            color,
        ),
        '╾' => line(canvas, x, y, w, h, &[Arm::Right, Arm::Up], 0, light, color),
        '╿' => line(
            canvas,
            x,
            y,
            w,
            h,
            &[Arm::Right, Arm::Up],
            0b11,
            light,
            color,
        ),
        '═' => {
            double_arm(canvas, x, y, w, h, Arm::Left, light, color);
            double_arm(canvas, x, y, w, h, Arm::Right, light, color);
        }
        '║' => {
            double_arm(canvas, x, y, w, h, Arm::Up, light, color);
            double_arm(canvas, x, y, w, h, Arm::Down, light, color);
        }
        '╭' => line(
            canvas,
            x,
            y,
            w,
            h,
            &[Arm::Right, Arm::Down],
            0,
            light,
            color,
        ),
        '╮' => line(canvas, x, y, w, h, &[Arm::Left, Arm::Down], 0, light, color),
        '╰' => line(canvas, x, y, w, h, &[Arm::Right, Arm::Up], 0, light, color),
        '╯' => line(canvas, x, y, w, h, &[Arm::Left, Arm::Up], 0, light, color),
        '╱' | '╲' | '╳' => {}
        _ if (0x2504..=0x250B).contains(&code) => {
            let index = code - 0x2504;
            dash(
                canvas,
                x,
                y,
                w,
                h,
                index % 4 < 2,
                if index < 4 { 3 } else { 4 },
                if index % 2 == 0 { light } else { heavy },
                color,
            );
        }
        _ if (0x250C..=0x251B).contains(&code) => {
            let group = (code - 0x250C) / 4;
            let index = ((code - 0x250C) % 4) as u8;
            let dirs: &[Arm] = match group {
                0 => &[Arm::Right, Arm::Down],
                1 => &[Arm::Left, Arm::Down],
                2 => &[Arm::Right, Arm::Up],
                _ => &[Arm::Left, Arm::Up],
            };
            line(
                canvas,
                x,
                y,
                w,
                h,
                dirs,
                match index {
                    1 => 0b01,
                    2 => 0b10,
                    3 => 0b11,
                    _ => 0,
                },
                light,
                color,
            );
        }
        _ if (0x251C..=0x253B).contains(&code) => {
            let group = (code - 0x251C) / 8;
            let weights = ((code - 0x251C) % 8) as u8;
            let dirs: &[Arm] = match group {
                0 => &[Arm::Right, Arm::Down, Arm::Up],
                1 => &[Arm::Left, Arm::Down, Arm::Up],
                2 => &[Arm::Left, Arm::Right, Arm::Down],
                _ => &[Arm::Left, Arm::Right, Arm::Up],
            };
            line(canvas, x, y, w, h, dirs, weights, light, color);
        }
        _ if (0x253C..=0x254B).contains(&code) => {
            line(
                canvas,
                x,
                y,
                w,
                h,
                &[Arm::Right, Arm::Down, Arm::Up, Arm::Left],
                (code - 0x253C) as u8,
                light,
                color,
            );
        }
        _ if (0x2552..=0x255D).contains(&code) => {
            let group = (code - 0x2552) / 3;
            let idx = (code - 0x2552) % 3;
            let dirs: &[Arm] = match group {
                0 => &[Arm::Right, Arm::Down],
                1 => &[Arm::Left, Arm::Down],
                2 => &[Arm::Right, Arm::Up],
                _ => &[Arm::Left, Arm::Up],
            };
            let mask = match idx {
                0 => 0b01,
                1 => 0b10,
                _ => 0b11,
            };
            double_line(canvas, x, y, w, h, dirs, mask, light, color);
        }
        _ if (0x255E..=0x2569).contains(&code) => {
            let group = (code - 0x255E) / 3;
            let idx = (code - 0x255E) % 3;
            let (dirs, mask): (&[Arm], u8) = match group {
                0 => (&[Arm::Right, Arm::Down, Arm::Up], match idx { 0 => 0b001, 1 => 0b110, _ => 0b111 }),
                1 => (&[Arm::Left, Arm::Down, Arm::Up], match idx { 0 => 0b001, 1 => 0b110, _ => 0b111 }),
                2 => (&[Arm::Left, Arm::Right, Arm::Down], match idx { 0 => 0b110, 1 => 0b001, _ => 0b111 }),
                _ => (&[Arm::Left, Arm::Right, Arm::Up], match idx { 0 => 0b110, 1 => 0b001, _ => 0b111 }),
            };
            double_line(canvas, x, y, w, h, dirs, mask, light, color);
        }
        _ if (0x256A..=0x256C).contains(&code) => {
            let mask: u8 = match code {
                0x256A => 0b1001,
                0x256B => 0b0110,
                _ => 0b1111,
            };
            double_line(
                canvas,
                x,
                y,
                w,
                h,
                &[Arm::Right, Arm::Down, Arm::Up, Arm::Left],
                mask,
                light,
                color,
            );
        }
        _ => {}
    }
}
