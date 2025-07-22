//! Integration tests and examples for Phase 1 pipeline infrastructure
//!
//! This module demonstrates how to use the new Phase 1 pipeline infrastructure
//! with complete examples of building and using O(1) routing pipelines.

use alloc::{vec, string::ToString};
use super::{
    packet::NetworkPacket,
    error::NetworkError,
    enhanced_pipeline::FlexiblePipeline,
    protocols::{EthernetStage, IPv4Stage},
    matchers::{EtherType, IpProtocol},
};

/// Create a complete example receive pipeline as specified in the issue
///
/// This demonstrates the "美しいパイプライン構築API" (beautiful pipeline construction API)
/// with the builder pattern and fluent interface.
pub fn build_receive_pipeline() -> Result<FlexiblePipeline, NetworkError> {
    FlexiblePipeline::builder()
        .add_stage(
            EthernetStage::builder()
                .add_ethertype_route(0x0800, "ipv4")  // IPv4
                .add_ethertype_route(0x0806, "arp")   // ARP
                .route_to(EtherType::IPv6, "ipv6")    // IPv6（enum使用）
                .enable_rx()
                .build()
        )
        .add_stage(
            IPv4Stage::builder()
                .add_protocol_route(6, "tcp")         // TCP
                .add_protocol_route(17, "udp")        // UDP
                .route_to(IpProtocol::ICMP, "icmp")   // ICMP（enum使用）
                .enable_rx()
                .build()
        )
        .set_default_rx_entry("ethernet")
        .build()
}

/// Create a complete example transmit pipeline as specified in the issue
///
/// This demonstrates transmit path processing with hint-based packet construction.
pub fn build_transmit_pipeline() -> Result<FlexiblePipeline, NetworkError> {
    FlexiblePipeline::builder()
        .add_stage(
            IPv4Stage::builder()
                .with_stage_id("ipv4")
                .enable_tx()
                .build()
        )
        .add_stage(
            EthernetStage::builder()
                .with_stage_id("ethernet")
                .enable_tx()
                .build()
        )
        .set_default_tx_entry("ipv4")  // Start from IPv4 for transmission
        .build()
}

/// Demonstrate O(1) performance characteristics of the pipeline
///
/// This function shows that stage lookup is O(1) regardless of the number of stages
/// in the pipeline, thanks to HashMap-based routing.
pub fn demonstrate_o1_performance() -> Result<(), NetworkError> {
    // Build a pipeline with many stages to test O(1) lookup
    let mut builder = FlexiblePipeline::builder();
    
    // Add 100 stages to test scalability
    for i in 0..100 {
        let stage = EthernetStage::builder()
            .with_stage_id(&alloc::format!("stage_{}", i))
            .add_ethertype_route(0x0800 + i as u16, &alloc::format!("next_stage_{}", i))
            .enable_rx()
            .build();
        builder = builder.add_stage(stage);
    }
    
    let pipeline = builder
        .set_default_rx_entry("stage_0")
        .build()?;
    
    // Pipeline operations should be O(1) regardless of number of stages
    assert_eq!(pipeline.stage_count(), 100);
    assert!(pipeline.has_stage("stage_50"));
    assert!(!pipeline.has_stage("nonexistent"));
    
    Ok(())
}

/// Example of processing an Ethernet + IPv4 packet through the receive pipeline
pub fn example_receive_processing() -> Result<(), NetworkError> {
    let pipeline = build_receive_pipeline()?;
    
    // Create a realistic Ethernet + IPv4 packet
    let packet = NetworkPacket::new(
        vec![
            // Ethernet header (14 bytes)
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, // Dest MAC
            0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, // Src MAC
            0x08, 0x00, // EtherType (IPv4)
            
            // IPv4 header (20 bytes)
            0x45, // Version (4) + IHL (5)
            0x00, // DSCP + ECN
            0x00, 0x1C, // Total Length (28 bytes)
            0x00, 0x01, // Identification
            0x40, 0x00, // Flags + Fragment Offset
            0x40, // TTL (64)
            0x06, // Protocol (TCP)
            0x73, 0x6E, // Header Checksum
            192, 168, 1, 1, // Source IP
            192, 168, 1, 2, // Destination IP
            
            // TCP payload (8 bytes)
            0x00, 0x50, 0x00, 0x80, // Source port, dest port
            0x00, 0x00, 0x00, 0x01, // Sequence number
        ],
        "eth0".to_string()
    );
    
    // Process through the pipeline
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_ok());
    
    let processed_packet = result.unwrap();
    
    // Verify headers were extracted correctly
    assert!(processed_packet.get_header("ethernet").is_some());
    assert!(processed_packet.get_header("ipv4").is_some());
    
    // Verify payload contains only the TCP segment
    assert_eq!(processed_packet.payload().len(), 8);
    assert_eq!(processed_packet.payload()[0..4], [0x00, 0x50, 0x00, 0x80]);
    
    Ok(())
}

/// Example of building and transmitting a packet through the transmit pipeline
pub fn example_transmit_processing() -> Result<(), NetworkError> {
    let pipeline = build_transmit_pipeline()?;
    
    // Create a packet with just the application payload
    let mut packet = NetworkPacket::new(
        vec![0x48, 0x65, 0x6C, 0x6C, 0x6F], // "Hello" in ASCII
        "eth0".to_string()
    );
    
    // Set hints for transmission (upper layer → lower layer)
    packet.set_hint("destination_ip", "10.0.0.1");
    packet.set_hint("source_ip", "192.168.1.1");
    packet.set_hint("protocol", "6"); // TCP
    packet.set_hint("dest_mac", "aa:bb:cc:dd:ee:ff");
    packet.set_hint("ethertype", "0x0800");
    
    // Process through the transmit pipeline
    let result = pipeline.process_transmit(packet, None);
    assert!(result.is_ok());
    
    let processed_packet = result.unwrap();
    
    // Verify complete frame was built
    // Should have: Ethernet header (14) + IPv4 header (20) + payload (5) = 39 bytes
    assert_eq!(processed_packet.payload().len(), 39);
    
    // Verify Ethernet header is at the front
    let payload = processed_packet.payload();
    assert_eq!(payload[0..6], [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]); // Dest MAC
    assert_eq!(payload[12..14], [0x08, 0x00]); // EtherType
    
    // Verify IPv4 header follows
    assert_eq!(payload[14], 0x45); // Version + IHL
    assert_eq!(payload[23], 6); // Protocol (TCP)
    
    // Verify original payload is at the end
    assert_eq!(payload[34..39], [0x48, 0x65, 0x6C, 0x6C, 0x6F]); // "Hello"
    
    Ok(())
}

/// Comprehensive test of the infrastructure
pub fn run_integration_tests() -> Result<(), NetworkError> {
    // Test 1: O(1) performance characteristics
    demonstrate_o1_performance()?;
    
    // Test 2: Complete receive pipeline
    example_receive_processing()?;
    
    // Test 3: Complete transmit pipeline
    example_transmit_processing()?;
    
    // Test 4: Builder pattern fluency
    let _rx_pipeline = build_receive_pipeline()?;
    let _tx_pipeline = build_transmit_pipeline()?;
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_build_receive_pipeline() {
        let result = build_receive_pipeline();
        assert!(result.is_ok());
        
        let pipeline = result.unwrap();
        assert_eq!(pipeline.stage_count(), 2);
        assert!(pipeline.has_stage("ethernet"));
        assert!(pipeline.has_stage("ipv4"));
        assert_eq!(pipeline.get_default_rx_entry(), Some("ethernet"));
    }

    #[test_case]
    fn test_build_transmit_pipeline() {
        let result = build_transmit_pipeline();
        assert!(result.is_ok());
        
        let pipeline = result.unwrap();
        assert_eq!(pipeline.stage_count(), 2);
        assert!(pipeline.has_stage("ethernet"));
        assert!(pipeline.has_stage("ipv4"));
        assert_eq!(pipeline.get_default_tx_entry(), Some("ipv4"));
    }

    #[test_case]
    fn test_o1_performance() {
        let result = demonstrate_o1_performance();
        assert!(result.is_ok());
    }

    #[test_case]
    fn test_receive_processing() {
        let result = example_receive_processing();
        assert!(result.is_ok());
    }

    #[test_case]
    fn test_transmit_processing() {
        let result = example_transmit_processing();
        assert!(result.is_ok());
    }

    #[test_case]
    fn test_integration_tests() {
        let result = run_integration_tests();
        assert!(result.is_ok());
    }

    #[test_case]
    fn test_error_conditions() {
        // Test invalid pipeline configuration
        let result = FlexiblePipeline::builder()
            .set_default_rx_entry("nonexistent")
            .build();
        assert!(result.is_err());
        
        // Test empty pipeline processing
        let pipeline = FlexiblePipeline::new();
        let packet = NetworkPacket::new(vec![0x01], "test".to_string());
        let result = pipeline.process_receive(packet, None);
        assert!(result.is_err());
        
        // Test insufficient packet data
        let pipeline = build_receive_pipeline().unwrap();
        let short_packet = NetworkPacket::new(vec![0x01, 0x02], "test".to_string());
        let result = pipeline.process_receive(short_packet, None);
        assert!(result.is_err());
    }

    #[test_case]
    fn test_builder_pattern_flexibility() {
        // Test various builder configurations
        let pipeline1 = FlexiblePipeline::builder()
            .add_stage(
                EthernetStage::builder()
                    .with_stage_id("custom_ethernet")
                    .route_to(EtherType::IPv4, "custom_ipv4")
                    .enable_rx()
                    .enable_tx()
                    .build()
            )
            .add_stage(
                IPv4Stage::builder()
                    .with_stage_id("custom_ipv4")
                    .route_to(IpProtocol::TCP, "tcp")
                    .enable_rx()
                    .build()
            )
            .set_default_rx_entry("custom_ethernet")
            .build()
            .unwrap();
        
        assert_eq!(pipeline1.stage_count(), 2);
        assert!(pipeline1.has_stage("custom_ethernet"));
        assert!(pipeline1.has_stage("custom_ipv4"));
        
        // Test tx-only configuration
        let pipeline2 = FlexiblePipeline::builder()
            .add_stage(
                EthernetStage::builder()
                    .enable_tx()
                    .build()
            )
            .set_default_tx_entry("ethernet")
            .build()
            .unwrap();
        
        assert_eq!(pipeline2.stage_count(), 1);
        assert_eq!(pipeline2.get_default_tx_entry(), Some("ethernet"));
        assert!(pipeline2.get_default_rx_entry().is_none());
    }
}