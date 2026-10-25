extern crate alloc;

#[path = "../../../user/bin/src/terminal/viewport.rs"]
mod viewport;

use scarlet_ui::{
    Color, Event, Point, Rect, Size, State, StateId, TextGrid, TextGridBuffer, TextGridCell, View,
    pipeline::RenderingPipeline,
};
use viewport::TerminalViewport;

fn mounted(caret: Rect) -> (RenderingPipeline, State<f32>, TextGrid) {
    let pan = State::new(StateId::new(700), 0.0);
    let grid = State::new(
        StateId::new(701),
        TextGridBuffer::new(48, 30, TextGridCell::blank(Color::WHITE, Color::BLACK)),
    );
    let content = TextGrid::new(grid).cell_size(10.0, 20.0);
    let mut pipeline = RenderingPipeline::new();
    pipeline.set_root(
        TerminalViewport {
            content: content.clone(),
            caret: Some(caret),
            pan: pan.clone(),
        }
        .create_element(),
    );
    pipeline.layout_initial();
    pipeline.resize(Size::new(480.0, 600.0));
    (pipeline, pan, content)
}

fn frame(pipeline: &mut RenderingPipeline, occlusion: Option<Rect>) {
    pipeline.handle_event(&Event::InputPanelOcclusionChanged { occlusion });
    pipeline.request_redraw();
    pipeline.render_for_present().unwrap();
    let root = pipeline.element_tree().root().unwrap();
    assert_eq!(root.bounds().size, Size::new(480.0, 600.0));
    assert_eq!(root.children()[0].bounds().size, Size::new(480.0, 600.0));
}

#[test]
fn keyboard_reveals_pty_caret_without_resizing_and_restores_on_hide() {
    let (mut pipeline, pan, _) = mounted(Rect::from_xywh(20.0, 580.0, 10.0, 20.0));
    let keyboard = Some(Rect::from_xywh(0.0, 360.0, 480.0, 240.0));
    frame(&mut pipeline, keyboard);
    assert_eq!(pan.get(), 240.0);
    assert_eq!(
        pipeline.element_tree().root().unwrap().children()[0].position(),
        Point::new(0.0, -240.0)
    );
    for _ in 0..3 {
        frame(&mut pipeline, keyboard);
        assert_eq!(pan.get(), 240.0);
    }
    frame(&mut pipeline, None);
    assert_eq!(pan.get(), 0.0);
    assert_eq!(
        pipeline.element_tree().root().unwrap().children()[0].position(),
        Point::ZERO
    );
}

#[test]
fn output_cursor_and_floating_panel_changes_recompute_absolute_pan() {
    let (mut pipeline, pan, content) = mounted(Rect::from_xywh(20.0, 580.0, 10.0, 20.0));
    let keyboard = Some(Rect::from_xywh(0.0, 360.0, 480.0, 240.0));
    frame(&mut pipeline, keyboard);
    // PTY output can move the caret without a UI key event or focus change.
    pipeline
        .element_tree_mut()
        .root_mut()
        .unwrap()
        .update(&TerminalViewport {
            content,
            caret: Some(Rect::from_xywh(20.0, 380.0, 10.0, 20.0)),
            pan: pan.clone(),
        });
    frame(&mut pipeline, keyboard);
    assert_eq!(pan.get(), 40.0);
    frame(
        &mut pipeline,
        Some(Rect::from_xywh(200.0, 360.0, 280.0, 240.0)),
    );
    assert_eq!(
        pan.get(),
        0.0,
        "a floating panel beside the caret does not obscure it"
    );
}
