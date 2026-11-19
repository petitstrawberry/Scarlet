//! Allowed messages on the two unnamed login compositor connections.
use sws_protocol::{self as protocol, ClientMessageRef};
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LoginPeer {
    Editor,
    Panel,
}
pub fn login_request_allowed(peer: LoginPeer, msg: &ClientMessageRef<'_>, windows: &[u32]) -> bool {
    use ClientMessageRef::*;
    use protocol::input_panel::Request as Panel;
    match msg {
        CreateWindow { window_type, .. } => {
            windows.is_empty()
                && *window_type
                    == match peer {
                        LoginPeer::Editor => protocol::window_types::NORMAL,
                        LoginPeer::Panel => protocol::window_types::INPUT_PANEL,
                    }
        }
        GetCapabilities {} | GetScreenSize {} | GetOutputScale {} | GetInputEnvironment {} => true,
        SetWindowGeometry { window_id, .. }
        | SetWindowSizeLimits { window_id, .. }
        | SetWindowHasAlphaContent { window_id, .. }
        | SetSurfaceRegions { window_id, .. }
        | SetGamepadInput { window_id, .. }
        | SetTouchInput { window_id, .. }
        | SetCursorIcon { window_id, .. }
        | ResizeWindow { window_id, .. }
        | DestroyWindow { window_id }
        | RequestFrame { window_id, .. }
        | RequestFrameCredit { window_id, .. }
        | UpdateBuffer { window_id, .. } => windows.contains(window_id),
        SetFullscreen { window_id } | TextInputCreate { window_id, .. } => {
            peer == LoginPeer::Editor && windows.contains(window_id)
        }
        TextInputEnable { .. }
        | TextInputDestroy { .. }
        | TextInputDisable { .. }
        | TextInputAcceptDismiss { .. }
        | TextInputSetCursorRect { .. }
        | TextInputSetSurroundingText { .. }
        | TextInputSetContentType { .. }
        | TextInputSetTextChangeCause { .. }
        | TextInputCommitState { .. }
        | InputPanel(Panel::Show { .. }) => peer == LoginPeer::Editor,
        InputPanel(Panel::Register { window_id }) => {
            peer == LoginPeer::Panel && windows.contains(window_id)
        }
        InputPanel(Panel::SetVisible { .. } | Panel::Dismiss { .. } | Panel::Key { .. }) => {
            peer == LoginPeer::Panel
        }
        _ => false,
    }
}
