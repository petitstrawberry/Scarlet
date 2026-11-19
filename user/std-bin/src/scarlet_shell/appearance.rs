//! Shared Scarlet shell surface and control styles.
use scarlet_ui::prelude::*;
pub const CARD: Color = Color::rgba_f32(0.22, 0.22, 0.22, 0.72);
pub const FROST: Color = Color::rgba_f32(28.0 / 255.0, 28.0 / 255.0, 32.0 / 255.0, 0.48);
pub const TEXT: Color = Color::WHITE;
pub const MUTED: Color = Color::rgba_f32(235.0 / 255.0, 235.0 / 255.0, 245.0 / 255.0, 0.6);
pub const ACCENT: Color = Color::rgba_f32(0.88, 0.25, 0.25, 1.0);
pub const LINE: Color = Color::rgba_f32(1.0, 1.0, 1.0, 0.10);
pub const RADIUS: f32 = 20.0;
pub const ACTION_HEIGHT: f32 = 44.0;

pub const FLOATING_ACTIVE: Color = Color::rgba_f32(56.0 / 255.0, 56.0 / 255.0, 59.0 / 255.0, 0.58);

/// Shell action buttons center their intrinsic label inside the entire touch target.
pub fn action_surface(
    button: Button,
    size: Size,
    fill: Color,
    outline: Color,
    selected: bool,
) -> impl View + Clone {
    Surface::floating(
        ZStack::new((button,))
            .alignment(Alignment::Center)
            .frame(size.width, size.height),
    )
    .elevation(ElevationRole::Flat)
    .fill(fill)
    .bordered(false)
    .corner_radius(10.0)
    .border_rounded(outline, if selected { 2.0 } else { 1.0 }, 10.0)
}
