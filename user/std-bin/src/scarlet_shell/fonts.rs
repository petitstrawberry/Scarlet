//! Load the distribution UI face before terminal and symbol fallback glyphs.
use std::{io, sync::OnceLock};

pub fn initialize() -> io::Result<()> {
    static LOADED: OnceLock<()> = OnceLock::new();
    if LOADED.get().is_some() {
        return Ok(());
    }
    let primary = std::fs::read("/share/fonts/Mplus1-Regular.ttf")?;
    scarlet_ui::graphics::set_default_font(primary.leak())
        .map_err(|_| io::Error::other("invalid system font"))?;
    if let Ok(fallback) = std::fs::read("/share/fonts/JetBrainsMonoNerdFontMono-Regular.ttf") {
        let _ = scarlet_ui::graphics::add_default_font_fallback(fallback.leak());
    }
    let _ = LOADED.set(());
    Ok(())
}
