//! Native IPv6 address, wire-header, and static-routing foundation.
//!
//! This layer intentionally stops below IPv6 sockets and neighbor discovery.
//! It forwards Ethernet traffic only when a lower layer is supplied explicitly
//! or the existing Ethernet layer is globally registered. Link-local neighbor
//! discovery, solicitation, extension-header processing, and fragment
//! reassembly belong to later integration slices.
//!
//! IPv6-aware upper layers use `ipv6_src`, `ipv6_dst`, and `ip_protocol` in
//! [`LayerContext`]. The layer writes `ipv6_next_hop`, `ip_version = [6]`,
//! `eth_type = 0x86dd`, and the interface name for lower layers. Until neighbor
//! discovery is integrated, a non-local transmission also requires a six-byte
//! `eth_dst_mac`; IPv6 addresses are never placed in the IPv4 `next_hop` key.

use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::network::packet::PacketBuffer;
use crate::network::protocol_stack::{
    LayerContext, NetworkLayer, NetworkLayerStats, get_network_manager,
};
use crate::network::socket::SocketError;
use crate::sync::IrqRwSpinLock;

pub use super::ip::Ipv6Address;

/// Fixed IPv6 base-header size.
pub const IPV6_HEADER_LEN: usize = 40;
/// IPv6 minimum link MTU. This layer does not perform fragmentation.
pub const IPV6_MIN_MTU: usize = 1280;

pub(crate) const IPV6_ETHERNET_HEADROOM: usize = IPV6_HEADER_LEN + 14;

/// Protocol numbers carried in the IPv6 Next Header field.
pub mod protocol {
    pub const HOP_BY_HOP: u8 = 0;
    pub const TCP: u8 = 6;
    pub const UDP: u8 = 17;
    pub const ROUTING: u8 = 43;
    pub const FRAGMENT: u8 = 44;
    pub const ESP: u8 = 50;
    pub const AH: u8 = 51;
    pub const ICMPV6: u8 = 58;
    pub const NO_NEXT_HEADER: u8 = 59;
    pub const DESTINATION_OPTIONS: u8 = 60;
    pub const MOBILITY: u8 = 135;
    pub const HIP: u8 = 139;
    pub const SHIM6: u8 = 140;
}

/// Whether a Next Header value requires extension-header processing that this
/// bounded slice does not implement.
pub const fn is_unsupported_extension_header(next_header: u8) -> bool {
    matches!(
        next_header,
        protocol::HOP_BY_HOP
            | protocol::ROUTING
            | protocol::FRAGMENT
            | protocol::AH
            | protocol::DESTINATION_OPTIONS
            | protocol::MOBILITY
            | protocol::HIP
            | protocol::SHIM6
    )
}

/// Errors from validating or encoding an IPv6 base header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ipv6HeaderError {
    PacketTooShort,
    InvalidVersion(u8),
    TruncatedPayload { declared: usize, available: usize },
    FlowLabelOutOfRange(u32),
    PayloadTooLarge(usize),
}

/// Errors from calculating an upper-layer checksum over an IPv6 pseudo-header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ipv6ChecksumError {
    PayloadTooLarge(usize),
}

/// A validated IPv6 packet view. Link-layer padding is retained separately and
/// never passed to an upper-layer protocol handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedIpv6<'a> {
    pub header: Ipv6Header,
    pub payload: &'a [u8],
    pub trailing: &'a [u8],
}

/// The fixed 40-byte IPv6 base header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv6Header {
    pub traffic_class: u8,
    /// A 20-bit value. Encoding rejects values wider than 20 bits.
    pub flow_label: u32,
    pub payload_length: u16,
    pub next_header: u8,
    pub hop_limit: u8,
    pub source: Ipv6Address,
    pub destination: Ipv6Address,
}

impl Ipv6Header {
    pub const fn new(
        payload_length: u16,
        next_header: u8,
        hop_limit: u8,
        source: Ipv6Address,
        destination: Ipv6Address,
    ) -> Self {
        Self {
            traffic_class: 0,
            flow_label: 0,
            payload_length,
            next_header,
            hop_limit,
            source,
            destination,
        }
    }

    /// Decode one base header and split exactly `payload_length` bytes from any
    /// trailing link-layer padding.
    pub fn decode(packet: &[u8]) -> Result<ParsedIpv6<'_>, Ipv6HeaderError> {
        if packet.len() < IPV6_HEADER_LEN {
            return Err(Ipv6HeaderError::PacketTooShort);
        }

        let first = u32::from_be_bytes([packet[0], packet[1], packet[2], packet[3]]);
        let version = (first >> 28) as u8;
        if version != 6 {
            return Err(Ipv6HeaderError::InvalidVersion(version));
        }

        let payload_length = u16::from_be_bytes([packet[4], packet[5]]);
        let payload_end = IPV6_HEADER_LEN + usize::from(payload_length);
        if payload_end > packet.len() {
            return Err(Ipv6HeaderError::TruncatedPayload {
                declared: usize::from(payload_length),
                available: packet.len() - IPV6_HEADER_LEN,
            });
        }

        let mut source = [0; 16];
        let mut destination = [0; 16];
        source.copy_from_slice(&packet[8..24]);
        destination.copy_from_slice(&packet[24..40]);
        let header = Self {
            traffic_class: ((first >> 20) & 0xff) as u8,
            flow_label: first & 0x000f_ffff,
            payload_length,
            next_header: packet[6],
            hop_limit: packet[7],
            source: Ipv6Address::from_bytes(source),
            destination: Ipv6Address::from_bytes(destination),
        };

        Ok(ParsedIpv6 {
            header,
            payload: &packet[IPV6_HEADER_LEN..payload_end],
            trailing: &packet[payload_end..],
        })
    }

    /// Encode the fixed header into an existing output buffer.
    pub fn encode(&self, output: &mut [u8; IPV6_HEADER_LEN]) -> Result<(), Ipv6HeaderError> {
        if self.flow_label > 0x000f_ffff {
            return Err(Ipv6HeaderError::FlowLabelOutOfRange(self.flow_label));
        }

        let first = 0x6000_0000 | (u32::from(self.traffic_class) << 20) | self.flow_label;
        output[..4].copy_from_slice(&first.to_be_bytes());
        output[4..6].copy_from_slice(&self.payload_length.to_be_bytes());
        output[6] = self.next_header;
        output[7] = self.hop_limit;
        output[8..24].copy_from_slice(&self.source.as_bytes());
        output[24..40].copy_from_slice(&self.destination.as_bytes());
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<[u8; IPV6_HEADER_LEN], Ipv6HeaderError> {
        let mut bytes = [0; IPV6_HEADER_LEN];
        self.encode(&mut bytes)?;
        Ok(bytes)
    }

    /// Construct a header whose payload length is derived from an owned slice.
    pub fn for_payload(
        payload: &[u8],
        next_header: u8,
        hop_limit: u8,
        source: Ipv6Address,
        destination: Ipv6Address,
    ) -> Result<Self, Ipv6HeaderError> {
        let payload_length = u16::try_from(payload.len())
            .map_err(|_| Ipv6HeaderError::PayloadTooLarge(payload.len()))?;
        Ok(Self::new(
            payload_length,
            next_header,
            hop_limit,
            source,
            destination,
        ))
    }
}

/// Calculate the IPv6 upper-layer checksum from RFC 8200 section 8.1.
///
/// For checksum generation, call this with the transport checksum field set to
/// zero and store the result. For verification, include the received checksum
/// field and require [`verify_pseudo_header_checksum`] to be true. UDP must
/// encode a generated zero result as `0xffff`.
pub fn pseudo_header_checksum(
    source: Ipv6Address,
    destination: Ipv6Address,
    next_header: u8,
    payload: &[u8],
) -> Result<u16, Ipv6ChecksumError> {
    let checksum = !pseudo_header_sum(source, destination, next_header, payload)?;
    Ok(if checksum == 0 { 0xffff } else { checksum })
}

fn pseudo_header_sum(
    source: Ipv6Address,
    destination: Ipv6Address,
    next_header: u8,
    payload: &[u8],
) -> Result<u16, Ipv6ChecksumError> {
    let payload_len = u32::try_from(payload.len())
        .map_err(|_| Ipv6ChecksumError::PayloadTooLarge(payload.len()))?;
    let mut sum = 0u32;
    sum = add_checksum_bytes(sum, &source.as_bytes());
    sum = add_checksum_bytes(sum, &destination.as_bytes());
    sum = add_checksum_bytes(sum, &payload_len.to_be_bytes());
    sum = add_checksum_bytes(sum, &[0, 0, 0, next_header]);
    sum = add_checksum_bytes(sum, payload);
    Ok(fold_checksum(sum))
}

/// Verify an IPv6 upper-layer checksum that is already present in `payload`.
pub fn verify_pseudo_header_checksum(
    source: Ipv6Address,
    destination: Ipv6Address,
    next_header: u8,
    payload: &[u8],
) -> Result<bool, Ipv6ChecksumError> {
    Ok(pseudo_header_sum(source, destination, next_header, payload)? == 0xffff)
}

fn add_checksum_bytes(mut sum: u32, bytes: &[u8]) -> u32 {
    let mut chunks = bytes.chunks_exact(2);
    for chunk in &mut chunks {
        sum += u32::from(u16::from_be_bytes([chunk[0], chunk[1]]));
        sum = (sum & 0xffff) + (sum >> 16);
    }
    if let Some(byte) = chunks.remainder().first() {
        sum += u32::from(*byte) << 8;
        sum = (sum & 0xffff) + (sum >> 16);
    }
    sum
}

fn fold_checksum(mut sum: u32) -> u16 {
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    sum as u16
}

/// Errors from IPv6 address and static-route configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ipv6ConfigError {
    InvalidPrefixLength(u8),
    EmptyInterface,
    InvalidGateway,
    UnusableAddress,
}

/// A canonical IPv6 network prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ipv6Prefix {
    network: Ipv6Address,
    prefix_len: u8,
}

impl Ipv6Prefix {
    pub fn new(address: Ipv6Address, prefix_len: u8) -> Result<Self, Ipv6ConfigError> {
        let network = address
            .masked(prefix_len)
            .ok_or(Ipv6ConfigError::InvalidPrefixLength(prefix_len))?;
        Ok(Self {
            network,
            prefix_len,
        })
    }

    pub const fn address(self) -> Ipv6Address {
        self.network
    }

    pub const fn prefix_len(self) -> u8 {
        self.prefix_len
    }

    pub fn contains(self, address: Ipv6Address) -> bool {
        self.network.contains(address, self.prefix_len)
    }
}

/// An address assigned to one interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv6AddressInfo {
    address: Ipv6Address,
    prefix_len: u8,
    is_primary: bool,
}

impl Ipv6AddressInfo {
    pub fn new(
        address: Ipv6Address,
        prefix_len: u8,
        is_primary: bool,
    ) -> Result<Self, Ipv6ConfigError> {
        if address.is_unspecified()
            || address.is_loopback()
            || address.is_multicast()
            || address.is_ipv4_mapped()
        {
            return Err(Ipv6ConfigError::UnusableAddress);
        }
        Ipv6Prefix::new(address, prefix_len)?;
        Ok(Self {
            address,
            prefix_len,
            is_primary,
        })
    }

    pub const fn address(&self) -> Ipv6Address {
        self.address
    }

    pub const fn prefix_len(&self) -> u8 {
        self.prefix_len
    }

    pub const fn is_primary(&self) -> bool {
        self.is_primary
    }

    pub fn prefix(&self) -> Ipv6Prefix {
        Ipv6Prefix::new(self.address, self.prefix_len).expect("validated IPv6 address prefix")
    }
}

/// A static IPv6 route. A `None` gateway is directly connected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ipv6RouteEntry {
    prefix: Ipv6Prefix,
    gateway: Option<Ipv6Address>,
    interface: String,
    metric: u32,
}

impl Ipv6RouteEntry {
    pub fn new(
        destination: Ipv6Address,
        prefix_len: u8,
        gateway: Option<Ipv6Address>,
        interface: impl Into<String>,
        metric: u32,
    ) -> Result<Self, Ipv6ConfigError> {
        if gateway.is_some_and(|address| {
            address.is_unspecified()
                || address.is_loopback()
                || address.is_multicast()
                || address.is_ipv4_mapped()
        }) {
            return Err(Ipv6ConfigError::InvalidGateway);
        }
        let interface = interface.into();
        if interface.is_empty() {
            return Err(Ipv6ConfigError::EmptyInterface);
        }
        Ok(Self {
            prefix: Ipv6Prefix::new(destination, prefix_len)?,
            gateway,
            interface,
            metric,
        })
    }

    pub const fn prefix(&self) -> Ipv6Prefix {
        self.prefix
    }

    pub const fn gateway(&self) -> Option<Ipv6Address> {
        self.gateway
    }

    pub fn interface(&self) -> &str {
        &self.interface
    }

    pub const fn metric(&self) -> u32 {
        self.metric
    }
}

/// Result of a static route lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ipv6RouteSelection {
    pub interface: String,
    pub source: Ipv6Address,
    pub next_hop: Ipv6Address,
    pub gateway: Option<Ipv6Address>,
    pub prefix_len: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteCandidate {
    interface: String,
    source: Option<Ipv6Address>,
    gateway: Option<Ipv6Address>,
    prefix_len: u8,
    metric: u32,
    primary: bool,
}

impl RouteCandidate {
    fn is_better_than(&self, current: &Self) -> bool {
        self.prefix_len > current.prefix_len
            || (self.prefix_len == current.prefix_len && self.metric < current.metric)
            || (self.prefix_len == current.prefix_len
                && self.metric == current.metric
                && self.primary
                && !current.primary)
            || (self.prefix_len == current.prefix_len
                && self.metric == current.metric
                && self.primary == current.primary
                && self.interface == current.interface
                && self.source < current.source)
            || (self.prefix_len == current.prefix_len
                && self.metric == current.metric
                && self.primary == current.primary
                && self.interface < current.interface)
    }

    fn selection(self, destination: Ipv6Address, source: Ipv6Address) -> Ipv6RouteSelection {
        Ipv6RouteSelection {
            interface: self.interface,
            source,
            next_hop: self.gateway.unwrap_or(destination),
            gateway: self.gateway,
            prefix_len: self.prefix_len,
        }
    }
}

/// Native IPv6 encapsulation and receive dispatch.
pub struct Ipv6Layer {
    addresses: IrqRwSpinLock<BTreeMap<String, Vec<Ipv6AddressInfo>>>,
    routing_table: IrqRwSpinLock<Vec<Ipv6RouteEntry>>,
    protocols: IrqRwSpinLock<BTreeMap<u8, Arc<dyn NetworkLayer>>>,
    traffic_policies: IrqRwSpinLock<BTreeMap<String, Arc<super::traffic_policy::Policy>>>,
    global_traffic_policy: IrqRwSpinLock<Option<Arc<super::traffic_policy::Policy>>>,
    stats: IrqRwSpinLock<NetworkLayerStats>,
    default_hop_limit: u8,
}

impl Ipv6Layer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            addresses: IrqRwSpinLock::new(BTreeMap::new()),
            routing_table: IrqRwSpinLock::new(Vec::new()),
            protocols: IrqRwSpinLock::new(BTreeMap::new()),
            traffic_policies: IrqRwSpinLock::new(BTreeMap::new()),
            global_traffic_policy: IrqRwSpinLock::new(None),
            stats: IrqRwSpinLock::new(NetworkLayerStats::default()),
            default_hop_limit: 64,
        })
    }

    pub fn with_hop_limit(hop_limit: u8) -> Arc<Self> {
        let mut layer = Self::new();
        Arc::get_mut(&mut layer)
            .expect("new IPv6 layer is not shared")
            .default_hop_limit = hop_limit;
        layer
    }

    /// Install an address. A primary address replaces the interface's previous
    /// primary while preserving secondary addresses.
    pub fn add_address(
        &self,
        interface: &str,
        info: Ipv6AddressInfo,
    ) -> Result<(), Ipv6ConfigError> {
        if interface.is_empty() {
            return Err(Ipv6ConfigError::EmptyInterface);
        }
        if info.is_primary() {
            self.set_primary_address(interface, info)?;
            return Ok(());
        }

        let mut addresses = self.addresses.write();
        let entries = addresses.entry(interface.to_string()).or_default();
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| entry.address() == info.address())
        {
            *existing = info;
        } else {
            entries.push(info);
        }
        Ok(())
    }

    pub fn set_primary_address(
        &self,
        interface: &str,
        mut info: Ipv6AddressInfo,
    ) -> Result<(), Ipv6ConfigError> {
        if interface.is_empty() {
            return Err(Ipv6ConfigError::EmptyInterface);
        }
        info.is_primary = true;
        let mut addresses = self.addresses.write();
        let entries = addresses.entry(interface.to_string()).or_default();
        entries.retain(|entry| entry.address() != info.address() && !entry.is_primary());
        entries.push(info);
        Ok(())
    }

    pub fn remove_address(&self, interface: &str, address: Ipv6Address) {
        if let Some(entries) = self.addresses.write().get_mut(interface) {
            entries.retain(|entry| entry.address() != address);
        }
    }

    pub fn get_addresses(&self, interface: &str) -> Vec<Ipv6AddressInfo> {
        self.addresses
            .read()
            .get(interface)
            .cloned()
            .unwrap_or_default()
    }

    pub fn get_primary_address(&self, interface: &str) -> Option<Ipv6Address> {
        self.addresses
            .read()
            .get(interface)?
            .iter()
            .find(|entry| entry.is_primary())
            .map(|entry| entry.address())
    }

    pub fn get_primary_ip(&self, interface: &str) -> Option<Ipv6Address> {
        self.get_primary_address(interface)
    }

    pub fn interface_for_address(&self, address: Ipv6Address) -> Option<String> {
        let addresses = self.addresses.read();
        let mut found = None;
        for (interface, entries) in addresses.iter() {
            if entries.iter().any(|entry| entry.address() == address) {
                if found.is_some() {
                    return None;
                }
                found = Some(interface.clone());
            }
        }
        found
    }

    pub fn is_address_on_interface(&self, interface: &str, address: Ipv6Address) -> bool {
        self.addresses
            .read()
            .get(interface)
            .is_some_and(|entries| entries.iter().any(|entry| entry.address() == address))
    }

    /// Loopback is local even when no physical interface is configured.
    pub fn is_local_address(&self, address: Ipv6Address) -> bool {
        address.is_loopback()
            || self
                .addresses
                .read()
                .values()
                .any(|entries| entries.iter().any(|entry| entry.address() == address))
    }

    pub fn remove_interface(&self, interface: &str) {
        self.addresses.write().remove(interface);
        self.routing_table
            .write()
            .retain(|route| route.interface() != interface);
        self.traffic_policies.write().remove(interface);
    }

    pub fn add_route(&self, route: Ipv6RouteEntry) {
        let mut routes = self.routing_table.write();
        if let Some(existing) = routes.iter_mut().find(|existing| {
            existing.prefix() == route.prefix()
                && existing.interface() == route.interface()
                && existing.gateway() == route.gateway()
        }) {
            *existing = route;
        } else {
            routes.push(route);
        }
        routes.sort_by(|left, right| {
            right
                .prefix
                .prefix_len()
                .cmp(&left.prefix.prefix_len())
                .then(left.metric.cmp(&right.metric))
        });
    }

    pub fn remove_route(&self, destination: Ipv6Address, prefix_len: u8) {
        let Ok(prefix) = Ipv6Prefix::new(destination, prefix_len) else {
            return;
        };
        self.routing_table
            .write()
            .retain(|route| route.prefix() != prefix);
    }

    pub fn set_default_route(
        &self,
        gateway: Ipv6Address,
        interface: &str,
        metric: u32,
    ) -> Result<(), Ipv6ConfigError> {
        let route = Ipv6RouteEntry::new(
            Ipv6Address::UNSPECIFIED,
            0,
            Some(gateway),
            interface,
            metric,
        )?;
        self.routing_table.write().retain(|existing| {
            existing.prefix().prefix_len() != 0 || existing.interface() != route.interface()
        });
        self.add_route(route);
        Ok(())
    }

    pub fn clear_default_route(&self, interface: &str) {
        self.routing_table
            .write()
            .retain(|route| route.prefix().prefix_len() != 0 || route.interface() != interface);
    }

    /// Choose the longest matching prefix, then the lowest metric. Connected
    /// prefixes participate at their actual prefix length, so a default route
    /// cannot hide an on-link destination. Loopback and assigned local targets
    /// are not static-route destinations; local delivery must select them first.
    pub fn select_route(&self, destination: Ipv6Address) -> Option<Ipv6RouteSelection> {
        if self.is_local_address(destination) {
            return None;
        }
        let routes = self.routing_table.read().clone();
        let addresses = self.addresses.read().clone();
        let mut best = None;

        for route in routes {
            if !route.prefix().contains(destination) {
                continue;
            }
            let next_hop = route.gateway().unwrap_or(destination);
            let Some(source) = self.source_for_interface(route.interface(), next_hop) else {
                continue;
            };
            let candidate = RouteCandidate {
                interface: route.interface().to_string(),
                source: Some(source),
                gateway: route.gateway(),
                prefix_len: route.prefix().prefix_len(),
                metric: route.metric(),
                primary: false,
            };
            if best
                .as_ref()
                .is_none_or(|current| candidate.is_better_than(current))
            {
                best = Some(candidate);
            }
        }

        for (interface, entries) in addresses {
            for entry in entries {
                let prefix = entry.prefix();
                if !prefix.contains(destination) {
                    continue;
                }
                let candidate = RouteCandidate {
                    interface: interface.clone(),
                    source: Some(entry.address()),
                    gateway: None,
                    prefix_len: entry.prefix_len(),
                    metric: 0,
                    primary: entry.is_primary(),
                };
                if best
                    .as_ref()
                    .is_none_or(|current| candidate.is_better_than(current))
                {
                    best = Some(candidate);
                }
            }
        }

        let candidate = best?;
        let source = candidate.source?;
        Some(candidate.selection(destination, source))
    }

    /// IPv4-compatible convenience API for callers that need source and gateway
    /// rather than the richer route selection.
    pub fn select_source(
        &self,
        destination: Ipv6Address,
    ) -> Option<(String, Ipv6Address, Option<Ipv6Address>)> {
        self.select_route(destination)
            .map(|route| (route.interface, route.source, route.gateway))
    }

    /// Return the next hop for a destination pinned to one interface. The outer
    /// `None` means no matching route; `Some(None)` is an on-link destination.
    pub fn next_hop_on_interface(
        &self,
        destination: Ipv6Address,
        interface: &str,
    ) -> Option<Option<Ipv6Address>> {
        let mut best: Option<(u8, u32, Option<Ipv6Address>)> = None;
        for route in self.routing_table.read().iter() {
            if route.interface() == interface && route.prefix().contains(destination) {
                let candidate = (route.prefix().prefix_len(), route.metric(), route.gateway());
                if best.as_ref().is_none_or(|(prefix, metric, _)| {
                    candidate.0 > *prefix || (candidate.0 == *prefix && candidate.1 < *metric)
                }) {
                    best = Some(candidate);
                }
            }
        }
        let addresses = self.addresses.read();
        for entry in addresses.get(interface).into_iter().flatten() {
            if entry.prefix().contains(destination) {
                let candidate = (entry.prefix_len(), 0, None);
                if best.as_ref().is_none_or(|(prefix, metric, _)| {
                    candidate.0 > *prefix || (candidate.0 == *prefix && candidate.1 < *metric)
                }) {
                    best = Some(candidate);
                }
            }
        }
        best.map(|(_, _, gateway)| gateway)
    }

    fn source_for_interface(&self, interface: &str, next_hop: Ipv6Address) -> Option<Ipv6Address> {
        let addresses = self.addresses.read();
        let entries = addresses.get(interface)?;
        entries
            .iter()
            .filter(|entry| entry.prefix().contains(next_hop))
            .max_by_key(|entry| (entry.prefix_len(), entry.is_primary(), entry.address()))
            .map(|entry| entry.address())
            .or_else(|| {
                entries
                    .iter()
                    .find(|entry| entry.is_primary())
                    .map(|entry| entry.address())
            })
            .or_else(|| entries.first().map(|entry| entry.address()))
    }

    pub fn get_protocol_handler(&self, next_header: u8) -> Option<Arc<dyn NetworkLayer>> {
        self.protocols.read().get(&next_header).cloned()
    }

    pub(super) fn set_traffic_policy(
        &self,
        interface: &str,
        policy: Option<Arc<super::traffic_policy::Policy>>,
    ) {
        if let Some(policy) = policy {
            self.traffic_policies
                .write()
                .insert(interface.to_string(), policy);
        } else {
            self.traffic_policies.write().remove(interface);
        }
    }

    pub(super) fn set_global_traffic_policy(
        &self,
        policy: Option<Arc<super::traffic_policy::Policy>>,
    ) {
        *self.global_traffic_policy.write() = policy;
    }

    pub(super) fn clear_traffic_meter(&self, meter: &Arc<super::traffic_policy::Meter>) {
        self.traffic_policies
            .write()
            .retain(|_, policy| !policy.uses_meter(meter));
        let mut global = self.global_traffic_policy.write();
        if global
            .as_ref()
            .is_some_and(|policy| policy.uses_meter(meter))
        {
            *global = None;
        }
    }

    pub(super) fn clear_links_using_traffic_meter(
        &self,
        meter: &Arc<super::traffic_policy::Meter>,
    ) {
        self.traffic_policies
            .write()
            .retain(|_, policy| !policy.uses_meter(meter));
    }

    pub(super) fn admit_traffic(
        &self,
        interface: &str,
        direction: super::traffic::Direction,
        bytes: usize,
    ) -> bool {
        let global = self.global_traffic_policy.read().clone();
        if global.is_some_and(|policy| !policy.admit(direction, bytes)) {
            return false;
        }
        let policy = self.traffic_policies.read().get(interface).cloned();
        policy.is_none_or(|policy| policy.admit(direction, bytes))
    }

    fn has_ingress_policy(&self) -> bool {
        self.global_traffic_policy.read().is_some() || !self.traffic_policies.read().is_empty()
    }

    fn dispatch_local(
        &self,
        payload: &[u8],
        header: &Ipv6Header,
        context: Option<&LayerContext>,
    ) -> Result<(), SocketError> {
        let handler = self
            .get_protocol_handler(header.next_header)
            .ok_or(SocketError::ProtocolNotSupported)?;
        let mut proto_context = LayerContext::new();
        if let Some(context) = context {
            if let Some(budget) = context.traffic_budget() {
                proto_context.set_traffic_budget(budget.clone());
            }
            if let Some(interface) = context.get("interface") {
                proto_context.set("interface", interface);
            }
            if let Some(mac) = context.get("eth_src_mac") {
                proto_context.set("eth_src_mac", mac);
            }
            if let Some(mac) = context.get("eth_dst_mac") {
                proto_context.set("eth_dst_mac", mac);
            }
        }
        proto_context.set("ipv6_src", &header.source.as_bytes());
        proto_context.set("ipv6_dst", &header.destination.as_bytes());
        proto_context.set("ip_protocol", &[header.next_header]);
        proto_context.set("ip_version", &[6]);
        handler.receive(payload, Some(&proto_context))
    }

    /// Deliver a payload to a locally configured IPv6 destination. This is the
    /// explicit local-injection path; wire ingress never treats `::1` as local.
    /// Link traffic policy and the transmit budget do not apply without a link.
    pub fn deliver_local(
        &self,
        payload: &[u8],
        source: Ipv6Address,
        destination: Ipv6Address,
        next_header: u8,
        context: Option<&LayerContext>,
    ) -> Result<(), SocketError> {
        if is_unsupported_extension_header(next_header) {
            return Err(SocketError::ProtocolNotSupported);
        }
        if self.default_hop_limit == 0 {
            return Err(SocketError::InvalidPacket);
        }
        if destination.is_unspecified()
            || destination.is_multicast()
            || !self.is_local_address(destination)
        {
            return Err(SocketError::NoRoute);
        }
        if source.is_multicast()
            || source.is_unspecified()
            || source.is_ipv4_mapped()
            || (source.is_loopback() && !destination.is_loopback())
        {
            return Err(SocketError::InvalidAddress);
        }
        if !source.is_loopback() {
            let context_interface = context
                .and_then(|context| context.get("interface"))
                .and_then(|value| core::str::from_utf8(value).ok());
            match context_interface {
                Some(interface) if !self.is_address_on_interface(interface, source) => {
                    return Err(SocketError::InvalidAddress);
                }
                None if self.interface_for_address(source).is_none() => {
                    return Err(SocketError::InvalidAddress);
                }
                _ => {}
            }
        }
        if next_header == protocol::NO_NEXT_HEADER {
            return if payload.is_empty() {
                Ok(())
            } else {
                Err(SocketError::InvalidPacket)
            };
        }

        let header = Ipv6Header::for_payload(
            payload,
            next_header,
            self.default_hop_limit,
            source,
            destination,
        )
        .map_err(|_| SocketError::MessageTooLarge)?;
        self.dispatch_local(payload, &header, context)
    }

    fn payload_ip6(context: &LayerContext, key: &str) -> Result<Ipv6Address, SocketError> {
        let bytes = context.get(key).ok_or(SocketError::InvalidPacket)?;
        if bytes.len() != 16 {
            return Err(SocketError::InvalidAddress);
        }
        let mut address = [0; 16];
        address.copy_from_slice(bytes);
        Ok(Ipv6Address::from_bytes(address))
    }
}

impl NetworkLayer for Ipv6Layer {
    fn register_protocol(&self, next_header: u16, handler: Arc<dyn NetworkLayer>) {
        if let Ok(next_header) = u8::try_from(next_header) {
            self.protocols.write().insert(next_header, handler);
        }
    }

    fn send(
        &self,
        packet: &[u8],
        context: &LayerContext,
        next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        self.send_owned(
            PacketBuffer::from_slice(packet, IPV6_ETHERNET_HEADROOM)?,
            context,
            next_layers,
        )
    }

    fn send_owned(
        &self,
        mut packet: PacketBuffer,
        context: &LayerContext,
        next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        let destination = Self::payload_ip6(context, "ipv6_dst")?;
        if destination.is_unspecified() || destination.is_multicast() {
            return Err(SocketError::InvalidAddress);
        }
        let next_header = context
            .get("ip_protocol")
            .filter(|protocol| protocol.len() == 1)
            .map(|protocol| protocol[0])
            .ok_or(SocketError::InvalidPacket)?;
        if is_unsupported_extension_header(next_header) {
            return Err(SocketError::ProtocolNotSupported);
        }
        if self.default_hop_limit == 0
            || (next_header == protocol::NO_NEXT_HEADER && !packet.as_slice().is_empty())
            || context
                .get("ip_version")
                .is_some_and(|version| version != &[6])
        {
            return Err(SocketError::InvalidPacket);
        }

        let explicit_source = context
            .get("ipv6_src")
            .map(|bytes| {
                if bytes.len() != 16 {
                    return Err(SocketError::InvalidAddress);
                }
                let mut address = [0; 16];
                address.copy_from_slice(bytes);
                Ok(Ipv6Address::from_bytes(address))
            })
            .transpose()?;
        let explicit_interface = context
            .get("interface")
            .and_then(|value| core::str::from_utf8(value).ok())
            .map(Cow::Borrowed);
        if destination.is_link_local() && explicit_interface.is_none() {
            return Err(SocketError::InvalidAddress);
        }

        if self.is_local_address(destination) {
            let source = explicit_source.unwrap_or_else(|| {
                if destination.is_loopback() {
                    Ipv6Address::LOOPBACK
                } else {
                    explicit_interface
                        .as_deref()
                        .and_then(|interface| self.get_primary_address(interface))
                        .unwrap_or(destination)
                }
            });
            self.deliver_local(
                packet.as_slice(),
                source,
                destination,
                next_header,
                Some(context),
            )?;
            let mut stats = self.stats.write();
            stats.packets_sent += 1;
            stats.bytes_sent += (packet.len() + IPV6_HEADER_LEN) as u64;
            return Ok(());
        }

        let (interface_name, source, gateway) = match explicit_source {
            Some(source) => {
                let interface = explicit_interface
                    .or_else(|| self.interface_for_address(source).map(Cow::Owned))
                    .ok_or(SocketError::NoRoute)?;
                let gateway = self
                    .next_hop_on_interface(destination, &interface)
                    .ok_or(SocketError::NoRoute)?;
                (interface, source, gateway)
            }
            None => {
                let route = self.select_route(destination).ok_or(SocketError::NoRoute)?;
                (Cow::Owned(route.interface), route.source, route.gateway)
            }
        };

        if source.is_loopback()
            || source.is_multicast()
            || source.is_unspecified()
            || source.is_ipv4_mapped()
            || !self.is_address_on_interface(interface_name.as_ref(), source)
        {
            return Err(SocketError::InvalidAddress);
        }

        let eth_destination = context
            .get("eth_dst_mac")
            .filter(|mac| mac.len() >= 6)
            .ok_or(SocketError::NoRoute)?;

        if let Some(interface) = get_network_manager().get_interface(&interface_name) {
            let mtu = usize::try_from(interface.mtu()).unwrap_or(0);
            let required = packet
                .len()
                .checked_add(IPV6_HEADER_LEN)
                .ok_or(SocketError::MessageTooLarge)?;
            if mtu < IPV6_MIN_MTU || required > mtu {
                return Err(SocketError::MessageTooLarge);
            }
        } else if next_layers.is_empty() {
            return Err(SocketError::NoRoute);
        }

        let payload_length: u16 =
            u16::try_from(packet.len()).map_err(|_| SocketError::MessageTooLarge)?;
        let mut header = Ipv6Header::new(
            payload_length,
            next_header,
            self.default_hop_limit,
            source,
            destination,
        );
        header.traffic_class = match context.get("ip_traffic_class") {
            None => 0,
            Some(value) if value.len() == 1 => value[0],
            Some(_) => return Err(SocketError::InvalidPacket),
        };
        header.flow_label = match context.get("ip_flow_label") {
            None => 0,
            Some(value) => {
                let value = <[u8; 4]>::try_from(value).map_err(|_| SocketError::InvalidPacket)?;
                u32::from_be_bytes(value)
            }
        };
        let encoded_header = header.to_bytes().map_err(|_| SocketError::InvalidPacket)?;
        packet.prepend(&encoded_header)?;
        let packet_size = packet.len();

        if !self.admit_traffic(
            &interface_name,
            super::traffic::Direction::Transmit,
            packet_size,
        ) {
            return Err(SocketError::PermissionDenied);
        }
        if let Some(budget) = context.traffic_budget() {
            packet.reserve_traffic(budget)?;
        }

        let mut lower_context = LayerContext::new();
        if let Some(budget) = context.traffic_budget() {
            lower_context.set_traffic_budget(budget.clone());
        }
        lower_context.set("eth_type", &super::ethernet::ether_type::IPV6.to_be_bytes());
        lower_context.set("eth_dst_mac", &eth_destination[..6]);
        lower_context.set("interface", interface_name.as_bytes());
        lower_context.set("ipv6_src", &source.as_bytes());
        lower_context.set("ipv6_dst", &destination.as_bytes());
        lower_context.set("ip_version", &[6]);
        lower_context.set("ipv6_next_hop", &gateway.unwrap_or(destination).as_bytes());

        if let Some(layer) = next_layers.first() {
            layer.send_owned(packet, &lower_context, &next_layers[1..])?;
        } else if let Some(ethernet) = get_network_manager().get_layer("ethernet") {
            ethernet.send_owned(packet, &lower_context, &[])?;
        } else {
            return Err(SocketError::NoRoute);
        }

        let mut stats = self.stats.write();
        stats.packets_sent += 1;
        stats.bytes_sent += packet_size as u64;
        Ok(())
    }

    fn receive(&self, packet: &[u8], context: Option<&LayerContext>) -> Result<(), SocketError> {
        let parsed = match Ipv6Header::decode(packet) {
            Ok(parsed) => parsed,
            Err(_) => {
                self.stats.write().protocol_errors += 1;
                return Err(SocketError::InvalidPacket);
            }
        };
        let header = parsed.header;
        let total_length = IPV6_HEADER_LEN + usize::from(header.payload_length);

        if header.hop_limit == 0 {
            self.stats.write().protocol_errors += 1;
            return Err(SocketError::InvalidPacket);
        }

        if header.source.is_multicast()
            || header.source.is_unspecified()
            || header.source.is_loopback()
            || header.source.is_ipv4_mapped()
            || header.destination.is_unspecified()
            || header.destination.is_loopback()
            || header.destination.is_ipv4_mapped()
        {
            self.stats.write().protocol_errors += 1;
            return Err(SocketError::InvalidAddress);
        }
        if is_unsupported_extension_header(header.next_header) {
            self.stats.write().protocol_errors += 1;
            return Err(SocketError::ProtocolNotSupported);
        }
        if header.destination.is_multicast() {
            return Err(SocketError::ProtocolNotSupported);
        }

        let interface = context
            .and_then(|context| context.get("interface"))
            .and_then(|value| core::str::from_utf8(value).ok());
        if (header.source.is_link_local() || header.destination.is_link_local())
            && interface.is_none()
        {
            return Err(SocketError::AddressNotAvailable);
        }
        if self.has_ingress_policy() && interface.is_none() {
            return Err(SocketError::PermissionDenied);
        }
        if let Some(interface) = interface
            && !self.admit_traffic(interface, super::traffic::Direction::Receive, total_length)
        {
            return Err(SocketError::PermissionDenied);
        }

        if !self.is_local_address(header.destination) {
            return Err(SocketError::NoRoute);
        }
        if interface
            .is_some_and(|interface| !self.is_address_on_interface(interface, header.destination))
        {
            return Err(SocketError::AddressNotAvailable);
        }

        {
            let mut stats = self.stats.write();
            stats.packets_received += 1;
            stats.bytes_received += total_length as u64;
        }
        if header.next_header == protocol::NO_NEXT_HEADER {
            return if parsed.payload.is_empty() {
                Ok(())
            } else {
                Err(SocketError::InvalidPacket)
            };
        }
        self.dispatch_local(parsed.payload, &header, context)
    }

    fn name(&self) -> &'static str {
        "IPv6"
    }

    fn stats(&self) -> NetworkLayerStats {
        self.stats.read().clone()
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use core::sync::atomic::{AtomicUsize, Ordering};

    use crate::sync::IrqSpinLock;

    use super::*;

    fn address(value: u16) -> Ipv6Address {
        Ipv6Address::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, value)
    }

    #[test_case]
    fn base_header_uses_strict_length_and_splits_link_padding() {
        let header = Ipv6Header::new(4, protocol::UDP, 63, address(1), address(2));
        let mut packet = Vec::new();
        packet.extend_from_slice(&header.to_bytes().unwrap());
        packet.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]);
        packet.extend_from_slice(&[0; 10]);

        let parsed = Ipv6Header::decode(&packet).unwrap();
        assert_eq!(parsed.header, header);
        assert_eq!(parsed.payload, &[0xaa, 0xbb, 0xcc, 0xdd]);
        assert_eq!(parsed.trailing, &[0; 10]);
        assert_eq!(
            Ipv6Header::decode(&packet[..IPV6_HEADER_LEN - 1]),
            Err(Ipv6HeaderError::PacketTooShort)
        );

        packet[0] = 0x50;
        assert_eq!(
            Ipv6Header::decode(&packet),
            Err(Ipv6HeaderError::InvalidVersion(5))
        );
        packet[0] = 0x60;
        packet[5] = 99;
        assert_eq!(
            Ipv6Header::decode(&packet),
            Err(Ipv6HeaderError::TruncatedPayload {
                declared: 99,
                available: 14,
            })
        );
    }

    #[test_case]
    fn base_header_encoding_validates_flow_label_and_fields() {
        let mut header = Ipv6Header::new(8, protocol::TCP, 64, address(1), address(2));
        header.traffic_class = 0xab;
        header.flow_label = 0x12345;
        let bytes = header.to_bytes().unwrap();
        assert_eq!(&bytes[..4], &[0x6a, 0xb1, 0x23, 0x45]);
        assert_eq!(&bytes[4..6], &[0, 8]);
        assert_eq!(bytes[6], protocol::TCP);
        assert_eq!(bytes[7], 64);

        header.flow_label = 0x0010_0000;
        assert_eq!(
            header.to_bytes(),
            Err(Ipv6HeaderError::FlowLabelOutOfRange(0x0010_0000))
        );
    }

    #[test_case]
    fn extension_and_fragment_headers_are_classified_for_rejection() {
        assert!(is_unsupported_extension_header(protocol::HOP_BY_HOP));
        assert!(is_unsupported_extension_header(protocol::ROUTING));
        assert!(is_unsupported_extension_header(protocol::FRAGMENT));
        assert!(is_unsupported_extension_header(
            protocol::DESTINATION_OPTIONS
        ));
        assert!(!is_unsupported_extension_header(protocol::TCP));
        assert!(!is_unsupported_extension_header(protocol::UDP));
        assert!(!is_unsupported_extension_header(protocol::ICMPV6));
    }

    #[test_case]
    fn pseudo_header_checksum_matches_a_known_vector() {
        let checksum = pseudo_header_checksum(
            address(1),
            address(2),
            protocol::UDP,
            &[0x00, 0x01, 0x00, 0x02],
        )
        .unwrap();
        assert_eq!(checksum, 0xa472);

        let mut carried = [0x00, 0x00, 0x00, 0x02];
        let checksum =
            pseudo_header_checksum(address(1), address(2), protocol::UDP, &carried).unwrap();
        carried[..2].copy_from_slice(&checksum.to_be_bytes());
        assert!(
            verify_pseudo_header_checksum(address(1), address(2), protocol::UDP, &carried,)
                .unwrap()
        );
        assert_eq!(
            pseudo_header_checksum(
                Ipv6Address::UNSPECIFIED,
                Ipv6Address::UNSPECIFIED,
                protocol::NO_NEXT_HEADER,
                &[0xff, 0xc2],
            )
            .unwrap(),
            0xffff
        );
    }

    #[test_case]
    fn routing_uses_longest_prefix_then_metric_without_hiding_connected_routes() {
        let layer = Ipv6Layer::new();
        layer
            .add_address("eth0", Ipv6AddressInfo::new(address(2), 64, true).unwrap())
            .unwrap();
        layer.set_default_route(address(9), "eth0", 100).unwrap();

        let direct = layer.select_route(address(42)).unwrap();
        assert_eq!(direct.interface, "eth0");
        assert_eq!(direct.source, address(2));
        assert_eq!(direct.next_hop, address(42));
        assert_eq!(direct.gateway, None);
        assert_eq!(direct.prefix_len, 64);

        let routed = layer
            .select_route(Ipv6Address::new(0x2001, 0xdb9, 0, 0, 0, 0, 0, 1))
            .unwrap();
        assert_eq!(routed.interface, "eth0");
        assert_eq!(routed.source, address(2));
        assert_eq!(routed.next_hop, address(9));
        assert_eq!(routed.gateway, Some(address(9)));
        assert_eq!(routed.prefix_len, 0);
        assert_eq!(layer.select_route(Ipv6Address::LOOPBACK), None);
    }

    #[test_case]
    fn routing_prefers_a_more_specific_route_and_lower_metric() {
        let layer = Ipv6Layer::new();
        for (interface, last) in [("eth0", 2), ("eth1", 3)] {
            layer
                .add_address(
                    interface,
                    Ipv6AddressInfo::new(address(last), 64, true).unwrap(),
                )
                .unwrap();
        }
        layer.set_default_route(address(9), "eth0", 100).unwrap();
        layer.set_default_route(address(10), "eth1", 10).unwrap();
        assert_eq!(
            layer
                .select_route(Ipv6Address::new(0x3000, 0, 0, 0, 0, 0, 0, 1))
                .unwrap()
                .interface,
            "eth1"
        );

        layer.add_route(
            Ipv6RouteEntry::new(
                Ipv6Address::new(0x2001, 0xdb8, 0, 1, 0, 0, 0, 0),
                64,
                Some(address(9)),
                "eth0",
                500,
            )
            .unwrap(),
        );
        let selected = layer
            .select_route(Ipv6Address::new(0x2001, 0xdb8, 0, 1, 0, 0, 0, 8))
            .unwrap();
        assert_eq!(selected.interface, "eth0");
        assert_eq!(selected.source, address(2));
        assert_eq!(selected.prefix_len, 64);
    }

    #[test_case]
    fn connected_source_prefers_primary_and_address_configuration_rejects_unusable_values() {
        let layer = Ipv6Layer::new();
        layer
            .add_address("eth0", Ipv6AddressInfo::new(address(3), 64, false).unwrap())
            .unwrap();
        layer
            .add_address("eth0", Ipv6AddressInfo::new(address(2), 64, true).unwrap())
            .unwrap();

        assert_eq!(layer.select_route(address(42)).unwrap().source, address(2));
        for address in [
            Ipv6Address::UNSPECIFIED,
            Ipv6Address::LOOPBACK,
            Ipv6Address::new(0xff02, 0, 0, 0, 0, 0, 0, 1),
            Ipv6Address::from_ipv4_mapped(super::super::ipv4::Ipv4Address::new(192, 0, 2, 1)),
        ] {
            assert_eq!(
                Ipv6AddressInfo::new(address, 64, false),
                Err(Ipv6ConfigError::UnusableAddress)
            );
        }
    }

    #[test_case]
    fn route_without_source_does_not_shadow_a_connected_route() {
        let layer = Ipv6Layer::new();
        layer
            .add_address("eth0", Ipv6AddressInfo::new(address(2), 48, true).unwrap())
            .unwrap();
        layer.add_route(Ipv6RouteEntry::new(address(0), 96, Some(address(9)), "eth1", 0).unwrap());

        let selected = layer.select_route(address(42)).unwrap();
        assert_eq!(selected.interface, "eth0");
        assert_eq!(selected.source, address(2));
        assert_eq!(selected.prefix_len, 48);
    }

    struct CountHandler {
        packets: AtomicUsize,
        interface: IrqSpinLock<Option<String>>,
        ipv6_context: IrqSpinLock<bool>,
    }

    impl CountHandler {
        fn new() -> Self {
            Self {
                packets: AtomicUsize::new(0),
                interface: IrqSpinLock::new(None),
                ipv6_context: IrqSpinLock::new(false),
            }
        }
    }

    impl NetworkLayer for CountHandler {
        fn register_protocol(&self, _: u16, _: Arc<dyn NetworkLayer>) {}

        fn send(
            &self,
            _: &[u8],
            _: &LayerContext,
            _: &[Arc<dyn NetworkLayer>],
        ) -> Result<(), SocketError> {
            Ok(())
        }

        fn receive(&self, _: &[u8], context: Option<&LayerContext>) -> Result<(), SocketError> {
            self.packets.fetch_add(1, Ordering::SeqCst);
            *self.interface.lock() = context
                .and_then(|context| context.get("interface"))
                .and_then(|value| core::str::from_utf8(value).ok())
                .map(ToString::to_string);
            *self.ipv6_context.lock() = context.is_some_and(|context| {
                context.get("ipv6_src").is_some()
                    && context.get("ipv6_dst").is_some()
                    && context.get("ip_src").is_none()
                    && context.get("ip_dst").is_none()
            });
            Ok(())
        }

        fn name(&self) -> &'static str {
            "count"
        }

        fn as_any(&self) -> &dyn core::any::Any {
            self
        }
    }

    #[test_case]
    fn loopback_injection_reaches_a_registered_protocol_handler() {
        let layer = Ipv6Layer::new();
        let handler = Arc::new(CountHandler::new());
        layer.register_protocol(u16::from(protocol::UDP), handler.clone());
        let mut context = LayerContext::new();
        context.set("interface", b"lo");

        layer
            .deliver_local(
                &[1, 2, 3],
                Ipv6Address::LOOPBACK,
                Ipv6Address::LOOPBACK,
                protocol::UDP,
                Some(&context),
            )
            .unwrap();
        assert_eq!(handler.packets.load(Ordering::SeqCst), 1);
        assert_eq!(handler.interface.lock().as_deref(), Some("lo"));
        assert!(*handler.ipv6_context.lock());
    }

    #[test_case]
    fn loopback_never_arrives_as_wire_ingress() {
        let layer = Ipv6Layer::new();
        let handler = Arc::new(CountHandler::new());
        layer.register_protocol(u16::from(protocol::UDP), handler.clone());
        let header = Ipv6Header::new(3, protocol::UDP, 64, address(1), Ipv6Address::LOOPBACK);
        let mut packet = Vec::new();
        packet.extend_from_slice(&header.to_bytes().unwrap());
        packet.extend_from_slice(&[1, 2, 3]);

        assert_eq!(
            layer.receive(&packet, None),
            Err(SocketError::InvalidAddress)
        );
        assert_eq!(handler.packets.load(Ordering::SeqCst), 0);
    }

    #[test_case]
    fn no_next_header_accepts_only_an_empty_payload() {
        let layer = Ipv6Layer::new();
        layer
            .add_address("eth0", Ipv6AddressInfo::new(address(2), 64, true).unwrap())
            .unwrap();
        let mut context = LayerContext::new();
        context.set("interface", b"eth0");

        let empty = Ipv6Header::new(0, protocol::NO_NEXT_HEADER, 64, address(1), address(2));
        layer
            .receive(&empty.to_bytes().unwrap(), Some(&context))
            .unwrap();

        let expired = Ipv6Header::new(0, protocol::NO_NEXT_HEADER, 0, address(1), address(2));
        assert_eq!(
            layer.receive(&expired.to_bytes().unwrap(), Some(&context)),
            Err(SocketError::InvalidPacket)
        );

        let nonempty = Ipv6Header::new(1, protocol::NO_NEXT_HEADER, 64, address(1), address(2));
        let mut packet = Vec::new();
        packet.extend_from_slice(&nonempty.to_bytes().unwrap());
        packet.push(0);
        assert_eq!(
            layer.receive(&packet, Some(&context)),
            Err(SocketError::InvalidPacket)
        );
    }

    #[test_case]
    fn fragment_header_is_rejected_before_dispatch() {
        let layer = Ipv6Layer::new();
        layer
            .add_address("eth0", Ipv6AddressInfo::new(address(2), 64, true).unwrap())
            .unwrap();
        let handler = Arc::new(CountHandler::new());
        layer.register_protocol(u16::from(protocol::FRAGMENT), handler.clone());
        let header = Ipv6Header::new(8, protocol::FRAGMENT, 64, address(1), address(2));
        let mut packet = Vec::new();
        packet.extend_from_slice(&header.to_bytes().unwrap());
        packet.extend_from_slice(&[0; 8]);
        let mut context = LayerContext::new();
        context.set("interface", b"eth0");

        assert_eq!(
            layer.receive(&packet, Some(&context)),
            Err(SocketError::ProtocolNotSupported)
        );
        assert_eq!(handler.packets.load(Ordering::SeqCst), 0);
    }

    struct OwnedCapture(IrqSpinLock<Option<PacketBuffer>>);

    impl NetworkLayer for OwnedCapture {
        fn register_protocol(&self, _: u16, _: Arc<dyn NetworkLayer>) {}

        fn send(
            &self,
            _: &[u8],
            _: &LayerContext,
            _: &[Arc<dyn NetworkLayer>],
        ) -> Result<(), SocketError> {
            panic!("IPv6 must forward the owned packet")
        }

        fn send_owned(
            &self,
            packet: PacketBuffer,
            context: &LayerContext,
            _: &[Arc<dyn NetworkLayer>],
        ) -> Result<(), SocketError> {
            assert_eq!(context.get("eth_type"), Some(&[0x86, 0xdd][..]));
            assert_eq!(
                context.get("ipv6_next_hop"),
                Some(&address(42).as_bytes()[..])
            );
            assert_eq!(context.get("next_hop"), None);
            assert_eq!(context.get("ip_src"), None);
            assert_eq!(context.get("ipv6_src"), Some(&address(2).as_bytes()[..]));
            *self.0.lock() = Some(packet);
            Ok(())
        }

        fn receive(&self, _: &[u8], _: Option<&LayerContext>) -> Result<(), SocketError> {
            Ok(())
        }

        fn name(&self) -> &'static str {
            "owned-capture"
        }

        fn as_any(&self) -> &dyn core::any::Any {
            self
        }
    }

    #[test_case]
    fn owned_send_preserves_link_token_and_wire_header() {
        let layer = Ipv6Layer::new();
        layer
            .add_address(
                "eth-test",
                Ipv6AddressInfo::new(address(2), 64, true).unwrap(),
            )
            .unwrap();
        layer.set_default_route(address(9), "eth-test", 0).unwrap();
        let capture = Arc::new(OwnedCapture(IrqSpinLock::new(None)));
        let mut packet = PacketBuffer::from_slice(&[0xa5; 32], IPV6_ETHERNET_HEADROOM).unwrap();
        packet.set_link_token(Some((7, 11)));
        let mut context = LayerContext::new();
        context.set("ipv6_dst", &address(42).as_bytes());
        context.set("ip_protocol", &[protocol::UDP]);
        context.set("eth_dst_mac", &[0x02, 0, 0, 0, 0, 1]);

        layer
            .send_owned(packet, &context, &[capture.clone()])
            .unwrap();
        let captured = capture.0.lock();
        let packet = captured.as_ref().unwrap();
        assert_eq!(packet.link_token(), Some((7, 11)));
        assert_eq!(packet.len(), IPV6_HEADER_LEN + 32);
        let parsed = Ipv6Header::decode(packet.as_slice()).unwrap();
        assert_eq!(parsed.header.next_header, protocol::UDP);
        assert_eq!(parsed.header.source, address(2));
        assert_eq!(parsed.header.destination, address(42));
        assert_eq!(parsed.payload, &[0xa5; 32]);
    }
}
