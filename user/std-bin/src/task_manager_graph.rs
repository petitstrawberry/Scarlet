//! Bounded, pixel-sized triangle geometry for Task Manager's SGFX graphs.
//!
//! Kept independent of the renderer so edge cases can be tested on the host.

pub const HISTORY_CAPACITY: usize = 60;
pub const MAX_VERTICES: usize = 5 * 6 + (HISTORY_CAPACITY - 1) * 24;

/// Emit a triangle list in clip space. Work and storage depend on the number
/// of samples, never the number of pixels in the graph.
pub fn emit_vertices(
    history: &[u32],
    width: f32,
    height: f32,
    grid: [f32; 4],
    line: [f32; 4],
    mut emit: impl FnMut([f32; 4], [f32; 4]),
) {
    if !width.is_finite() || !height.is_finite() || width < 1.0 || height < 1.0 {
        return;
    }
    let mut quad = |points: [[f32; 2]; 4], colors: [[f32; 4]; 4]| {
        for i in [0, 1, 2, 0, 2, 3] {
            emit(
                [
                    points[i][0].clamp(0.0, width) / width * 2.0 - 1.0,
                    1.0 - points[i][1].clamp(0.0, height) / height * 2.0,
                    0.0,
                    1.0,
                ],
                colors[i],
            );
        }
    };
    let inset = 6.0_f32.min(height * 0.25);
    let top = inset;
    let bottom = height - inset;
    let left = 2.0_f32.min(width * 0.25);
    let right = width - left;
    for division in 0..=4 {
        let y = top + (bottom - top) * (division as f32 * 0.25);
        quad(
            [
                [0.0, y - 0.5],
                [width, y - 0.5],
                [width, y + 0.5],
                [0.0, y + 0.5],
            ],
            [grid; 4],
        );
    }

    let history = &history[history.len().saturating_sub(HISTORY_CAPACITY)..];
    let point = |index: usize, sample: u32| {
        let x = if history.len() <= 1 {
            right
        } else {
            left + (right - left) * (index as f32 / (history.len() - 1) as f32)
        };
        [
            x,
            bottom - (bottom - top) * (sample.min(1_000) as f32 / 1_000.0),
        ]
    };
    if history.len() == 1 {
        let [x, y] = point(0, history[0]);
        quad(
            [
                [x - 1.5, y - 1.5],
                [x + 1.5, y - 1.5],
                [x + 1.5, y + 1.5],
                [x - 1.5, y + 1.5],
            ],
            [line; 4],
        );
        return;
    }

    // Draw the translucent area first, then a solid stroke with a one-pixel
    // alpha fringe. Stroke width stays constant when the window is resized.
    let mut fill = line;
    fill[3] *= 0.12;
    for (index, pair) in history.windows(2).enumerate() {
        let a = point(index, pair[0]);
        let b = point(index + 1, pair[1]);
        quad([a, b, [b[0], bottom], [a[0], bottom]], [fill; 4]);
    }
    let mut transparent = line;
    transparent[3] = 0.0;
    for (index, pair) in history.windows(2).enumerate() {
        let a = point(index, pair[0]);
        let b = point(index + 1, pair[1]);
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let length = dx.hypot(dy).max(f32::MIN_POSITIVE);
        let normal = [-dy / length, dx / length];
        let offset =
            |p: [f32; 2], distance: f32| [p[0] + normal[0] * distance, p[1] + normal[1] * distance];
        quad(
            [
                offset(a, -0.9),
                offset(b, -0.9),
                offset(b, 0.9),
                offset(a, 0.9),
            ],
            [line; 4],
        );
        quad(
            [
                offset(a, -1.9),
                offset(b, -1.9),
                offset(b, -0.9),
                offset(a, -0.9),
            ],
            [transparent, transparent, line, line],
        );
        quad(
            [
                offset(a, 0.9),
                offset(b, 0.9),
                offset(b, 1.9),
                offset(a, 1.9),
            ],
            [line, line, transparent, transparent],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertices(samples: &[u32], width: f32, height: f32) -> Vec<([f32; 4], [f32; 4])> {
        let mut result = Vec::new();
        emit_vertices(
            samples,
            width,
            height,
            [0.5; 4],
            [1.0; 4],
            |position, color| result.push((position, color)),
        );
        result
    }

    #[test]
    fn empty_history_has_grid_and_single_sample_has_a_visible_marker() {
        assert_eq!(vertices(&[], 800.0, 170.0).len(), 30);
        assert_eq!(vertices(&[500], 800.0, 170.0).len(), 36);
    }

    #[test]
    fn clamps_usage_and_keeps_the_latest_sixty_samples() {
        assert_eq!(
            vertices(&[0, u32::MAX], 800.0, 170.0),
            vertices(&[0, 1_000], 800.0, 170.0)
        );
        let samples: Vec<_> = (0..200).collect();
        assert_eq!(
            vertices(&samples, 800.0, 170.0),
            vertices(&samples[140..], 800.0, 170.0)
        );
        assert_eq!(vertices(&samples, 800.0, 170.0).len(), MAX_VERTICES);
    }

    #[test]
    fn resized_graphs_always_have_finite_clipped_triangle_vertices() {
        for width in [1.0, 2.0, 180.0, 800.0, 3_840.0] {
            for height in [1.0, 2.0, 62.0, 170.0, 2_160.0] {
                for samples in [&[][..], &[0][..], &[1_000][..], &[0, u32::MAX, 0, 500][..]] {
                    let mesh = vertices(samples, width, height);
                    assert_eq!(mesh.len() % 3, 0);
                    assert!(mesh.len() <= MAX_VERTICES);
                    for (position, color) in mesh {
                        assert!(position.iter().chain(&color).all(|value| value.is_finite()));
                        assert!((-1.0..=1.0).contains(&position[0]));
                        assert!((-1.0..=1.0).contains(&position[1]));
                        assert_eq!(&position[2..], &[0.0, 1.0]);
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_or_empty_viewports_emit_nothing() {
        for invalid in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(vertices(&[0, 1_000], invalid, 100.0).is_empty());
            assert!(vertices(&[0, 1_000], 100.0, invalid).is_empty());
        }
    }
}
