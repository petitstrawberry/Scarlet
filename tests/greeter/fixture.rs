//! CPU-only snapshots of the production View tree; no authentication is run.
#[path = "../../user/std-bin/src/greeter_ui.rs"]
mod greeter_ui;
#[path = "../../user/std-bin/src/scarlet_shell/wallpaper.rs"]
mod wallpaper;
use scarlet_ui::{RenderingPipeline, prelude::*};
use std::io::Write;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let font = std::fs::read(&args[1]).unwrap();
    scarlet_ui::graphics::set_default_font(font.leak()).unwrap();
    let width = args[2].parse().unwrap();
    let height = args[3].parse().unwrap();
    let mut state = [0; 36];
    state[0] = 1;
    state[2] = 8;
    state[3] = 5;
    state[4..9].copy_from_slice(b"alice");
    if let Ok(font) = std::fs::read(
        std::path::Path::new(&args[1]).with_file_name("JetBrainsMonoNerdFontMono-Regular.ttf"),
    ) {
        let _ = scarlet_ui::graphics::add_default_font_fallback(font.leak());
    }
    if args.get(5).is_some_and(|arg| arg == "picker") {
        state[0] = 4;
        state[2] = 0;
    } else if args.get(5).is_some_and(|arg| arg == "failed") {
        state[1] = 1;
        state[2] = 0;
    }
    let screen = greeter_ui::Screen {
        width,
        height,
        display: state,
        covered: args.get(6).and_then(|n| n.parse().ok()).unwrap_or(0.),
    };
    let mut pipeline = RenderingPipeline::new();
    let value = State::new(scarlet_ui::generate_state_id(), screen.value());
    pipeline.set_root(
        screen
            .window(
                Default::default(),
                wallpaper::load_background_state().login_bitmap(width, height),
                value,
                true,
                State::new(scarlet_ui::generate_state_id(), "09:41".to_owned()),
            )
            .create_element(),
    );
    pipeline.layout_initial();
    let frame = pipeline.render().expect("CPU frame");
    let mut output = std::fs::File::create(&args[4]).unwrap();
    write!(output, "P6\n{width} {height}\n255\n").unwrap();
    for pixel in frame.data().chunks_exact(4) {
        output.write_all(&[pixel[2], pixel[1], pixel[0]]).unwrap();
    }
}
