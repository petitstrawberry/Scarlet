//! Exercise the compositor policy that lets the hidden launcher prepare pixels.
#[allow(dead_code)]
#[path = "../../../user/std-bin/src/sws/frame_callback.rs"]
mod frame_callback;
#[allow(dead_code)]
#[path = "../../../user/std-bin/src/sws/shell_presentation.rs"]
mod shell_presentation;

use frame_callback::frame_callback_is_ready;
use shell_presentation::{fixed_depth_visibility, prepares_while_workspace_hidden};
use sws_protocol::workspace::{OVERVIEW_DRAWER_APP_ID, ShellPresentation};

#[test]
fn hidden_home_can_submit_catalog_updates_before_its_first_reveal() {
    let home = Some(b"org.scarlet-os.desktop.shell.home".as_slice());
    assert_eq!(
        fixed_depth_visibility(home, ShellPresentation::Workspace),
        Some(false)
    );
    let prepares = prepares_while_workspace_hidden(home, true, true, false);
    assert!(prepares);
    // No output frame advances the counter for this hidden surface. Both its
    // bootstrap and later catalog commits still need a grant; otherwise the
    // first opening pays all layout, glyph rasterization and upload costs.
    assert!(frame_callback_is_ready(false, false, false, 0, 0, prepares));
    assert!(frame_callback_is_ready(false, true, false, 0, 0, prepares));
    assert_eq!(
        fixed_depth_visibility(home, ShellPresentation::Workspace),
        Some(false)
    );
    assert_eq!(
        fixed_depth_visibility(home, ShellPresentation::Home),
        Some(true)
    );
}

#[test]
fn preparation_is_limited_to_unminimized_shell_backgrounds() {
    for id in [
        "org.scarlet-os.desktop.shell.home",
        "org.scarlet-os.desktop.shell.home-backdrop",
        OVERVIEW_DRAWER_APP_ID,
    ] {
        let id = Some(id.as_bytes());
        assert!(prepares_while_workspace_hidden(id, true, true, false));
        assert!(!prepares_while_workspace_hidden(id, false, true, false));
        assert!(!prepares_while_workspace_hidden(id, true, false, false));
        assert!(!prepares_while_workspace_hidden(id, true, true, true));
    }
    for id in [None, Some(b"org.scarlet-os.notepad".as_slice())] {
        let prepares = prepares_while_workspace_hidden(id, true, true, false);
        assert!(!prepares);
        assert!(!frame_callback_is_ready(false, true, false, 3, 2, prepares));
    }
    // Once revealed, Home uses ordinary presentation pacing again.
    assert!(!frame_callback_is_ready(true, true, false, 3, 3, false));
    assert!(frame_callback_is_ready(true, true, false, 4, 3, false));
}
