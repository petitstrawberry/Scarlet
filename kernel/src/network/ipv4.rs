//! IPv4 protocol layer
//!
//! This module provides IPv4 packet handling for the network stack.
//! It implements the NetworkLayer trait for IPv4 encapsulation/decapsulation.
//!
//! # Design
//!
//! The Ipv4Layer manages:
//! - Multiple IPv4 addresses per interface (primary + secondary)
//! - Routing table for destination-based forwarding
//! - Source IP selection based on routing decisions
//!
//! This design supports multiple network interfaces with multiple IP addresses each.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::RwLock;

use crate::early_println;
use crate::network::protocol_stack::{
    LayerContext, NetworkLayer, NetworkLayerStats, get_network_manager,
};
use crate::network::socket::SocketError;

/// IPv4 address
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ipv4Address(pub [u8; 4]);

impl Ipv4Address {
    /// Create a new IPv4 address
    pub fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }

    /// Create IPv4 address from bytes
    pub fn from_bytes(bytes: [u8; 4]) -> Self {
        Self(bytes)
    }

    /// Get address as bytes
    pub fn as_bytes(&self) -> [u8; 4] {
        self.0
    }

    /// Convert to big-endian u32
    pub fn to_u32_be(&self) -> u32 {
        u32::from_be_bytes(self.0)
    }

    /// Convert from big-endian u32
    pub fn from_u32_be(addr: u32) -> Self {
        Self(addr.to_be_bytes())
    }

    /// Check if this is a broadcast address (255.255.255.255)
    pub fn is_broadcast(&self) -> bool {
        self.0 == [255, 255, 255, 255]
    }

    /// Check if this is a loopback address (127.0.0.0/8)
    pub fn is_loopback(&self) -> bool {
        self.0[0] == 127
    }

    /// Check if this is the "any" address (0.0.0.0)
    pub fn is_any(&self) -> bool {
        self.0 == [0, 0, 0, 0]
    }
}

/// IPv4 header (minimum 20 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Ipv4Header {
    /// Version (4 bits) + IHL (4 bits)
    pub version_ihl: u8,
    /// Type of Service
    pub tos: u8,
    /// Total Length (16 bits)
    pub total_length: u16,
    /// Identification (16 bits)
    pub identification: u16,
    /// Flags (3 bits) + Fragment Offset (13 bits)
    pub flags_fragment: u16,
    /// Time to Live
    pub ttl: u8,
    /// Protocol (8 bits)
    pub protocol: u8,
    /// Header Checksum (16 bits)
    pub checksum: u16,
    /// Source IP (32 bits)
    pub source_ip: [u8; 4],
    /// Destination IP (32 bits)
    pub dest_ip: [u8; 4],
}

impl Ipv4Header {
    /// Create a new IPv4 header
    pub fn new() -> Self {
        Self {
            version_ihl: 0x45, // Version=4, IHL=5 (20 bytes)
            tos: 0,
            total_length: 0,
            identification: 0,
            flags_fragment: 0,
            ttl: 64,
            protocol: 0,
            checksum: 0,
            source_ip: [0, 0, 0, 0],
            dest_ip: [0, 0, 0, 0],
        }
    }

    /// Get IP version (always 4)
    pub fn version(&self) -> u8 {
        self.version_ihl >> 4
    }

    /// Get IHL (Internet Header Length) in 32-bit words
    pub fn ihl(&self) -> u8 {
        self.version_ihl & 0x0F
    }

    /// Get header length in bytes
    pub fn header_length(&self) -> usize {
        (self.ihl() as usize) * 4
    }

    /// Calculate checksum
    pub fn calculate_checksum(&self) -> u16 {
        let mut bytes = self.to_bytes();
        if bytes.len() >= 12 {
            bytes[10] = 0;
            bytes[11] = 0;
        }
        checksum_from_bytes(&bytes)
    }

    /// Serialize header to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(20);
        bytes.push(self.version_ihl);
        bytes.push(self.tos);
        bytes.extend_from_slice(&self.total_length.to_be_bytes());
        bytes.extend_from_slice(&self.identification.to_be_bytes());
        bytes.extend_from_slice(&self.flags_fragment.to_be_bytes());
        bytes.push(self.ttl);
        bytes.push(self.protocol);
        bytes.extend_from_slice(&self.checksum.to_be_bytes());
        bytes.extend_from_slice(&self.source_ip);
        bytes.extend_from_slice(&self.dest_ip);
        bytes
    }

    /// Parse header from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 20 {
            return None;
        }

        let version_ihl = bytes[0];
        let version = version_ihl >> 4;
        if version != 4 {
            return None;
        }

        let ihl = version_ihl & 0x0F;
        let header_len = (ihl as usize) * 4;
        if bytes.len() < header_len {
            return None;
        }

        Some(Self {
            version_ihl,
            tos: bytes[1],
            total_length: u16::from_be_bytes([bytes[2], bytes[3]]),
            identification: u16::from_be_bytes([bytes[4], bytes[5]]),
            flags_fragment: u16::from_be_bytes([bytes[6], bytes[7]]),
            ttl: bytes[8],
            protocol: bytes[9],
            checksum: u16::from_be_bytes([bytes[10], bytes[11]]),
            source_ip: [bytes[12], bytes[13], bytes[14], bytes[15]],
            dest_ip: [bytes[16], bytes[17], bytes[18], bytes[19]],
        })
    }
}

/// IPv4 protocol numbers
pub mod protocol {
    /// ICMP
    pub const ICMP: u8 = 1;
    /// TCP
    pub const TCP: u8 = 6;
    /// UDP
    pub const UDP: u8 = 17;
    /// IPv6 encapsulation
    pub const IPV6: u8 = 41;
}

/// IPv4 address information for an interface
#[derive(Debug, Clone)]
pub struct Ipv4AddressInfo {
    /// The IPv4 address
    pub address: Ipv4Address,
    /// Network mask
    pub netmask: Ipv4Address,
    /// Broadcast address (optional)
    pub broadcast: Option<Ipv4Address>,
    /// Whether this is the primary address for the interface
    pub is_primary: bool,
}

/// Routing table entry
#[derive(Debug, Clone)]
pub struct RouteEntry {
    /// Destination network
    pub destination: Ipv4Address,
    /// Network mask
    pub netmask: Ipv4Address,
    /// Gateway (None for directly connected networks)
    pub gateway: Option<Ipv4Address>,
    /// Outgoing interface name
    pub interface: String,
    /// Route metric (lower is preferred)
    pub metric: u32,
}

/// IPv4 layer
///
/// Handles IPv4 packet encapsulation and decapsulation.
/// Manages multiple addresses per interface and routing table.
pub struct Ipv4Layer {
    /// Interface name -> list of IPv4 addresses
    addresses: RwLock<BTreeMap<String, Vec<Ipv4AddressInfo>>>,
    /// Routing table (ordered by specificity)
    routing_table: RwLock<Vec<RouteEntry>>,
    /// Protocol handlers registered by protocol number
    protocols: RwLock<BTreeMap<u8, alloc::sync::Arc<dyn NetworkLayer>>>,
    /// Statistics
    stats: RwLock<NetworkLayerStats>,
    /// Default TTL
    default_ttl: u8,
}

impl Ipv4Layer {
    /// Create a new IPv4 layer
    pub fn new() -> alloc::sync::Arc<Self> {
        alloc::sync::Arc::new(Self {
            addresses: RwLock::new(BTreeMap::new()),
            routing_table: RwLock::new(Vec::new()),
            protocols: RwLock::new(BTreeMap::new()),
            stats: RwLock::new(NetworkLayerStats::default()),
            default_ttl: 64,
        })
    }

    /// Initialize and register the IPv4 layer with NetworkManager
    ///
    /// Registers with NetworkManager and registers itself with EthernetLayer
    /// for EtherType 0x0800 (IPv4).
    ///
    /// # Panics
    ///
    /// Panics if EthernetLayer is not registered (must be initialized first).
    pub fn init(network_manager: &crate::network::NetworkManager) {
        let layer = Self::new();
        network_manager.register_layer("ip", layer.clone());

        // Register with Ethernet layer for IPv4 packets (EtherType 0x0800)
        let ethernet = network_manager
            .get_layer("ethernet")
            .expect("EthernetLayer must be initialized before Ipv4Layer");
        ethernet.register_protocol(crate::network::ethernet::ether_type::IPV4, layer);
    }

    /// Add an IPv4 address to an interface
    pub fn add_address(&self, interface: &str, info: Ipv4AddressInfo) {
        let mut addrs = self.addresses.write();
        addrs
            .entry(interface.to_string())
            .or_insert_with(Vec::new)
            .push(info);
    }

    /// Remove an IPv4 address from an interface
    pub fn remove_address(&self, interface: &str, ip: Ipv4Address) {
        let mut addrs = self.addresses.write();
        if let Some(list) = addrs.get_mut(interface) {
            list.retain(|a| a.address != ip);
        }
    }

    /// Get all addresses for an interface
    pub fn get_addresses(&self, interface: &str) -> Vec<Ipv4AddressInfo> {
        self.addresses
            .read()
            .get(interface)
            .cloned()
            .unwrap_or_default()
    }

    /// Get primary IP address for an interface
    pub fn get_primary_ip(&self, interface: &str) -> Option<Ipv4Address> {
        self.addresses
            .read()
            .get(interface)?
            .iter()
            .find(|a| a.is_primary)
            .map(|a| a.address)
    }

    /// Add a route to the routing table
    pub fn add_route(&self, entry: RouteEntry) {
        let mut table = self.routing_table.write();
        table.push(entry);
        // Sort by netmask specificity (more specific routes first)
        table.sort_by(|a, b| {
            let a_bits = a.netmask.to_u32_be().count_ones();
            let b_bits = b.netmask.to_u32_be().count_ones();
            b_bits.cmp(&a_bits).then(a.metric.cmp(&b.metric))
        });
    }

    /// Remove a route from the routing table
    pub fn remove_route(&self, destination: Ipv4Address, netmask: Ipv4Address) {
        let mut table = self.routing_table.write();
        table.retain(|r| r.destination != destination || r.netmask != netmask);
    }

    /// Set default gateway
    pub fn set_default_gateway(&self, gateway: Ipv4Address, interface: &str) {
        self.add_route(RouteEntry {
            destination: Ipv4Address::new(0, 0, 0, 0),
            netmask: Ipv4Address::new(0, 0, 0, 0),
            gateway: Some(gateway),
            interface: interface.to_string(),
            metric: 100,
        });
    }

    /// Select source IP and interface for a destination
    ///
    /// Returns (interface_name, source_ip, optional_gateway)
    pub fn select_source(
        &self,
        dest: Ipv4Address,
    ) -> Option<(String, Ipv4Address, Option<Ipv4Address>)> {
        let table = self.routing_table.read();

        // Find matching route
        for route in table.iter() {
            if self.ip_matches_route(dest, route) {
                if let Some(src_ip) = self.get_primary_ip(&route.interface) {
                    return Some((route.interface.clone(), src_ip, route.gateway));
                }
            }
        }

        // Fallback: check if destination is on a directly connected network
        let addrs = self.addresses.read();
        for (iface, ips) in addrs.iter() {
            for ip_info in ips {
                if self.same_subnet(dest, ip_info.address, ip_info.netmask) {
                    return Some((iface.clone(), ip_info.address, None));
                }
            }
        }

        // Last resort: use any available primary IP
        for (iface, ips) in addrs.iter() {
            if let Some(primary) = ips.iter().find(|a| a.is_primary) {
                return Some((iface.clone(), primary.address, None));
            }
        }

        None
    }

    /// Check if an IP matches a route
    fn ip_matches_route(&self, ip: Ipv4Address, route: &RouteEntry) -> bool {
        self.same_subnet(ip, route.destination, route.netmask)
    }

    /// Check if two IPs are in the same subnet
    fn same_subnet(&self, ip1: Ipv4Address, ip2: Ipv4Address, mask: Ipv4Address) -> bool {
        let ip1_u32 = ip1.to_u32_be();
        let ip2_u32 = ip2.to_u32_be();
        let mask_u32 = mask.to_u32_be();
        (ip1_u32 & mask_u32) == (ip2_u32 & mask_u32)
    }

    /// Check if an IP is local (assigned to any interface)
    pub fn is_local_ip(&self, ip: Ipv4Address) -> bool {
        self.addresses
            .read()
            .values()
            .any(|ips| ips.iter().any(|a| a.address == ip))
    }

    /// Get protocol handler for a protocol number
    pub fn get_protocol_handler(
        &self,
        proto_num: u8,
    ) -> Option<alloc::sync::Arc<dyn NetworkLayer>> {
        self.protocols.read().get(&proto_num).cloned()
    }
}

impl NetworkLayer for Ipv4Layer {
    fn register_protocol(&self, proto_num: u16, handler: alloc::sync::Arc<dyn NetworkLayer>) {
        self.protocols.write().insert(proto_num as u8, handler);
    }

    fn send(
        &self,
        packet: &[u8],
        context: &LayerContext,
        next_layers: &[alloc::sync::Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // Get destination IP from context (required)
        let dest_ip_bytes = context
            .get("ip_dst")
            .and_then(|ip| {
                if ip.len() >= 4 {
                    Some([ip[0], ip[1], ip[2], ip[3]])
                } else {
                    None
                }
            })
            .ok_or(SocketError::InvalidPacket)?;
        let dest_ip = Ipv4Address::from_bytes(dest_ip_bytes);

        // Get protocol number from context
        let protocol = context
            .get("ip_protocol")
            .and_then(|p| if !p.is_empty() { Some(p[0]) } else { None })
            .unwrap_or(protocol::TCP);

        // Get source IP from context, or select based on routing
        let (interface_name, src_ip_bytes, gateway) = if let Some(ip_src) = context.get("ip_src") {
            if ip_src.len() >= 4 {
                // Source IP explicitly set - still need to check routing for gateway
                let iface = context
                    .get("interface")
                    .and_then(|b| core::str::from_utf8(b).ok())
                    .map(String::from)
                    .or_else(|| {
                        get_network_manager()
                            .get_default_interface()
                            .map(|i| String::from(i.name()))
                    })
                    .ok_or(SocketError::NoRoute)?;

                // Look up gateway from routing table for this destination
                let gateway = self.select_source(dest_ip).and_then(|(_, _, gw)| gw);

                (iface, [ip_src[0], ip_src[1], ip_src[2], ip_src[3]], gateway)
            } else {
                return Err(SocketError::InvalidAddress);
            }
        } else {
            // Select source IP based on routing table
            let (iface, src_ip, gw) = self.select_source(dest_ip).ok_or(SocketError::NoRoute)?;
            (iface, src_ip.0, gw)
        };

        // Build IPv4 header
        let mut header = Ipv4Header::new();
        header.source_ip = src_ip_bytes;
        header.dest_ip = dest_ip_bytes;
        header.protocol = protocol;
        header.ttl = self.default_ttl;

        // Calculate total length (header + packet)
        let total_length = (20 + packet.len()) as u16;
        header.total_length = total_length;

        // Calculate and set checksum
        header.checksum = header.calculate_checksum();

        // Serialize header
        let mut ip_packet = header.to_bytes();

        // Create IP packet: header + payload
        ip_packet.extend_from_slice(packet);

        early_println!(
            "[IPv4] Send: {} bytes (src: {}.{}.{}.{}, dst: {}.{}.{}.{}, proto: {}, iface: {})",
            ip_packet.len(),
            src_ip_bytes[0],
            src_ip_bytes[1],
            src_ip_bytes[2],
            src_ip_bytes[3],
            dest_ip_bytes[0],
            dest_ip_bytes[1],
            dest_ip_bytes[2],
            dest_ip_bytes[3],
            protocol,
            interface_name
        );

        // Prepare context for Ethernet layer
        let mut eth_context = context.clone();
        eth_context.set(
            "eth_type",
            &crate::network::ethernet::ether_type::IPV4.to_be_bytes(),
        );
        eth_context.set("interface", interface_name.as_bytes());
        eth_context.set("ip_src", &src_ip_bytes);

        // If we have a gateway, set that as the next-hop for ARP resolution
        if let Some(gw) = gateway {
            eth_context.set("next_hop", &gw.0);
        } else {
            eth_context.set("next_hop", &dest_ip_bytes);
        }

        // Forward to Ethernet layer
        if !next_layers.is_empty() {
            next_layers[0].send(&ip_packet, &eth_context, &next_layers[1..])?;
        } else if let Some(eth_layer) = get_network_manager().get_layer("ethernet") {
            eth_layer.send(&ip_packet, &eth_context, &[])?;
        }

        // Update statistics
        let mut stats = self.stats.write();
        stats.packets_sent += 1;
        stats.bytes_sent += ip_packet.len() as u64;

        Ok(())
    }

    fn receive(&self, packet: &[u8], _context: Option<&LayerContext>) -> Result<(), SocketError> {
        // Parse IPv4 header
        let header = Ipv4Header::from_bytes(packet).ok_or(SocketError::InvalidPacket)?;

        let header_len = header.header_length();
        let total_length = usize::from(header.total_length);

        if packet.len() < header_len {
            return Err(SocketError::InvalidPacket);
        }
        if total_length < header_len || total_length > packet.len() {
            return Err(SocketError::InvalidPacket);
        }

        early_println!(
            "[IPv4] RX: total_len={} src={}.{}.{}.{} dst={}.{}.{}.{} proto={}",
            total_length,
            header.source_ip[0],
            header.source_ip[1],
            header.source_ip[2],
            header.source_ip[3],
            header.dest_ip[0],
            header.dest_ip[1],
            header.dest_ip[2],
            header.dest_ip[3],
            header.protocol
        );

        // Verify checksum (header.checksum is already in host order)
        let calculated_checksum = checksum_from_bytes(&packet[..header_len]);
        let header_checksum = unsafe { core::ptr::addr_of!(header.checksum).read_unaligned() };
        if calculated_checksum != header_checksum {
            early_println!(
                "[IPv4] Checksum mismatch: calculated=0x{:04X}, header=0x{:04X}",
                calculated_checksum,
                header_checksum
            );
            let mut stats = self.stats.write();
            stats.protocol_errors += 1;
            return Err(SocketError::InvalidPacket);
        }

        let payload = &packet[header_len..total_length];

        early_println!(
            "[IPv4] Recv: {} bytes (src: {}.{}.{}.{}, dst: {}.{}.{}.{}, proto: {})",
            packet.len(),
            header.source_ip[0],
            header.source_ip[1],
            header.source_ip[2],
            header.source_ip[3],
            header.dest_ip[0],
            header.dest_ip[1],
            header.dest_ip[2],
            header.dest_ip[3],
            header.protocol
        );

        // Update statistics
        let mut stats = self.stats.write();
        stats.packets_received += 1;
        stats.bytes_received += total_length as u64;

        // Route to protocol handler based on protocol field
        let protocols = self.protocols.read();
        if let Some(handler) = protocols.get(&header.protocol) {
            let mut proto_context = LayerContext::new();
            proto_context.set("ip_src", &header.source_ip);
            proto_context.set("ip_dst", &header.dest_ip);
            handler.receive(payload, Some(&proto_context))
        } else {
            // No handler for this protocol - log and drop
            Err(SocketError::ProtocolNotSupported)
        }
    }

    fn name(&self) -> &'static str {
        "IPv4"
    }

    fn stats(&self) -> NetworkLayerStats {
        self.stats.read().clone()
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

fn checksum_from_bytes(header_bytes: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;

    while i + 1 < header_bytes.len() {
        if i == 10 {
            i += 2;
            continue;
        }
        let word = u16::from_be_bytes([header_bytes[i], header_bytes[i + 1]]);
        sum += word as u32;
        i += 2;
    }

    if i < header_bytes.len() {
        let word = u16::from_be_bytes([header_bytes[i], 0]);
        sum += word as u32;
    }

    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !sum as u16
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;

    #[test_case]
    fn test_ipv4_address() {
        let addr = Ipv4Address::new(192, 168, 1, 100);
        assert_eq!(addr.as_bytes(), [192, 168, 1, 100]);
        assert!(!addr.is_broadcast());
        assert!(!addr.is_loopback());
        assert!(!addr.is_any());

        let broadcast = Ipv4Address::new(255, 255, 255, 255);
        assert!(broadcast.is_broadcast());

        let loopback = Ipv4Address::new(127, 0, 0, 1);
        assert!(loopback.is_loopback());

        let any = Ipv4Address::new(0, 0, 0, 0);
        assert!(any.is_any());
    }

    #[test_case]
    fn test_ipv4_address_u32_conversion() {
        let addr = Ipv4Address::new(192, 168, 1, 100);
        assert_eq!(addr.to_u32_be(), u32::from_be_bytes([192, 168, 1, 100]));

        let from_u32 = Ipv4Address::from_u32_be(0xC0A80164u32);
        assert_eq!(from_u32, addr);
    }

    #[test_case]
    fn test_ipv4_header_creation() {
        let mut header = Ipv4Header::new();
        header.source_ip = [192, 168, 1, 100];
        header.dest_ip = [192, 168, 1, 1];
        header.protocol = protocol::TCP;
        header.total_length = (20 + 10) as u16;

        assert_eq!(header.version(), 4);
        assert_eq!(header.ihl(), 5);
        assert_eq!(header.header_length(), 20);
        assert_eq!(header.protocol, protocol::TCP);
    }

    #[test_case]
    fn test_ipv4_header_serialization() {
        let mut header = Ipv4Header::new();
        header.source_ip = [192, 168, 1, 100];
        header.dest_ip = [192, 168, 1, 1];
        header.protocol = protocol::TCP;
        header.total_length = 30;

        let bytes = header.to_bytes();
        assert_eq!(bytes.len(), 20);
        assert_eq!(bytes[0], 0x45); // Version=4, IHL=5
        assert_eq!(u16::from_be_bytes([bytes[2], bytes[3]]), 30);
        assert_eq!(&bytes[12..16], [192, 168, 1, 100]);
        assert_eq!(&bytes[16..20], [192, 168, 1, 1]);
    }

    #[test_case]
    fn test_ipv4_header_parsing() {
        let mut bytes = vec![
            0x45, // Version=4, IHL=5
            0x00, // TOS
            0x00, 0x1E, // Total length = 30
            0x00, 0x01, // Identification
            0x00, 0x00, // Flags+Fragment
            0x40, // TTL = 64
            0x06, // Protocol = TCP
            0x00, 0x00, // Checksum (placeholder)
            0xC0, 0xA8, 0x01, 0x64, // Source IP = 192.168.1.100
            0xC0, 0xA8, 0x01, 0x01, // Dest IP = 192.168.1.1
        ];

        let header = Ipv4Header::from_bytes(&bytes).unwrap();
        assert_eq!(header.version(), 4);
        assert_eq!(header.ihl(), 5);
        let total_length = unsafe { core::ptr::addr_of!(header.total_length).read_unaligned() };
        assert_eq!(total_length, 30);
        assert_eq!(header.protocol, protocol::TCP);
        assert_eq!(header.source_ip, [192, 168, 1, 100]);
        assert_eq!(header.dest_ip, [192, 168, 1, 1]);
        assert_eq!(header.ttl, 64);
    }

    #[test_case]
    fn test_ipv4_header_invalid_version() {
        let mut bytes = alloc::vec![0x55u8; 20]; // Invalid version (5)
        assert!(Ipv4Header::from_bytes(&bytes).is_none());
    }

    #[test_case]
    fn test_ipv4_header_too_short() {
        let bytes = [0u8; 10];
        assert!(Ipv4Header::from_bytes(&bytes).is_none());
    }

    #[test_case]
    fn test_ipv4_layer_creation() {
        let ip_layer = Ipv4Layer::new();
        // New layer has no addresses
        assert!(ip_layer.get_addresses("eth0").is_empty());
    }

    #[test_case]
    fn test_ipv4_layer_add_address() {
        let ip_layer = Ipv4Layer::new();

        let ip = Ipv4Address::new(192, 168, 1, 100);
        ip_layer.add_address(
            "eth0",
            Ipv4AddressInfo {
                address: ip,
                netmask: Ipv4Address::new(255, 255, 255, 0),
                broadcast: Some(Ipv4Address::new(192, 168, 1, 255)),
                is_primary: true,
            },
        );

        let addrs = ip_layer.get_addresses("eth0");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].address, ip);
        assert!(addrs[0].is_primary);
    }

    #[test_case]
    fn test_ipv4_layer_multiple_addresses() {
        let ip_layer = Ipv4Layer::new();

        // Add primary address
        ip_layer.add_address(
            "eth0",
            Ipv4AddressInfo {
                address: Ipv4Address::new(192, 168, 1, 100),
                netmask: Ipv4Address::new(255, 255, 255, 0),
                broadcast: None,
                is_primary: true,
            },
        );

        // Add secondary address
        ip_layer.add_address(
            "eth0",
            Ipv4AddressInfo {
                address: Ipv4Address::new(192, 168, 1, 101),
                netmask: Ipv4Address::new(255, 255, 255, 0),
                broadcast: None,
                is_primary: false,
            },
        );

        let addrs = ip_layer.get_addresses("eth0");
        assert_eq!(addrs.len(), 2);
        assert_eq!(
            ip_layer.get_primary_ip("eth0"),
            Some(Ipv4Address::new(192, 168, 1, 100))
        );
    }

    #[test_case]
    fn test_ipv4_layer_routing() {
        let ip_layer = Ipv4Layer::new();

        // Add address to eth0
        ip_layer.add_address(
            "eth0",
            Ipv4AddressInfo {
                address: Ipv4Address::new(192, 168, 1, 100),
                netmask: Ipv4Address::new(255, 255, 255, 0),
                broadcast: None,
                is_primary: true,
            },
        );

        // Add route for local subnet
        ip_layer.add_route(RouteEntry {
            destination: Ipv4Address::new(192, 168, 1, 0),
            netmask: Ipv4Address::new(255, 255, 255, 0),
            gateway: None,
            interface: "eth0".to_string(),
            metric: 0,
        });

        // Add default route
        ip_layer.set_default_gateway(Ipv4Address::new(192, 168, 1, 1), "eth0");

        // Test routing to local subnet - should use direct route
        let result = ip_layer.select_source(Ipv4Address::new(192, 168, 1, 50));
        assert!(result.is_some());
        let (iface, src_ip, gw) = result.unwrap();
        assert_eq!(iface, "eth0");
        assert_eq!(src_ip, Ipv4Address::new(192, 168, 1, 100));
        assert!(gw.is_none()); // Direct route, no gateway

        // Test routing to external address - should use default gateway
        let result = ip_layer.select_source(Ipv4Address::new(8, 8, 8, 8));
        assert!(result.is_some());
        let (iface, src_ip, gw) = result.unwrap();
        assert_eq!(iface, "eth0");
        assert_eq!(src_ip, Ipv4Address::new(192, 168, 1, 100));
        assert_eq!(gw, Some(Ipv4Address::new(192, 168, 1, 1)));
    }

    #[test_case]
    fn test_ipv4_is_local_ip() {
        let ip_layer = Ipv4Layer::new();

        ip_layer.add_address(
            "eth0",
            Ipv4AddressInfo {
                address: Ipv4Address::new(192, 168, 1, 100),
                netmask: Ipv4Address::new(255, 255, 255, 0),
                broadcast: None,
                is_primary: true,
            },
        );

        assert!(ip_layer.is_local_ip(Ipv4Address::new(192, 168, 1, 100)));
        assert!(!ip_layer.is_local_ip(Ipv4Address::new(192, 168, 1, 101)));
    }

    #[test_case]
    fn test_ipv4_checksum() {
        let mut header = Ipv4Header::new();
        header.source_ip = [192, 168, 1, 100];
        header.dest_ip = [192, 168, 1, 1];
        header.protocol = protocol::TCP;
        header.ttl = 64;
        header.total_length = 20;
        header.identification = 0;
        header.flags_fragment = 0;
        header.tos = 0;

        let checksum = header.calculate_checksum();
        // Just verify that checksum calculation runs without panicking
        assert_ne!(checksum, 0);
    }

    #[test_case]
    fn test_ipv4_checksum_known_vector() {
        let header = Ipv4Header {
            version_ihl: 0x45,
            tos: 0x00,
            total_length: 0x003C,
            identification: 0x1C46,
            flags_fragment: 0x4000,
            ttl: 0x40,
            protocol: 0x06,
            checksum: 0x0000,
            source_ip: [192, 168, 0, 1],
            dest_ip: [192, 168, 0, 199],
        };

        let checksum = header.calculate_checksum();
        assert_eq!(checksum, 0x9C5D);
    }

    #[test_case]
    fn test_protocol_constants() {
        assert_eq!(protocol::ICMP, 1);
        assert_eq!(protocol::TCP, 6);
        assert_eq!(protocol::UDP, 17);
        assert_eq!(protocol::IPV6, 41);
    }
}
