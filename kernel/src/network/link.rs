//! Link snapshots and incarnation-checked network configuration.
use super::{NetworkManager, ipv4::Ipv4Address};
use alloc::{collections::BTreeMap, string::String, vec::Vec};
use scarlet_abi::network::*;

pub(super) struct LinkRegistry {
    next_id: u64,
    epochs: BTreeMap<String, u64>,
    records: BTreeMap<String, NetworkLinkInfoV1>,
}
impl LinkRegistry {
    pub(super) const fn new() -> Self {
        Self {
            next_id: 1,
            records: BTreeMap::new(),
            epochs: BTreeMap::new(),
        }
    }
    pub(super) fn register(&mut self, name: &str) -> Result<(), &'static str> {
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or("Link IDs exhausted")?;
        let mut record = NetworkLinkInfoV1 {
            id,
            generation: 1,
            ..Default::default()
        };
        record.name[..name.len()].copy_from_slice(name.as_bytes());
        self.records.insert(String::from(name), record);
        self.epochs.insert(String::from(name), 0);
        Ok(())
    }
    pub(super) fn remove(&mut self, name: &str) {
        self.records.remove(name);
        self.epochs.remove(name);
    }
    fn refresh(&mut self, manager: &NetworkManager) {
        for (name, record) in &mut self.records {
            let Some(interface) = manager.get_interface(name) else {
                continue;
            };
            let epoch = interface.link_epoch();
            let old_epoch = self.epochs.get_mut(name).expect("registered link epoch");
            let next = NetworkLinkInfoV1 {
                mac_address: *interface.mac_address().as_bytes(),
                state: interface.link_state(),
                kind: interface.link_kind(),
                mtu: interface.mtu(),
                ..*record
            };
            if next != *record || *old_epoch != epoch {
                *record = NetworkLinkInfoV1 {
                    generation: record.generation.saturating_add(1),
                    ..next
                };
            }
            *old_epoch = epoch;
        }
    }
}
impl NetworkManager {
    pub fn link_snapshot(&self) -> Vec<NetworkLinkInfoV1> {
        let mut links = self.link_lifecycle.lock();
        links.refresh(self);
        links.records.values().copied().collect()
    }

    /// A stale DHCP/association completion must never configure a replacement NIC.
    pub fn update_link_ipv4(&self, request: NetworkUpdateIpv4V1) -> Result<(), &'static str> {
        let mut links = self.link_lifecycle.lock();
        links.refresh(self);
        let link = links
            .records
            .values()
            .find(|link| link.id == request.id)
            .ok_or("Link removed")?;
        if link.generation != request.generation {
            return Err("Link changed");
        }
        if request.reserved != 0 || request.flags & !7 != 0 {
            return Err("Invalid flags");
        }
        let name = link.interface_name().ok_or("Invalid link name")?;
        if request.flags & IPV4_CLEAR != 0 {
            if request.flags != IPV4_CLEAR
                || request.address != [0; 4]
                || request.netmask != [0; 4]
                || request.gateway != [0; 4]
                || request.metric != 0
            {
                return Err("Invalid clear request");
            }
            return super::config::clear_interface_ipv4(name);
        }
        if request.flags & IPV4_HAS_GATEWAY == 0 && request.gateway != [0; 4] {
            return Err("Unexpected gateway");
        }
        if !link.is_up() {
            return Err("Link not ready");
        }
        super::config::configure_interface_ipv4(
            name,
            Ipv4Address::from_bytes(request.address),
            Ipv4Address::from_bytes(request.netmask),
            (request.flags & IPV4_HAS_GATEWAY != 0)
                .then(|| Ipv4Address::from_bytes(request.gateway)),
            request.metric,
            request.flags & IPV4_MAKE_DEFAULT != 0,
        )
    }
}
