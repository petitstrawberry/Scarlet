//! Shared link/IP observations for the bar and Control Center.

use alloc::string::String;
use alloc::vec::Vec;
use scarlet_abi::network::{
    LINK_DOWN, LINK_KIND_ETHERNET, LINK_KIND_WIFI, LINK_UP, NetworkLinkInfoV1,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkKind {
    Ethernet,
    Wifi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConnectionState {
    Unknown,
    Disconnected,
    Configuring,
    Connected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceStatus {
    pub name: String,
    pub kind: NetworkKind,
    pub state: ConnectionState,
    pub address: Option<[u8; 4]>,
    pub is_default: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkStatus {
    pub available: bool,
    pub interfaces: Vec<InterfaceStatus>,
}

impl NetworkStatus {
    /// An address indicates local IP configuration, not verified Internet access.
    pub fn from_links(links: &[NetworkLinkInfoV1], addresses: &[(String, [u8; 4], bool)]) -> Self {
        let mut interfaces = Vec::new();
        for link in links {
            let kind = match link.kind {
                LINK_KIND_ETHERNET => NetworkKind::Ethernet,
                LINK_KIND_WIFI => NetworkKind::Wifi,
                _ => continue,
            };
            let Some(name) = link.interface_name() else {
                continue;
            };
            let configured = addresses
                .iter()
                .find(|(iface, ip, _)| iface == name && *ip != [0; 4] && link.state == LINK_UP);
            let state = match link.state {
                LINK_UP if configured.is_some() => ConnectionState::Connected,
                LINK_UP => ConnectionState::Configuring,
                LINK_DOWN => ConnectionState::Disconnected,
                _ => ConnectionState::Unknown,
            };
            interfaces.push(InterfaceStatus {
                name: String::from(name),
                kind,
                state,
                address: configured.map(|(_, ip, _)| *ip),
                is_default: configured.is_some_and(|(_, _, primary)| *primary),
            });
        }
        // Prefer the actual default route, then usable links. Registration
        // order and interface-name prefixes do not determine the active uplink.
        interfaces.sort_by(|a, b| {
            b.is_default
                .cmp(&a.is_default)
                .then_with(|| b.state.cmp(&a.state))
                .then_with(|| a.name.cmp(&b.name))
        });
        Self {
            available: true,
            interfaces,
        }
    }

    /// The bar shows only the usable default route, never a secondary link.
    pub fn default_route(&self) -> Option<&InterfaceStatus> {
        self.interfaces.iter().find(|interface| {
            self.available && interface.is_default && interface.state == ConnectionState::Connected
        })
    }

    /// Retain device identities while discarding an unsuccessful sample.
    pub fn unavailable(&self) -> Self {
        Self {
            available: false,
            interfaces: self
                .interfaces
                .iter()
                .cloned()
                .map(|mut interface| {
                    interface.state = ConnectionState::Unknown;
                    interface.address = None;
                    interface.is_default = false;
                    interface
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn link(id: u64, name: &str, kind: u8, state: u8) -> NetworkLinkInfoV1 {
        let mut link = NetworkLinkInfoV1 {
            id,
            generation: 1,
            kind,
            state,
            ..Default::default()
        };
        link.name[..name.len()].copy_from_slice(name.as_bytes());
        link
    }

    #[test]
    fn carrier_and_ip_configuration_are_distinct() {
        let wifi = link(1, "radio7", LINK_KIND_WIFI, LINK_UP);
        let pending = NetworkStatus::from_links(&[wifi], &[]);
        assert_eq!(pending.interfaces[0].state, ConnectionState::Configuring);
        assert!(pending.default_route().is_none());
        let addresses = vec![(String::from("radio7"), [192, 168, 1, 8], true)];
        let ready = NetworkStatus::from_links(&[wifi], &addresses);
        assert_eq!(ready.default_route().unwrap().kind, NetworkKind::Wifi);
        let down = NetworkStatus::from_links(
            &[NetworkLinkInfoV1 {
                state: LINK_DOWN,
                ..wifi
            }],
            &addresses,
        );
        assert_eq!(down.interfaces[0].state, ConnectionState::Disconnected);
        assert!(down.default_route().is_none());
        assert_eq!(down.interfaces[0].address, None);
        assert!(!down.interfaces[0].is_default);
    }

    #[test]
    fn only_the_default_route_is_selected_when_both_kinds_are_connected() {
        let links = [
            link(1, "cable0", LINK_KIND_ETHERNET, LINK_DOWN),
            link(2, "radio7", LINK_KIND_WIFI, LINK_UP),
            link(3, "usb0", LINK_KIND_ETHERNET, LINK_UP),
        ];
        let addresses = vec![
            (String::from("radio7"), [192, 168, 1, 8], true),
            (String::from("usb0"), [192, 168, 77, 2], false),
        ];
        let snapshot = NetworkStatus::from_links(&links, &addresses);
        assert_eq!(snapshot.interfaces[0].name, "radio7");
        assert_eq!(snapshot.default_route().unwrap().kind, NetworkKind::Wifi);
        assert_eq!(snapshot.default_route().unwrap().name, "radio7");
        let fallback = NetworkStatus::from_links(
            &links,
            &[
                (String::from("radio7"), [192, 168, 1, 8], false),
                (String::from("usb0"), [192, 168, 77, 2], true),
            ],
        );
        assert_eq!(
            fallback.default_route().unwrap().kind,
            NetworkKind::Ethernet
        );
        assert_eq!(fallback.default_route().unwrap().name, "usb0");
        let failed = snapshot.unavailable();
        assert!(!failed.available);
        assert!(failed.default_route().is_none());
        assert!(
            failed
                .interfaces
                .iter()
                .all(|i| i.state == ConnectionState::Unknown)
        );
        assert!(
            failed
                .interfaces
                .iter()
                .all(|i| i.address.is_none() && !i.is_default)
        );
    }

    #[test]
    fn hot_removal_and_missing_addresses_do_not_keep_old_connectivity() {
        let links = [link(1, "usb0", LINK_KIND_ETHERNET, LINK_UP)];
        let snapshot = NetworkStatus::from_links(&links, &[(String::from("usb0"), [0; 4], true)]);
        assert_eq!(snapshot.interfaces[0].state, ConnectionState::Configuring);
        assert!(snapshot.default_route().is_none());
        let removed =
            NetworkStatus::from_links(&[], &[(String::from("usb0"), [192, 168, 77, 2], true)]);
        assert!(removed.available);
        assert!(removed.interfaces.is_empty());
        assert!(removed.default_route().is_none());
    }

    #[test]
    fn a_configured_secondary_link_without_a_default_route_is_not_an_indicator() {
        let links = [link(1, "usb0", LINK_KIND_ETHERNET, LINK_UP)];
        let snapshot =
            NetworkStatus::from_links(&links, &[(String::from("usb0"), [10, 0, 3, 15], false)]);
        assert_eq!(snapshot.interfaces[0].state, ConnectionState::Connected);
        assert!(snapshot.default_route().is_none());
    }
}
