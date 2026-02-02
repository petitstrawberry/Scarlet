//! IPv4 protocol layer
//!
//! This module provides IPv4 packet handling for the network stack.
//! It implements the NetworkLayer trait for IPv4 encapsulation/decapsulation.

use alloc::vec::Vec;
use spin::RwLock;

use crate::early_println;
use crate::network::ethernet::ETHERNET_HEADER_SIZE;
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

/// IPv4 layer
///
/// Handles IPv4 packet encapsulation and decapsulation.
/// Routes packets based on protocol field.
pub struct Ipv4Layer {
    /// Local IP address
    local_ip: RwLock<Ipv4Address>,
    /// Protocol handlers registered by protocol number
    protocols: RwLock<alloc::collections::BTreeMap<u8, alloc::sync::Arc<dyn NetworkLayer>>>,
    /// Statistics
    stats: RwLock<NetworkLayerStats>,
    /// Default TTL
    default_ttl: u8,
}

impl Ipv4Layer {
    /// Create a new IPv4 layer
    pub fn new(local_ip: Ipv4Address) -> alloc::sync::Arc<Self> {
        alloc::sync::Arc::new(Self {
            local_ip: RwLock::new(local_ip),
            protocols: RwLock::new(alloc::collections::BTreeMap::new()),
            stats: RwLock::new(NetworkLayerStats::default()),
            default_ttl: 64,
        })
    }

    /// Get local IP address
    pub fn get_local_ip(&self) -> Ipv4Address {
        *self.local_ip.read()
    }

    /// Set local IP address
    pub fn set_local_ip(&self, ip: Ipv4Address) {
        *self.local_ip.write() = ip;
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
        let local_ip = *self.local_ip.read();

        // Get destination IP from context
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

        // Get protocol number from context
        let protocol = context
            .get("ip_protocol")
            .and_then(|p| if !p.is_empty() { Some(p[0]) } else { None })
            .unwrap_or(protocol::TCP);

        // Get source IP from context or use local IP
        let src_ip_bytes = context
            .get("ip_src")
            .and_then(|ip| {
                if ip.len() >= 4 {
                    Some([ip[0], ip[1], ip[2], ip[3]])
                } else {
                    None
                }
            })
            .unwrap_or(local_ip.0);

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
            "[IPv4] Send: {} bytes (src: {}.{}.{}.{}, dst: {}.{}.{}.{}, proto: {})",
            ip_packet.len(),
            src_ip_bytes[0],
            src_ip_bytes[1],
            src_ip_bytes[2],
            src_ip_bytes[3],
            dest_ip_bytes[0],
            dest_ip_bytes[1],
            dest_ip_bytes[2],
            dest_ip_bytes[3],
            protocol
        );

        // Forward to Ethernet layer
        let mut eth_context = context.clone();
        eth_context.set(
            "eth_type",
            &crate::network::ethernet::ether_type::IPV4.to_be_bytes(),
        );
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

    fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
        // Parse IPv4 header
        let header = Ipv4Header::from_bytes(packet).ok_or(SocketError::InvalidPacket)?;

        let header_len = header.header_length();

        if packet.len() < header_len {
            return Err(SocketError::InvalidPacket);
        }

        // Verify checksum
        let calculated_checksum = checksum_from_bytes(&packet[..header_len]);
        if calculated_checksum != header.checksum {
            let header_checksum = unsafe { core::ptr::addr_of!(header.checksum).read_unaligned() };
            early_println!(
                "[IPv4] Checksum mismatch: calculated=0x{:04X}, header=0x{:04X}",
                calculated_checksum,
                header_checksum
            );
            let mut stats = self.stats.write();
            stats.protocol_errors += 1;
            return Err(SocketError::InvalidPacket);
        }

        let payload = &packet[header_len..];

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
        stats.bytes_received += packet.len() as u64;

        // Route to protocol handler based on protocol field
        let protocols = self.protocols.read();
        if let Some(handler) = protocols.get(&header.protocol) {
            handler.receive(payload)
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
        let ip = Ipv4Address::new(192, 168, 1, 100);
        let ip_layer = Ipv4Layer::new(ip);
        assert_eq!(ip_layer.get_local_ip(), ip);
    }

    #[test_case]
    fn test_ipv4_layer_set_ip() {
        let ip1 = Ipv4Address::new(192, 168, 1, 100);
        let ip_layer = Ipv4Layer::new(ip1);

        let ip2 = Ipv4Address::new(10, 0, 0, 1);
        ip_layer.set_local_ip(ip2);
        assert_eq!(ip_layer.get_local_ip(), ip2);
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
