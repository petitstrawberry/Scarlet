//! Addressed Bluetooth operations and read-only updates for the Settings page.
use super::*;
use alloc::collections::BTreeMap;
use scarlet_bluetooth::{Address, Client, DiscoveredPeer};

// Keep the three reads of an inventory and each command + readback together.
// Only worker threads take this lock. An idle poll never disables the controls.
static BROKER: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Refresh,
    Scan,
    Pair(Address),
    Connect(Address),
    Disconnect(Address),
    Forget(Address),
}

impl Action {
    pub(super) fn address(self) -> Option<Address> {
        match self {
            Self::Pair(a) | Self::Connect(a) | Self::Disconnect(a) | Self::Forget(a) => Some(a),
            Self::Refresh | Self::Scan => None,
        }
    }

    pub(super) fn button(self) -> &'static str {
        match self {
            Self::Pair(_) => "Pair",
            Self::Connect(_) => "Connect",
            Self::Disconnect(_) => "Disconnect",
            Self::Forget(_) => "Forget device",
            Self::Refresh => "Refresh",
            Self::Scan => "Search for devices",
        }
    }

    fn progress(self) -> &'static str {
        match self {
            Self::Refresh => "Refreshing Bluetooth…",
            Self::Scan => "Searching for nearby devices…",
            Self::Pair(_) => "Pairing…",
            Self::Connect(_) => "Connecting…",
            Self::Disconnect(_) => "Disconnecting…",
            Self::Forget(_) => "Forgetting device…",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Device {
    pub address: Address,
    pub name: Option<String>,
    pub saved: bool,
    pub connected: bool,
}

impl Device {
    pub(super) fn action(&self) -> Action {
        if self.connected {
            Action::Disconnect(self.address)
        } else if self.saved {
            Action::Connect(self.address)
        } else {
            Action::Pair(self.address)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Inventory {
    pub adapter: String,
    pub ready: bool,
    pub devices: Vec<Device>,
    pub scanned: bool,
    identity: Option<Address>,
}

impl Inventory {
    fn read(
        &self,
        client: &Client,
        scan: Option<Vec<DiscoveredPeer>>,
    ) -> scarlet_bluetooth::Result<Self> {
        let status = client.status()?;
        let saved = client.saved()?;
        let connected = client.list()?;
        let same_adapter = self.identity == Some(status.adapter);
        let scanned = same_adapter && status.ready && (scan.is_some() || self.scanned);
        let mut rows = BTreeMap::new();
        let mut merge = |address, name: Option<&str>, saved, connected| {
            let row = rows.entry(address).or_insert(Device {
                address,
                name: None,
                saved: false,
                connected: false,
            });
            if let Some(name) = name {
                let clean: String = name.chars().filter(|c| !c.is_control()).collect();
                if !clean.trim().is_empty() {
                    row.name = Some(clean.trim().into());
                }
            }
            row.saved |= saved;
            row.connected |= connected;
        };
        if scanned {
            if let Some(nearby) = scan {
                for peer in nearby {
                    merge(peer.address, peer.name.as_deref(), false, false);
                }
            } else {
                for row in self.devices.iter().filter(|d| !d.saved && !d.connected) {
                    merge(row.address, row.name.as_deref(), false, false);
                }
            }
        }
        for peer in saved {
            merge(peer.address, peer.name.as_deref(), true, false);
        }
        for peer in connected {
            merge(peer.address, peer.name.as_deref(), false, true);
        }
        let mut devices: Vec<_> = rows.into_values().collect();
        devices.sort_by(|a, b| (&a.name, a.address).cmp(&(&b.name, b.address)));
        let availability = if !status.ready {
            "Not ready"
        } else if status.busy {
            "Busy"
        } else {
            "Ready"
        };
        Ok(Self {
            identity: Some(status.adapter),
            adapter: format!("Adapter {} · {availability}", status.adapter),
            ready: status.ready && !status.busy,
            devices,
            scanned,
        })
    }

    fn refreshed(&self, client: &Client, scan: Option<Vec<DiscoveredPeer>>) -> Self {
        self.read(client, scan).unwrap_or_else(|error| Self {
            adapter: format!("Bluetooth unavailable · {error}"),
            ..Self::default()
        })
    }

    fn allows(&self, action: Action) -> bool {
        if action == Action::Refresh {
            return true;
        }
        if !self.ready {
            return false;
        }
        match action {
            Action::Scan => true,
            Action::Forget(a) => self.device(a).is_some_and(|row| row.saved),
            _ => action
                .address()
                .and_then(|a| self.device(a))
                .is_some_and(|row| row.action() == action),
        }
    }

    fn device(&self, address: Address) -> Option<&Device> {
        self.devices.iter().find(|row| row.address == address)
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Panel {
    pub inventory: Inventory,
    pub pending: Option<Action>,
    pub confirming: Option<Address>,
    pub feedback: String,
    pub feedback_address: Option<Address>,
}

impl Panel {
    pub(super) fn enabled(&self, action: Action) -> bool {
        self.pending.is_none() && self.inventory.allows(action)
    }

    fn adopt(&mut self, next: Inventory) {
        if next.identity != self.inventory.identity
            || !next.ready
            || self
                .confirming
                .is_some_and(|a| self.inventory.device(a) != next.device(a))
        {
            self.confirming = None;
        }
        if next.identity != self.inventory.identity
            || self
                .feedback_address
                .is_some_and(|a| self.inventory.device(a) != next.device(a))
        {
            self.feedback.clear();
            self.feedback_address = None;
        }
        self.inventory = next;
    }
}

pub(super) fn confirm(state: &State<Panel>, address: Option<Address>) {
    state.update(|p| {
        if address.is_none() || address.is_some_and(|a| p.enabled(Action::Forget(a))) {
            p.confirming = address;
        }
    });
}

fn perform(
    client: &Client,
    action: Action,
    inventory: &Inventory,
    scan: &mut Option<Vec<DiscoveredPeer>>,
) -> scarlet_bluetooth::Result<String> {
    // A row can change between painting and activation (or while waiting for a
    // poll). Recheck its adapter and the action before sending a mutation.
    if let Some(address) = action.address() {
        let fresh = inventory.read(client, None)?;
        if fresh.identity != inventory.identity
            || !fresh.allows(action)
            || matches!(action, Action::Forget(_))
                && fresh.device(address) != inventory.device(address)
        {
            return Err("Device state changed. Check its current actions and try again.".into());
        }
    }
    match action {
        Action::Refresh => Ok(String::new()),
        Action::Scan => client.scan().map(|peers| {
            *scan = Some(peers);
            String::new()
        }),
        Action::Pair(a) => client.pair(a).map(|_| format!("Paired with {a}.")),
        Action::Connect(a) => client.connect(a).map(|_| format!("Connected to {a}.")),
        Action::Disconnect(a) => client.disconnect(Some(a)).map(|_| {
            if inventory.device(a).is_some_and(|d| d.saved) {
                format!("Disconnected from {a}. Device is still paired.")
            } else {
                format!("Disconnected from {a}.")
            }
        }),
        Action::Forget(a) => client.forget(a).map(|_| format!("Forgot {a}.")),
    }
}

pub(super) fn run(state: &State<Panel>, action: Action) {
    let mut inventory = None;
    state.update(|p| {
        if p.enabled(action) && !matches!(action, Action::Forget(a) if p.confirming != Some(a)) {
            p.pending = Some(action);
            p.confirming = None;
            p.feedback = action.progress().into();
            p.feedback_address = action.address();
            inventory = Some(p.inventory.clone());
        }
    });
    let Some(mut inventory) = inventory else {
        return;
    };
    let state = state.clone();
    thread::spawn(move || {
        let _guard = BROKER.lock();
        let client = Client::new(0);
        let mut scan = None;
        let result = perform(&client, action, &inventory, &mut scan);
        if action == Action::Scan && result.is_err() {
            inventory.scanned = false;
        }
        let next = inventory.refreshed(&client, scan);
        state.update(|p| {
            p.adopt(next);
            p.pending = None;
            p.feedback_address = action.address();
            p.feedback = result.unwrap_or_else(|error| match action.address() {
                Some(a) => format!("{} ({a}) failed · {error}", action.button()),
                None => format!("{} failed · {error}", action.button()),
            });
        });
    });
}

// Refresh live and saved peers while the page is open, without starting inquiry
// or replacing a user's confirmation, focus or feedback on unchanged polls.
pub(super) fn poll(state: &State<Panel>) {
    let _guard = BROKER.lock();
    let current = state.get();
    if current.pending.is_some() {
        return;
    }
    let next = current.inventory.refreshed(&Client::new(0), None);
    if next != current.inventory {
        state.update(|p| {
            if p.pending.is_none() {
                p.adopt(next);
            }
        });
    }
}
