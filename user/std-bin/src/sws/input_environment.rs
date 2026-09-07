//! Authoritative system-wide input and tablet-presentation state.

use std::env;
use std::sync::Mutex;
use sws_protocol::WindowingMode;

const TABLET_MODE_KNOWN: u32 = sws_protocol::input_environment_known_flags::TABLET_MODE;
const LID_CLOSED_KNOWN: u32 = sws_protocol::input_environment_known_flags::LID_CLOSED;
const WINDOWING_MODE_KNOWN: u32 = sws_protocol::input_environment_known_flags::WINDOWING_MODE;
const TABLET_OVERRIDE_KNOWN: u32 =
    sws_protocol::input_environment_known_flags::TABLET_MODE_OVERRIDE_ACTIVE;
const WINDOWING_OVERRIDE_KNOWN: u32 =
    sws_protocol::input_environment_known_flags::WINDOWING_MODE_OVERRIDE_ACTIVE;
const TABLET_MODE_STATE: u32 = sws_protocol::input_environment_state_flags::TABLET_MODE;
const LID_CLOSED_STATE: u32 = sws_protocol::input_environment_state_flags::LID_CLOSED;
const FOCUSED_WINDOWING_STATE: u32 = sws_protocol::input_environment_state_flags::FOCUSED_WINDOWING;
const TABLET_OVERRIDE_STATE: u32 =
    sws_protocol::input_environment_state_flags::TABLET_MODE_OVERRIDE_ACTIVE;
const WINDOWING_OVERRIDE_STATE: u32 =
    sws_protocol::input_environment_state_flags::WINDOWING_MODE_OVERRIDE_ACTIVE;

/// A complete, self-consistent input-environment snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    /// Nonzero version advanced after every externally visible change.
    pub generation: u32,
    /// Bits whose corresponding state values are known.
    pub known_flags: u32,
    /// Current boolean state bits.
    pub state_flags: u32,
    /// Currently present input-device capability bits.
    pub capability_flags: u32,
}

impl Snapshot {
    const fn initial() -> Self {
        Self {
            generation: 1,
            known_flags: 0,
            state_flags: 0,
            capability_flags: 0,
        }
    }

    /// Return whether tablet mode is both known and enabled.
    pub const fn tablet_mode(self) -> bool {
        self.known_flags & TABLET_MODE_KNOWN != 0 && self.state_flags & TABLET_MODE_STATE != 0
    }

    /// Return whether the lid state is both known and closed.
    pub const fn lid_closed(self) -> bool {
        self.known_flags & LID_CLOSED_KNOWN != 0 && self.state_flags & LID_CLOSED_STATE != 0
    }

    /// Return the effective system-wide windowing policy.
    pub const fn windowing_mode(self) -> WindowingMode {
        if self.state_flags & FOCUSED_WINDOWING_STATE != 0 {
            WindowingMode::Focused
        } else {
            WindowingMode::Freeform
        }
    }

    /// Return whether tablet posture is currently forced by an override.
    pub const fn tablet_mode_override_active(self) -> bool {
        self.state_flags & TABLET_OVERRIDE_STATE != 0
    }

    /// Return whether windowing policy is currently forced by an override.
    pub const fn windowing_mode_override_active(self) -> bool {
        self.state_flags & WINDOWING_OVERRIDE_STATE != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EnvironmentState {
    snapshot: Snapshot,
    hardware_tablet_mode: Option<bool>,
    hardware_lid_closed: Option<bool>,
    tablet_mode_override: Option<bool>,
    windowing_mode_override: Option<WindowingMode>,
}

impl EnvironmentState {
    const fn initial() -> Self {
        Self {
            snapshot: Snapshot::initial(),
            hardware_tablet_mode: None,
            hardware_lid_closed: None,
            tablet_mode_override: None,
            windowing_mode_override: None,
        }
    }

    fn recompute_effective_flags(&mut self) {
        let mut known_flags =
            WINDOWING_MODE_KNOWN | TABLET_OVERRIDE_KNOWN | WINDOWING_OVERRIDE_KNOWN;
        let mut state_flags = 0;

        let tablet_mode = self.tablet_mode_override.or(self.hardware_tablet_mode);
        if let Some(tablet_mode) = tablet_mode {
            known_flags |= TABLET_MODE_KNOWN;
            if tablet_mode {
                state_flags |= TABLET_MODE_STATE;
            }
        }

        if let Some(lid_closed) = self.hardware_lid_closed {
            known_flags |= LID_CLOSED_KNOWN;
            if lid_closed {
                state_flags |= LID_CLOSED_STATE;
            }
        }

        let windowing_mode = self.windowing_mode_override.unwrap_or_else(|| {
            if tablet_mode == Some(true) {
                WindowingMode::Focused
            } else {
                WindowingMode::Freeform
            }
        });
        if windowing_mode == WindowingMode::Focused {
            state_flags |= FOCUSED_WINDOWING_STATE;
        }
        if self.tablet_mode_override.is_some() {
            state_flags |= TABLET_OVERRIDE_STATE;
        }
        if self.windowing_mode_override.is_some() {
            state_flags |= WINDOWING_OVERRIDE_STATE;
        }

        self.snapshot.known_flags = known_flags;
        self.snapshot.state_flags = state_flags;
    }
}

static STATE: Mutex<EnvironmentState> = Mutex::new(EnvironmentState::initial());
#[cfg(test)]
pub(super) static TEST_STATE_LOCK: Mutex<()> = Mutex::new(());

fn next_generation(generation: u32) -> u32 {
    let generation = generation.wrapping_add(1);
    if generation == 0 { 1 } else { generation }
}

/// Parse a startup tablet-mode override.
///
/// # Arguments
///
/// * `value` - Environment value to parse, without any variable-name prefix.
///
/// # Returns
///
/// `Some(true)` for tablet values, `Some(false)` for laptop values, and
/// `None` for an unrecognized value.
pub fn parse_tablet_mode_override(value: &str) -> Option<bool> {
    let value = value.trim();
    if ["1", "true", "yes", "on", "tablet"]
        .iter()
        .any(|candidate| value.eq_ignore_ascii_case(candidate))
    {
        Some(true)
    } else if ["0", "false", "no", "off", "laptop"]
        .iter()
        .any(|candidate| value.eq_ignore_ascii_case(candidate))
    {
        Some(false)
    } else {
        None
    }
}

/// Parse a startup windowing-mode override.
///
/// # Arguments
///
/// * `value` - Environment value to parse.
///
/// # Returns
///
/// A forced mode for `focused` or `freeform`; `None` keeps posture-derived
/// policy and also covers `auto` and unrecognized values.
pub fn parse_windowing_mode_override(value: &str) -> Option<WindowingMode> {
    let value = value.trim();
    if ["focused", "focus", "tablet"]
        .iter()
        .any(|candidate| value.eq_ignore_ascii_case(candidate))
    {
        Some(WindowingMode::Focused)
    } else if ["freeform", "floating", "desktop"]
        .iter()
        .any(|candidate| value.eq_ignore_ascii_case(candidate))
    {
        Some(WindowingMode::Freeform)
    } else {
        None
    }
}

/// Initialize system state from `SCARLET_TABLET_MODE` and
/// `SCARLET_TABLET_WINDOWING`.
///
/// Startup values are real overrides: later hardware reports continue to be
/// sampled underneath them, and clearing an override immediately reveals the
/// latest hardware-derived state.
///
/// # Returns
///
/// The initialized full snapshot.
pub fn initialize_from_env() -> Snapshot {
    let tablet_mode_override = env::var("SCARLET_TABLET_MODE")
        .ok()
        .as_deref()
        .and_then(parse_tablet_mode_override);
    let windowing_mode_override = env::var("SCARLET_TABLET_WINDOWING")
        .ok()
        .as_deref()
        .and_then(parse_windowing_mode_override);
    initialize(tablet_mode_override, windowing_mode_override)
}

fn initialize(
    tablet_mode_override: Option<bool>,
    windowing_mode_override: Option<WindowingMode>,
) -> Snapshot {
    let mut state = EnvironmentState {
        tablet_mode_override,
        windowing_mode_override,
        ..EnvironmentState::initial()
    };
    state.recompute_effective_flags();
    let snapshot = state.snapshot;
    *STATE.lock().expect("SWS input-environment mutex poisoned") = state;
    snapshot
}

/// Read the current full snapshot atomically.
///
/// # Returns
///
/// A copy of the authoritative snapshot.
pub fn snapshot() -> Snapshot {
    STATE
        .lock()
        .expect("SWS input-environment mutex poisoned")
        .snapshot
}

fn apply_hardware_update(field: &mut Option<bool>, update: Option<Option<bool>>) {
    if let Some(update) = update {
        *field = update;
    }
}

fn finish_update(state: &mut EnvironmentState, previous: Snapshot) -> Option<Snapshot> {
    state.recompute_effective_flags();
    if state.snapshot.known_flags == previous.known_flags
        && state.snapshot.state_flags == previous.state_flags
        && state.snapshot.capability_flags == previous.capability_flags
    {
        return None;
    }
    state.snapshot.generation = next_generation(previous.generation);
    Some(state.snapshot)
}

/// Apply an optional hardware posture report.
///
/// Hardware state is always retained even while an override masks it.
///
/// # Arguments
///
/// * `tablet_mode` - `None` to preserve the field, `Some(None)` to make it
///   unknown, or `Some(Some(value))` to publish a known value.
/// * `lid_closed` - `None` to preserve the field, `Some(None)` to make it
///   unknown, or `Some(Some(value))` to publish a known value.
///
/// # Returns
///
/// The new externally visible snapshot when it changed, otherwise `None`.
pub fn update_posture(
    tablet_mode: Option<Option<bool>>,
    lid_closed: Option<Option<bool>>,
) -> Option<Snapshot> {
    let mut state = STATE.lock().expect("SWS input-environment mutex poisoned");
    let previous = state.snapshot;
    apply_hardware_update(&mut state.hardware_tablet_mode, tablet_mode);
    apply_hardware_update(&mut state.hardware_lid_closed, lid_closed);
    finish_update(&mut state, previous)
}

/// Set or clear the system-wide tablet-mode override.
///
/// # Arguments
///
/// * `tablet_mode` - Forced posture, or `None` to return to hardware detection.
///
/// # Returns
///
/// The new snapshot when the effective state or override metadata changed.
pub fn set_tablet_mode_override(tablet_mode: Option<bool>) -> Option<Snapshot> {
    let mut state = STATE.lock().expect("SWS input-environment mutex poisoned");
    if state.tablet_mode_override == tablet_mode {
        return None;
    }
    let previous = state.snapshot;
    state.tablet_mode_override = tablet_mode;
    finish_update(&mut state, previous)
}

/// Set or clear the system-wide windowing-mode override.
///
/// # Arguments
///
/// * `windowing_mode` - Forced policy, or `None` for posture-derived policy.
///
/// # Returns
///
/// The new snapshot when the effective state or override metadata changed.
pub fn set_windowing_mode_override(windowing_mode: Option<WindowingMode>) -> Option<Snapshot> {
    let mut state = STATE.lock().expect("SWS input-environment mutex poisoned");
    if state.windowing_mode_override == windowing_mode {
        return None;
    }
    let previous = state.snapshot;
    state.windowing_mode_override = windowing_mode;
    finish_update(&mut state, previous)
}

/// Replace the present-device capability bitset.
///
/// # Arguments
///
/// * `capability_flags` - Complete replacement capability bitset.
///
/// # Returns
///
/// The new full snapshot when the bitset changed, otherwise `None`.
pub fn update_capabilities(capability_flags: u32) -> Option<Snapshot> {
    let mut state = STATE.lock().expect("SWS input-environment mutex poisoned");
    if state.snapshot.capability_flags == capability_flags {
        return None;
    }
    state.snapshot.capability_flags = capability_flags;
    state.snapshot.generation = next_generation(state.snapshot.generation);
    Some(state.snapshot)
}

/// Encode a snapshot in SWS protocol field order.
///
/// # Arguments
///
/// * `snapshot` - Full snapshot to encode.
///
/// # Returns
///
/// A fixed-width protocol payload containing four little-endian `u32` values.
pub fn protocol_payload(snapshot: Snapshot) -> [u8; 16] {
    sws_protocol::payload_input_environment_changed(
        snapshot.generation,
        snapshot.known_flags,
        snapshot.state_flags,
        snapshot.capability_flags,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsers_accept_documented_values_case_insensitively() {
        for value in ["1", "true", "YES", "On", "tablet", " TABLET "] {
            assert_eq!(parse_tablet_mode_override(value), Some(true));
        }
        for value in ["0", "false", "NO", "Off", "laptop", " LAPTOP "] {
            assert_eq!(parse_tablet_mode_override(value), Some(false));
        }
        assert_eq!(parse_tablet_mode_override(""), None);
        assert_eq!(parse_tablet_mode_override("convertible"), None);

        assert_eq!(
            parse_windowing_mode_override(" FOCUSED "),
            Some(WindowingMode::Focused)
        );
        assert_eq!(
            parse_windowing_mode_override("floating"),
            Some(WindowingMode::Freeform)
        );
        assert_eq!(parse_windowing_mode_override("auto"), None);
    }

    #[test]
    fn generation_advances_only_for_externally_visible_changes() {
        let _test_guard = TEST_STATE_LOCK
            .lock()
            .expect("input-environment test mutex poisoned");
        let initial = initialize(None, None);
        assert_eq!(initial.generation, 1);
        assert_eq!(initial.windowing_mode(), WindowingMode::Freeform);
        assert_eq!(next_generation(u32::MAX), 1);
        assert_eq!(update_posture(None, None), None);

        let tablet =
            update_posture(Some(Some(true)), None).expect("tablet state should become known");
        assert_eq!(tablet.generation, 2);
        assert_eq!(tablet.windowing_mode(), WindowingMode::Focused);
        assert_eq!(update_posture(Some(Some(true)), None), None);

        let lid = update_posture(None, Some(Some(false))).expect("lid state should become known");
        assert_eq!(lid.generation, 3);
        assert_eq!(update_capabilities(0), None);
        let devices = update_capabilities(0b111).expect("capabilities should change");
        assert_eq!(devices.generation, 4);
    }

    #[test]
    fn tablet_override_masks_but_does_not_discard_hardware_state() {
        let _test_guard = TEST_STATE_LOCK
            .lock()
            .expect("input-environment test mutex poisoned");
        initialize(None, None);
        let laptop = update_posture(Some(Some(false)), None).expect("hardware becomes known");
        assert!(!laptop.tablet_mode());

        let forced = set_tablet_mode_override(Some(true)).expect("override becomes active");
        assert!(forced.tablet_mode());
        assert!(forced.tablet_mode_override_active());
        assert_eq!(forced.windowing_mode(), WindowingMode::Focused);

        assert_eq!(update_posture(Some(Some(true)), None), None);
        assert_eq!(update_posture(Some(Some(false)), None), None);

        let restored = set_tablet_mode_override(None).expect("override metadata changes");
        assert!(!restored.tablet_mode());
        assert!(!restored.tablet_mode_override_active());
        assert_eq!(restored.windowing_mode(), WindowingMode::Freeform);
    }

    #[test]
    fn windowing_override_is_independent_from_posture() {
        let _test_guard = TEST_STATE_LOCK
            .lock()
            .expect("input-environment test mutex poisoned");
        initialize(Some(true), Some(WindowingMode::Freeform));
        let initial = snapshot();
        assert!(initial.tablet_mode());
        assert_eq!(initial.windowing_mode(), WindowingMode::Freeform);
        assert!(initial.windowing_mode_override_active());

        let derived = set_windowing_mode_override(None).expect("override should clear");
        assert_eq!(derived.windowing_mode(), WindowingMode::Focused);
        assert!(!derived.windowing_mode_override_active());
    }

    #[test]
    fn explicit_unknown_clears_only_the_selected_hardware_field() {
        let _test_guard = TEST_STATE_LOCK
            .lock()
            .expect("input-environment test mutex poisoned");
        initialize(None, None);
        let known = update_posture(Some(Some(true)), Some(Some(true)))
            .expect("hardware fields should become known");
        assert!(known.tablet_mode());
        assert!(known.lid_closed());

        let unknown = update_posture(Some(None), None).expect("tablet should become unknown");
        assert_eq!(unknown.known_flags & TABLET_MODE_KNOWN, 0);
        assert_eq!(unknown.state_flags & TABLET_MODE_STATE, 0);
        assert!(unknown.lid_closed());
        assert_eq!(update_posture(Some(None), None), None);
    }

    #[test]
    fn protocol_encoding_uses_snapshot_order() {
        let snapshot = Snapshot {
            generation: 0x0102_0304,
            known_flags: 0x1112_1314,
            state_flags: 0x2122_2324,
            capability_flags: 0x3132_3334,
        };
        assert_eq!(
            protocol_payload(snapshot),
            [4, 3, 2, 1, 20, 19, 18, 17, 36, 35, 34, 33, 52, 51, 50, 49,]
        );
    }
}
