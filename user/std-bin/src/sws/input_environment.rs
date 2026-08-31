//! Authoritative SWS input-environment state.

use std::env;
use std::sync::Mutex;

const TABLET_MODE_KNOWN: u32 = sws_protocol::input_environment_known_flags::TABLET_MODE;
const LID_CLOSED_KNOWN: u32 = sws_protocol::input_environment_known_flags::LID_CLOSED;
const TABLET_MODE_STATE: u32 = sws_protocol::input_environment_state_flags::TABLET_MODE;
const LID_CLOSED_STATE: u32 = sws_protocol::input_environment_state_flags::LID_CLOSED;

/// A complete, self-consistent input-environment snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    /// Nonzero version advanced after every effective change.
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
}

static SNAPSHOT: Mutex<Snapshot> = Mutex::new(Snapshot::initial());
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

/// Initialize the authoritative snapshot from `SCARLET_TABLET_MODE`.
///
/// This must run once during startup, before compositor IPC begins accepting
/// clients. Hardware posture events remain free to replace the initial value.
///
/// # Returns
///
/// The initialized full snapshot.
pub fn initialize_from_env() -> Snapshot {
    let tablet_mode = env::var("SCARLET_TABLET_MODE")
        .ok()
        .as_deref()
        .and_then(parse_tablet_mode_override);
    initialize(tablet_mode)
}

fn initialize(tablet_mode: Option<bool>) -> Snapshot {
    let mut snapshot = Snapshot::initial();
    if let Some(tablet_mode) = tablet_mode {
        snapshot.known_flags |= TABLET_MODE_KNOWN;
        if tablet_mode {
            snapshot.state_flags |= TABLET_MODE_STATE;
        }
    }
    *SNAPSHOT
        .lock()
        .expect("SWS input-environment mutex poisoned") = snapshot;
    snapshot
}

/// Read the current full snapshot atomically.
///
/// # Returns
///
/// A copy of the authoritative snapshot.
pub fn snapshot() -> Snapshot {
    *SNAPSHOT
        .lock()
        .expect("SWS input-environment mutex poisoned")
}

fn apply_state_update(
    snapshot: &mut Snapshot,
    known_bit: u32,
    state_bit: u32,
    update: Option<Option<bool>>,
) {
    let Some(update) = update else {
        return;
    };
    let Some(value) = update else {
        snapshot.known_flags &= !known_bit;
        snapshot.state_flags &= !state_bit;
        return;
    };
    snapshot.known_flags |= known_bit;
    if value {
        snapshot.state_flags |= state_bit;
    } else {
        snapshot.state_flags &= !state_bit;
    }
}

/// Apply an optional hardware posture report.
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
/// The new full snapshot when known/state bits changed, otherwise `None`.
pub fn update_posture(
    tablet_mode: Option<Option<bool>>,
    lid_closed: Option<Option<bool>>,
) -> Option<Snapshot> {
    let mut current = SNAPSHOT
        .lock()
        .expect("SWS input-environment mutex poisoned");
    let mut updated = *current;
    apply_state_update(
        &mut updated,
        TABLET_MODE_KNOWN,
        TABLET_MODE_STATE,
        tablet_mode,
    );
    apply_state_update(&mut updated, LID_CLOSED_KNOWN, LID_CLOSED_STATE, lid_closed);
    if updated.known_flags == current.known_flags && updated.state_flags == current.state_flags {
        return None;
    }
    updated.generation = next_generation(current.generation);
    *current = updated;
    Some(updated)
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
    let mut current = SNAPSHOT
        .lock()
        .expect("SWS input-environment mutex poisoned");
    if current.capability_flags == capability_flags {
        return None;
    }
    current.capability_flags = capability_flags;
    current.generation = next_generation(current.generation);
    Some(*current)
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
    fn parser_accepts_documented_values_case_insensitively() {
        for value in ["1", "true", "YES", "On", "tablet", " TABLET "] {
            assert_eq!(parse_tablet_mode_override(value), Some(true));
        }
        for value in ["0", "false", "NO", "Off", "laptop", " LAPTOP "] {
            assert_eq!(parse_tablet_mode_override(value), Some(false));
        }
        assert_eq!(parse_tablet_mode_override(""), None);
        assert_eq!(parse_tablet_mode_override("convertible"), None);
    }

    #[test]
    fn generation_advances_only_for_effective_changes() {
        let _test_guard = TEST_STATE_LOCK
            .lock()
            .expect("input-environment test mutex poisoned");
        let initial = initialize(None);
        assert_eq!(initial.generation, 1);
        assert_eq!(next_generation(u32::MAX), 1);
        assert_eq!(update_posture(None, None), None);

        let tablet =
            update_posture(Some(Some(true)), None).expect("tablet state should become known");
        assert_eq!(tablet.generation, 2);
        assert_eq!(update_posture(Some(Some(true)), None), None);

        let lid = update_posture(None, Some(Some(false))).expect("lid state should become known");
        assert_eq!(lid.generation, 3);
        assert_eq!(update_capabilities(0), None);
        let devices = update_capabilities(0b111).expect("capabilities should change");
        assert_eq!(devices.generation, 4);
    }

    #[test]
    fn explicit_unknown_clears_only_the_selected_field() {
        let _test_guard = TEST_STATE_LOCK
            .lock()
            .expect("input-environment test mutex poisoned");
        let initial = initialize(Some(true));
        assert!(initial.tablet_mode());

        let lid = update_posture(None, Some(Some(true))).expect("lid should become known");
        assert!(lid.tablet_mode());
        assert!(lid.lid_closed());

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
