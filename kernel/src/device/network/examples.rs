//! Example implementations for the network pipeline
//!
//! This module contains example handlers and conditions that demonstrate
//! how to use the flexible network pipeline architecture.

use alloc::{boxed::Box, string::String};
use super::{
    packet::NetworkPacket,
    error::NetworkError,
    traits::{RxStageHandler, ProcessorCondition, NextAction},
    pipeline::{FlexibleStage, RxStageProcessor, FlexiblePipeline},
    network_manager::NetworkManager,
};

/// Example condition that matches packets based on first byte value
pub struct FirstByteCondition {
    expected_value: u8,
}

impl FirstByteCondition {
    pub fn new(expected_value: u8) -> Self {
        Self { expected_value }
    }
}

impl ProcessorCondition for FirstByteCondition {
    fn matches(&self, packet: &NetworkPacket) -> bool {
        let payload = packet.payload();
        !payload.is_empty() && payload[0] == self.expected_value
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

/// Create a simple example pipeline demonstrating the infrastructure (receive path)
pub fn create_simple_rx_pipeline() -> FlexiblePipeline {
    let mut pipeline = FlexiblePipeline::new();
    
    // Create a simple processing stage with generic examples
    let mut processing_stage = FlexibleStage::new("processing");
    processing_stage.add_rx_processor(RxStageProcessor::new(
        Box::new(FirstByteCondition::new(0x42)), // Match packets starting with 0x42
        Box::new(LoggingRxHandler::new("byte_42")),
        NextAction::Complete,
    ));
    processing_stage.add_rx_processor(RxStageProcessor::new(
        Box::new(AlwaysMatchCondition), // Catch-all for other packets
        Box::new(DropRxHandler), // Drop remaining packets in this example
        NextAction::drop_with_reason("No specific handler for this packet type"),
    ));
    
    // Add stage to pipeline
    pipeline.add_stage(processing_stage).unwrap();
    
    // Set processing as rx entry point
    pipeline.set_default_rx_entry_stage("processing").unwrap();
    
    pipeline
}

/// Create a NetworkManager with the simple example pipeline
pub fn create_simple_network_manager() -> NetworkManager {
    let pipeline = create_simple_rx_pipeline();
    NetworkManager::with_pipeline(pipeline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test_case]
    fn test_first_byte_condition() {
        let matching_packet = NetworkPacket::new(
            vec![0x42, 0x01, 0x02, 0x03],
            String::from("eth0"),
        );
        
        let non_matching_packet = NetworkPacket::new(
            vec![0x41, 0x01, 0x02, 0x03],
            String::from("eth0"),
        );
        
        let empty_packet = NetworkPacket::new(
            vec![],
            String::from("eth0"),
        );
        
        let condition = FirstByteCondition::new(0x42);
        
        assert!(condition.matches(&matching_packet));
        assert!(!condition.matches(&non_matching_packet));
        assert!(!condition.matches(&empty_packet));
    }

    #[test_case]
    fn test_always_match_condition() {
        let packet = NetworkPacket::new(
            vec![0xAA, 0xBB, 0xCC],
            String::from("eth0"),
        );
        
        let empty_packet = NetworkPacket::new(
            vec![],
            String::from("eth0"),
        );
        
        let condition = AlwaysMatchCondition;
        
        assert!(condition.matches(&packet));
        assert!(condition.matches(&empty_packet));
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
        assert!(pipeline.has_stage("processing"));
        assert_eq!(pipeline.get_default_rx_entry_stage(), Some("processing"));
    }

    #[test_case]
    fn test_simple_network_manager() {
        let manager = create_simple_network_manager();
        assert_eq!(manager.stage_count(), 1);
        assert!(manager.has_stage("processing"));
        
        // Create a packet that matches the FirstByteCondition (0x42)
        let packet = NetworkPacket::new(
            vec![
                0x42, // First byte that triggers logging handler
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
    fn test_packet_drop_example() {
        let manager = create_simple_network_manager();
        
        // Create a packet that doesn't match FirstByteCondition (not 0x42)
        let packet = NetworkPacket::new(
            vec![
                0x41, // First byte that doesn't match, will be dropped
                b'T', b'e', b's', b't',
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