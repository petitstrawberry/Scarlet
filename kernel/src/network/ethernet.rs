//! Ethernet protocol layer
//!
//! This module provides Ethernet II frame handling for the network stack.
//! It implements the NetworkLayer trait for Ethernet encapsulation/decapsulation.

use alloc::vec::Vec;
use spin::RwLock;

use crate::device::network::MacAddress;
use crate::network::protocol_stack::{LayerContext, NetworkLayer, NetworkLayerStats};
use crate::network::socket::SocketError;

/// Ethernet frame header (14 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct EthernetHeader {
    /// Destination MAC address (6 bytes)
    pub dest_mac: [u8; 6],
    /// Source MAC address (6 bytes)
    pub src_mac: [u8; 6],
    /// EtherType (2 bytes) - protocol identifier
    pub ether_type: u16,
}

impl EthernetHeader {
    /// Create a new Ethernet header
    pub fn new(dest_mac: [u8; 6], src_mac: [u8; 6], ether_type: u16) -> Self {
        Self {
            dest_mac,
            src_mac,
            ether_type,
        }
    }

    /// Serialize header to bytes
    pub fn to_bytes(&self) -> [u8; 14] {
        let mut bytes = [0u8; 14];
        bytes[0..6].copy_from_slice(&self.dest_mac);
        bytes[6..12].copy_from_slice(&self.src_mac);
        bytes[12..14].copy_from_slice(&self.ether_type.to_be_bytes());
        bytes
    }

    /// Parse header from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 14 {
            return None;
        }
        let mut dest_mac = [0u8; 6];
        let mut src_mac = [0u8; 6];
        dest_mac.copy_from_slice(&bytes[0..6]);
        src_mac.copy_from_slice(&bytes[6..12]);
        let ether_type = u16::from_be_bytes([bytes[12], bytes[13]]);
        Some(Self {
            dest_mac,
            src_mac,
            ether_type,
        })
    }

    /// Get EtherType as big-endian bytes
    pub fn ether_type_be(&self) -> [u8; 2] {
        self.ether_type.to_be_bytes()
    }
}

/// Ethernet EtherType constants
pub mod ether_type {
    /// IPv4 protocol
    pub const IPV4: u16 = 0x0800;
    /// ARP protocol
    pub const ARP: u16 = 0x0806;
    /// IPv6 protocol
    pub const IPV6: u16 = 0x86DD;
    /// VLAN-tagged frame (802.1Q)
    pub const VLAN: u16 = 0x8100;
}

/// Maximum Transmission Unit for Ethernet (standard)
pub const ETHERNET_MTU: usize = 1500;

/// Minimum Ethernet frame size (64 bytes including FCS)
pub const ETHERNET_MIN_SIZE: usize = 64;

/// Ethernet header size
pub const ETHERNET_HEADER_SIZE: usize = 14;

/// Ethernet layer
///
/// Handles Ethernet II frame encapsulation and decapsulation.
/// Routes frames based on EtherType field.
pub struct EthernetLayer {
    /// Source MAC address
    src_mac: RwLock<MacAddress>,
    /// Protocol handlers registered by EtherType
    protocols: RwLock<alloc::collections::BTreeMap<u16, alloc::sync::Arc<dyn NetworkLayer>>>,
    /// Statistics
    stats: RwLock<NetworkLayerStats>,
}

impl EthernetLayer {
    /// Create a new Ethernet layer
    pub fn new(src_mac: MacAddress) -> alloc::sync::Arc<Self> {
        alloc::sync::Arc::new(Self {
            src_mac: RwLock::new(src_mac),
            protocols: RwLock::new(alloc::collections::BTreeMap::new()),
            stats: RwLock::new(NetworkLayerStats::default()),
        })
    }

    /// Get source MAC address
    pub fn get_src_mac(&self) -> MacAddress {
        *self.src_mac.read()
    }

    /// Set source MAC address
    pub fn set_src_mac(&self, mac: MacAddress) {
        *self.src_mac.write() = mac;
    }
}

impl NetworkLayer for EthernetLayer {
    fn register_protocol(&self, proto_num: u16, handler: alloc::sync::Arc<dyn NetworkLayer>) {
        self.protocols.write().insert(proto_num, handler);
    }

    fn send(
        &self,
        packet: &[u8],
        context: &LayerContext,
        _next_layers: &[alloc::sync::Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        let src_mac = *self.src_mac.read();

        // Try to get destination MAC from context
        // For broadcast, use FF:FF:FF:FF:FF:FF
        let dest_mac = if let Some(dest_ip) = context.get("ip_dst") {
            // Check if it's a broadcast address
            if dest_ip.len() >= 4 {
                let ip_bytes = [dest_ip[0], dest_ip[1], dest_ip[2], dest_ip[3]];
                if ip_bytes == [255, 255, 255, 255] {
                    [0xFF; 6]
                } else {
                    // TODO: Use ARP to resolve MAC address
                    // For now, use a placeholder (will be resolved by ARP)
                    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
                }
            } else {
                [0x00; 6]
            }
        } else if let Some(mac_bytes) = context.get("eth_dst_mac") {
            let mut mac = [0u8; 6];
            if mac_bytes.len() >= 6 {
                mac.copy_from_slice(&mac_bytes[0..6]);
            }
            mac
        } else {
            // Default to broadcast
            [0xFF; 6]
        };

        // Get EtherType from context (IP protocol)
        let ether_type = context
            .get("ip_protocol")
            .and_then(|p| {
                if !p.is_empty() {
                    Some(p[0] as u16)
                } else {
                    None
                }
            })
            .unwrap_or(ether_type::IPV4);

        // Build Ethernet frame: header + packet
        let header = EthernetHeader::new(dest_mac, src_mac.0, ether_type);
        let total_size = ETHERNET_HEADER_SIZE + packet.len();

        // In a real implementation, we would send this through NetworkDevice
        // For now, just log that we're sending
            "[Ethernet] Send: {} bytes (dst: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}, type: 0x{:04X})",
            total_size,
            dest_mac[0],
            dest_mac[1],
            dest_mac[2],
            dest_mac[3],
            dest_mac[4],
            dest_mac[5],
            ether_type
        );

        // Update statistics
        let mut stats = self.stats.write();
        stats.packets_sent += 1;
        stats.bytes_sent += total_size as u64;

        Ok(())
    }

    fn receive(&self, frame: &[u8]) -> Result<(), SocketError> {
        if frame.len() < ETHERNET_HEADER_SIZE {
            return Err(SocketError::InvalidPacket);
        }

        // Parse Ethernet header
        let header = EthernetHeader::from_bytes(&frame[..ETHERNET_HEADER_SIZE])
            .ok_or(SocketError::InvalidPacket)?;

        let payload = &frame[ETHERNET_HEADER_SIZE..];

            "[Ethernet] Recv: {} bytes (src: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}, type: 0x{:04X})",
            frame.len(),
            header.src_mac[0],
            header.src_mac[1],
            header.src_mac[2],
            header.src_mac[3],
            header.src_mac[4],
            header.src_mac[5],
            header.ether_type
        );

        // Update statistics
        let mut stats = self.stats.write();
        stats.packets_received += 1;
        stats.bytes_received += frame.len() as u64;

        // Route to protocol handler based on EtherType
        let protocols = self.protocols.read();
        if let Some(handler) = protocols.get(&header.ether_type) {
            handler.receive(payload)
        } else if header.ether_type == ether_type::IPV4
            || header.ether_type == ether_type::ARP
            || header.ether_type == ether_type::IPV6
        {
            // No handler for this EtherType, but frame is valid
            Err(SocketError::ProtocolNotSupported)
        } else {
            // Unknown EtherType - log and drop
            Ok(())
        }
    }

    fn name(&self) -> &'static str {
        "Ethernet"
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
    use super::*;

    #[test_case]
    fn test_ethernet_header_serialization() {
        let src_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let dest_mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let ether_type = 0x0800; // IPv4

        let header = EthernetHeader::new(dest_mac, src_mac, ether_type);
        let bytes = header.to_bytes();

        assert_eq!(bytes.len(), 14);
        assert_eq!(&bytes[0..6], &dest_mac);
        assert_eq!(&bytes[6..12], &src_mac);
        assert_eq!(u16::from_be_bytes([bytes[12], bytes[13]]), ether_type);
    }

    #[test_case]
    fn test_ethernet_header_parsing() {
        let mut bytes = [0u8; 14];
        bytes[0..6].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        bytes[6..12].copy_from_slice(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        bytes[12..14].copy_from_slice(&0x08u16.to_be_bytes());

        let header = EthernetHeader::from_bytes(&bytes).unwrap();
        assert_eq!(header.dest_mac, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        assert_eq!(header.src_mac, [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        assert_eq!(header.ether_type, 0x0800);
    }

    #[test_case]
    fn test_ethernet_header_invalid_length() {
        let bytes = [0u8; 10];
        assert!(EthernetHeader::from_bytes(&bytes).is_none());
    }

    #[test_case]
    fn test_ether_type_constants() {
        assert_eq!(ether_type::IPV4, 0x0800);
        assert_eq!(ether_type::ARP, 0x0806);
        assert_eq!(ether_type::IPV6, 0x86DD);
        assert_eq!(ether_type::VLAN, 0x8100);
    }

    #[test_case]
    fn test_ethernet_layer_creation() {
        let mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        let eth_layer = EthernetLayer::new(mac);
        assert_eq!(eth_layer.get_src_mac(), mac);
    }

    #[test_case]
    fn test_ethernet_layer_set_mac() {
        let mac1 = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        let eth_layer = EthernetLayer::new(mac1);

        let mac2 = MacAddress::new([0x00, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE]);
        eth_layer.set_src_mac(mac2);
        assert_eq!(eth_layer.get_src_mac(), mac2);
    }
}
