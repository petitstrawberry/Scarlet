//! Integration tests for protocol pipeline

#![cfg(test)]

use crate::network::*;
use crate::network::protocols::*;
use crate::network::test_helpers::DropStageBuilder;
use alloc::{vec, string::String};

#[test_case]
fn test_complete_receive_pipeline() {
    // Build a complete receive pipeline
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            EthernetStage::builder()
                .route_to(EtherType::IPv4, "ipv4")
                .route_to(EtherType::ARP, "arp")
                .enable_rx()
                .build()
        )
        .add_stage(
            IPv4Stage::builder()
                .route_to(IpProtocol::TCP, "tcp")
                .route_to(IpProtocol::UDP, "udp")
                .enable_rx()
                .build()
        )
        .add_stage(ArpStage::builder().enable_rx().build())
        .add_stage(DropStageBuilder::with_stage_id("tcp").build())
        .add_stage(DropStageBuilder::with_stage_id("udp").build())
        .set_default_rx_entry("ethernet")
        .build()
        .unwrap();
    
    // Create complete Ethernet + IPv4 + TCP packet
    let complete_packet = vec![
        // Ethernet header
        0x02, 0x00, 0x00, 0x00, 0x00, 0x02, // Dest MAC
        0x02, 0x00, 0x00, 0x00, 0x00, 0x01, // Src MAC
        0x08, 0x00,                         // EtherType: IPv4
        // IPv4 header
        0x45, 0x00, 0x00, 0x28,             // Version, IHL, TOS, Total Length
        0x12, 0x34, 0x40, 0x00,             // ID, Flags, Fragment Offset
        0x40, 0x06, 0x00, 0x00,             // TTL, Protocol=TCP, Checksum
        192, 168, 1, 1,                     // Source IP
        192, 168, 1, 2,                     // Dest IP
        // TCP data (simplified)
        0x50, 0x50, 0x51, 0x51,
    ];
    
    let packet = NetworkPacket::new(complete_packet);
    let result = pipeline.process_receive(packet, None).unwrap();
    
    // Should have processed through ethernet -> ipv4 -> tcp pipeline
    // Check that ethernet and ipv4 headers were saved
    assert!(result.get_header("ethernet").is_some());
    assert!(result.get_header("ipv4").is_some());
    
    // Check hints from both layers
    assert_eq!(result.get_hint("src_mac"), Some("02:00:00:00:00:01"));
    assert_eq!(result.get_hint("ether_type"), Some("0x0800"));
    assert_eq!(result.get_hint("src_ip"), Some("192.168.1.1"));
    assert_eq!(result.get_hint("ip_protocol"), Some("6"));
}

#[test_case]
fn test_stage_identifier_implementation() {
    // Test that protocol stages implement StageIdentifier correctly
    use crate::network::pipeline::StageIdentifier;
    
    // Test Ethernet stage identifier
    assert_eq!(EthernetStage::stage_id(), "ethernet");
    
    // Test IPv4 stage identifier
    assert_eq!(IPv4Stage::stage_id(), "ipv4");
    
    // Test ARP stage identifier
    assert_eq!(ArpStage::stage_id(), "arp");
}

#[test_case]
fn test_complete_receive_pipeline_with_typed_routing() {
    use crate::network::test_helpers::{TcpProtocol, UdpProtocol};
    
    // Build a complete receive pipeline using typed routing methods
    let pipeline = FlexiblePipeline::builder()
        .add_stage(
            EthernetStage::builder()
                .route_to_typed::<IPv4Stage>(EtherType::IPv4)
                .route_to_typed::<ArpStage>(EtherType::ARP)
                .enable_rx()
                .build()
        )
        .add_stage(
            IPv4Stage::builder()
                .route_to_typed::<TcpProtocol>(IpProtocol::TCP)
                .route_to_typed::<UdpProtocol>(IpProtocol::UDP)
                .enable_rx()
                .build()
        )
        .add_stage(ArpStage::builder().enable_rx().build())
        .add_stage(DropStageBuilder::with_stage_id("tcp").build())
        .add_stage(DropStageBuilder::with_stage_id("udp").build())
        .set_default_rx_entry_typed::<EthernetStage>()
        .build()
        .unwrap();
    
    // Verify that the pipeline has all the required stages
    assert!(pipeline.has_stage_typed::<EthernetStage>());
    assert!(pipeline.has_stage_typed::<IPv4Stage>());
    assert!(pipeline.has_stage_typed::<ArpStage>());
    assert!(pipeline.has_stage("tcp"));
    assert!(pipeline.has_stage("udp"));
    
    // Create complete Ethernet + IPv4 + TCP packet
    let complete_packet = vec![
        // Ethernet header
        0x02, 0x00, 0x00, 0x00, 0x00, 0x02, // Dest MAC
        0x02, 0x00, 0x00, 0x00, 0x00, 0x01, // Src MAC
        0x08, 0x00,                         // EtherType: IPv4
        // IPv4 header
        0x45, 0x00, 0x00, 0x28,             // Version/IHL, ToS, Total Length
        0x12, 0x34, 0x40, 0x00,             // ID, Flags/Fragment
        0x40, 0x06, 0x00, 0x00,             // TTL, Protocol (TCP), Checksum
        0xC0, 0xA8, 0x01, 0x01,             // Source IP (192.168.1.1)
        0xC0, 0xA8, 0x01, 0x02,             // Dest IP (192.168.1.2)
        // TCP payload
        0x00, 0x50, 0x1F, 0x90,             // Source Port, Dest Port
    ];
    
    let packet = NetworkPacket::new(complete_packet);
    
    // Process the packet through the typed pipeline
    let result = pipeline.process_receive(packet, None);
    assert!(result.is_ok());
    
    let final_packet = result.unwrap();
    
    // Verify that all layers processed the packet correctly
    assert!(final_packet.get_header("ethernet").is_some());
    assert!(final_packet.get_header("ipv4").is_some());
    assert_eq!(final_packet.get_hint("ether_type"), Some("0x0800"));
    assert_eq!(final_packet.get_hint("ip_protocol"), Some("6"));
}