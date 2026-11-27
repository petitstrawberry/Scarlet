//! In-memory broker for the production Bluetooth presenter in the Winit preview.
#[cfg(test)]
extern crate alloc;
#[cfg(test)]
#[path = "bluetooth_check.rs"]
mod tests;

use std::sync::Mutex;

pub type Address = &'static str;
pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug)]
pub struct DiscoveredPeer {
    pub address: Address,
    pub name: Option<String>,
}
pub type ConnectedPeer = DiscoveredPeer;

pub struct Status {
    pub adapter: Address,
    pub ready: bool,
    pub busy: bool,
}

#[derive(Clone, Copy)]
struct Peer {
    address: Address,
    name: &'static str,
    bonded: bool,
    connected: bool,
}

impl Peer {
    fn row(&self) -> DiscoveredPeer {
        DiscoveredPeer {
            address: self.address,
            name: Some(self.name.into()),
        }
    }
}

const PEERS: [Peer; 3] = [
    Peer {
        address: "02:00:00:00:00:10",
        name: "Fixture gamepad",
        bonded: true,
        connected: false,
    },
    Peer {
        address: "02:00:00:00:00:20",
        name: "Fixture keyboard",
        bonded: false,
        connected: false,
    },
    Peer {
        address: "02:00:00:00:00:30",
        name: "Fixture headphones",
        bonded: true,
        connected: true,
    },
];

static ADAPTER: Mutex<Option<[Peer; 3]>> = Mutex::new(None);

pub fn initialize(empty: bool) {
    *ADAPTER.lock().unwrap() = (!empty).then_some(PEERS);
}

pub struct Client(u32);

impl Client {
    pub fn new(adapter: u32) -> Self {
        Self(adapter)
    }

    fn with_peers<T>(&self, f: impl FnOnce(&mut [Peer; 3]) -> Result<T>) -> Result<T> {
        let mut adapter = ADAPTER.lock().unwrap();
        let peers = adapter
            .as_mut()
            .filter(|_| self.0 == 0)
            .ok_or("Fixture adapter unavailable")?;
        f(peers)
    }

    pub fn status(&self) -> Result<Status> {
        self.with_peers(|_| {
            Ok(Status {
                adapter: "02:00:00:00:00:01",
                ready: true,
                busy: false,
            })
        })
    }
    pub fn saved(&self) -> Result<Vec<DiscoveredPeer>> {
        self.with_peers(|peers| Ok(peers.iter().filter(|p| p.bonded).map(Peer::row).collect()))
    }
    pub fn list(&self) -> Result<Vec<ConnectedPeer>> {
        self.with_peers(|peers| {
            Ok(peers
                .iter()
                .filter(|p| p.connected)
                .map(Peer::row)
                .collect())
        })
    }
    pub fn scan(&self) -> Result<Vec<DiscoveredPeer>> {
        self.with_peers(|peers| Ok(peers.iter().map(Peer::row).collect()))
    }

    fn change(&self, address: Address, f: impl FnOnce(&mut Peer) -> Result<()>) -> Result<()> {
        self.with_peers(|peers| {
            f(peers
                .iter_mut()
                .find(|p| p.address == address)
                .ok_or("Unknown fixture device")?)
        })
    }
    pub fn pair(&self, address: Address) -> Result<()> {
        self.change(address, |p| {
            p.bonded = true;
            p.connected = true;
            Ok(())
        })
    }
    pub fn connect(&self, address: Address) -> Result<()> {
        self.change(address, |p| {
            if !p.bonded {
                return Err("Pair this fixture device first".into());
            }
            p.connected = true;
            Ok(())
        })
    }
    pub fn forget(&self, address: Address) -> Result<()> {
        self.change(address, |p| {
            p.connected = false;
            p.bonded = false;
            Ok(())
        })
    }
    pub fn disconnect(&self, address: Option<Address>) -> Result<()> {
        self.change(address.ok_or("Select a fixture device")?, |p| {
            p.connected = false;
            Ok(())
        })
    }
}
