//! Terminal-local fonts and cell metrics, shared by painting and PTY sizing.
use alloc::{boxed::Box, vec::Vec};
use scarlet_ui::{FontStack, Size, graphics};
#[cfg(not(target_os = "scarlet"))]
use std::io::Read;
use std::{fs::File, sync::OnceLock};

pub(super) fn font_stack() -> Option<&'static FontStack> {
    static FONT: OnceLock<Option<FontStack>> = OnceLock::new();
    FONT.get_or_init(|| {
        let primary = read_font("/share/fonts/JetBrainsMonoNerdFontMono-Regular.ttf")?;
        let mut font = FontStack::new(primary).ok()?;
        for path in [
            "/share/fonts/Mplus1-Regular.ttf",
            "/share/fonts/NotoSansCJKjp-Regular.otf",
        ] {
            if let Some(bytes) = read_font(path) {
                let _ = font.add_fallback(bytes);
            }
        }
        Some(font)
    })
    .as_ref()
}

fn read_font(path: &str) -> Option<&'static [u8]> {
    let mut file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let count = file.read(&mut chunk).ok()?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    // Loaded once for the process; FontStack keeps references to these bytes.
    Some(Box::leak(bytes.into_boxed_slice()))
}

pub(super) fn cell_size(font_size: f32, font: Option<&FontStack>) -> Size {
    let measure = |text: &str| match font {
        Some(font) => graphics::measure_text_sized_with_font_stack(text, font_size, font),
        None => graphics::measure_text_sized(text, font_size),
    };
    let line_height = match font {
        Some(font) => graphics::line_height_sized_with_font_stack(font_size, font),
        None => graphics::line_height_sized(font_size),
    };
    // M/W bound Latin advances even if the installed monospace face is absent.
    Size::new(
        measure("M").0.max(measure("W").0).max(1) as f32,
        line_height.max(1) as f32,
    )
}
