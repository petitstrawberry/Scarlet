//! Pointer-lock policy and device-independent capture state.

use std::collections::BTreeMap;
use std::vec::Vec;

/// Reason a pointer-lock request cannot be granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerLockDenial {
    /// The requesting connection does not own the target window.
    NotOwned,
    /// The target is ineligible or compositor interaction owns the pointer.
    Denied,
}

/// Active client-owned pointer capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerLockState {
    pub client_id: usize,
    pub window_id: u32,
    last_absolute: Option<(i32, i32)>,
}

/// Pointer ownership fields shared by click focus and lock request handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerInteractionState {
    pub focused_window_id: Option<u32>,
    pub implicit_grab_window_id: Option<u32>,
    pub locked_window_id: Option<u32>,
}

impl PointerInteractionState {
    /// Apply the focus/grab portion of a compositor button-press transition.
    pub fn button_pressed(&mut self, window_id: u32, accepts_focus: bool) {
        self.implicit_grab_window_id = Some(window_id);
        if accepts_focus {
            self.focused_window_id = Some(window_id);
        }
    }

    /// Transfer a focused same-window implicit grab into explicit pointer lock.
    pub fn request_lock(&mut self, window_id: u32) -> bool {
        if self.focused_window_id != Some(window_id)
            || implicit_grab_conflicts(self.implicit_grab_window_id, window_id)
            || self
                .locked_window_id
                .is_some_and(|locked| locked != window_id)
        {
            return false;
        }
        self.implicit_grab_window_id = None;
        self.locked_window_id = Some(window_id);
        true
    }
}

/// Destination for one locked input packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputRoute {
    /// Ordinary SWS client window.
    Window { window_id: u32 },
    /// Window represented by an extension client.
    Extension {
        extension_id: u32,
        external_client_id: u32,
        window_id: u32,
    },
}

/// Correlated response required by a nonzero routed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrelatedReply {
    /// One-way request; only the asynchronous state event is sent.
    None,
    /// Successful or idempotent request response.
    State { request_id: u8, locked: bool },
    /// Rejected request response.
    Error { request_id: u8 },
}

/// Preserve request routing for compositor-mediated pointer-lock outcomes.
pub const fn correlated_reply(
    request_id: u8,
    requested_locked: bool,
    accepted: bool,
) -> CorrelatedReply {
    if request_id == 0 {
        CorrelatedReply::None
    } else if accepted {
        CorrelatedReply::State {
            request_id,
            locked: requested_locked,
        }
    } else {
        CorrelatedReply::Error { request_id }
    }
}

/// Select the queue consumed by the target window's owning connection.
pub const fn input_route(window_id: u32, extension_owner: Option<(u32, u32)>) -> InputRoute {
    match extension_owner {
        Some((extension_id, external_client_id)) => InputRoute::Extension {
            extension_id,
            external_client_id,
            window_id,
        },
        None => InputRoute::Window { window_id },
    }
}

/// Enqueue through the same normal/extension queue boundary used by SWS IPC.
pub fn enqueue_routed_event<T, F>(
    normal: &mut BTreeMap<u32, Vec<T>>,
    extension: &mut BTreeMap<(u32, u32), Vec<T>>,
    route: InputRoute,
    event: T,
    push: F,
) where
    F: FnOnce(&mut Vec<T>, T),
{
    let events = match route {
        InputRoute::Window { window_id } => normal.entry(window_id).or_insert_with(Vec::new),
        InputRoute::Extension {
            extension_id,
            external_client_id,
            ..
        } => extension
            .entry((extension_id, external_client_id))
            .or_insert_with(Vec::new),
    };
    push(events, event);
}

/// Consume queued input for an ordinary window.
pub fn take_window_events<T>(normal: &mut BTreeMap<u32, Vec<T>>, window_id: u32) -> Vec<T> {
    normal
        .get_mut(&window_id)
        .map(core::mem::take)
        .unwrap_or_default()
}

/// Consume queued input for an extension-managed client.
pub fn take_extension_events<T>(
    extension: &mut BTreeMap<(u32, u32), Vec<T>>,
    extension_id: u32,
    external_client_id: u32,
) -> Vec<T> {
    extension
        .get_mut(&(extension_id, external_client_id))
        .map(core::mem::take)
        .unwrap_or_default()
}

/// Whether an implicit button grab belongs to a different window.
pub const fn implicit_grab_conflicts(
    grab_window_id: Option<u32>,
    requested_window_id: u32,
) -> bool {
    match grab_window_id {
        Some(grab_window_id) => grab_window_id != requested_window_id,
        None => false,
    }
}

/// Return the captured window, or `None` for ordinary hit-tested routing.
pub const fn captured_window(state: Option<PointerLockState>) -> Option<u32> {
    match state {
        Some(state) => Some(state.window_id),
        None => None,
    }
}

/// Whether the compositor cursor layer should be rendered.
pub const fn cursor_visible(state: Option<PointerLockState>) -> bool {
    state.is_none()
}

/// State reported for a request result.
///
/// Every denial is confirmed as unlocked so clients can clear in-flight state.
pub fn confirmed_lock_state(
    requested_locked: bool,
    result: &Result<(), PointerLockDenial>,
) -> bool {
    requested_locked && result.is_ok()
}

impl PointerLockState {
    /// Create capture state with absolute-device jump suppression armed.
    pub const fn new(client_id: usize, window_id: u32) -> Self {
        Self {
            client_id,
            window_id,
            last_absolute: None,
        }
    }

    /// Convert an absolute sample to relative motion.
    ///
    /// The first sample establishes the device baseline and returns `None`.
    pub fn absolute_delta(&mut self, x: i32, y: i32) -> Option<(i32, i32)> {
        let delta = self
            .last_absolute
            .map(|(last_x, last_y)| (x.saturating_sub(last_x), y.saturating_sub(last_y)));
        self.last_absolute = Some((x, y));
        delta
    }

    /// Whether a window lifecycle/focus snapshot requires forced release.
    pub fn must_release(
        self,
        owner_client_id: Option<usize>,
        visible: bool,
        minimized: bool,
        focused: bool,
        is_keyboard_focus: bool,
    ) -> bool {
        owner_client_id != Some(self.client_id)
            || !visible
            || minimized
            || !focused
            || !is_keyboard_focus
    }
}

/// Validate a request against connection, presentation, focus, and grab state.
pub fn validate_request(
    owner_client_id: Option<usize>,
    requesting_client_id: usize,
    visible: bool,
    minimized: bool,
    focused: bool,
    is_keyboard_focus: bool,
    compositor_grab_active: bool,
) -> Result<(), PointerLockDenial> {
    if owner_client_id != Some(requesting_client_id) {
        return Err(PointerLockDenial::NotOwned);
    }
    if !visible || minimized || !focused || !is_keyboard_focus || compositor_grab_active {
        return Err(PointerLockDenial::Denied);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_foreign_unfocused_hidden_minimized_and_grabbed_windows() {
        assert_eq!(
            validate_request(Some(4), 9, true, false, true, true, false),
            Err(PointerLockDenial::NotOwned)
        );
        for denied in [
            (false, false, true, true, false),
            (true, true, true, true, false),
            (true, false, false, true, false),
            (true, false, true, false, false),
            (true, false, true, true, true),
        ] {
            assert_eq!(
                validate_request(Some(4), 4, denied.0, denied.1, denied.2, denied.3, denied.4,),
                Err(PointerLockDenial::Denied)
            );
        }
        assert_eq!(
            validate_request(Some(4), 4, true, false, true, true, false),
            Ok(())
        );
    }

    #[test]
    fn absolute_motion_suppresses_first_jump_and_then_reports_delta() {
        let mut state = PointerLockState::new(4, 27);
        assert_eq!(state.absolute_delta(800, 600), None);
        assert_eq!(state.absolute_delta(797, 608), Some((-3, 8)));
    }

    #[test]
    fn focus_visibility_lifetime_changes_force_release() {
        let state = PointerLockState::new(4, 27);
        assert!(!state.must_release(Some(4), true, false, true, true));
        assert!(state.must_release(Some(5), true, false, true, true));
        assert!(state.must_release(Some(4), false, false, true, true));
        assert!(state.must_release(Some(4), true, true, true, true));
        assert!(state.must_release(Some(4), true, false, false, true));
        assert!(state.must_release(Some(4), true, false, true, false));
        assert!(state.must_release(None, true, false, true, true));
    }

    #[test]
    fn capture_hides_cursor_routes_owner_and_denial_confirms_unlocked() {
        let state = PointerLockState::new(4, 27);
        assert!(cursor_visible(None));
        assert_eq!(captured_window(None), None);
        assert!(!cursor_visible(Some(state)));
        assert_eq!(captured_window(Some(state)), Some(27));

        let denied = Err(PointerLockDenial::Denied);
        assert!(!confirmed_lock_state(true, &denied));
        assert!(confirmed_lock_state(true, &Ok(())));
        assert!(!confirmed_lock_state(false, &Ok(())));
    }

    #[test]
    fn locked_input_uses_extension_queue_when_present() {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum Event {
            RelativeX(i32),
            RelativeY(i32),
            Button(bool),
        }

        let mut normal = BTreeMap::new();
        let mut extension = BTreeMap::new();
        let route = input_route(27, Some((8, 19)));
        for event in [
            Event::RelativeX(4),
            Event::RelativeY(-2),
            Event::Button(true),
        ] {
            enqueue_routed_event(&mut normal, &mut extension, route, event, Vec::push);
        }

        assert!(take_window_events(&mut normal, 27).is_empty());
        assert_eq!(
            take_extension_events(&mut extension, 8, 19),
            [
                Event::RelativeX(4),
                Event::RelativeY(-2),
                Event::Button(true),
            ]
        );
        assert!(take_extension_events(&mut extension, 8, 19).is_empty());
    }

    #[test]
    fn same_window_implicit_grab_can_transfer_to_pointer_lock() {
        assert!(!implicit_grab_conflicts(None, 27));
        assert!(!implicit_grab_conflicts(Some(27), 27));
        assert!(implicit_grab_conflicts(Some(28), 27));
    }

    #[test]
    fn button_focus_grab_then_request_transitions_to_locked() {
        let mut state = PointerInteractionState {
            focused_window_id: None,
            implicit_grab_window_id: None,
            locked_window_id: None,
        };
        state.button_pressed(27, true);
        assert_eq!(state.focused_window_id, Some(27));
        assert_eq!(state.implicit_grab_window_id, Some(27));

        assert!(state.request_lock(27));
        assert_eq!(state.focused_window_id, Some(27));
        assert_eq!(state.implicit_grab_window_id, None);
        assert_eq!(state.locked_window_id, Some(27));
    }

    #[test]
    fn nonzero_request_ids_receive_success_duplicate_and_denial_responses() {
        assert_eq!(correlated_reply(0, true, true), CorrelatedReply::None);
        let accepted = CorrelatedReply::State {
            request_id: 7,
            locked: true,
        };
        assert_eq!(correlated_reply(7, true, true), accepted);
        assert_eq!(correlated_reply(7, true, true), accepted); // idempotent duplicate
        assert_eq!(
            correlated_reply(9, true, false),
            CorrelatedReply::Error { request_id: 9 }
        );
    }
}
