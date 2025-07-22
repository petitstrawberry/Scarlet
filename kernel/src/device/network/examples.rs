//! Example implementations for the network pipeline
//!
//! This module contains example handlers and conditions that demonstrate
//! how to use the flexible network pipeline architecture.

use alloc::{boxed::Box, string::String, vec::Vec};
use super::{
    packet::NetworkPacket,
    error::NetworkError,
    traits::{RxStageHandler, TxStageBuilder, ProcessorCondition, NextAction},
    pipeline::{FlexibleStage, RxStageProcessor, TxStageProcessor, FlexiblePipeline},
    network_manager::NetworkManager,
};

/// Example Ethernet frame handler for receive path
///
/// Processes Ethernet frames by extracting the 14-byte header
/// and determining the next stage based on EtherType.
pub struct EthernetRxHandler;

impl RxStageHandler for EthernetRxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
        // Validate minimum Ethernet frame size
        packet.validate_payload_size(14)?;
        
        let payload = packet.payload().to_vec(); // Copy to avoid borrow conflicts
        
        // Extract Ethernet header (14 bytes: 6 dst + 6 src + 2 ethertype)
        packet.add_header("ethernet", payload[0..14].to_vec());
        
        // Set remaining payload (IP header and beyond)
        packet.set_payload(payload[14..].to_vec());
        
        Ok(())
    }
}

/// Example Ethernet frame builder for transmit path
///
/// Builds Ethernet frames by reading hints and prepending Ethernet header.
pub struct EthernetTxBuilder;

impl TxStageBuilder for EthernetTxBuilder {
    fn build(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
        // Get required hints from upper layers
        let dest_mac = packet.get_hint("dest_mac")
            .ok_or(NetworkError::missing_hint("dest_mac"))?;
        let src_mac = packet.get_hint("src_mac")
            .ok_or(NetworkError::missing_hint("src_mac"))?;
        let ether_type = packet.get_hint("ether_type")
            .ok_or(NetworkError::missing_hint("ether_type"))?;
        
        // Parse MAC addresses and EtherType
        let dest_bytes = Self::parse_mac(dest_mac)?;
        let src_bytes = Self::parse_mac(src_mac)?;
        let ether_type_val = u16::from_str_radix(ether_type, 16)
            .map_err(|_| NetworkError::invalid_packet("Invalid EtherType format"))?;
        
        // Build Ethernet header (14 bytes)
        let mut eth_header = Vec::with_capacity(14);
        eth_header.extend_from_slice(&dest_bytes);     // 6 bytes dest MAC
        eth_header.extend_from_slice(&src_bytes);      // 6 bytes src MAC
        eth_header.extend_from_slice(&ether_type_val.to_be_bytes()); // 2 bytes EtherType
        
        // Prepend header to payload
        let mut new_payload = eth_header;
        new_payload.extend_from_slice(packet.payload());
        packet.set_payload(new_payload);
        
        Ok(())
    }
}

impl EthernetTxBuilder {
    fn parse_mac(mac_str: &str) -> Result<[u8; 6], NetworkError> {
        let parts: Vec<&str> = mac_str.split(':').collect();
        if parts.len() != 6 {
            return Err(NetworkError::invalid_packet("Invalid MAC address format"));
        }
        
        let mut mac_bytes = [0u8; 6];
        for (i, part) in parts.iter().enumerate() {
            mac_bytes[i] = u8::from_str_radix(part, 16)
                .map_err(|_| NetworkError::invalid_packet("Invalid MAC address format"))?;
        }
        
        Ok(mac_bytes)
    }
}

/// Example condition for IPv4 packets (EtherType 0x0800)
pub struct IPv4EtherTypeCondition;

impl ProcessorCondition for IPv4EtherTypeCondition {
    fn matches(&self, packet: &NetworkPacket) -> bool {
        let payload = packet.payload();
        if payload.len() >= 14 {
            // Check EtherType field (bytes 12-13) for IPv4 (0x0800)
            let ether_type = u16::from_be_bytes([payload[12], payload[13]]);
            ether_type == 0x0800
        } else {
            false
        }
    }
}

/// Example condition for ARP packets (EtherType 0x0806)
pub struct ARPEtherTypeCondition;

impl ProcessorCondition for ARPEtherTypeCondition {
    fn matches(&self, packet: &NetworkPacket) -> bool {
        let payload = packet.payload();
        if payload.len() >= 14 {
            // Check EtherType field for ARP (0x0806)
            let ether_type = u16::from_be_bytes([payload[12], payload[13]]);
            ether_type == 0x0806
        } else {
            false
        }
    }
}

/// Example IPv4 header handler for receive path
///
/// Processes IPv4 packets by extracting the variable-length header
/// and determining next stage based on protocol field.
pub struct IPv4RxHandler;

impl RxStageHandler for IPv4RxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
        // Validate minimum IPv4 header size
        packet.validate_payload_size(20)?;
        
        let payload = packet.payload().to_vec(); // Copy to avoid borrow conflicts
        
        // Extract header length from IHL field (lower 4 bits of first byte)
        let ihl = (payload[0] & 0x0F) as usize;
        let header_len = ihl * 4;
        
        // Validate header length
        if header_len < 20 || header_len > payload.len() {
            return Err(NetworkError::invalid_packet(
                &alloc::format!("Invalid IPv4 header length: {}", header_len)
            ));
        }
        
        // Extract IPv4 header
        packet.add_header("ipv4", payload[0..header_len].to_vec());
        
        // Set remaining payload (transport layer and beyond)
        packet.set_payload(payload[header_len..].to_vec());
        
        Ok(())
    }
}

/// Example condition for TCP packets (IP Protocol 6)
pub struct TCPProtocolCondition;

impl ProcessorCondition for TCPProtocolCondition {
    fn matches(&self, packet: &NetworkPacket) -> bool {
        let payload = packet.payload();
        if payload.len() >= 10 {
            // Check protocol field (byte 9) for TCP (6)
            payload[9] == 6
        } else {
            false
        }
    }
}

/// Example condition for UDP packets (IP Protocol 17)
pub struct UDPProtocolCondition;

impl ProcessorCondition for UDPProtocolCondition {
    fn matches(&self, packet: &NetworkPacket) -> bool {
        let payload = packet.payload();
        if payload.len() >= 10 {
            // Check protocol field (byte 9) for UDP (17)
            payload[9] == 17
        } else {
            false
        }
    }
}

/// Example condition for ICMP packets (IP Protocol 1)
pub struct ICMPProtocolCondition;

impl ProcessorCondition for ICMPProtocolCondition {
    fn matches(&self, packet: &NetworkPacket) -> bool {
        let payload = packet.payload();
        if payload.len() >= 10 {
            // Check protocol field (byte 9) for ICMP (1)
            payload[9] == 1
        } else {
            false
        }
    }
}

/// Example TCP handler for receive path
///
/// Processes TCP segments by extracting the variable-length header.
pub struct TCPRxHandler;

impl RxStageHandler for TCPRxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
        // Validate minimum TCP header size
        packet.validate_payload_size(20)?;
        
        let payload = packet.payload().to_vec(); // Copy to avoid borrow conflicts
        
        // Extract TCP header length from data offset field (upper 4 bits of byte 12)
        let data_offset = (payload[12] >> 4) as usize;
        let header_len = data_offset * 4;
        
        // Validate header length
        if header_len < 20 || header_len > payload.len() {
            return Err(NetworkError::invalid_packet(
                &alloc::format!("Invalid TCP header length: {}", header_len)
            ));
        }
        
        // Extract TCP header
        packet.add_header("tcp", payload[0..header_len].to_vec());
        
        // Set remaining payload (application data)
        packet.set_payload(payload[header_len..].to_vec());
        
        Ok(())
    }
}

/// Example UDP handler for receive path
///
/// Processes UDP datagrams by extracting the fixed 8-byte header.
pub struct UDPRxHandler;

impl RxStageHandler for UDPRxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
        // Validate minimum UDP header size
        packet.validate_payload_size(8)?;
        
        let payload = packet.payload().to_vec(); // Copy to avoid borrow conflicts
        
        // UDP header is always 8 bytes
        packet.add_header("udp", payload[0..8].to_vec());
        
        // Set remaining payload (application data)
        packet.set_payload(payload[8..].to_vec());
        
        Ok(())
    }
}

/// Simple condition that matches all packets (for examples)
pub struct AlwaysMatchCondition;

impl ProcessorCondition for AlwaysMatchCondition {
    fn matches(&self, _packet: &NetworkPacket) -> bool {
        true
    }
}

/// Example "drop all" handler for testing (receive path)
pub struct DropRxHandler;

impl RxStageHandler for DropRxHandler {
    fn handle(&self, _packet: &mut NetworkPacket) -> Result<(), NetworkError> {
        // This handler doesn't modify the packet - it will be dropped by NextAction
        Ok(())
    }
}

/// Example logging handler that adds metadata without modifying packet
pub struct LoggingRxHandler {
    stage_name: String,
}

impl LoggingRxHandler {
    pub fn new(stage_name: &str) -> Self {
        Self {
            stage_name: String::from(stage_name),
        }
    }
}

impl RxStageHandler for LoggingRxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<(), NetworkError> {
        // Add a log entry as metadata (this could be extended to actual logging)
        packet.add_header(
            &alloc::format!("{}_log", self.stage_name),
            alloc::format!("Processed by {}", self.stage_name).into_bytes(),
        );
        Ok(())
    }
}

/// Create a basic example pipeline for Ethernet -> IPv4 -> TCP/UDP processing (receive path)
pub fn create_basic_rx_pipeline() -> FlexiblePipeline {
    let mut pipeline = FlexiblePipeline::new();
    
    // Create Ethernet stage
    let mut ethernet_stage = FlexibleStage::new("ethernet");
    ethernet_stage.add_rx_processor(RxStageProcessor::new(
        Box::new(IPv4EtherTypeCondition),
        Box::new(EthernetRxHandler),
        NextAction::jump_to("ipv4"),
    ));
    ethernet_stage.add_rx_processor(RxStageProcessor::new(
        Box::new(ARPEtherTypeCondition),
        Box::new(DropRxHandler), // Drop ARP packets for this example
        NextAction::drop_with_reason("ARP not supported in basic pipeline"),
    ));
    
    // Create IPv4 stage
    let mut ipv4_stage = FlexibleStage::new("ipv4");
    ipv4_stage.add_rx_processor(RxStageProcessor::new(
        Box::new(TCPProtocolCondition),
        Box::new(IPv4RxHandler),
        NextAction::jump_to("tcp"),
    ));
    ipv4_stage.add_rx_processor(RxStageProcessor::new(
        Box::new(UDPProtocolCondition),
        Box::new(IPv4RxHandler),
        NextAction::jump_to("udp"),
    ));
    ipv4_stage.add_rx_processor(RxStageProcessor::new(
        Box::new(ICMPProtocolCondition),
        Box::new(IPv4RxHandler),
        NextAction::drop_with_reason("ICMP not supported in basic pipeline"),
    ));
    
    // Create TCP stage
    let mut tcp_stage = FlexibleStage::new("tcp");
    tcp_stage.add_rx_processor(RxStageProcessor::new(
        Box::new(AlwaysMatchCondition), // Accept all TCP
        Box::new(TCPRxHandler),
        NextAction::Complete, // Deliver to application
    ));
    
    // Create UDP stage
    let mut udp_stage = FlexibleStage::new("udp");
    udp_stage.add_rx_processor(RxStageProcessor::new(
        Box::new(AlwaysMatchCondition), // Accept all UDP
        Box::new(UDPRxHandler),
        NextAction::Complete, // Deliver to application
    ));
    
    // Add stages to pipeline
    pipeline.add_stage(ethernet_stage).unwrap();
    pipeline.add_stage(ipv4_stage).unwrap();
    pipeline.add_stage(tcp_stage).unwrap();
    pipeline.add_stage(udp_stage).unwrap();
    
    // Set ethernet as rx entry point
    pipeline.set_default_rx_entry_stage("ethernet").unwrap();
    
    pipeline
}

/// Create a NetworkManager with the basic receive pipeline
pub fn create_basic_network_manager() -> NetworkManager {
    let pipeline = create_basic_rx_pipeline();
    NetworkManager::with_pipeline(pipeline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test_case]
    fn test_ethernet_handler() {
        let mut packet = NetworkPacket::new(
            vec![
                // Ethernet header (14 bytes)
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, // Destination MAC
                0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, // Source MAC
                0x08, 0x00,                         // EtherType (IPv4)
                // Payload
                0xCC, 0xDD, 0xEE, 0xFF,
            ],
            String::from("eth0"),
        );
        
        let handler = EthernetHandler;
        let result = handler.handle(&mut packet);
        assert!(result.is_ok());
        
        // Check that header was extracted
        let eth_header = packet.get_header("ethernet").unwrap();
        assert_eq!(eth_header.len(), 14);
        assert_eq!(eth_header[12..14], [0x08, 0x00]); // EtherType
        
        // Check that payload was updated
        assert_eq!(packet.payload(), &[0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test_case]
    fn test_ethernet_conditions() {
        let ipv4_packet = NetworkPacket::new(
            vec![
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55,
                0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB,
                0x08, 0x00, // IPv4 EtherType
                0xCC, 0xDD,
            ],
            String::from("eth0"),
        );
        
        let arp_packet = NetworkPacket::new(
            vec![
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55,
                0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB,
                0x08, 0x06, // ARP EtherType
                0xCC, 0xDD,
            ],
            String::from("eth0"),
        );
        
        let ipv4_condition = IPv4EtherTypeCondition;
        let arp_condition = ARPEtherTypeCondition;
        
        assert!(ipv4_condition.matches(&ipv4_packet));
        assert!(!ipv4_condition.matches(&arp_packet));
        
        assert!(!arp_condition.matches(&ipv4_packet));
        assert!(arp_condition.matches(&arp_packet));
    }

    #[test_case]
    fn test_ipv4_handler() {
        let mut packet = NetworkPacket::new(
            vec![
                // IPv4 header (20 bytes minimum)
                0x45,                   // Version=4, IHL=5 (20 bytes)
                0x00,                   // TOS
                0x00, 0x28,             // Total length
                0x00, 0x00,             // ID
                0x40, 0x00,             // Flags + Fragment offset
                0x40,                   // TTL
                0x06,                   // Protocol (TCP)
                0x00, 0x00,             // Checksum
                192, 168, 1, 1,         // Source IP
                192, 168, 1, 2,         // Dest IP
                // TCP payload
                0xAA, 0xBB, 0xCC, 0xDD,
            ],
            String::from("eth0"),
        );
        
        let handler = IPv4Handler;
        let result = handler.handle(&mut packet);
        assert!(result.is_ok());
        
        // Check that header was extracted (20 bytes)
        let ipv4_header = packet.get_header("ipv4").unwrap();
        assert_eq!(ipv4_header.len(), 20);
        assert_eq!(ipv4_header[9], 0x06); // Protocol field (TCP)
        
        // Check that payload was updated
        assert_eq!(packet.payload(), &[0xAA, 0xBB, 0xCC, 0xDD]);
    }

    #[test_case]
    fn test_protocol_conditions() {
        let tcp_packet = NetworkPacket::new(
            vec![
                0x45, 0x00, 0x00, 0x28, 0x00, 0x00, 0x40, 0x00, 0x40,
                0x06, // TCP protocol
                0x00, 0x00,
                192, 168, 1, 1, 192, 168, 1, 2,
                0xAA, 0xBB,
            ],
            String::from("eth0"),
        );
        
        let udp_packet = NetworkPacket::new(
            vec![
                0x45, 0x00, 0x00, 0x28, 0x00, 0x00, 0x40, 0x00, 0x40,
                0x11, // UDP protocol
                0x00, 0x00,
                192, 168, 1, 1, 192, 168, 1, 2,
                0xAA, 0xBB,
            ],
            String::from("eth0"),
        );
        
        let tcp_condition = TCPProtocolCondition;
        let udp_condition = UDPProtocolCondition;
        let icmp_condition = ICMPProtocolCondition;
        
        assert!(tcp_condition.matches(&tcp_packet));
        assert!(!tcp_condition.matches(&udp_packet));
        
        assert!(!udp_condition.matches(&tcp_packet));
        assert!(udp_condition.matches(&udp_packet));
        
        assert!(!icmp_condition.matches(&tcp_packet));
        assert!(!icmp_condition.matches(&udp_packet));
    }

    #[test_case]
    fn test_tcp_handler() {
        let mut packet = NetworkPacket::new(
            vec![
                // TCP header (20 bytes minimum)
                0x12, 0x34,             // Source port
                0x56, 0x78,             // Dest port
                0x00, 0x00, 0x00, 0x01, // Sequence number
                0x00, 0x00, 0x00, 0x01, // Ack number
                0x50,                   // Data offset = 5 (20 bytes)
                0x18,                   // Flags
                0x20, 0x00,             // Window size
                0x00, 0x00,             // Checksum
                0x00, 0x00,             // Urgent pointer
                // Application data
                b'H', b'e', b'l', b'l', b'o',
            ],
            String::from("eth0"),
        );
        
        let handler = TCPHandler;
        let result = handler.handle(&mut packet);
        assert!(result.is_ok());
        
        // Check that header was extracted
        let tcp_header = packet.get_header("tcp").unwrap();
        assert_eq!(tcp_header.len(), 20);
        assert_eq!(tcp_header[0..2], [0x12, 0x34]); // Source port
        
        // Check that payload was updated
        assert_eq!(packet.payload(), b"Hello");
    }

    #[test_case]
    fn test_udp_handler() {
        let mut packet = NetworkPacket::new(
            vec![
                // UDP header (8 bytes)
                0x12, 0x34,             // Source port
                0x56, 0x78,             // Dest port
                0x00, 0x0D,             // Length (13 = 8 header + 5 data)
                0x00, 0x00,             // Checksum
                // Application data
                b'H', b'e', b'l', b'l', b'o',
            ],
            String::from("eth0"),
        );
        
        let handler = UDPHandler;
        let result = handler.handle(&mut packet);
        assert!(result.is_ok());
        
        // Check that header was extracted
        let udp_header = packet.get_header("udp").unwrap();
        assert_eq!(udp_header.len(), 8);
        assert_eq!(udp_header[0..2], [0x12, 0x34]); // Source port
        
        // Check that payload was updated
        assert_eq!(packet.payload(), b"Hello");
    }

    #[test_case]
    fn test_logging_handler() {
        let mut packet = NetworkPacket::new(
            vec![0xAA, 0xBB, 0xCC],
            String::from("eth0"),
        );
        
        let handler = LoggingHandler::new("test_stage");
        let result = handler.handle(&mut packet);
        assert!(result.is_ok());
        
        // Check that log entry was added
        let log_header = packet.get_header("test_stage_log").unwrap();
        let log_message = String::from_utf8(log_header.to_vec()).unwrap();
        assert_eq!(log_message, "Processed by test_stage");
        
        // Original payload should be unchanged
        assert_eq!(packet.payload(), &[0xAA, 0xBB, 0xCC]);
    }

    #[test_case]
    fn test_basic_pipeline_creation() {
        let pipeline = create_basic_pipeline();
        assert_eq!(pipeline.stage_count(), 4);
        assert!(pipeline.has_stage("ethernet"));
        assert!(pipeline.has_stage("ipv4"));
        assert!(pipeline.has_stage("tcp"));
        assert!(pipeline.has_stage("udp"));
        assert_eq!(pipeline.get_default_entry_stage(), Some("ethernet"));
    }

    #[test_case]
    fn test_basic_network_manager() {
        let manager = create_basic_network_manager();
        assert_eq!(manager.stage_count(), 4);
        assert!(manager.has_stage("ethernet"));
        
        // Create a simple IPv4+TCP packet
        let packet = NetworkPacket::new(
            vec![
                // Ethernet header
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55,
                0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB,
                0x08, 0x00, // IPv4 EtherType
                // IPv4 header
                0x45, 0x00, 0x00, 0x3C, 0x00, 0x00, 0x40, 0x00, 0x40,
                0x06, // TCP protocol
                0x00, 0x00,
                192, 168, 1, 1, 192, 168, 1, 2,
                // TCP header
                0x12, 0x34, 0x56, 0x78,
                0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
                0x50, 0x18, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00,
                // Data
                b'T', b'e', b's', b't',
            ],
            String::from("eth0"),
        );
        
        let result = manager.process_packet(packet);
        assert!(result.is_ok());
        
        let stats = manager.get_stats();
        assert_eq!(stats.packets_processed, 1);
        assert_eq!(stats.packets_completed, 1);
    }

    #[test_case]
    fn test_arp_packet_dropped() {
        let manager = create_basic_network_manager();
        
        // Create an ARP packet
        let packet = NetworkPacket::new(
            vec![
                // Ethernet header with ARP EtherType
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55,
                0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB,
                0x08, 0x06, // ARP EtherType
                // ARP data
                0x00, 0x01, 0x08, 0x00, 0x06, 0x04, 0x00, 0x01,
            ],
            String::from("eth0"),
        );
        
        let result = manager.process_packet(packet);
        assert!(result.is_ok()); // Dropping is successful
        
        let stats = manager.get_stats();
        assert_eq!(stats.packets_dropped, 1);
        assert_eq!(stats.packets_completed, 0);
    }
}