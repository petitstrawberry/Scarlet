//! Tests for the new protocol implementations

#![cfg(test)]

use crate::network::*;
use crate::network::protocols::*;
use alloc::{vec, format};

#[test_case]
fn test_ethernet_stage_builder() {
    let stage = EthernetStage::builder()
        .route_to(EtherType::IPv4, "ipv4")
        .route_to(EtherType::ARP, "arp")
        .add_ethertype_route(0x86DD, "ipv6")
        .enable_both()
        .with_src_mac([0x02, 0x00, 0x00, 0x00, 0x00, 0x01])
        .build();
    
    assert_eq!(stage.stage_id, "ethernet");
    assert!(stage.rx_handler.is_some());
    assert!(stage.tx_handler.is_some());
}

#[test_case]
fn test_ipv4_stage_builder() {
    let stage = IPv4Stage::builder()
        .route_to(IpProtocol::TCP, "tcp")
        .route_to(IpProtocol::UDP, "udp")
        .add_protocol_route(1, "icmp")
        .enable_both()
        .with_default_ttl(128)
        .build();
    
    assert_eq!(stage.stage_id, "ipv4");
    assert!(stage.rx_handler.is_some());
    assert!(stage.tx_handler.is_some());
}

#[test_case]
fn test_arp_stage_builder() {
    let stage = ArpStage::builder()
        .enable_both()
        .build();
    
    assert_eq!(stage.stage_id, "arp");
    assert!(stage.rx_handler.is_some());
    assert!(stage.tx_handler.is_some());
}

#[test_case]
fn test_ethernet_receive_processing() {
    // Create Ethernet frame with IPv4 payload
    let mut ethernet_frame = vec![
        // Destination MAC: 02:00:00:00:00:02
        0x02, 0x00, 0x00, 0x00, 0x00, 0x02,
        // Source MAC: 02:00:00:00:00:01
        0x02, 0x00, 0x00, 0x00, 0x00, 0x01,
        // EtherType: IPv4 (0x0800)
        0x08, 0x00,
        // Payload
        0x45, 0x00, 0x00, 0x20, // IPv4 header start
    ];
    ethernet_frame.extend_from_slice(&[0; 16]); // Padding
    
    let mut packet = NetworkPacket::new(ethernet_frame);
    
    let matcher = EtherTypeToStage::new()
        .add_mapping(0x0800, "ipv4")
        .add_mapping(0x0806, "arp");
    
    let handler = EthernetRxHandler::new(matcher);
    let result = handler.handle(&mut packet).unwrap();
    
    // Should route to IPv4
    assert_eq!(result, NextAction::JumpTo("ipv4".to_string()));
    
    // Check hints
    assert_eq!(packet.get_hint("src_mac"), Some("02:00:00:00:00:01"));
    assert_eq!(packet.get_hint("dest_mac"), Some("02:00:00:00:00:02"));
    assert_eq!(packet.get_hint("ether_type"), Some("0x0800"));
    
    // Check header saved
    assert!(packet.get_header("ethernet").is_some());
    assert_eq!(packet.get_header("ethernet").unwrap().len(), 14);
    
    // Check payload updated (should be IPv4 packet without ethernet header)
    assert_eq!(packet.payload().len(), 20);
    assert_eq!(packet.payload()[0], 0x45); // IPv4 version + IHL
}

#[test_case]
fn test_ethernet_vlan_processing() {
    // Create VLAN-tagged Ethernet frame
    let vlan_frame = vec![
        // Destination MAC
        0x02, 0x00, 0x00, 0x00, 0x00, 0x02,
        // Source MAC
        0x02, 0x00, 0x00, 0x00, 0x00, 0x01,
        // VLAN EtherType (0x8100)
        0x81, 0x00,
        // VLAN Tag (Priority=1, VLAN ID=100)
        0x20, 0x64,
        // Actual EtherType: IPv4 (0x0800)
        0x08, 0x00,
        // Payload
        0x45, 0x00,
    ];
    
    let mut packet = NetworkPacket::new(vlan_frame);
    
    let matcher = EtherTypeToStage::new()
        .add_mapping(0x0800, "ipv4");
    
    let handler = EthernetRxHandler::new(matcher);
    let result = handler.handle(&mut packet).unwrap();
    
    // Should route to IPv4
    assert_eq!(result, NextAction::JumpTo("ipv4".to_string()));
    
    // Check VLAN hints
    assert_eq!(packet.get_hint("vlan_tag"), Some("8292")); // 0x2064 = 8292
    assert_eq!(packet.get_hint("vlan_priority"), Some("1"));
    assert_eq!(packet.get_hint("vlan_id"), Some("100"));
    assert_eq!(packet.get_hint("ether_type"), Some("0x0800"));
    
    // Check header includes VLAN tag
    assert_eq!(packet.get_header("ethernet").unwrap().len(), 18);
}

#[test_case]
fn test_ipv4_receive_processing() {
    // Create IPv4 packet with TCP payload
    let ipv4_packet = vec![
        0x45,       // Version=4, IHL=5
        0x00,       // TOS
        0x00, 0x28, // Total length = 40
        0x12, 0x34, // Identification
        0x40, 0x00, // Flags + Fragment offset (Don't Fragment)
        0x40,       // TTL = 64
        0x06,       // Protocol = TCP
        0x00, 0x00, // Checksum (will be ignored)
        // Source IP: 192.168.1.1
        192, 168, 1, 1,
        // Destination IP: 192.168.1.2
        192, 168, 1, 2,
        // TCP payload
        0x50, 0x00, 0x50, 0x50, // TCP header start
    ];
    
    let mut packet = NetworkPacket::new(ipv4_packet);
    
    let matcher = IpProtocolToStage::new()
        .add_mapping(6, "tcp")
        .add_mapping(17, "udp");
    
    let handler = IPv4RxHandler::new(matcher);
    let result = handler.handle(&mut packet).unwrap();
    
    // Should route to TCP
    assert_eq!(result, NextAction::JumpTo("tcp".to_string()));
    
    // Check hints
    assert_eq!(packet.get_hint("src_ip"), Some("192.168.1.1"));
    assert_eq!(packet.get_hint("dest_ip"), Some("192.168.1.2"));
    assert_eq!(packet.get_hint("ip_protocol"), Some("6"));
    assert_eq!(packet.get_hint("ip_ttl"), Some("64"));
    assert_eq!(packet.get_hint("ip_version"), Some("4"));
    
    // Check header saved
    assert!(packet.get_header("ipv4").is_some());
    assert_eq!(packet.get_header("ipv4").unwrap().len(), 20);
    
    // Check payload updated (should be TCP packet without IPv4 header)
    assert_eq!(packet.payload().len(), 4);
    assert_eq!(packet.payload()[0], 0x50); // TCP header start
}

#[test_case]
fn test_arp_receive_processing() {
    // Create ARP request packet
    let arp_packet = vec![
        // Hardware type: Ethernet (1)
        0x00, 0x01,
        // Protocol type: IPv4 (0x0800)
        0x08, 0x00,
        // Hardware length: 6
        0x06,
        // Protocol length: 4
        0x04,
        // Operation: Request (1)
        0x00, 0x01,
        // Sender MAC: 02:00:00:00:00:01
        0x02, 0x00, 0x00, 0x00, 0x00, 0x01,
        // Sender IP: 192.168.1.1
        192, 168, 1, 1,
        // Target MAC: 00:00:00:00:00:00
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        // Target IP: 192.168.1.2
        192, 168, 1, 2,
    ];
    
    let mut packet = NetworkPacket::new(arp_packet);
    
    let handler = ArpRxHandler::new();
    let result = handler.handle(&mut packet).unwrap();
    
    // ARP processing should complete
    assert_eq!(result, NextAction::Complete);
    
    // Check hints
    assert_eq!(packet.get_hint("arp_operation"), Some("1"));
    assert_eq!(packet.get_hint("arp_operation_name"), Some("Request"));
    assert_eq!(packet.get_hint("arp_sender_mac"), Some("02:00:00:00:00:01"));
    assert_eq!(packet.get_hint("arp_sender_ip"), Some("192.168.1.1"));
    assert_eq!(packet.get_hint("arp_target_mac"), Some("00:00:00:00:00:00"));
    assert_eq!(packet.get_hint("arp_target_ip"), Some("192.168.1.2"));
    
    // Check header saved
    assert!(packet.get_header("arp").is_some());
    assert_eq!(packet.get_header("arp").unwrap().len(), 28);
}

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
fn test_ethernet_enum_constants() {
    assert_eq!(EtherType::IPv4.as_u16(), 0x0800);
    assert_eq!(EtherType::ARP.as_u16(), 0x0806);
    assert_eq!(EtherType::IPv6.as_u16(), 0x86DD);
    
    assert_eq!(EtherType::from_u16(0x0800), Some(EtherType::IPv4));
    assert_eq!(EtherType::from_u16(0x0806), Some(EtherType::ARP));
    assert_eq!(EtherType::from_u16(0x9999), None);
    
    assert_eq!(EtherType::IPv4.name(), "IPv4");
    assert_eq!(EtherType::ARP.name(), "ARP");
}

#[test_case]
fn test_ip_protocol_enum_constants() {
    assert_eq!(IpProtocol::TCP.as_u8(), 6);
    assert_eq!(IpProtocol::UDP.as_u8(), 17);
    assert_eq!(IpProtocol::ICMP.as_u8(), 1);
    
    assert_eq!(IpProtocol::from_u8(6), Some(IpProtocol::TCP));
    assert_eq!(IpProtocol::from_u8(17), Some(IpProtocol::UDP));
    assert_eq!(IpProtocol::from_u8(99), None);
    
    assert_eq!(IpProtocol::TCP.name(), "TCP");
    assert_eq!(IpProtocol::UDP.name(), "UDP");
}

#[test_case]
fn test_o1_hashmap_routing() {
    // Test O(1) EtherType routing
    let ethernet_matcher = EtherTypeToStage::new()
        .add_mapping(0x0800, "ipv4")
        .add_mapping(0x0806, "arp")
        .add_mapping(0x86DD, "ipv6");
    
    assert_eq!(ethernet_matcher.get_next_stage(0x0800).unwrap(), "ipv4");
    assert_eq!(ethernet_matcher.get_next_stage(0x0806).unwrap(), "arp");
    assert_eq!(ethernet_matcher.get_next_stage(0x86DD).unwrap(), "ipv6");
    assert!(ethernet_matcher.get_next_stage(0x9999).is_err());
    
    // Test O(1) IP Protocol routing
    let ip_matcher = IpProtocolToStage::new()
        .add_mapping(6, "tcp")
        .add_mapping(17, "udp")
        .add_mapping(1, "icmp");
    
    assert_eq!(ip_matcher.get_next_stage(6).unwrap(), "tcp");
    assert_eq!(ip_matcher.get_next_stage(17).unwrap(), "udp");
    assert_eq!(ip_matcher.get_next_stage(1).unwrap(), "icmp");
    assert!(ip_matcher.get_next_stage(99).is_err());
}