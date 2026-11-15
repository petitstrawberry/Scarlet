//! Frame permission at presentation boundaries, including scene bootstrap.

/// Next-encode permission uses visibility, not a previous commit's retirement.
pub(super) const fn frame_credit_is_eligible(
    is_presented: bool,
    has_submitted_frame: bool,
    awaiting_scene_registration: bool,
    prepares_while_hidden: bool,
) -> bool {
    prepares_while_hidden || is_presented || (!has_submitted_frame && awaiting_scene_registration)
}

/// At most one credit per nominal display tick, without accumulating a backlog
/// after a late GPU frame or hidden window. This is not a hardware-vblank ack.
pub(super) const fn frame_credit_deadline(
    last_tick: Option<u64>,
    now_ns: u64,
    interval_ns: u64,
) -> Option<u64> {
    if interval_ns == 0 {
        return None;
    }
    match last_tick {
        Some(last) if last >= now_ns / interval_ns => match last.checked_add(1) {
            Some(next) => next.checked_mul(interval_ns),
            None => None,
        },
        _ => Some(now_ns),
    }
}

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
    fn credits_overlap_unfinished_gpu_frames_without_acknowledging_them() {
        let mut last_tick = None;
        let mut queued = 0;
        // No commit or presentation advances at all while the CPU prepares
        // four distinct frames. Credits never certify any buffer's retirement.
        for now in 0..64 {
            if queued < 4 && frame_credit_deadline(last_tick, now, 16).is_some_and(|d| d <= now) {
                queued += 1;
                last_tick = Some(now / 16);
            }
            assert_eq!(queued, 1 + now / 16);
            assert!(!frame_callback_is_ready(true, true, false, 7, 7, false));
        }
        assert_eq!(queued, 4);
    }

    #[test]
    fn repeated_requests_and_late_frames_do_not_accumulate_credits() {
        assert_eq!(frame_credit_deadline(None, 10, 16), Some(10));
        for now in 10..16 {
            assert_eq!(frame_credit_deadline(Some(0), now, 16), Some(16));
        }
        assert_eq!(frame_credit_deadline(Some(0), 16, 16), Some(16));
        // Resume after a long pause: one current credit, never a backlog burst.
        assert_eq!(frame_credit_deadline(Some(0), 1000, 16), Some(1000));
        assert_eq!(frame_credit_deadline(Some(1000 / 16), 1000, 16), Some(1008));
        assert_eq!(frame_credit_deadline(Some(u64::MAX), u64::MAX, 1), None);
        assert_eq!(frame_credit_deadline(None, 0, 0), None);
    }

    #[test]
    fn credits_respect_visibility_and_bootstrap_but_not_prior_commit() {
        assert!(frame_credit_is_eligible(true, true, false, false));
        assert!(frame_credit_is_eligible(true, false, false, false));
        assert!(!frame_credit_is_eligible(false, true, false, false));
        assert!(frame_credit_is_eligible(false, false, true, false));
        assert!(!frame_credit_is_eligible(false, true, true, false));
        assert!(frame_credit_is_eligible(false, true, false, true));
    }

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
