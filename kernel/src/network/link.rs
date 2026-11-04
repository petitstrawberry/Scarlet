//! Link snapshots and incarnation-checked network configuration.
use super::{NetworkManager, ipv4::Ipv4Address};
use alloc::{collections::BTreeMap, string::String, vec::Vec};
use scarlet_abi::network::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkStatisticsError {
    InvalidRequest,
    NotFound,
    Changed,
}

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

    /// Read one interface's existing counters without holding the link lifecycle
    /// lock while acquiring a driver or observation lock.
    pub(crate) fn read_link_statistics(
        &self,
        request: NetworkReadLinkStatisticsV1,
    ) -> Result<NetworkReadLinkStatisticsV1, LinkStatisticsError> {
        if !request.is_query() {
            return Err(LinkStatisticsError::InvalidRequest);
        }

        let (name, observed) = self
            .link_incarnation(request.id)
            .ok_or(LinkStatisticsError::NotFound)?;
        if observed.generation != request.generation {
            return Err(LinkStatisticsError::Changed);
        }

        let interface = self
            .get_interface(&name)
            .ok_or(LinkStatisticsError::Changed)?;
        let stats = interface.stats();

        // A same-name replacement must not attribute the new device's counters
        // to an old 920 observation.
        let (current_name, current) = self
            .link_incarnation(request.id)
            .ok_or(LinkStatisticsError::Changed)?;
        if current_name != name || current.generation != request.generation {
            return Err(LinkStatisticsError::Changed);
        }

        Ok(NetworkReadLinkStatisticsV1 {
            version: LINK_STATISTICS_VERSION_V1,
            size: LINK_STATISTICS_SIZE_V1,
            id: request.id,
            generation: request.generation,
            rx_packets: stats.rx_packets,
            rx_bytes: stats.rx_bytes,
            tx_packets: stats.tx_packets,
            tx_bytes: stats.tx_bytes,
            layer: LINK_STATISTICS_LAYER_LINK,
            reserved: 0,
        })
    }

    fn link_incarnation(&self, id: u64) -> Option<(String, NetworkLinkInfoV1)> {
        let mut links = self.link_lifecycle.lock();
        links.refresh(self);
        links
            .records
            .values()
            .find(|link| link.id == id)
            .and_then(|link| {
                link.interface_name()
                    .map(|name| (String::from(name), *link))
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::network::{DevicePacket, MacAddress};
    use crate::network::{InterfaceStats, NetworkInterface};
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicU8, Ordering};

    struct StatsInterface {
        name: &'static str,
        state: AtomicU8,
        stats: InterfaceStats,
    }

    impl NetworkInterface for StatsInterface {
        fn name(&self) -> &str {
            self.name
        }

        fn mac_address(&self) -> MacAddress {
            MacAddress::new([2, 0, 0, 0, 0, 1])
        }

        fn link_state(&self) -> u8 {
            self.state.load(Ordering::Acquire)
        }

        fn ip_address(&self) -> Option<Ipv4Address> {
            None
        }

        fn set_ip_address(&self, _: Ipv4Address) {}

        fn clear_ip_address(&self) {}

        fn send(&self, _: DevicePacket) -> Result<(), &'static str> {
            Err("statistics test interface")
        }

        fn poll(&self) -> Result<Vec<DevicePacket>, &'static str> {
            Ok(Vec::new())
        }

        fn stats(&self) -> InterfaceStats {
            self.stats.clone()
        }
    }

    fn manager_with_stats() -> (NetworkManager, Arc<StatsInterface>, NetworkLinkInfoV1) {
        let manager = NetworkManager::new();
        let interface = Arc::new(StatsInterface {
            name: "stats0",
            state: AtomicU8::new(LINK_DOWN),
            stats: InterfaceStats {
                tx_packets: 17,
                tx_bytes: 0x1234,
                rx_packets: 23,
                rx_bytes: 0x5678,
                drops: 2,
                errors: 3,
            },
        });
        manager
            .register_interface("stats0", interface.clone())
            .unwrap();
        let link = manager.link_snapshot()[0];
        (manager, interface, link)
    }

    #[test_case]
    fn statistics_round_trip_exact_link_counters_and_identity() {
        let (manager, _interface, link) = manager_with_stats();
        let result = manager
            .read_link_statistics(NetworkReadLinkStatisticsV1::query(
                link.id,
                link.generation,
            ))
            .unwrap();
        assert_eq!(result.version, LINK_STATISTICS_VERSION_V1);
        assert_eq!(result.size, LINK_STATISTICS_SIZE_V1);
        assert_eq!(result.id, link.id);
        assert_eq!(result.generation, link.generation);
        assert_eq!(result.rx_packets, 23);
        assert_eq!(result.rx_bytes, 0x5678);
        assert_eq!(result.tx_packets, 17);
        assert_eq!(result.tx_bytes, 0x1234);
        assert_eq!(result.layer, LINK_STATISTICS_LAYER_LINK);
        assert_eq!(result.reserved, 0);
    }

    #[test_case]
    fn statistics_requests_reject_output_fields_before_lookup() {
        let (manager, _interface, link) = manager_with_stats();
        let mut request = NetworkReadLinkStatisticsV1::query(link.id, link.generation);
        request.rx_bytes = 1;
        assert_eq!(
            manager.read_link_statistics(request),
            Err(LinkStatisticsError::InvalidRequest)
        );

        request.rx_bytes = 0;
        request.layer = LINK_STATISTICS_LAYER_LINK;
        assert_eq!(
            manager.read_link_statistics(request),
            Err(LinkStatisticsError::InvalidRequest)
        );
    }

    #[test_case]
    fn statistics_distinguish_unknown_and_stale_links() {
        let (manager, interface, link) = manager_with_stats();
        assert_eq!(
            manager.read_link_statistics(NetworkReadLinkStatisticsV1::query(
                link.id + 1,
                link.generation,
            )),
            Err(LinkStatisticsError::NotFound)
        );

        interface.state.store(LINK_UP, Ordering::Release);
        assert_eq!(
            manager.read_link_statistics(NetworkReadLinkStatisticsV1::query(
                link.id,
                link.generation,
            )),
            Err(LinkStatisticsError::Changed)
        );
    }
}
