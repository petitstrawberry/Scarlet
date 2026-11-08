//! Frame permission at presentation boundaries, including scene bootstrap.

/// A scene awaiting membership needs one frame to establish its parent role.
/// It may draw that frame while compositor-hidden. Fixed-depth shell surfaces
/// also prepare while hidden, without depending on an output presentation that
/// does not include them. Ordinary apps retain visible-frame pacing.
pub(super) const fn frame_callback_is_ready(
    is_presented: bool,
    has_submitted_frame: bool,
    awaiting_scene_registration: bool,
    presentation_counter: u64,
    requested_after_present: u64,
    prepares_while_hidden: bool,
) -> bool {
    prepares_while_hidden
        || (!has_submitted_frame && awaiting_scene_registration)
        || (is_presented
            && (!has_submitted_frame || presentation_counter > requested_after_present))
}

pub(super) const fn frame_callback_target(
    presentation_counter: u64,
    last_submission_counter: Option<u64>,
) -> u64 {
    match last_submission_counter {
        Some(counter) => counter,
        None => presentation_counter,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_callbacks_wait_for_visibility_and_a_new_presentation_boundary() {
        assert!(frame_callback_is_ready(true, false, false, 0, 0, false));
        assert!(!frame_callback_is_ready(false, false, false, 1, 0, false));
        assert!(!frame_callback_is_ready(true, true, false, 4, 4, false));
        assert!(frame_callback_is_ready(true, true, false, 5, 4, false));
    }

    #[test]
    fn pending_scene_gets_a_bootstrap_frame_without_becoming_visible() {
        // A client needs this grant to submit the first frame that finalizes
        // membership. Waiting for workspace visibility would deadlock it.
        assert!(frame_callback_is_ready(false, false, true, 4, 4, false));
        // The exception ends after submission, even before the pending marker
        // is cleared. Hidden/minimized apps must not receive a rendering loop.
        assert!(!frame_callback_is_ready(false, true, true, 5, 4, false));
        assert!(!frame_callback_is_ready(true, true, true, 4, 4, false));
        assert!(frame_callback_is_ready(true, true, false, 5, 4, false));
    }

    #[test]
    fn late_frame_request_targets_the_commit_that_already_presented() {
        let target = frame_callback_target(5, Some(4));
        assert_eq!(target, 4);
        assert!(frame_callback_is_ready(true, true, false, 5, target, false));
        let pending_target = frame_callback_target(5, Some(5));
        assert_eq!(pending_target, 5);
        assert!(!frame_callback_is_ready(
            true,
            true,
            false,
            5,
            pending_target,
            false,
        ));
    }
}
