//! Mutually exclusive shell foregrounds. Retained GPU images never cross depths.
use sws_protocol::workspace::{OVERVIEW_DRAWER_APP_ID, ShellPresentation};

pub(super) fn fixed_depth_visibility(
    app_id: Option<&[u8]>,
    presentation: ShellPresentation,
) -> Option<bool> {
    match app_id {
        Some(b"org.scarlet-os.desktop.shell.home") => Some(presentation == ShellPresentation::Home),
        Some(id) if id == OVERVIEW_DRAWER_APP_ID.as_bytes() => {
            Some(presentation == ShellPresentation::Overview)
        }
        _ => None,
    }
}

pub(super) fn shell_background_must_be_withheld(
    previous: ShellPresentation,
    current: ShellPresentation,
) -> bool {
    // Legacy dynamic shell surfaces need a fresh frame when their depth changes.
    // Fixed-depth foregrounds are handled by visibility, retaining their own pixels.
    previous != current && current != ShellPresentation::Workspace
}

#[cfg(test)]
mod tests {
    use super::*;
    use ShellPresentation::{Home, Overview, Workspace};

    #[test]
    fn catalog_and_overview_handle_are_never_visible_together() {
        let home = Some(b"org.scarlet-os.desktop.shell.home".as_slice());
        let handle = Some(OVERVIEW_DRAWER_APP_ID.as_bytes());
        assert_eq!(fixed_depth_visibility(home, Home), Some(true));
        assert_eq!(fixed_depth_visibility(home, Overview), Some(false));
        assert_eq!(fixed_depth_visibility(home, Workspace), Some(false));
        assert_eq!(fixed_depth_visibility(handle, Home), Some(false));
        assert_eq!(fixed_depth_visibility(handle, Overview), Some(true));
        assert_eq!(fixed_depth_visibility(handle, Workspace), Some(false));
        assert_eq!(
            fixed_depth_visibility(
                Some(b"org.scarlet-os.desktop.shell.home-backdrop"),
                Overview
            ),
            None
        );
        assert_eq!(
            fixed_depth_visibility(
                Some(sws_protocol::workspace::CONSOLE_HOME_APP_ID.as_bytes()),
                Home
            ),
            None
        );
    }

    #[test]
    fn handle_surface_has_no_catalog_pixels_or_hit_area_over_cards() {
        let mut handle = crate::window::Window::new(7, 0, 1128, 1920, 72);
        handle.window_type = crate::window::WindowType::ShellBackground;
        assert!(!handle.contains_presentation_point(960, 600));
        assert!(!handle.contains_presentation_point(960, 1100));
        assert!(handle.contains_presentation_point(960, 1164));
        // A newly shown fixed-depth handle does not need a replacement catalog frame.
        handle.workspace_visible = false;
        assert!(!handle.is_presented());
        handle.workspace_visible = true;
        assert!(handle.is_presented());
    }
}
