//! Opt-in CPU preparation benchmark; does not measure GPU submission or FPS.
use super::*;
use scarlet_ui::Canvas;

fn baseline_draw_usage_history(
    canvas: &mut Canvas<'_>,
    width: u32,
    height: u32,
    history: &[u32],
    background: Color,
    grid: Color,
    line: Color,
) {
    canvas.fill_rect(0, 0, width, height, background);

    let right = width.saturating_sub(1) as i32;
    let bottom = height.saturating_sub(1) as i32;
    for division in 0..=4 {
        let y = bottom.saturating_mul(division) / 4;
        canvas.draw_line(0, y, right, y, grid);
    }

    if history.len() < 2 || width < 2 || height < 2 {
        return;
    }

    let start = history.len().saturating_sub(width as usize);
    let visible = &history[start..];
    let span = visible.len().saturating_sub(1).max(1);
    let graph_bottom = bottom.saturating_sub(6);
    let graph_top = 6;
    let graph_height = graph_bottom.saturating_sub(graph_top).max(1);
    let mut previous = None;

    for (index, sample) in visible.iter().copied().enumerate() {
        let x = (index as u64 * right as u64 / span as u64) as i32;
        let value = sample.min(1_000) as i32;
        let y = graph_bottom - graph_height * value / 1_000;
        if let Some((previous_x, previous_y)) = previous {
            canvas.draw_line(previous_x, previous_y, x, y, line);
        }
        previous = Some((x, y));
    }
}

#[test]
#[ignore = "CPU preparation benchmark; run in release mode with --ignored --nocapture"]
fn compare_software_raster_and_sgfx_mesh_preparation() {
    use std::hint::black_box;
    const ITERATIONS: u32 = 2_000;
    let history: Vec<_> = (0..CPU_HISTORY_CAPACITY)
        .map(|i| (i * 173 % 1_001) as u32)
        .collect();
    for (width, height) in [(180u32, 62u32), (900, 170), (1_800, 600)] {
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            let mut canvas = Canvas::new(&mut pixels, width, height);
            baseline_draw_usage_history(
                &mut canvas,
                width,
                height,
                black_box(&history),
                Color::BLACK,
                Color::gray(0.2),
                Color::WHITE,
            );
            black_box(&pixels);
        }
        let software = start.elapsed();
        let graph = UsageGraph::new(history.clone(), Color::BLACK, Color::WHITE);
        let mut element = graph.create_element();
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            let render = element
                .render_object_mut()
                .unwrap()
                .as_any_mut()
                .downcast_mut::<UsageGraphRenderObject>()
                .unwrap();
            // Rebuild every iteration to model a new sample, including mesh,
            // immutable frame allocation and updating the retained canvas.
            render.dirty = true;
            render.layout(LayoutConstraints::tight(width as f32, height as f32));
            render.flush_layout_notifications();
            black_box(render.revision);
        }
        let sgfx = start.elapsed();
        println!(
            "{width}x{height}: CPU raster {:.2} us/update, SGFX preparation {:.2} us/update; raster {} bytes, mesh <= {} bytes",
            software.as_secs_f64() * 1e6 / ITERATIONS as f64,
            sgfx.as_secs_f64() * 1e6 / ITERATIONS as f64,
            pixels.len(),
            task_manager_graph::MAX_VERTICES * std::mem::size_of::<SgfxCanvasVertex>()
        );
    }
}
