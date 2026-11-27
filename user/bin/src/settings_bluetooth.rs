//! The settings page talks to the Bluetooth service on a worker thread.
use super::*;
use alloc::collections::BTreeMap;
use scarlet_bluetooth::{Address, Client, ConnectedPeer, DiscoveredPeer};

#[derive(Clone, Copy)]
pub(super) enum Action {
    Refresh,
    Scan,
    Pair(Address),
    Connect(Address),
    Disconnect(Address),
}

impl Action {
    pub(super) fn button(self) -> &'static str {
        match self {
            Self::Pair(_) => "Pair",
            Self::Connect(_) => "Connect",
            Self::Disconnect(_) => "Disconnect",
            Self::Refresh => "Refresh",
            Self::Scan => "Find devices",
        }
    }

    fn progress(self) -> &'static str {
        match self {
            Self::Refresh => "Checking Bluetooth…",
            Self::Scan => "Looking for nearby devices…",
            Self::Pair(_) => "Pairing…",
            Self::Connect(_) => "Connecting…",
            Self::Disconnect(_) => "Disconnecting…",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Device {
    address: Address,
    name: Option<String>,
    bonded: bool,
    connected: bool,
}

impl Device {
    pub(super) fn action(&self) -> Action {
        if self.connected {
            Action::Disconnect(self.address)
        } else if self.bonded {
            Action::Connect(self.address)
        } else {
            Action::Pair(self.address)
        }
    }

    pub(super) fn label(&self) -> String {
        let state = if self.connected {
            "Connected"
        } else if self.bonded {
            "Paired"
        } else {
            "Nearby"
        };
        match &self.name {
            Some(name) => format!("{name} · {} · {state}", self.address),
            None => format!("{} · {state}", self.address),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Panel {
    pub loaded: bool,
    pub ready: bool,
    pub busy: bool,
    pub adapter: String,
    pub feedback: String,
    pub devices: Vec<Device>,
    identity: Option<Address>,
    discovered: Vec<DiscoveredPeer>,
}

fn devices(
    nearby: &[DiscoveredPeer],
    bonded: Vec<Address>,
    connected: Vec<ConnectedPeer>,
) -> Vec<Device> {
    let mut rows = BTreeMap::new();
    let mut merge = |address, name: Option<&str>, paired, online| {
        let row = rows.entry(address).or_insert(Device {
            address,
            name: None,
            bonded: false,
            connected: false,
        });
        if let Some(name) = name {
            let cleaned: String = name.chars().filter(|c| !c.is_control()).collect();
            if !cleaned.trim().is_empty() {
                row.name = Some(cleaned);
            }
        }
        row.bonded |= paired;
        row.connected |= online;
    };
    for peer in nearby {
        merge(peer.address, peer.name.as_deref(), false, false);
    }
    for address in bonded {
        merge(address, None, true, false);
    }
    for peer in connected {
        merge(peer.address, peer.name.as_deref(), false, true);
    }
    let mut rows: Vec<_> = rows.into_values().collect();
    rows.sort_by(|a, b| {
        (!a.connected, !a.bonded, &a.name, a.address).cmp(&(
            !b.connected,
            !b.bonded,
            &b.name,
            b.address,
        ))
    });
    rows
}

impl Panel {
    fn refresh(&mut self, client: &Client) -> scarlet_bluetooth::Result<()> {
        let status = client.status()?;
        if self.identity != Some(status.adapter) {
            self.discovered.clear();
        }
        self.identity = Some(status.adapter);
        self.adapter = format!(
            "{} · {}",
            status.adapter,
            if status.ready { "Ready" } else { "Not ready" }
        );
        self.ready = status.ready && !status.busy;
        self.devices = devices(&self.discovered, client.bonded()?, client.list()?);
        Ok(())
    }

    fn perform(&mut self, client: &Client, action: Action) -> scarlet_bluetooth::Result<String> {
        match action {
            Action::Refresh => Ok(String::new()),
            Action::Scan => {
                self.discovered = client.scan()?;
                Ok(if self.discovered.is_empty() {
                    String::from("No nearby devices found.")
                } else {
                    String::new()
                })
            }
            Action::Pair(peer) => client.pair(peer).map(|_| format!("Paired with {peer}.")),
            Action::Connect(peer) => client
                .connect(peer)
                .map(|_| format!("Connected to {peer}.")),
            Action::Disconnect(peer) => client
                .disconnect(Some(peer))
                .map(|_| format!("Disconnected from {peer}.")),
        }
    }
}

pub(super) fn run(app: &SettingsApp, action: Action) {
    let mut panel = app.bluetooth.get();
    if panel.busy {
        return;
    }
    let selected = panel
        .devices
        .get(app.bluetooth_index.get())
        .map(|p| p.address);
    panel.loaded = true;
    panel.busy = true;
    panel.feedback = String::from(action.progress());
    app.bluetooth.set(panel.clone());
    let state = app.bluetooth.clone();
    let index = app.bluetooth_index.clone();
    thread::spawn(move || {
        let client = Client::new(0);
        panel.feedback = match panel.perform(&client, action) {
            Ok(message) | Err(message) => message,
        };
        if let Err(error) = panel.refresh(&client) {
            panel.ready = false;
            panel.adapter = String::from("Bluetooth unavailable");
            panel.devices.clear();
            if !panel.feedback.is_empty() {
                panel.feedback.push('\n');
            }
            panel.feedback.push_str(&error);
        }
        let next_index = selected
            .and_then(|address| {
                panel
                    .devices
                    .iter()
                    .position(|peer| peer.address == address)
            })
            .or_else(|| (panel.devices.len() == 1).then_some(0))
            .unwrap_or(usize::MAX);
        index.set(next_index);
        panel.busy = false;
        state.set(panel);
    });
}
