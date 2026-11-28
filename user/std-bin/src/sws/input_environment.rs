//! Physical input state and the applied Tablet experience for each output.

use std::collections::BTreeMap;
use std::sync::Mutex;
use sws_protocol::{
    input_environment_known_flags as known, input_environment_state_flags as flags,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub generation: u32,
    pub known_flags: u32,
    pub state_flags: u32,
    pub capability_flags: u32,
}

impl Snapshot {
    pub fn hardware_tablet_mode(self) -> bool {
        self.state_flags & flags::HARDWARE_TABLET_MODE != 0
    }
}

/// Published only after the compositor has installed the workspace projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputPolicy {
    pub id: u32,
    pub tablet: bool,
}

struct EnvironmentState {
    generation: u32,
    hardware_tablet: Option<bool>,
    lid_closed: Option<bool>,
    capabilities: u32,
    // Primary first, including after hotplug.
    outputs: Vec<OutputPolicy>,
    clients: BTreeMap<usize, u32>,
}

impl EnvironmentState {
    const fn new() -> Self {
        Self {
            generation: 1,
            hardware_tablet: None,
            lid_closed: None,
            capabilities: 0,
            outputs: Vec::new(),
            clients: BTreeMap::new(),
        }
    }

    fn snapshot(&self, output: Option<u32>) -> Snapshot {
        let policy = self
            .outputs
            .iter()
            .find(|p| Some(p.id) == output)
            .or(self.outputs.first());
        let mut known_flags = known::TABLET_MODE;
        let mut state_flags = 0;
        if policy.is_some_and(|p| p.tablet) {
            state_flags |= flags::TABLET_MODE;
        }
        if let Some(tablet) = self.hardware_tablet {
            known_flags |= known::HARDWARE_TABLET_MODE;
            if tablet {
                state_flags |= flags::HARDWARE_TABLET_MODE;
            }
        }
        if let Some(closed) = self.lid_closed {
            known_flags |= known::LID_CLOSED;
            if closed {
                state_flags |= flags::LID_CLOSED;
            }
        }
        Snapshot {
            generation: self.generation,
            known_flags,
            state_flags,
            capability_flags: self.capabilities,
        }
    }

    fn advance(&mut self) {
        self.generation = self.generation.wrapping_add(1).max(1);
    }

    fn set_client_output(&mut self, client: usize, output: u32) -> Option<Snapshot> {
        let previous = self.clients.insert(client, output);
        if self.snapshot(previous) == self.snapshot(Some(output)) {
            return None;
        }
        self.advance();
        Some(self.snapshot(Some(output)))
    }
}

static STATE: Mutex<EnvironmentState> = Mutex::new(EnvironmentState::new());
#[cfg(test)]
pub(super) static TEST_STATE_LOCK: Mutex<()> = Mutex::new(());

pub fn snapshot() -> Snapshot {
    STATE.lock().expect("input environment").snapshot(None)
}

pub fn client_snapshot(client: usize) -> Snapshot {
    let state = STATE.lock().expect("input environment");
    state.snapshot(state.clients.get(&client).copied())
}

pub fn forget_client(client: usize) {
    STATE
        .lock()
        .expect("input environment")
        .clients
        .remove(&client);
}

pub fn set_client_output(client: usize, output: u32) -> Option<Snapshot> {
    STATE
        .lock()
        .expect("input environment")
        .set_client_output(client, output)
}

pub fn update_outputs(
    outputs: Vec<OutputPolicy>,
    tablet: Option<Option<bool>>,
    lid: Option<Option<bool>>,
) -> bool {
    let mut state = STATE.lock().expect("input environment");
    let tablet = tablet.unwrap_or(state.hardware_tablet);
    let lid = lid.unwrap_or(state.lid_closed);
    if state.outputs == outputs && state.hardware_tablet == tablet && state.lid_closed == lid {
        return false;
    }
    state.outputs = outputs;
    state.hardware_tablet = tablet;
    state.lid_closed = lid;
    state.advance();
    true
}

pub fn update_capabilities(flags: u32) -> bool {
    let mut state = STATE.lock().expect("input environment");
    if state.capabilities == flags {
        return false;
    }
    state.capabilities = flags;
    state.advance();
    true
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
    fn tablet_projection_preserves_physical_posture_lid_and_capabilities() {
        let mut state = EnvironmentState::new();
        state.hardware_tablet = Some(true);
        state.lid_closed = Some(false);
        state.capabilities = sws_protocol::input_environment_capability_flags::DIRECT_TOUCH;
        state.outputs = vec![
            OutputPolicy {
                id: 0,
                tablet: false,
            },
            OutputPolicy {
                id: 8,
                tablet: true,
            },
        ];
        let primary = state.snapshot(None);
        let external = state.set_client_output(7, 8).unwrap();
        assert_eq!(external.generation, primary.generation + 1);
        assert_eq!(state.set_client_output(7, 8), None);
        assert_eq!(primary.state_flags, flags::HARDWARE_TABLET_MODE);
        assert_eq!(
            external.state_flags,
            flags::HARDWARE_TABLET_MODE | flags::TABLET_MODE
        );
        for view in [primary, external] {
            assert_eq!(
                view.known_flags,
                known::TABLET_MODE | known::HARDWARE_TABLET_MODE | known::LID_CLOSED
            );
            assert_eq!(view.capability_flags, state.capabilities);
        }
        // Generation 2, known bits 0/1/5, applied Tablet and folded, direct touch.
        assert_eq!(
            protocol_payload(external),
            [2, 0, 0, 0, 0x23, 0, 0, 0, 0x21, 0, 0, 0, 1, 0, 0, 0]
        );
        state.outputs.pop();
        assert_eq!(state.snapshot(Some(8)), state.snapshot(None));
    }

    #[test]
    fn sensor_and_device_updates_remain_visible_under_an_explicit_experience() {
        let _guard = TEST_STATE_LOCK.lock().unwrap();
        let previous = std::mem::replace(&mut *STATE.lock().unwrap(), EnvironmentState::new());
        let outputs = vec![OutputPolicy {
            id: 0,
            tablet: true,
        }];
        assert!(update_outputs(
            outputs.clone(),
            Some(Some(false)),
            Some(Some(true))
        ));
        let laptop = snapshot();
        assert_ne!(laptop.state_flags & flags::TABLET_MODE, 0);
        assert!(!laptop.hardware_tablet_mode());
        assert!(update_outputs(outputs.clone(), Some(Some(true)), None));
        assert!(snapshot().hardware_tablet_mode());
        assert!(!update_outputs(outputs.clone(), Some(Some(true)), None));
        assert!(update_capabilities(7));
        assert!(!update_capabilities(7));
        assert!(update_outputs(outputs, Some(None), None));
        let unknown = snapshot();
        assert_eq!(unknown.generation, laptop.generation + 3);
        assert_eq!(unknown.known_flags & known::HARDWARE_TABLET_MODE, 0);
        assert_ne!(unknown.state_flags & flags::LID_CLOSED, 0);
        assert_ne!(unknown.state_flags & flags::TABLET_MODE, 0);
        assert_eq!(unknown.capability_flags, 7);
        *STATE.lock().unwrap() = previous;
    }
}
