//! Exercise the mounted production destination, including its keyboard order.
use super::*;
use scarlet_ui::{
    element::RenderElement,
    event::{Event, KeyCode, KeyEvent},
    pipeline::RenderingPipeline,
    views::{ButtonRenderObject, TextRenderObject},
};
use std::time::{Duration, Instant};

static FIXTURE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn key(pipeline: &mut RenderingPipeline, keycode: KeyCode) {
    let modifiers = Default::default();
    for event in [
        KeyEvent::Pressed { keycode, modifiers },
        KeyEvent::Released { keycode, modifiers },
    ] {
        pipeline.handle_event(&Event::Keyboard(event));
    }
}

fn collect(element: &dyn Element, labels: &mut Vec<String>, buttons: &mut Vec<Button>) {
    if let Some(el) = element
        .as_any()
        .downcast_ref::<RenderElement<Text, TextRenderObject>>()
    {
        labels.push(el.view().content().to_owned());
    }
    if let Some(el) = element
        .as_any()
        .downcast_ref::<RenderElement<Button, ButtonRenderObject>>()
    {
        labels.push(el.view().label().to_owned());
        buttons.push(el.view().clone());
    }
    for child in element.children() {
        collect(child.as_ref(), labels, buttons);
    }
}

fn frame(
    pipeline: &mut RenderingPipeline,
    state: &State<settings_bluetooth::Panel>,
) -> (Vec<String>, Vec<Button>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.get().pending.is_some() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    let _ = pipeline.render_for_present().unwrap();
    let mut labels = Vec::new();
    let mut buttons = Vec::new();
    collect(
        pipeline.element_tree().root().unwrap(),
        &mut labels,
        &mut buttons,
    );
    (labels, buttons)
}

#[test]
fn keyboard_discovery_and_addressed_row_controls() {
    let _fixture = FIXTURE.lock().unwrap();
    let app = SettingsApp::fixture(false);
    settings_bluetooth::poll(&app.bluetooth);
    let mut pipeline = RenderingPipeline::new();
    pipeline.set_root(BluetoothPage(app.bluetooth.clone()).create_element());
    pipeline.layout_initial();
    pipeline.resize(Size::new(640.0, 480.0));

    let (labels, buttons) = frame(&mut pipeline, &app.bluetooth);
    assert!(labels.iter().any(|l| l == "Pair new device"));
    assert!(labels.iter().any(|l| l.contains("pairing mode")));
    let button_names: Vec<_> = buttons.iter().map(Button::label).collect();
    assert_eq!(
        button_names,
        [
            "Refresh",
            "Search for devices",
            "Disconnect",
            "Forget device",
            "Connect",
            "Forget device"
        ]
    );
    assert!(labels.windows(5).any(|s| s
        == [
            "Paired devices · Offline",
            "Fixture gamepad",
            "02:00:00:00:00:10",
            "Connect",
            "Forget device"
        ]));

    for k in [KeyCode::Tab, KeyCode::Tab, KeyCode::Enter] {
        key(&mut pipeline, k);
    }
    let (labels, _) = frame(&mut pipeline, &app.bluetooth);
    assert!(
        labels.windows(4).any(|s| s
            == [
                "Discovered devices",
                "Fixture keyboard",
                "02:00:00:00:00:20",
                "Pair"
            ]),
        "{labels:?}"
    );
    for k in [KeyCode::Tab, KeyCode::Enter] {
        key(&mut pipeline, k);
    }
    let (labels, buttons) = frame(&mut pipeline, &app.bluetooth);
    assert!(labels.iter().any(|l| l == "Paired with 02:00:00:00:00:20."));
    let forget = buttons
        .iter()
        .filter(|b| b.label() == "Forget device")
        .nth(1)
        .unwrap();
    forget.invoke_on_click();
    let (labels, buttons) = frame(&mut pipeline, &app.bluetooth);
    assert!(labels.windows(4).any(|s| s[0] == "Fixture keyboard"
        && s[2].starts_with("Forget and disconnect")
        && s[3] == "Cancel"));
    buttons
        .iter()
        .find(|b| b.label() == "Cancel")
        .unwrap()
        .invoke_on_click();
    let (labels, buttons) = frame(&mut pipeline, &app.bluetooth);
    assert!(!labels.iter().any(|l| l == "Confirm forget"));
    buttons
        .iter()
        .filter(|b| b.label() == "Forget device")
        .nth(1)
        .unwrap()
        .invoke_on_click();
    let (_, buttons) = frame(&mut pipeline, &app.bluetooth);
    buttons
        .iter()
        .find(|b| b.label() == "Confirm forget")
        .unwrap()
        .invoke_on_click();
    let (labels, _) = frame(&mut pipeline, &app.bluetooth);
    assert!(!labels.iter().any(|l| l == "Fixture keyboard"));
    assert!(labels.iter().any(|l| l == "Forgot 02:00:00:00:00:20."));
    assert!(
        labels
            .windows(3)
            .any(|s| s == ["Fixture headphones", "02:00:00:00:00:30", "Disconnect"])
    );

    scarlet_bluetooth::initialize(true);
    settings_bluetooth::poll(&app.bluetooth);
    let (labels, _) = frame(&mut pipeline, &app.bluetooth);
    assert!(
        labels
            .iter()
            .any(|l| l.contains("Fixture adapter unavailable"))
    );
    assert!(!labels.iter().any(|l| l == "Disconnect"));
}

#[test]
fn open_destination_tracks_external_reconnect_and_disconnect() {
    use scarlet_ui::event::{MouseButton, MouseEvent};
    let _fixture = FIXTURE.lock().unwrap();
    let app = SettingsApp::fixture(false);
    let mut pipeline = RenderingPipeline::new();
    pipeline.set_root(settings_navigation(app.clone()).create_element());
    pipeline.layout_initial();
    pipeline.resize(Size::new(800.0, 540.0));
    let _ = pipeline.render_for_present().unwrap();
    for event in [
        MouseEvent::ButtonPressed {
            button: MouseButton::Left,
            x: 60,
            y: 180,
            click_count: 1,
        },
        MouseEvent::ButtonReleased {
            button: MouseButton::Left,
            x: 60,
            y: 180,
            click_count: 1,
        },
    ] {
        pipeline.handle_event(&Event::Mouse(event));
    }
    let (labels, _) = frame(&mut pipeline, &app.bluetooth);
    assert!(
        labels.iter().any(|l| l.contains("Wake a paired device")),
        "{labels:?}"
    );
    let gamepad = "02:00:00:00:00:10";
    let broker = scarlet_bluetooth::Client::new(0);
    for online in [true, false] {
        if online {
            broker.connect(gamepad).unwrap();
        } else {
            broker.disconnect(Some(gamepad)).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(8);
        while !app
            .bluetooth
            .get()
            .inventory
            .devices
            .iter()
            .any(|d| d.address == gamepad && d.connected == online)
        {
            assert!(
                Instant::now() < deadline,
                "Open Bluetooth destination missed external change"
            );
            thread::sleep(Duration::from_millis(20));
        }
        let (labels, _) = frame(&mut pipeline, &app.bluetooth);
        let group = if online {
            "Connected devices"
        } else {
            "Paired devices · Offline"
        };
        let action = if online { "Disconnect" } else { "Connect" };
        assert!(
            labels
                .windows(5)
                .any(|s| s == [group, "Fixture gamepad", gamepad, action, "Forget device"]),
            "{labels:?}"
        );
        assert!(
            !labels.iter().any(|l| l == "Fixture keyboard"),
            "Status polling must never start discovery"
        );
    }
}
