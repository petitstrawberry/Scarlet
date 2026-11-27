//! The Bluetooth destination in the Settings navigation.
use super::*;
use settings_bluetooth::{Action, Device, Panel, confirm, run};

#[derive(Clone)]
pub(crate) struct BluetoothPage(pub(crate) State<Panel>);

impl View for BluetoothPage {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(scarlet_ui::element::ComponentElement::new_with_builder(
            self.clone(),
            |page| Box::new(page.content()),
        ))
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        vec![&self.0]
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

// A natural-height stack keeps every row and confirmation in the page's single
// scroll and keyboard focus order, including devices with multi-line messages.
#[derive(Clone, Default)]
struct Children(Vec<Box<dyn View>>);

impl Children {
    fn push(&mut self, view: impl View) {
        self.0.push(Box::new(view));
    }
}

impl ViewTuple for Children {
    fn create_elements(&self) -> Vec<Box<dyn Element>> {
        self.0.iter().map(|v| v.create_element()).collect()
    }
    fn clone_views(&self) -> Vec<Box<dyn View>> {
        self.0.clone()
    }
    fn collect_listenables<'a>(&'a self, out: &mut Vec<&'a dyn Listenable>) {
        for child in &self.0 {
            out.extend(child.listenables());
        }
    }
}

impl BluetoothPage {
    fn button(&self, panel: &Panel, action: Action, label: &str) -> Button {
        let state = self.0.clone();
        Button::new(label)
            .enabled(panel.enabled(action))
            .on_click(move || run(&state, action))
    }

    fn row(&self, panel: &Panel, device: &Device) -> impl View + Clone {
        let address = device.address;
        let name = device.name.as_deref().unwrap_or("Unnamed device");
        let identity = vstack! {
            Text::new(name).font_size(15.0).line_limit(Some(2)),
            settings_note(format!("{address}")),
        }
        .spacing(3.0)
        .alignment(Alignment::Leading);
        let mut details = Children::default();
        if panel.confirming == Some(address) {
            details.push(identity);
            details.push(settings_note(if device.connected {
                "Forget and disconnect this device? You’ll need to pair it again to reconnect."
            } else {
                "Forget this device? You’ll need to pair it again to reconnect."
            }));
            let cancel = self.0.clone();
            details.push(
                hstack! {
                    Button::new("Cancel").on_click(move || confirm(&cancel, None)),
                    self.button(panel, Action::Forget(address), "Confirm forget"),
                }
                .spacing(8.0),
            );
        } else {
            let action = device.action();
            let mut controls = Children::default();
            controls.push(
                self.button(panel, action, action.button())
                    .key(action.button()),
            );
            if device.saved {
                let state = self.0.clone();
                controls.push(
                    Button::new("Forget device")
                        .enabled(panel.enabled(Action::Forget(address)))
                        .on_click(move || confirm(&state, Some(address)))
                        .key("forget"),
                );
            }
            details.push(
                hstack! {
                    identity.frame_width(f32::INFINITY).content_alignment(Alignment::Leading),
                    HStack::new(controls).spacing(8.0),
                }
                .spacing(12.0),
            );
        }
        if panel.feedback_address == Some(address) && !panel.feedback.is_empty() {
            details.push(settings_note(&panel.feedback));
        }
        VStack::new(details)
            .spacing(5.0)
            .alignment(Alignment::Leading)
            .key(format!("{address}"))
    }

    fn group(
        &self,
        panel: &Panel,
        belongs: impl Fn(&Device) -> bool,
        empty: &str,
    ) -> VStack<Children> {
        let mut rows = Children(
            panel
                .inventory
                .devices
                .iter()
                .filter(|d| belongs(d))
                .map(|d| self.row(panel, d).clone_view())
                .collect(),
        );
        if rows.0.is_empty() && panel.inventory.ready && !empty.is_empty() {
            rows.push(settings_note(empty));
        }
        VStack::new(rows)
            .spacing(16.0)
            .alignment(Alignment::Leading)
    }

    fn content(&self) -> impl View + Clone {
        let panel = self.0.get();
        let inventory = &panel.inventory;
        let row_has_feedback = panel
            .feedback_address
            .is_some_and(|a| inventory.devices.iter().any(|d| d.address == a));
        let global_feedback = if row_has_feedback {
            String::new()
        } else {
            panel.feedback.clone()
        };
        let empty_discovery = if inventory.scanned {
            "No new devices found."
        } else {
            ""
        };
        settings_page(vstack! {
            with_feedback(hstack! {
                settings_note(if inventory.adapter.is_empty() { "Checking Bluetooth…" } else { &inventory.adapter }),
                self.button(&panel, Action::Refresh, "Refresh"),
            }.spacing(12.0), global_feedback).padding_insets(EdgeInsets::symmetric(0.0, 16.0)),
            settings_section(vstack! {
                settings_note("Put your device in pairing mode before searching."),
                self.button(&panel, Action::Scan, if inventory.scanned { "Search again" } else { "Search for devices" }),
                Text::new("Discovered devices").font_size(14.0),
                self.group(&panel, |d| !d.saved && !d.connected, empty_discovery),
            }.spacing(10.0).alignment(Alignment::Leading), "Pair new device", 16.0),
            settings_section(self.group(&panel, |d| d.connected, "No devices connected."), "Connected devices", 16.0),
            settings_section(self.group(&panel, |d| d.saved && !d.connected, "No offline paired devices."), "Paired devices · Offline", 16.0),
        }.spacing(14.0).alignment(Alignment::Leading))
    }
}
