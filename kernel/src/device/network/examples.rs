//! Example implementations for the network pipeline
//!
//! This module contains example handlers and conditions that demonstrate
//! how to use the flexible network pipeline architecture.

use alloc::{boxed::Box, string::String, vec::Vec};
use super::{
    packet::NetworkPacket,
    error::NetworkError,
    traits::{RxStageHandler, TxStageBuilder, ProcessorCondition, NextAction},
    pipeline::{FlexibleStage, RxStageProcessor, FlexiblePipeline},
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

/// Create a simple example pipeline for basic Ethernet processing (receive path)
pub fn create_simple_rx_pipeline() -> FlexiblePipeline {
    let mut pipeline = FlexiblePipeline::new();
    
    // Create Ethernet stage
    let mut ethernet_stage = FlexibleStage::new("ethernet");
    ethernet_stage.add_rx_processor(RxStageProcessor::new(
        Box::new(IPv4EtherTypeCondition),
        Box::new(EthernetRxHandler),
        NextAction::Complete, // Complete processing after Ethernet
    ));
    ethernet_stage.add_rx_processor(RxStageProcessor::new(
        Box::new(ARPEtherTypeCondition),
        Box::new(DropRxHandler), // Drop ARP packets for this example
        NextAction::drop_with_reason("ARP not supported in simple pipeline"),
    ));
    
    // Add stage to pipeline
    pipeline.add_stage(ethernet_stage).unwrap();
    
    // Set ethernet as rx entry point
    pipeline.set_default_rx_entry_stage("ethernet").unwrap();
    
    pipeline
}

/// Create a NetworkManager with the simple receive pipeline
pub fn create_simple_network_manager() -> NetworkManager {
    let pipeline = create_simple_rx_pipeline();
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
        
        let handler = EthernetRxHandler;
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
    fn test_logging_handler() {
        let mut packet = NetworkPacket::new(
            vec![0xAA, 0xBB, 0xCC],
            String::from("eth0"),
        );
        
        let handler = LoggingRxHandler::new("test_stage");
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
    fn test_simple_pipeline_creation() {
        let pipeline = create_simple_rx_pipeline();
        assert_eq!(pipeline.stage_count(), 1);
        assert!(pipeline.has_stage("ethernet"));
        assert_eq!(pipeline.get_default_rx_entry_stage(), Some("ethernet"));
    }

    #[test_case]
    fn test_simple_network_manager() {
        let manager = create_simple_network_manager();
        assert_eq!(manager.stage_count(), 1);
        assert!(manager.has_stage("ethernet"));
        
        // Create a simple IPv4 Ethernet packet
        let packet = NetworkPacket::new(
            vec![
                // Ethernet header
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55,
                0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB,
                0x08, 0x00, // IPv4 EtherType
                // Some payload
                b'T', b'e', b's', b't',
            ],
            String::from("eth0"),
        );
        
        let result = manager.process_rx_packet(packet);
        assert!(result.is_ok());
        
        let stats = manager.get_stats();
        assert_eq!(stats.packets_processed, 1);
        assert_eq!(stats.packets_completed, 1);
    }

    #[test_case]
    fn test_arp_packet_dropped() {
        let manager = create_simple_network_manager();
        
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
        
        let result = manager.process_rx_packet(packet);
        assert!(result.is_ok()); // Dropping is successful
        
        let stats = manager.get_stats();
        assert_eq!(stats.packets_dropped, 1);
        assert_eq!(stats.packets_completed, 0);
    }
}