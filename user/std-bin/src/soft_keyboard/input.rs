//! Held-key lifetime is independent of the render tree. No UI or OS dependency.
use sws_protocol::input_panel::Context;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Contact {
    pub seat: u32,
    pub id: u64,
    pub serial: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stroke {
    pub context: Context,
    pub code: u16,
    pub modifiers: u32,
}
struct Press {
    contact: Contact,
    origin: (i32, i32),
    stroke: Option<Stroke>,
    next_ns: u64,
}
#[derive(Default)]
pub(super) struct HeldKeys(Vec<Press>);
impl HeldKeys {
    pub fn begin(&mut self, contact: Contact, x: i32, y: i32) {
        self.end(contact.seat, contact.id);
        self.0.push(Press {
            contact,
            origin: (x, y),
            stroke: None,
            next_ns: 0,
        });
    }
    pub fn end(&mut self, seat: u32, id: u64) {
        self.0
            .retain(|press| (press.contact.seat, press.contact.id) != (seat, id));
    }
    pub fn motion(&mut self, seat: u32, id: u64, x: i32, y: i32) {
        // Match the existing touch arena's cancellation slop. A cancelled
        // contact never starts repeating again by moving back over the key.
        self.0.retain(|press| {
            let dx = i64::from(x) - i64::from(press.origin.0);
            let dy = i64::from(y) - i64::from(press.origin.1);
            (press.contact.seat, press.contact.id) != (seat, id)
                || dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy)) <= 64
        });
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    pub fn arm(&mut self, contact: Option<Contact>, stroke: Stroke, now_ns: u64) {
        // IME switching is a toggle; holding it must not flip repeatedly.
        if stroke.code == 85 {
            return;
        }
        if let Some(press) = self
            .0
            .iter_mut()
            .find(|press| Some(press.contact) == contact)
        {
            press.stroke = Some(stroke);
            press.next_ns = now_ns.saturating_add(420_000_000);
        }
    }
    pub fn active(&self) -> bool {
        self.0.iter().any(|press| press.stroke.is_some())
    }
    /// Drain releases/cancels before calling this. Never catch up in a burst.
    pub fn due(&mut self, now_ns: u64) -> Vec<Stroke> {
        let mut due = Vec::new();
        for press in &mut self.0 {
            if let Some(stroke) = press.stroke
                && now_ns >= press.next_ns
            {
                due.push(stroke);
                press.next_ns = now_ns.saturating_add(55_000_000);
            }
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CONTACT: Contact = Contact {
        seat: 1,
        id: 2,
        serial: 3,
    };
    fn stroke() -> Stroke {
        Stroke {
            context: Context {
                context_id: 1,
                window_id: 7,
                generation: 3,
                ..Context::default()
            },
            code: 16,
            modifiers: 1,
        }
    }
    #[test]
    fn repeat_survives_view_rebuild_and_preserves_shift_for_the_held_stroke() {
        let mut held = HeldKeys::default();
        held.begin(CONTACT, 100, 100);
        held.arm(Some(CONTACT), stroke(), 10);
        // Rendering may replace all ElementIds here; no input state is reset.
        assert!(held.due(420_000_009).is_empty());
        assert_eq!(held.due(420_000_010), vec![stroke()]);
        assert!(held.due(420_000_011).is_empty());
        assert_eq!(held.due(4_000_000_000), vec![stroke()]);
        assert!(held.due(4_000_000_001).is_empty());
    }
    #[test]
    fn release_in_same_batch_as_down_cannot_arm_a_stuck_repeat() {
        let mut held = HeldKeys::default();
        held.begin(CONTACT, 100, 100);
        held.end(CONTACT.seat, CONTACT.id);
        held.arm(Some(CONTACT), stroke(), 0);
        assert!(!held.active());
        assert!(held.due(u64::MAX).is_empty());
    }
    #[test]
    fn release_or_cancel_is_processed_before_expired_repeat() {
        let mut held = HeldKeys::default();
        held.begin(CONTACT, 100, 100);
        held.arm(Some(CONTACT), stroke(), 0);
        held.end(CONTACT.seat, CONTACT.id);
        assert!(held.due(1_000_000_000).is_empty());
    }
    #[test]
    fn movement_cancels_repeat_without_retargeting_or_resuming() {
        let mut held = HeldKeys::default();
        held.begin(CONTACT, 100, 100);
        held.arm(Some(CONTACT), stroke(), 0);
        held.motion(CONTACT.seat, CONTACT.id, 105, 100);
        assert!(held.active());
        held.motion(CONTACT.seat, CONTACT.id, 109, 100);
        held.motion(CONTACT.seat, CONTACT.id, 100, 100);
        assert!(!held.active());
    }
    #[test]
    fn context_hide_resize_and_layer_reset_drop_all_held_keys() {
        let mut held = HeldKeys::default();
        held.begin(CONTACT, 100, 100);
        held.arm(Some(CONTACT), stroke(), 0);
        held.clear();
        assert!(held.due(u64::MAX).is_empty());
    }
    #[test]
    fn reused_contact_ids_do_not_inherit_old_pending_actions() {
        let mut held = HeldKeys::default();
        held.begin(CONTACT, 100, 100);
        let replacement = Contact {
            serial: 4,
            ..CONTACT
        };
        held.begin(replacement, 100, 100);
        held.arm(Some(CONTACT), stroke(), 0);
        assert!(!held.active());
        held.arm(Some(replacement), stroke(), 0);
        assert!(held.active());
    }
    #[test]
    fn ime_toggle_and_mouse_release_do_not_arm_touch_repeat() {
        let mut held = HeldKeys::default();
        held.begin(CONTACT, 100, 100);
        held.arm(
            Some(CONTACT),
            Stroke {
                code: 85,
                ..stroke()
            },
            0,
        );
        held.arm(None, stroke(), 0);
        assert!(!held.active());
    }
}
