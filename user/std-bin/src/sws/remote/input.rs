//! Conversion from the transport-neutral remote protocol to compositor input.

use sws_remote_protocol::ClientMessage;

use super::super::input::{
    CompositorInputEvent, KeyboardSource, PointerSource, push_pointer_button,
};

/// Convert one virtual-input protocol message into the existing SWS input path.
///
/// # Arguments
///
/// * `client_id` - Remote transport connection that owns injected key state.
/// * `message` - Decoded remote protocol message.
///
/// # Returns
///
/// The corresponding compositor event, or `None` when no direct enqueue is
/// needed. Pointer buttons enter the shared logical-seat aggregator here.
pub(crate) fn compositor_event(
    client_id: usize,
    message: &ClientMessage,
) -> Option<CompositorInputEvent> {
    match message {
        ClientMessage::Key { code, pressed } => Some(CompositorInputEvent::Keyboard {
            code: *code,
            value: if *pressed { 1 } else { 0 },
            source: KeyboardSource::Remote(client_id),
            synthetic: false,
        }),
        ClientMessage::PointerAbsolute { x, y } => {
            Some(CompositorInputEvent::MouseAbsolute { x: *x, y: *y })
        }
        ClientMessage::PointerButton { button, pressed } => {
            push_pointer_button(PointerSource::Remote(client_id), *button, *pressed);
            None
        }
        ClientMessage::PointerScroll { dx, dy } => {
            Some(CompositorInputEvent::MouseWheel { dx: *dx, dy: *dy })
        }
        ClientMessage::CreateCapture { .. }
        | ClientMessage::RegisterBuffer { .. }
        | ClientMessage::RequestFrame { .. } => None,
    }
}
