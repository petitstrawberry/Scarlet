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
            let observation = interface.link_observation();
            let epoch = observation.epoch;
            let old_epoch = self.epochs.get_mut(name).expect("registered link epoch");
            let next = NetworkLinkInfoV1 {
                mac_address: *observation.mac.as_bytes(),
                state: observation.state,
                kind: observation.kind,
                mtu: observation.mtu,
                ..*record
            };
            if next != *record || *old_epoch != epoch {
                crate::device::wireless::notify_transport_change();
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
    fn valid_traffic_name(name: &str) -> bool {
        !name.is_empty()
            && name.len() < 32
            && name != "."
            && name != ".."
            && !name.contains('/')
            && !name.as_bytes().contains(&0)
    }

    fn ipv4_layer(&self) -> Result<alloc::sync::Arc<dyn super::NetworkLayer>, &'static str> {
        self.get_layer("ip").ok_or("IPv4 layer missing")
    }

    pub fn set_traffic_meter(&self, name: &str, remaining: u64) -> Result<(), &'static str> {
        if !Self::valid_traffic_name(name) {
            return Err("Invalid traffic meter name");
        }
        let mut meters = self.traffic_meters.write();
        match meters.get(name) {
            Some(meter) => meter.reset(remaining),
            None => {
                meters.insert(String::from(name), super::traffic_policy::Meter::new(remaining));
                Ok(())
            }
        }
    }

    pub fn traffic_meter(&self, name: &str) -> Option<alloc::sync::Arc<super::traffic_policy::Meter>> {
        self.traffic_meters.read().get(name).cloned()
    }

    pub fn remove_traffic_meter(&self, name: &str) -> Result<(), &'static str> {
        if !Self::valid_traffic_name(name) {
            return Err("Invalid traffic meter name");
        }
        if let Some(meter) = self.traffic_meters.write().remove(name) {
            self.ipv4_layer()?
                .as_any()
                .downcast_ref::<super::ipv4::Ipv4Layer>()
                .ok_or("IPv4 layer unavailable")?
                .clear_traffic_meter(&meter);
        }
        Ok(())
    }

    pub fn set_interface_traffic_meter(
        &self,
        interface: &str,
        meter: Option<&str>,
    ) -> Result<(), &'static str> {
        let mut links = self.link_lifecycle.lock();
        links.refresh(self);
        let link = links.records.get(interface).ok_or("Link removed")?;
        let policy = match meter {
            Some(name) => {
                if !Self::valid_traffic_name(name) {
                    return Err("Invalid traffic meter name");
                }
                let meter = self.traffic_meter(name).ok_or("Traffic meter missing")?;
                Some(super::traffic_policy::Policy::deny_when_exhausted(meter))
            }
            None => None,
        };
        let name = link.interface_name().ok_or("Invalid link name")?;
        self.ipv4_layer()?
            .as_any()
            .downcast_ref::<super::ipv4::Ipv4Layer>()
            .ok_or("IPv4 layer unavailable")?
            .set_traffic_policy(name, policy);
        Ok(())
    }

    pub fn set_global_traffic_meter(&self, meter: Option<&str>) -> Result<(), &'static str> {
        let policy = match meter {
            Some(name) => {
                if !Self::valid_traffic_name(name) {
                    return Err("Invalid traffic meter name");
                }
                let meter = self.traffic_meter(name).ok_or("Traffic meter missing")?;
                Some(super::traffic_policy::Policy::deny_when_exhausted(meter))
            }
            None => None,
        };
        self.ipv4_layer()?
            .as_any()
            .downcast_ref::<super::ipv4::Ipv4Layer>()
            .ok_or("IPv4 layer unavailable")?
            .set_global_traffic_policy(policy);
        Ok(())
    }

    pub fn clear_link_traffic_meter(&self, name: &str) -> Result<(), &'static str> {
        if !Self::valid_traffic_name(name) {
            return Err("Invalid traffic meter name");
        }
        let Some(meter) = self.traffic_meter(name) else { return Ok(()); };
        self.ipv4_layer()?
            .as_any()
            .downcast_ref::<super::ipv4::Ipv4Layer>()
            .ok_or("IPv4 layer unavailable")?
            .clear_links_using_traffic_meter(&meter);
        Ok(())
    }

    /// Bind native IPv4 packet rules to an existing link incarnation. External
    /// control adapters must check their management authority before calling.
    /// Sharing a meter across policies explicitly aggregates those links only.
    pub fn set_link_traffic_policy(
        &self,
        id: u64,
        generation: u64,
        policy: Option<alloc::sync::Arc<super::traffic_policy::Policy>>,
    ) -> Result<(), &'static str> {
        let mut links = self.link_lifecycle.lock();
        links.refresh(self);
        let link = links.records.values().find(|link| link.id == id)
            .ok_or("Link removed")?;
        if link.generation != generation {
            return Err("Link changed");
        }
        let name = link.interface_name().ok_or("Invalid link name")?;
        let layer = self.get_layer("ip").ok_or("IPv4 layer missing")?;
        let ip = layer.as_any().downcast_ref::<super::ipv4::Ipv4Layer>()
            .ok_or("IPv4 layer unavailable")?;
        ip.set_traffic_policy(name, policy);
        Ok(())
    }

    pub fn link_snapshot(&self) -> Vec<NetworkLinkInfoV1> {
        let mut links = self.link_lifecycle.lock();
        links.refresh(self);
        links.records.values().copied().collect()
    }

    /// Retire configuration while a controlled port is closed. Synchronizes
    /// against incarnation-checked DHCP installation; caller must not hold its
    /// carrier observation lock while acquiring the lifecycle lock.
    pub fn retire_link_ipv4(&self, name: &str, id: u64) -> Result<(), &'static str> {
        let mut links = self.link_lifecycle.lock();
        if links.records.get(name).is_none_or(|link| link.id != id) {
            return Err("Link replaced");
        }
        super::config::clear_interface_ipv4(name)?;
        links.refresh(self);
        Ok(())
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
