//! Blocking Bluetooth broker client for Scarlet applications. Each call authenticates
//! a fresh connection. Pair success means that the broker durably saved the bond.
#![no_std]

extern crate alloc;

use alloc::{string::String, vec::Vec};
use core::{fmt, str::FromStr};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[doc(hidden)]
pub mod protocol;

/// A failed operation carries a broker or transport diagnostic suitable for display.
pub type Result<T> = core::result::Result<T, String>;

/// A Bluetooth address stored in HCI byte order. Display and parse use normal MAC order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Address(pub [u8; 6]);

impl fmt::Display for Address {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [f, e, d, c, b, a] = self.0;
        write!(out, "{a:02X}:{b:02X}:{c:02X}:{d:02X}:{e:02X}:{f:02X}")
    }
}

impl FromStr for Address {
    type Err = String;

    fn from_str(text: &str) -> Result<Self> {
        let mut address = [0; 6];
        let mut parts = text.split(':');
        for byte in address.iter_mut().rev() {
            let part = parts.next().unwrap_or_default();
            if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("expected a MAC in AA:BB:CC:DD:EE:FF form".into());
            }
            *byte = u8::from_str_radix(part, 16).unwrap();
        }
        if parts.next().is_some() || address == [0; 6] || address == [255; 6] {
            return Err("invalid peer MAC".into());
        }
        Ok(Self(address))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Status {
    pub adapter: Address,
    pub ready: bool,
    pub busy: bool,
    pub scanning: bool,
    pub found: u16,
    pub generation: u32,
    pub peer: Option<Address>,
    pub connected: bool,
    pub encrypted: bool,
    pub profile_ready: bool,
    /// Whether the controller offers a key to the privileged pairing handle.
    pub key_readable: bool,
    pub last_error: u32,
    /// Reconnection roles advertised by the remote Classic HID service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hid_reconnect_mode: Option<HidReconnectMode>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HidReconnectMode {
    None,
    Device,
    Host,
    Any,
}

impl fmt::Display for HidReconnectMode {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::None => "none",
            Self::Device => "device",
            Self::Host => "host",
            Self::Any => "any",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct DiscoveredPeer {
    pub address: Address,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ConnectedPeer {
    pub address: Address,
    pub name: Option<String>,
    pub encrypted: bool,
    pub profile_ready: bool,
    /// Whether the controller offers a key to the privileged pairing handle.
    pub key_readable: bool,
    pub last_error: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hid_reconnect_mode: Option<HidReconnectMode>,
}

use protocol::Command;

/// Select a numbered adapter, e.g. zero for /dev/bluetooth0.
#[derive(Clone, Copy, Debug, Default)]
pub struct Client {
    adapter: u32,
}

impl Client {
    pub const fn new(adapter: u32) -> Self {
        Self { adapter }
    }

    fn request<T: DeserializeOwned>(&self, cmd: Command) -> Result<T> {
        use alloc::format;
        let socket =
            protocol::open().map_err(|e| format!("Bluetooth service unavailable ({e:?})"))?;
        let bytes = protocol::exchange(&socket, self.adapter, cmd, true)?;
        serde_json::from_slice(&bytes).map_err(|_| "invalid typed Bluetooth service reply".into())
    }

    pub fn status(&self) -> Result<Status> {
        self.request(Command::Status)
    }

    /// Currently connected peers, including their profile state.
    pub fn list(&self) -> Result<Vec<ConnectedPeer>> {
        self.request(Command::Devices)
    }

    /// Locally saved peers for this adapter, including offline peers. Does not start inquiry.
    pub fn bonded(&self) -> Result<Vec<Address>> {
        self.request(Command::Bonded)
    }

    /// Saved bonds and their last known names, including offline peers. Does not start inquiry.
    pub fn saved(&self) -> Result<Vec<DiscoveredPeer>> {
        self.request(Command::Saved)
    }

    /// Disconnect the peer and remove its local key and name. Safe to retry.
    pub fn forget(&self, peer: Address) -> Result<()> {
        self.request(Command::Forget(peer.0))
    }

    pub fn scan(&self) -> Result<Vec<DiscoveredPeer>> {
        self.request(Command::Scan)
    }

    pub fn pair(&self, peer: Address) -> Result<Status> {
        self.request(Command::Pair(peer.0))
    }

    pub fn connect(&self, peer: Address) -> Result<Status> {
        self.request(Command::Connect(peer.0))
    }

    /// Without an address the controller must have exactly one connected peer.
    pub fn disconnect(&self, peer: Option<Address>) -> Result<Status> {
        self.request(Command::Disconnect(peer.map(|p| p.0)))
    }
}
