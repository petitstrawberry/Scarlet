//! ICMP protocol layer
//!
//! This module provides ICMP handling for network stack.
//! It implements NetworkLayer trait for ICMP messages.

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

use crate::network::ipv4::Ipv4Address;
use crate::network::protocol_stack::get_network_manager;
use crate::network::protocol_stack::{LayerContext, NetworkLayer, NetworkLayerStats};
use crate::network::socket::SocketError;

/// ICMP message types
pub mod message_type {
    /// Echo reply
    pub const ECHO_REPLY: u8 = 0;
    /// Destination unreachable
    pub const DESTINATION_UNREACHABLE: u8 = 3;
    /// Source quench
    pub const SOURCE_QUENCH: u8 = 4;
    /// Redirect
    pub const REDIRECT: u8 = 5;
    /// Echo request
    pub const ECHO_REQUEST: u8 = 8;
    /// Time exceeded
    pub const TIME_EXCEEDED: u8 = 11;
    /// Parameter problem
    pub const PARAMETER_PROBLEM: u8 = 12;
    /// Timestamp request
    pub const TIMESTAMP_REQUEST: u8 = 13;
    /// Timestamp reply
    pub const TIMESTAMP_REPLY: u8 = 14;
}

/// ICMP codes
pub mod code {
    /// No code
    pub const NO_CODE: u8 = 0;

    // Destination unreachable codes
    pub const NET_UNREACHABLE: u8 = 0;
    pub const HOST_UNREACHABLE: u8 = 1;
    pub const PROTOCOL_UNREACHABLE: u8 = 2;
    pub const PORT_UNREACHABLE: u8 = 3;
    pub const FRAGMENTATION_NEEDED: u8 = 4;
    pub const SOURCE_ROUTE_FAILED: u8 = 5;
}

/// ICMP header (4 bytes minimum)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct IcmpHeader {
    /// Message type
    pub message_type: u8,
    /// Message code
    pub code: u8,
    /// Checksum
    pub checksum: u16,
    /// Rest of header (varies by type)
    pub rest: [u8; 4],
}

impl IcmpHeader {
    /// Create a new ICMP header
    pub fn new(message_type: u8, code: u8) -> Self {
        Self {
            message_type,
            code,
            checksum: 0,
            rest: [0; 4],
        }
    }

    /// Calculate checksum
    pub fn calculate_checksum(&self, data: &[u8]) -> u16 {
        let mut sum: u32 = 0;

        // Header
        let header_bytes =
            unsafe { core::slice::from_raw_parts(self as *const IcmpHeader as *const u8, 8) };
        for chunk in header_bytes.chunks(2) {
            if chunk.len() == 2 {
                sum += u32::from_be_bytes([chunk[0], chunk[1]]);
            } else if chunk.len() == 1 {
                sum += (chunk[0] as u32) << 8;
            }
        }

        // Data
        for chunk in data.chunks(2) {
            if chunk.len() == 2 {
                sum += u32::from_be_bytes([chunk[0], chunk[1]]);
            } else if chunk.len() == 1 {
                sum += (chunk[0] as u32) << 8;
            }
        }

        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }

        !sum as u16
    }

    /// Serialize header to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8);
        bytes.push(self.message_type);
        bytes.push(self.code);
        bytes.extend_from_slice(&self.checksum.to_be_bytes());
        bytes.extend_from_slice(&self.rest);
        bytes
    }

    /// Parse header from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 {
            return None;
        }

        Some(Self {
            message_type: bytes[0],
            code: bytes[1],
            checksum: u16::from_be_bytes([bytes[2], bytes[3]]),
            rest: [bytes[4], bytes[5], bytes[6], bytes[7]],
        })
    }
}

/// ICMP Echo request/reply header
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct IcmpEcho {
    /// Identifier
    pub identifier: u16,
    /// Sequence number
    pub sequence: u16,
}

impl IcmpEcho {
    /// Create a new ICMP Echo header
    pub fn new(identifier: u16, sequence: u16) -> Self {
        Self {
            identifier,
            sequence,
        }
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> [u8; 4] {
        let mut bytes = [0u8; 4];
        bytes[0..2].copy_from_slice(&self.identifier.to_be_bytes());
        bytes[2..4].copy_from_slice(&self.sequence.to_be_bytes());
        bytes
    }

    /// Parse from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 {
            return None;
        }

        Some(Self {
            identifier: u16::from_be_bytes([bytes[0], bytes[1]]),
            sequence: u16::from_be_bytes([bytes[2], bytes[3]]),
        })
    }
}

/// ICMP layer
///
/// Handles ICMP messages for network diagnostics.
pub struct IcmpLayer {
    /// Statistics
    stats: RwLock<NetworkLayerStats>,
}

impl IcmpLayer {
    /// Create a new ICMP layer
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            stats: RwLock::new(NetworkLayerStats::default()),
        })
    }

    /// Send an ICMP Echo Request (ping)
    pub fn send_ping_request(
        &self,
        dest_ip: Ipv4Address,
        identifier: u16,
        sequence: u16,
        data: &[u8],
        next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // Build ICMP Echo Request header
        let header = IcmpHeader::new(message_type::ECHO_REQUEST, code::NO_CODE);
        let echo = IcmpEcho::new(identifier, sequence);

        // Combine header and data
        let mut icmp_data = Vec::with_capacity(8 + data.len());
        icmp_data.extend_from_slice(&header.to_bytes());
        icmp_data.extend_from_slice(&echo.to_bytes());
        icmp_data.extend_from_slice(data);

        // Calculate checksum
        let checksum = header.calculate_checksum(&[&echo.to_bytes(), data].concat());
        let mut icmp_packet = Vec::with_capacity(8 + data.len());
        icmp_packet.extend_from_slice(&header.to_bytes()[..2]); // Type + Code
        icmp_packet.extend_from_slice(&checksum.to_be_bytes());
        icmp_packet.extend_from_slice(&echo.to_bytes());
        icmp_packet.extend_from_slice(data);

        // Create IP context
        let mut ip_context = LayerContext::new();
        ip_context.set("ip_dst", &dest_ip.0);
        ip_context.set("ip_protocol", &[1]); // ICMP protocol

            "[ICMP] Ping {}.{}.{}.{} (id={}, seq={}, data_len={})",
            dest_ip[0],
            dest_ip[1],
            dest_ip[2],
            dest_ip[3],
            identifier,
            sequence,
            data.len()
        );

        // Send through IP layer
        if !next_layers.is_empty() {
            next_layers[0].send(&icmp_packet, &ip_context, &next_layers[1..])?;

            // Update statistics
            let mut stats = self.stats.write();
            stats.packets_sent += 1;
            stats.bytes_sent += icmp_packet.len() as u64;
        }

        Ok(())
    }

    /// Send an ICMP Echo Reply
    pub fn send_ping_reply(
        &self,
        dest_ip: Ipv4Address,
        identifier: u16,
        sequence: u16,
        data: &[u8],
        next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // Build ICMP Echo Reply header
        let header = IcmpHeader::new(message_type::ECHO_REPLY, code::NO_CODE);
        let echo = IcmpEcho::new(identifier, sequence);

        // Combine header and data
        let mut icmp_data = Vec::with_capacity(8 + data.len());
        icmp_data.extend_from_slice(&header.to_bytes());
        icmp_data.extend_from_slice(&echo.to_bytes());
        icmp_data.extend_from_slice(data);

        // Calculate checksum
        let checksum = header.calculate_checksum(&[&echo.to_bytes(), data].concat());
        let mut icmp_packet = Vec::with_capacity(8 + data.len());
        icmp_packet.extend_from_slice(&header.to_bytes()[..2]); // Type + Code
        icmp_packet.extend_from_slice(&checksum.to_be_bytes());
        icmp_packet.extend_from_slice(&echo.to_bytes());
        icmp_packet.extend_from_slice(data);

        // Create IP context
        let mut ip_context = LayerContext::new();
        ip_context.set("ip_dst", &dest_ip.0);
        ip_context.set("ip_protocol", &[1]); // ICMP protocol

            "[ICMP] Pong {}.{}.{}.{} (id={}, seq={}, data_len={})",
            dest_ip[0],
            dest_ip[1],
            dest_ip[2],
            dest_ip[3],
            identifier,
            sequence,
            data.len()
        );

        // Send through IP layer
        if !next_layers.is_empty() {
            next_layers[0].send(&icmp_packet, &ip_context, &next_layers[1..])?;

            // Update statistics
            let mut stats = self.stats.write();
            stats.packets_sent += 1;
            stats.bytes_sent += icmp_packet.len() as u64;
        }

        Ok(())
    }

    /// Process received ICMP packet
    pub fn receive_packet(&self, packet: &[u8]) -> Result<(), SocketError> {
        if packet.len() < 8 {
            return Err(SocketError::InvalidPacket);
        }

        // Parse ICMP header
        let header = IcmpHeader::from_bytes(&packet[..8]).ok_or(SocketError::InvalidPacket)?;

        let data = &packet[8..];

            "[ICMP] Recv: type={}, code={}, len={}",
            header.message_type,
            header.code,
            packet.len()
        );

        // Update statistics
        let mut stats = self.stats.write();
        stats.packets_received += 1;
        stats.bytes_received += packet.len() as u64;

        match header.message_type {
            message_type::ECHO_REQUEST => {
                // Handle ping request - send reply
                if data.len() >= 4 {
                    if let Some(echo) = IcmpEcho::from_bytes(data) {
                            "[ICMP] Ping request from (id={}, seq={})",
                            echo.identifier,
                            echo.sequence
                        );

                        // TODO: Send ping reply back
                        // Need to get source IP from IP layer context
                    }
                }
            }
            message_type::ECHO_REPLY => {
            }
            _ => {
            }
        }

        Ok(())
    }
}

impl NetworkLayer for IcmpLayer {
    fn register_protocol(&self, _proto_num: u16, _handler: Arc<dyn NetworkLayer>) {
        // ICMP is typically a leaf protocol
    }

    fn send(
        &self,
        _packet: &[u8],
        _context: &LayerContext,
        _next_layers: &[Arc<dyn NetworkLayer>],
    ) -> Result<(), SocketError> {
        // ICMP send is handled through specific methods
        Ok(())
    }

    fn receive(&self, packet: &[u8]) -> Result<(), SocketError> {
        self.receive_packet(packet)
    }

    fn name(&self) -> &'static str {
        "ICMP"
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
    fn test_icmp_header_creation() {
        let header = IcmpHeader::new(message_type::ECHO_REQUEST, code::NO_CODE);

        assert_eq!(header.message_type, message_type::ECHO_REQUEST);
        assert_eq!(header.code, code::NO_CODE);
        assert_eq!(header.rest, [0, 0, 0, 0]);
    }

    #[test_case]
    fn test_icmp_echo_header() {
        let echo = IcmpEcho::new(1234, 5678);

        assert_eq!(echo.identifier, 1234);
        assert_eq!(echo.sequence, 5678);
    }

    #[test_case]
    fn test_icmp_echo_serialization() {
        let echo = IcmpEcho::new(1234, 5678);
        let bytes = echo.to_bytes();

        assert_eq!(bytes.len(), 4);
        assert_eq!(u16::from_be_bytes([bytes[0], bytes[1]]), 1234);
        assert_eq!(u16::from_be_bytes([bytes[2], bytes[3]]), 5678);
    }

    #[test_case]
    fn test_icmp_echo_parsing() {
        let mut bytes = [0u8; 4];
        bytes[0..2].copy_from_slice(&1234u16.to_be_bytes());
        bytes[2..4].copy_from_slice(&5678u16.to_be_bytes());

        let echo = IcmpEcho::from_bytes(&bytes).unwrap();

        assert_eq!(echo.identifier, 1234);
        assert_eq!(echo.sequence, 5678);
    }

    #[test_case]
    fn test_icmp_header_parsing() {
        let mut bytes = [0u8; 8];
        bytes[0] = message_type::ECHO_REQUEST;
        bytes[1] = code::NO_CODE;
        bytes[2..4].copy_from_slice(&0x1234u16.to_be_bytes()); // Checksum
        bytes[4..8].copy_from_slice(&[0x00, 0x00, 0x00, 0x00]); // Rest

        let header = IcmpHeader::from_bytes(&bytes).unwrap();

        assert_eq!(header.message_type, message_type::ECHO_REQUEST);
        assert_eq!(header.code, code::NO_CODE);
        assert_eq!(header.rest, [0, 0, 0, 0]);
    }

    #[test_case]
    fn test_icmp_header_too_short() {
        let bytes = [0u8; 4];
        assert!(IcmpHeader::from_bytes(&bytes).is_none());
    }

    #[test_case]
    fn test_message_type_constants() {
        assert_eq!(message_type::ECHO_REPLY, 0);
        assert_eq!(message_type::DESTINATION_UNREACHABLE, 3);
        assert_eq!(message_type::ECHO_REQUEST, 8);
        assert_eq!(message_type::TIME_EXCEEDED, 11);
    }

    #[test_case]
    fn test_code_constants() {
        assert_eq!(code::NO_CODE, 0);
        assert_eq!(code::NET_UNREACHABLE, 0);
        assert_eq!(code::HOST_UNREACHABLE, 1);
        assert_eq!(code::PORT_UNREACHABLE, 3);
    }
}
