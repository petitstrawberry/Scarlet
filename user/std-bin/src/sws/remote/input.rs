//! Conversion from the transport-neutral remote protocol to compositor input.

use sws_remote_protocol::ClientMessage;

use super::super::input::CompositorInputEvent;

/// Convert one virtual-input protocol message into the existing SWS input path.
///
/// # Arguments
///
/// * `message` - Decoded remote protocol message.
///
/// # Returns
///
/// The corresponding compositor event, or `None` for capture-control messages.
pub(crate) fn compositor_event(message: &ClientMessage) -> Option<CompositorInputEvent> {
    match message {
        ClientMessage::Key { code, pressed } => Some(CompositorInputEvent::Keyboard {
            code: *code,
            pressed: *pressed,
        }),
        ClientMessage::PointerAbsolute { x, y } => {
            Some(CompositorInputEvent::MouseAbsolute { x: *x, y: *y })
        }
        ClientMessage::PointerButton { button, pressed } => {
            Some(CompositorInputEvent::MouseButton {
                button: *button,
                pressed: *pressed,
            })
        }
        ClientMessage::PointerScroll { dx, dy } => {
            Some(CompositorInputEvent::MouseWheel { dx: *dx, dy: *dy })
        }
        ClientMessage::CreateCapture { .. }
        | ClientMessage::RegisterBuffer { .. }
        | ClientMessage::RequestFrame { .. } => None,
    }
}
