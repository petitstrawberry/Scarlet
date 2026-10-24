extern crate alloc;

#[allow(dead_code)]
#[path = "../../../user/bin/src/terminal/typography.rs"]
mod typography;

use scarlet_ui::{
    Color, FontStack, Point, Size, TextGridBuffer, TextGridCell,
    element::{ElementRenderObject, LayoutConstraints},
    graphics,
    renderer::{CpuPaintRenderer, PaintContext},
    views::TextGridRenderObject,
};
use std::sync::Arc;

fn font() -> FontStack {
    FontStack::new(include_bytes!(
        "../../../bundles/desktop/fs/share/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"
    ))
    .unwrap()
    .with_fallback(include_bytes!(
        "../../../bundles/desktop/fs/share/fonts/Mplus1-Regular.ttf"
    ))
    .unwrap()
}

#[test]
fn terminal_uses_monospaced_advances_and_measured_line_height() {
    let font = font();
    for size in 10..=28 {
        let size = size as f32;
        assert_eq!(
            graphics::measure_text_sized_with_font_stack("iiii", size, &font).0,
            graphics::measure_text_sized_with_font_stack("WWWW", size, &font).0,
        );
        let cell = typography::cell_size(size, Some(&font));
        assert!(cell.height >= graphics::line_height_sized_with_font_stack(size, &font) as f32);
    }
}

#[test]
fn next_cells_do_not_erase_descenders_or_wide_characters() {
    let font = Arc::new(font());
    for size in [10.0, 16.0, 28.0] {
        let cell = typography::cell_size(size, Some(&font));
        let extent = Size::new(14.0 * cell.width, 2.0 * cell.height);
        let mut grid = TextGridBuffer::new(14, 2, TextGridCell::blank(Color::WHITE, Color::BLACK));
        grid.write_text(0, 0, "gjpqyMW日本語", Color::WHITE, Color::BLACK);
        let mut object = TextGridRenderObject::new(
            grid,
            cell.width,
            cell.height,
            size,
            Some((*font).clone()),
            Color::BLACK,
            None,
            Color::WHITE,
        );
        object.layout(LayoutConstraints::tight(extent.width, extent.height));
        let mut grid_paint = PaintContext::new();
        object.paint(&mut grid_paint, Point::ZERO);

        // Draw the same glyphs after all backgrounds. This independent order
        // retains any ink that a too-small following cell would erase.
        let mut reference = PaintContext::new();
        let mut column = 0;
        for ch in "gjpqyMW日本語".chars() {
            reference.draw_text_with_font_stack(
                Point::new(column as f32 * cell.width, 0.0),
                ch.to_string(),
                Color::WHITE,
                size,
                font.clone(),
            );
            column += scarlet_ui::text_grid_cell_width(ch);
        }
        for scale in [1000, 1250, 2000] {
            let mut actual = CpuPaintRenderer::new(extent, scale, Color::BLACK);
            actual.execute(&grid_paint);
            let mut expected = CpuPaintRenderer::new(extent, scale, Color::BLACK);
            expected.execute(&reference);
            let differences: Vec<_> = actual
                .buffer()
                .as_slice()
                .iter()
                .zip(expected.buffer().as_slice())
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .collect();
            if !differences.is_empty() {
                eprintln!(
                    "size={size} scale={scale} cell={cell:?} differing pixels={} first={:?}",
                    differences.len(),
                    differences
                        .iter()
                        .take(8)
                        .map(|(i, (a, b))| (
                            i % actual.buffer().width() as usize,
                            i / actual.buffer().width() as usize,
                            a,
                            b
                        ))
                        .collect::<Vec<_>>()
                );
            }
            assert!(
                differences.is_empty(),
                "terminal glyph overwritten at font size {size}, scale {scale}"
            );
        }
    }
}
