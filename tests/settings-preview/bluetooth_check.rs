//! Drive the production worker using the same in-memory broker as the preview.
use crate as scarlet_bluetooth;
use scarlet_ui::{State, StateId};
use std::{
    thread,
    time::{Duration, Instant},
};

#[path = "../../user/bin/src/settings_bluetooth.rs"]
mod settings_bluetooth;
use settings_bluetooth::{Action, Panel, confirm, poll, run};

fn wait(state: &State<Panel>) -> Panel {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let panel = state.get();
        if panel.pending.is_none() {
            return panel;
        }
        assert!(Instant::now() < deadline, "Bluetooth worker did not finish");
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn addressed_actions_reconcile_with_the_broker() {
    use Action::*;
    use scarlet_bluetooth::{Client, PEERS, initialize};
    let [gamepad, keyboard, headphones] = PEERS.map(|p| p.address);
    initialize(false);
    let state = State::new(StateId::new(1), Panel::default());
    let perform = |action| {
        run(&state, action);
        wait(&state)
    };
    let broker = Client::new(0);

    let initial = perform(Refresh);
    assert!(initial.inventory.ready);
    assert_eq!(initial.inventory.devices.len(), 2);
    assert_eq!(
        initial.inventory.devices[0].name.as_deref(),
        Some("Fixture gamepad")
    );
    assert!(initial.enabled(Connect(gamepad)));
    assert!(initial.enabled(Disconnect(headphones)));
    assert!(!initial.enabled(Pair(keyboard)));

    // Hold a broker call in flight to deterministically test progress and
    // duplicate/competing activations from controls painted before it began.
    let hold = scarlet_bluetooth::ADAPTER.lock().unwrap();
    run(&state, Connect(gamepad));
    run(&state, Disconnect(headphones));
    run(&state, Scan);
    let working = state.get();
    assert_eq!(working.pending, Some(Connect(gamepad)));
    assert_eq!(working.feedback_address, Some(gamepad));
    assert_eq!(working.feedback, "Connecting…");
    assert!(!working.enabled(Refresh));
    drop(hold);
    assert!(wait(&state).enabled(Disconnect(gamepad)));
    assert_eq!(broker.list().unwrap().len(), 2);

    let scanned = perform(Scan);
    assert_eq!(scanned.inventory.devices.len(), 3);
    assert!(scanned.enabled(Pair(keyboard)));
    assert!(!scanned.enabled(Forget(keyboard)));
    assert!(perform(Pair(keyboard)).enabled(Disconnect(keyboard)));
    assert!(perform(Disconnect(gamepad)).enabled(Connect(gamepad)));
    assert_eq!(broker.list().unwrap().len(), 2);

    // Confirmation is bound to one saved address and invalidated by an
    // external connection change, including a change between polls.
    confirm(&state, Some(gamepad));
    poll(&state);
    assert_eq!(state.get().confirming, Some(gamepad));
    perform(Forget(headphones));
    assert_eq!(broker.saved().unwrap().len(), 3);
    broker.connect(gamepad).unwrap();
    let stale = perform(Forget(gamepad));
    assert!(stale.feedback.contains("Device state changed"));
    assert_eq!(stale.feedback_address, Some(gamepad));
    assert_eq!(broker.saved().unwrap().len(), 3);
    confirm(&state, Some(gamepad));
    broker.disconnect(Some(gamepad)).unwrap();
    poll(&state);
    assert_eq!(state.get().confirming, None);
    assert!(state.get().enabled(Connect(gamepad)));
    confirm(&state, Some(gamepad));
    confirm(&state, None);
    perform(Forget(gamepad));
    assert_eq!(broker.saved().unwrap().len(), 3);
    confirm(&state, Some(gamepad));
    let forgotten = perform(Forget(gamepad));
    assert!(forgotten.feedback.contains(gamepad));
    assert!(
        !forgotten
            .inventory
            .devices
            .iter()
            .any(|d| d.address == gamepad)
    );
    assert_eq!(broker.saved().unwrap().len(), 2);
    assert!(perform(Scan).enabled(Pair(gamepad)));

    // A failed read drops stale scan results and an outstanding confirmation.
    confirm(&state, Some(headphones));
    initialize(true);
    poll(&state);
    let unavailable = state.get();
    assert!(
        unavailable
            .inventory
            .adapter
            .contains("Fixture adapter unavailable")
    );
    assert!(unavailable.inventory.devices.is_empty());
    assert_eq!(unavailable.confirming, None);
    assert!(!unavailable.enabled(Scan));
    assert!(unavailable.enabled(Refresh));
    initialize(false);
    poll(&state);
    assert!(state.get().enabled(Connect(gamepad)));
    assert!(!state.get().enabled(Pair(keyboard)));
}
