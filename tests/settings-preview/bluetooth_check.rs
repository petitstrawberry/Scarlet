//! Exercise the production page worker without opening a window or a native socket.
use crate as scarlet_bluetooth;
use scarlet_ui::{State, StateId};
use std::{
    thread,
    time::{Duration, Instant},
};

#[path = "../../user/bin/src/settings_bluetooth.rs"]
mod settings_bluetooth;
use settings_bluetooth::{Action, Panel};

struct SettingsApp {
    bluetooth: State<Panel>,
    bluetooth_index: State<usize>,
}

fn perform(app: &SettingsApp, action: Action) -> Panel {
    settings_bluetooth::run(app, action);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let panel = app.bluetooth.get();
        if !panel.busy {
            return panel;
        }
        assert!(
            Instant::now() < deadline,
            "Bluetooth preview worker did not finish"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn saved_and_discovered_devices_offer_their_own_actions() {
    scarlet_bluetooth::initialize(false);
    let app = SettingsApp {
        bluetooth: State::new(StateId::new(1), Panel::default()),
        bluetooth_index: State::new(StateId::new(2), usize::MAX),
    };
    let saved = perform(&app, Action::Refresh);
    assert!(saved.loaded && saved.ready);
    assert_eq!(saved.devices.len(), 1);
    assert_eq!(saved.devices[0].label(), "02:00:00:00:00:10 · Paired");
    assert_eq!(app.bluetooth_index.get(), 0);
    let connect = saved.devices[0].action();
    assert_eq!(connect.button(), "Connect");
    let connected = perform(&app, connect);
    let disconnect = connected.devices[0].action();
    assert_eq!(disconnect.button(), "Disconnect");
    assert_eq!(
        perform(&app, disconnect).devices[0].action().button(),
        "Connect"
    );

    let nearby = perform(&app, Action::Scan);
    assert_eq!(nearby.devices.len(), 2);
    let pair = nearby.devices[1].action();
    assert_eq!(pair.button(), "Pair");
    app.bluetooth_index.set(1);
    let paired = perform(&app, pair);
    let selected = &paired.devices[app.bluetooth_index.get()];
    assert_eq!(
        selected.label(),
        "Fixture keyboard · 02:00:00:00:00:20 · Connected"
    );
    assert_eq!(selected.action().button(), "Disconnect");

    scarlet_bluetooth::initialize(true);
    let unavailable = perform(&app, Action::Refresh);
    assert!(!unavailable.ready);
    assert!(unavailable.devices.is_empty());
    assert!(unavailable.feedback.contains("Fixture adapter unavailable"));
}
