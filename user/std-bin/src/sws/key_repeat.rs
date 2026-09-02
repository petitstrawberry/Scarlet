//! Compositor-owned keyboard repeat policy.

/// Delay before the first compositor-generated key repeat (500 ms).
pub(crate) const KEY_REPEAT_DELAY_NS: u64 = 500_000_000;
/// Interval between compositor-generated key repeats (20 Hz).
pub(crate) const KEY_REPEAT_INTERVAL_NS: u64 = 50_000_000;

/// Return whether an evdev key value represents a new physical press.
pub(crate) const fn is_initial_press(value: i32) -> bool {
    value == 1
}

/// Return whether a raw event is valid for a compositor-repeat-owned source.
pub(crate) const fn is_physical_key_value(value: i32) -> bool {
    value == 0 || value == 1
}

/// Return whether an event may be forwarded to a 0/1-only keyboard protocol.
pub(crate) const fn forward_to_binary_key_protocol(synthetic: bool) -> bool {
    !synthetic
}

/// Return whether an EventDevice read lacks one complete input record.
pub(crate) const fn should_retry_keyboard_read(bytes_read: usize, record_size: usize) -> bool {
    bytes_read != record_size
}

/// Identity of a keyboard event producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyboardSource {
    /// One Scarlet-native `/dev/keyboardN` stream.
    Local(u8),
    /// One authenticated remote-input transport connection.
    Remote(usize),
}

/// Physical keys currently held by each keyboard source.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct HeldKeys {
    keys: std::vec::Vec<(KeyboardSource, u16)>,
}

/// Physical owners of key presses consumed by a compositor-global action.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ConsumedKeys {
    keys: std::vec::Vec<(KeyboardSource, u16)>,
}

/// Tracks a modifier-only tap without stealing modifier chords from clients.
///
/// A tap completes only when the last tracked modifier is released and no
/// other key was pressed while it was held. The compositor still forwards the
/// modifier press and release to the focused client; this state only decides
/// whether a global action should run after the release has been delivered.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ModifierTapState {
    held_modifiers: std::vec::Vec<(KeyboardSource, u16)>,
    clean: bool,
}

impl ModifierTapState {
    /// Observe one physical key transition.
    ///
    /// # Arguments
    ///
    /// * `source` - Keyboard source that produced the transition.
    /// * `code` - Linux input-event key code.
    /// * `value` - Physical key value (`0` release or `1` press).
    /// * `modifier_codes` - Modifier keys that form the tap gesture.
    /// * `blocked_at_start` - Whether another key was already held when the
    ///   first modifier was pressed.
    ///
    /// # Returns
    ///
    /// `true` exactly once when a clean modifier-only tap completes.
    pub(crate) fn observe(
        &mut self,
        source: KeyboardSource,
        code: u16,
        value: i32,
        modifier_codes: &[u16],
        blocked_at_start: bool,
    ) -> bool {
        let is_modifier = modifier_codes.contains(&code);
        match value {
            1 if is_modifier => {
                let key = (source, code);
                if self.held_modifiers.contains(&key) {
                    return false;
                }
                if self.held_modifiers.is_empty() {
                    self.clean = !blocked_at_start;
                } else {
                    // Pressing both modifier keys is a chord, not a tap.
                    self.clean = false;
                }
                self.held_modifiers.push(key);
                false
            }
            1 => {
                if !self.held_modifiers.is_empty() {
                    self.clean = false;
                }
                false
            }
            0 if is_modifier => {
                let key = (source, code);
                if !self.held_modifiers.contains(&key) {
                    return false;
                }
                self.held_modifiers.retain(|held| *held != key);
                if self.held_modifiers.is_empty() {
                    let completed = self.clean;
                    self.clean = false;
                    completed
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Cancel tap state owned by a disconnected keyboard source.
    ///
    /// # Arguments
    ///
    /// * `source` - Keyboard source being disconnected.
    pub(crate) fn cancel_source(&mut self, source: KeyboardSource) {
        let previous_len = self.held_modifiers.len();
        self.held_modifiers
            .retain(|(held_source, _)| *held_source != source);
        if self.held_modifiers.len() != previous_len {
            // A disconnect-generated modifier release must never toggle the
            // shell, even if another source still owns a modifier key.
            self.clean = false;
        }
    }
}

impl ConsumedKeys {
    pub(crate) fn contains_code(&self, code: u16) -> bool {
        self.keys.iter().any(|(_, held_code)| *held_code == code)
    }

    pub(crate) fn press(&mut self, source: KeyboardSource, code: u16) {
        if !self.keys.contains(&(source, code)) {
            self.keys.push((source, code));
        }
    }

    pub(crate) fn release(&mut self, source: KeyboardSource, code: u16) -> bool {
        let consumed = self.keys.contains(&(source, code));
        self.keys.retain(|held| *held != (source, code));
        consumed
    }

    pub(crate) fn update_duplicate(&mut self, source: KeyboardSource, code: u16, value: i32) {
        match value {
            1 => self.press(source, code),
            0 => {
                self.release(source, code);
            }
            _ => {}
        }
    }

    pub(crate) fn drain_source(&mut self, source: KeyboardSource) {
        self.keys.retain(|(held_source, _)| *held_source != source);
    }
}

impl HeldKeys {
    /// Apply one physical press/release event.
    pub(crate) fn update(&mut self, source: KeyboardSource, code: u16, value: i32) -> bool {
        let was_held = self.keys.iter().any(|(_, held_code)| *held_code == code);
        match value {
            1 if !self.keys.contains(&(source, code)) => self.keys.push((source, code)),
            0 => self.keys.retain(|held| *held != (source, code)),
            _ => {}
        }
        let is_held = self.keys.iter().any(|(_, held_code)| *held_code == code);
        was_held != is_held
    }

    /// Return whether any source currently holds one of the supplied keys.
    pub(crate) fn has_any(&self, codes: &[u16]) -> bool {
        self.keys.iter().any(|(_, code)| codes.contains(code))
    }

    /// Return whether any source holds a key outside the supplied set.
    pub(crate) fn has_any_other_than(&self, codes: &[u16]) -> bool {
        self.keys.iter().any(|(_, code)| !codes.contains(code))
    }

    /// Return a remaining owner for a logical key, if any.
    pub(crate) fn source_for_code(&self, code: u16) -> Option<KeyboardSource> {
        self.keys
            .iter()
            .find_map(|(source, held_code)| (*held_code == code).then_some(*source))
    }

    /// Return the keys physically held by one source.
    pub(crate) fn codes_for_source(&self, source: KeyboardSource) -> std::vec::Vec<u16> {
        self.keys
            .iter()
            .filter_map(|(held_source, code)| (*held_source == source).then_some(*code))
            .collect()
    }
}

// Linux input-event key codes that must never auto-repeat.
const KEY_LEFTCTRL: u16 = 0x1d;
const KEY_LEFTSHIFT: u16 = 0x2a;
const KEY_RIGHTSHIFT: u16 = 0x36;
const KEY_LEFTALT: u16 = 0x38;
const KEY_CAPSLOCK: u16 = 0x3a;
const KEY_NUMLOCK: u16 = 0x45;
const KEY_SCROLLLOCK: u16 = 0x46;
const KEY_RIGHTCTRL: u16 = 0x61;
const KEY_RIGHTALT: u16 = 0x64;
const KEY_LEFTMETA: u16 = 0x7d;
const KEY_RIGHTMETA: u16 = 0x7e;
const KEY_FN: u16 = 0x1d0;
const KEY_POWER: u16 = 0x74;

/// State for one compositor-owned keyboard repeat candidate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct KeyRepeatState {
    key: Option<u16>,
    source: Option<KeyboardSource>,
    focus_window_id: Option<u32>,
    next_deadline_ns: Option<u64>,
}

impl KeyRepeatState {
    /// Observe a physical press/release from a compositor-repeat-owned source.
    pub(crate) fn handle_key_event(
        &mut self,
        code: u16,
        value: i32,
        source: KeyboardSource,
        focus_window_id: Option<u32>,
        now_ns: u64,
    ) {
        match value {
            0 if self.key == Some(code) && self.source == Some(source) => self.reset(),
            1 if is_repeatable_key(code) && focus_window_id.is_some() => {
                self.key = Some(code);
                self.source = Some(source);
                self.focus_window_id = focus_window_id;
                self.next_deadline_ns = Some(now_ns.saturating_add(KEY_REPEAT_DELAY_NS));
            }
            // All current SWS keyboard sources explicitly delegate repeat to
            // the compositor. Raw value 2 can neither establish nor transfer
            // ownership; only SWS itself emits repeat events downstream.
            _ => {}
        }
    }

    /// Cancel repeat when keyboard focus no longer matches the press target.
    pub(crate) fn cancel_if_focus_changed(&mut self, focus_window_id: Option<u32>) {
        if self.key.is_some() && self.focus_window_id != focus_window_id {
            self.reset();
        }
    }

    /// Cancel repeat only when it belongs to the specified input source.
    pub(crate) fn cancel_source(&mut self, source: KeyboardSource) {
        if self.source == Some(source) {
            self.reset();
        }
    }

    /// Preserve a repeat deadline while moving ownership to another physical
    /// device that still holds the same logical key.
    pub(crate) fn transfer_source(
        &mut self,
        old_source: KeyboardSource,
        new_source: KeyboardSource,
        code: u16,
    ) {
        if self.source == Some(old_source) && self.key == Some(code) {
            self.source = Some(new_source);
        }
    }

    /// Cancel the candidate when a compositor shortcut consumes its press.
    pub(crate) fn cancel_key(&mut self, source: KeyboardSource, code: u16) {
        if self.source == Some(source) && self.key == Some(code) {
            self.reset();
        }
    }

    /// Return one due repeat and advance its deadline without emitting bursts.
    pub(crate) fn take_due(
        &mut self,
        now_ns: u64,
        focus_window_id: Option<u32>,
    ) -> Option<(KeyboardSource, u16)> {
        self.cancel_if_focus_changed(focus_window_id);
        let deadline = self.next_deadline_ns?;
        if now_ns < deadline {
            return None;
        }
        self.next_deadline_ns = Some(now_ns.saturating_add(KEY_REPEAT_INTERVAL_NS));
        Some((self.source?, self.key?))
    }

    /// Return the next compositor-generated repeat deadline.
    pub(crate) fn next_deadline_ns(&self) -> Option<u64> {
        self.next_deadline_ns
    }

    /// Clear all repeat state.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

fn is_repeatable_key(code: u16) -> bool {
    !matches!(
        code,
        KEY_LEFTCTRL
            | KEY_RIGHTCTRL
            | KEY_LEFTSHIFT
            | KEY_RIGHTSHIFT
            | KEY_LEFTALT
            | KEY_RIGHTALT
            | KEY_LEFTMETA
            | KEY_RIGHTMETA
            | KEY_CAPSLOCK
            | KEY_NUMLOCK
            | KEY_SCROLLLOCK
            | KEY_POWER
            | KEY_FN
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_SPACE: u16 = 0x39;
    const KEY_C: u16 = 0x2e;
    const SUPER_KEYS: [u16; 2] = [KEY_LEFTMETA, KEY_RIGHTMETA];

    #[test]
    fn clean_modifier_tap_completes_on_release() {
        let source = KeyboardSource::Local(0);
        let mut tap = ModifierTapState::default();

        assert!(!tap.observe(source, KEY_LEFTMETA, 1, &SUPER_KEYS, false));
        assert!(tap.observe(source, KEY_LEFTMETA, 0, &SUPER_KEYS, false));
        assert!(!tap.observe(source, KEY_LEFTMETA, 0, &SUPER_KEYS, false));
    }

    #[test]
    fn chord_or_preexisting_key_cancels_modifier_tap() {
        let source = KeyboardSource::Local(0);
        let mut tap = ModifierTapState::default();

        tap.observe(source, KEY_LEFTMETA, 1, &SUPER_KEYS, false);
        tap.observe(source, KEY_SPACE, 1, &SUPER_KEYS, false);
        tap.observe(source, KEY_SPACE, 0, &SUPER_KEYS, false);
        assert!(!tap.observe(source, KEY_LEFTMETA, 0, &SUPER_KEYS, false));

        tap.observe(source, KEY_LEFTMETA, 1, &SUPER_KEYS, true);
        assert!(!tap.observe(source, KEY_LEFTMETA, 0, &SUPER_KEYS, false));
    }

    #[test]
    fn multiple_super_keys_and_disconnect_never_complete_a_tap() {
        let source = KeyboardSource::Remote(7);
        let mut tap = ModifierTapState::default();

        tap.observe(source, KEY_LEFTMETA, 1, &SUPER_KEYS, false);
        tap.observe(source, KEY_RIGHTMETA, 1, &SUPER_KEYS, false);
        assert!(!tap.observe(source, KEY_LEFTMETA, 0, &SUPER_KEYS, false));
        assert!(!tap.observe(source, KEY_RIGHTMETA, 0, &SUPER_KEYS, false));

        tap.observe(source, KEY_LEFTMETA, 1, &SUPER_KEYS, false);
        tap.cancel_source(source);
        assert!(!tap.observe(source, KEY_LEFTMETA, 0, &SUPER_KEYS, false));
    }

    #[test]
    fn repeat_starts_after_delay_and_uses_20_hz_interval() {
        assert_eq!(KEY_REPEAT_DELAY_NS, 500_000_000);
        assert_eq!(KEY_REPEAT_INTERVAL_NS, 50_000_000);
        let mut repeat = KeyRepeatState::default();
        repeat.handle_key_event(KEY_SPACE, 1, KeyboardSource::Local(0), Some(7), 1_000);

        assert_eq!(
            repeat.take_due(1_000 + KEY_REPEAT_DELAY_NS - 1, Some(7)),
            None
        );
        assert_eq!(
            repeat.take_due(1_000 + KEY_REPEAT_DELAY_NS, Some(7)),
            Some((KeyboardSource::Local(0), KEY_SPACE))
        );
        assert_eq!(
            repeat.take_due(
                1_000 + KEY_REPEAT_DELAY_NS + KEY_REPEAT_INTERVAL_NS - 1,
                Some(7)
            ),
            None
        );
        assert_eq!(
            repeat.take_due(
                1_000 + KEY_REPEAT_DELAY_NS + KEY_REPEAT_INTERVAL_NS,
                Some(7)
            ),
            Some((KeyboardSource::Local(0), KEY_SPACE))
        );
    }

    #[test]
    fn release_focus_change_and_reset_cancel_repeat() {
        let due = KEY_REPEAT_DELAY_NS;
        let mut repeat = KeyRepeatState::default();

        repeat.handle_key_event(KEY_SPACE, 1, KeyboardSource::Local(0), Some(7), 0);
        repeat.handle_key_event(KEY_SPACE, 0, KeyboardSource::Local(0), Some(7), 1);
        assert_eq!(repeat.take_due(due, Some(7)), None);

        repeat.handle_key_event(KEY_SPACE, 1, KeyboardSource::Local(0), Some(7), 0);
        repeat.cancel_if_focus_changed(Some(8));
        assert_eq!(repeat.take_due(due, Some(8)), None);

        repeat.handle_key_event(KEY_SPACE, 1, KeyboardSource::Local(0), Some(7), 0);
        repeat.reset();
        assert_eq!(repeat.take_due(due, Some(7)), None);
    }

    #[test]
    fn modifiers_locks_and_unfocused_keys_do_not_repeat() {
        for code in [
            KEY_LEFTCTRL,
            KEY_RIGHTCTRL,
            KEY_LEFTSHIFT,
            KEY_RIGHTSHIFT,
            KEY_LEFTALT,
            KEY_RIGHTALT,
            KEY_LEFTMETA,
            KEY_RIGHTMETA,
            KEY_CAPSLOCK,
            KEY_NUMLOCK,
            KEY_SCROLLLOCK,
            KEY_POWER,
            KEY_FN,
        ] {
            let mut repeat = KeyRepeatState::default();
            repeat.handle_key_event(code, 1, KeyboardSource::Local(0), Some(7), 0);
            assert_eq!(repeat.take_due(KEY_REPEAT_DELAY_NS, Some(7)), None);
        }

        let mut repeat = KeyRepeatState::default();
        repeat.handle_key_event(KEY_SPACE, 1, KeyboardSource::Local(0), None, 0);
        assert_eq!(repeat.take_due(KEY_REPEAT_DELAY_NS, None), None);
    }

    #[test]
    fn volume_keys_repeat_but_power_does_not() {
        for code in [0x72, 0x73] {
            let mut repeat = KeyRepeatState::default();
            repeat.handle_key_event(code, 1, KeyboardSource::Local(0), Some(7), 0);
            assert_eq!(
                repeat.take_due(KEY_REPEAT_DELAY_NS, Some(7)),
                Some((KeyboardSource::Local(0), code))
            );
        }

        let mut repeat = KeyRepeatState::default();
        repeat.handle_key_event(KEY_POWER, 1, KeyboardSource::Local(0), Some(7), 0);
        assert_eq!(repeat.take_due(KEY_REPEAT_DELAY_NS, Some(7)), None);
    }

    #[test]
    fn source_ownership_scopes_release_and_disconnect() {
        let mut repeat = KeyRepeatState::default();
        let owner = KeyboardSource::Remote(7);
        repeat.handle_key_event(KEY_SPACE, 1, owner, Some(9), 0);
        repeat.handle_key_event(KEY_SPACE, 0, KeyboardSource::Remote(8), Some(9), 10);
        repeat.cancel_source(KeyboardSource::Remote(8));
        assert_eq!(
            repeat.take_due(KEY_REPEAT_DELAY_NS, Some(9)),
            Some((owner, KEY_SPACE))
        );
        repeat.cancel_source(owner);
        assert_eq!(repeat, KeyRepeatState::default());
    }

    #[test]
    fn consumed_press_cancels_only_its_repeat_candidate() {
        let mut repeat = KeyRepeatState::default();
        let owner = KeyboardSource::Remote(7);
        repeat.handle_key_event(KEY_SPACE, 1, owner, Some(9), 0);
        repeat.cancel_key(KeyboardSource::Remote(8), KEY_SPACE);
        assert_eq!(
            repeat.take_due(KEY_REPEAT_DELAY_NS, Some(9)),
            Some((owner, KEY_SPACE))
        );

        repeat.cancel_key(owner, KEY_SPACE);
        assert_eq!(repeat.take_due(u64::MAX, Some(9)), None);
    }

    #[test]
    fn held_keys_disconnect_drains_only_owner_for_synthetic_keyups() {
        let mut held = HeldKeys::default();
        held.update(KeyboardSource::Local(0), KEY_SPACE, 1);
        held.update(KeyboardSource::Local(1), KEY_SPACE, 1);
        held.update(KeyboardSource::Remote(7), KEY_SPACE, 1);
        held.update(KeyboardSource::Remote(7), KEY_LEFTSHIFT, 1);

        assert_eq!(
            held.codes_for_source(KeyboardSource::Remote(8)),
            std::vec![]
        );
        assert_eq!(
            held.codes_for_source(KeyboardSource::Remote(7)),
            std::vec![KEY_SPACE, KEY_LEFTSHIFT]
        );
        assert!(!held.update(KeyboardSource::Remote(7), KEY_SPACE, 0));
        assert!(held.update(KeyboardSource::Remote(7), KEY_LEFTSHIFT, 0));
        assert!(held.has_any(&[KEY_SPACE]));
    }

    #[test]
    fn logical_key_transitions_are_or_aggregated_across_keyboards() {
        let mut held = HeldKeys::default();
        assert!(held.update(KeyboardSource::Local(0), KEY_SPACE, 1));
        assert!(!held.update(KeyboardSource::Local(1), KEY_SPACE, 1));
        assert!(!held.update(KeyboardSource::Local(0), KEY_SPACE, 0));
        assert!(held.has_any(&[KEY_SPACE]));
        assert!(held.update(KeyboardSource::Local(1), KEY_SPACE, 0));
        assert!(!held.has_any(&[KEY_SPACE]));
    }

    #[test]
    fn one_keyboard_can_hold_ctrl_and_c_concurrently_in_order() {
        let source = KeyboardSource::Local(0);
        let mut held = HeldKeys::default();
        assert!(held.update(source, KEY_LEFTCTRL, 1));
        assert!(held.update(source, KEY_C, 1));
        assert!(held.has_any(&[KEY_LEFTCTRL]));
        assert!(held.has_any(&[KEY_C]));
        assert!(held.update(source, KEY_C, 0));
        assert!(held.has_any(&[KEY_LEFTCTRL]));
        assert!(held.update(source, KEY_LEFTCTRL, 0));
    }

    #[test]
    fn modifiers_are_or_aggregated_until_the_last_keyboard_releases() {
        let mut held = HeldKeys::default();
        assert!(held.update(KeyboardSource::Local(0), KEY_LEFTCTRL, 1));
        assert!(!held.update(KeyboardSource::Local(1), KEY_LEFTCTRL, 1));
        assert!(!held.update(KeyboardSource::Local(0), KEY_LEFTCTRL, 0));
        assert!(held.has_any(&[KEY_LEFTCTRL]));
        assert!(held.update(KeyboardSource::Local(1), KEY_LEFTCTRL, 0));
    }

    #[test]
    fn held_key_filter_distinguishes_a_modifier_only_press() {
        let source = KeyboardSource::Local(0);
        let mut held = HeldKeys::default();
        held.update(source, KEY_LEFTMETA, 1);
        assert!(!held.has_any_other_than(&SUPER_KEYS));
        held.update(source, KEY_SPACE, 1);
        assert!(held.has_any_other_than(&SUPER_KEYS));
    }

    #[test]
    fn super_space_consumption_tracks_all_physical_press_and_release_owners() {
        let first = KeyboardSource::Local(0);
        let second = KeyboardSource::Remote(7);
        let mut consumed = ConsumedKeys::default();
        consumed.press(first, KEY_SPACE);
        assert!(consumed.contains_code(KEY_SPACE));
        consumed.update_duplicate(second, KEY_SPACE, 1);
        assert!(consumed.release(first, KEY_SPACE));
        assert!(consumed.contains_code(KEY_SPACE));
        assert!(consumed.release(second, KEY_SPACE));
        assert!(!consumed.contains_code(KEY_SPACE));
    }

    #[test]
    fn one_repeat_candidate_does_not_discard_other_held_keys() {
        let source = KeyboardSource::Local(0);
        let mut held = HeldKeys::default();
        let mut repeat = KeyRepeatState::default();
        held.update(source, KEY_LEFTCTRL, 1);
        held.update(source, KEY_C, 1);
        repeat.handle_key_event(KEY_C, 1, source, Some(3), 0);
        assert!(held.has_any(&[KEY_LEFTCTRL, KEY_C]));
        assert_eq!(
            repeat.take_due(KEY_REPEAT_DELAY_NS, Some(3)),
            Some((source, KEY_C))
        );
        assert!(held.has_any(&[KEY_LEFTCTRL]));
    }

    #[test]
    fn repeat_owner_can_transfer_without_resetting_deadline() {
        let mut repeat = KeyRepeatState::default();
        repeat.handle_key_event(KEY_SPACE, 1, KeyboardSource::Local(0), Some(7), 10);
        repeat.transfer_source(
            KeyboardSource::Local(0),
            KeyboardSource::Local(1),
            KEY_SPACE,
        );
        assert_eq!(
            repeat.take_due(10 + KEY_REPEAT_DELAY_NS, Some(7)),
            Some((KeyboardSource::Local(1), KEY_SPACE))
        );
    }

    #[test]
    fn zero_and_partial_reads_are_retryable_not_disconnects() {
        assert!(should_retry_keyboard_read(0, 16));
        assert!(should_retry_keyboard_read(1, 16));
        assert!(should_retry_keyboard_read(15, 16));
        assert!(!should_retry_keyboard_read(16, 16));
    }

    #[test]
    fn raw_repeat_cannot_establish_or_transfer_ownership_after_focus_change() {
        let mut repeat = KeyRepeatState::default();
        repeat.handle_key_event(KEY_SPACE, 2, KeyboardSource::Local(0), Some(7), 0);
        assert_eq!(repeat.take_due(KEY_REPEAT_DELAY_NS, Some(7)), None);

        repeat.handle_key_event(KEY_SPACE, 1, KeyboardSource::Local(0), Some(7), 0);
        repeat.cancel_if_focus_changed(Some(8));
        repeat.handle_key_event(KEY_SPACE, 2, KeyboardSource::Local(0), Some(8), 1);
        assert_eq!(repeat.take_due(KEY_REPEAT_DELAY_NS, Some(8)), None);
    }

    #[test]
    fn explicit_repeat_ownership_rejects_raw_value_two() {
        assert!(is_physical_key_value(0));
        assert!(is_physical_key_value(1));
        assert!(!is_physical_key_value(2));
    }

    #[test]
    fn synthetic_repeat_is_not_forwarded_as_duplicate_binary_press() {
        assert!(forward_to_binary_key_protocol(false));
        assert!(!forward_to_binary_key_protocol(true));
    }

    #[test]
    fn repeat_value_does_not_retrigger_press_only_shortcuts() {
        assert!(is_initial_press(1));
        assert!(!is_initial_press(2));
        assert!(!is_initial_press(0));
    }
}
